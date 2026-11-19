use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use crucible_admin::metric_history::History;
use crucible_admin::monitoring_http::{Control, Dashboard};
use serde_json::{Value, json};

const USAGE: &str = "\
Usage: crucible-admin --server <executable> --workdir <dir> --metrics-db <path> \
--log <path> [--bind <loopback:port>] -- [server arguments...]";
const DEFAULT_BIND: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9400);
const LOG_TAIL_BYTES: u64 = 64 * 1024;
const STOP_TIMEOUT: Duration = Duration::from_secs(5);
const STOP_POLL: Duration = Duration::from_millis(50);
const REAP_POLL: Duration = Duration::from_millis(250);

struct Config {
    server: PathBuf,
    workdir: PathBuf,
    metrics_db: PathBuf,
    log: PathBuf,
    bind: SocketAddr,
    server_args: Vec<OsString>,
}

fn parse_args(args: impl Iterator<Item = OsString>) -> Result<Option<Config>, String> {
    let mut args = args;
    let mut server = None;
    let mut workdir = None;
    let mut metrics_db = None;
    let mut log = None;
    let mut bind = DEFAULT_BIND;
    let mut server_args = Vec::new();
    while let Some(flag) = args.next() {
        match flag.to_str() {
            Some("--") => {
                server_args.extend(args);
                break;
            }
            Some("-h" | "--help") => return Ok(None),
            Some("--server") => server = Some(next_path(&mut args, "--server")?),
            Some("--workdir") => workdir = Some(next_path(&mut args, "--workdir")?),
            Some("--metrics-db") => metrics_db = Some(next_path(&mut args, "--metrics-db")?),
            Some("--log") => log = Some(next_path(&mut args, "--log")?),
            Some("--bind") => {
                let address = args.next().ok_or("--bind requires an address")?;
                bind = address
                    .to_str()
                    .and_then(|value| value.parse().ok())
                    .ok_or("--bind requires a numeric loopback address and port")?;
                if !bind.ip().is_loopback() {
                    return Err("--bind requires a loopback address".to_owned());
                }
            }
            _ => return Err(format!("unrecognized argument: {}", flag.to_string_lossy())),
        }
    }
    let metrics_db = metrics_db.ok_or("--metrics-db is required")?;
    let log = log.ok_or("--log is required")?;
    if metrics_db == log {
        return Err("--metrics-db and --log require separate files".to_owned());
    }
    if server_args.iter().any(|arg| arg == "--metrics-db") {
        return Err(
            "set --metrics-db before -- so the admin and server use the same history file"
                .to_owned(),
        );
    }
    Ok(Some(Config {
        server: server.ok_or("--server is required")?,
        workdir: workdir.ok_or("--workdir is required")?,
        metrics_db,
        log,
        bind,
        server_args,
    }))
}

fn next_path(args: &mut impl Iterator<Item = OsString>, flag: &str) -> Result<PathBuf, String> {
    let value = args
        .next()
        .filter(|value| !value.is_empty() && value != "--")
        .ok_or_else(|| format!("{flag} requires a path"))?;
    std::path::absolute(Path::new(&value)).map_err(|error| format!("{flag}: {error}"))
}

struct ServerProcess {
    config: Config,
    child: Option<Child>,
    last_exit: Option<i32>,
}

impl ServerProcess {
    fn reap(&mut self) -> Result<(), String> {
        let Some(child) = self.child.as_mut() else {
            return Ok(());
        };
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("could not check server process: {error}"))?
        {
            self.last_exit = status.code();
            self.child = None;
        }
        Ok(())
    }

    fn start(&mut self) -> Result<(), String> {
        self.reap()?;
        if self.child.is_some() {
            return Err("server is already running".to_owned());
        }
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.config.log)
            .map_err(|error| format!("could not open the server log: {error}"))?;
        let stderr = log
            .try_clone()
            .map_err(|error| format!("could not duplicate the server log handle: {error}"))?;
        let child = Command::new(&self.config.server)
            .current_dir(&self.config.workdir)
            .args(&self.config.server_args)
            .env("CRUCIBLE_METRICS_DB", &self.config.metrics_db)
            .stdin(Stdio::piped())
            .stdout(log)
            .stderr(stderr)
            .spawn()
            .map_err(|error| format!("could not start the server: {error}"))?;
        self.child = Some(child);
        Ok(())
    }

    fn stop(&mut self) -> Result<(), String> {
        self.reap()?;
        let Some(child) = self.child.as_mut() else {
            return Ok(());
        };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(b"quit\n");
        }
        let deadline = Instant::now() + STOP_TIMEOUT;
        loop {
            self.reap()?;
            if self.child.is_none() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                break;
            }
            thread::sleep(STOP_POLL);
        }
        let child = self.child.as_mut().expect("a child is still running");
        child
            .kill()
            .map_err(|error| format!("could not terminate the server: {error}"))?;
        self.last_exit = child
            .wait()
            .map_err(|error| format!("could not reap the server: {error}"))?
            .code();
        self.child = None;
        Ok(())
    }
}

impl Drop for ServerProcess {
    fn drop(&mut self) {
        if let Err(error) = self.stop() {
            eprintln!("crucible-admin: {error}");
        }
    }
}

fn log_tail(path: &Path) -> Result<String, String> {
    let read = || -> io::Result<_> {
        let file = match File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(String::new()),
            Err(error) => return Err(error),
        };
        let length = file.metadata()?.len();
        let offset = length.saturating_sub(LOG_TAIL_BYTES);
        let mut file = file;
        file.seek(SeekFrom::Start(offset))?;
        let mut bytes = Vec::new();
        file.take(LOG_TAIL_BYTES).read_to_end(&mut bytes)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    };
    read().map_err(|error| format!("could not read the server log: {error}"))
}

struct ProcessControl(Arc<Mutex<ServerProcess>>);

impl Control for ProcessControl {
    fn status(&mut self) -> Value {
        let mut server = self.0.lock().expect("server control lock");
        if let Err(error) = server.reap() {
            eprintln!("crucible-admin: {error}");
        }
        json!({
            "running": server.child.is_some(),
            "pid": server.child.as_ref().map(Child::id),
            "last_exit": server.last_exit,
        })
    }

    fn logs(&mut self) -> Result<String, String> {
        log_tail(&self.0.lock().expect("server control lock").config.log)
    }

    fn start(&mut self) -> Result<(), String> {
        self.0.lock().expect("server control lock").start()
    }

    fn stop(&mut self) -> Result<(), String> {
        self.0.lock().expect("server control lock").stop()
    }
}

fn run(config: Config) -> Result<(), String> {
    drop(
        History::open(&config.metrics_db)
            .map_err(|error| format!("could not initialize metrics history: {error}"))?,
    );
    let history = History::open_readonly(&config.metrics_db)
        .map_err(|error| format!("could not open metrics history: {error}"))?;
    let bind = config.bind;
    let server = Arc::new(Mutex::new(ServerProcess {
        config,
        child: None,
        last_exit: None,
    }));
    let (signal, shutdown) = mpsc::sync_channel(1);
    ctrlc::set_handler(move || {
        let _ = signal.try_send(());
    })
    .map_err(|error| format!("could not register shutdown handler: {error}"))?;
    let dashboard = Dashboard::start(bind, history, ProcessControl(server.clone()))?;
    println!(
        "Crucible admin listening on http://{}/",
        dashboard.local_addr()
    );
    while let Err(mpsc::RecvTimeoutError::Timeout) = shutdown.recv_timeout(REAP_POLL) {
        if let Ok(mut server) = server.try_lock()
            && let Err(error) = server.reap()
        {
            eprintln!("crucible-admin: {error}");
        }
    }
    drop(dashboard);
    server.lock().expect("server control lock").stop()
}

fn main() -> ExitCode {
    let config = match parse_args(std::env::args_os().skip(1)) {
        Ok(Some(config)) => config,
        Ok(None) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(error) => {
            eprintln!("crucible-admin: {error}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match run(config) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("crucible-admin: {error}");
            ExitCode::FAILURE
        }
    }
}
