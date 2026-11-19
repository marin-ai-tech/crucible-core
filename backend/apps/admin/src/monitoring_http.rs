use std::io::Cursor;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde_json::{Value, json};
use tiny_http::{Header, Method, Request, Response, Server};

use crate::metric_history::History;

const HTML: &[u8] = include_bytes!("../dashboard/index.html");
const CSS: &[u8] = include_bytes!("../dashboard/dashboard.css");
const JS: &[u8] = include_bytes!("../dashboard/dashboard.js");

pub trait Control: Send + 'static {
    fn status(&mut self) -> Value;
    fn logs(&mut self) -> Result<String, String>;
    fn start(&mut self) -> Result<(), String>;
    fn stop(&mut self) -> Result<(), String>;
}

pub struct Dashboard {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Dashboard {
    pub fn start(
        bind: SocketAddr,
        history: History,
        control: impl Control,
    ) -> Result<Self, String> {
        if !bind.ip().is_loopback() {
            return Err("metrics dashboard requires a loopback address".into());
        }
        let server =
            Server::http(bind).map_err(|error| format!("bind dashboard at {bind}: {error}"))?;
        let addr = server.server_addr().to_ip().expect("dashboard uses TCP");
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let worker = thread::Builder::new()
            .name("metrics-dashboard".into())
            .spawn(move || serve(server, history, control, &worker_stop))
            .map_err(|error| format!("start dashboard worker: {error}"))?;
        Ok(Self {
            addr,
            stop,
            worker: Some(worker),
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }
}

impl Drop for Dashboard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            tracing::error!("metrics dashboard worker panicked");
        }
    }
}

fn serve(server: Server, history: History, mut control: impl Control, stop: &AtomicBool) {
    while !stop.load(Ordering::Relaxed) {
        match server.recv_timeout(Duration::from_millis(250)) {
            Ok(Some(request)) => {
                let response = dispatch(&request, &history, &mut control);
                if let Err(error) = request.respond(response) {
                    tracing::debug!(%error, "could not send dashboard response");
                }
            }
            Ok(None) => {}
            Err(error) => {
                tracing::error!(%error, "metrics dashboard stopped receiving requests");
                break;
            }
        }
    }
}

type Reply = Response<Cursor<Vec<u8>>>;

fn dispatch(request: &Request, history: &History, control: &mut impl Control) -> Reply {
    let Some(host) = request_header(request, "Host").filter(|value| loopback_host(value)) else {
        return text(403, "Dashboard requires a loopback Host header.");
    };
    if request.method() == &Method::Post {
        return dispatch_control(request, host, control);
    }
    if request.method() != &Method::Get {
        return text(405, "Supported methods are GET and POST.")
            .with_header(header("Allow", "GET, POST"));
    }

    match request.url().split_once('?').unwrap_or((request.url(), "")) {
        ("/", "") => reply(200, "text/html; charset=utf-8", HTML),
        ("/dashboard.css", "") => reply(200, "text/css; charset=utf-8", CSS),
        ("/dashboard.js", "") => reply(200, "text/javascript; charset=utf-8", JS),
        ("/api/metrics", query) => match cursor(query) {
            Ok(before) => history_reply(history, before),
            Err(()) => text(400, "Expected a positive integer cursor named before."),
        },
        ("/api/status", "") => status_reply(history, control),
        ("/api/logs", "") => match control.logs() {
            Ok(logs) => json_reply(200, json!({ "logs": logs })),
            Err(error) => json_reply(503, json!({ "error": error })),
        },
        ("/api/start" | "/api/stop", "") => {
            text(405, "Process controls require POST.").with_header(header("Allow", "POST"))
        }
        _ => text(404, "Route not found."),
    }
}

fn request_header<'a>(request: &'a Request, name: &'static str) -> Option<&'a str> {
    request
        .headers()
        .iter()
        .find(|header| header.field.equiv(name))
        .map(|header| header.value.as_str())
}

fn dispatch_control(request: &Request, host: &str, control: &mut impl Control) -> Reply {
    if !matches!(request.url(), "/api/start" | "/api/stop") {
        return text(404, "Route not found.");
    }
    let origin = format!("http://{host}");
    if request_header(request, "Origin") != Some(origin.as_str())
        || request_header(request, "X-Crucible-Admin") != Some("1")
    {
        return json_reply(
            403,
            json!({ "error": "Open the local admin dashboard to control the server." }),
        );
    }
    let result = if request.url() == "/api/start" {
        control.start()
    } else {
        control.stop()
    };
    match result {
        Ok(()) => json_reply(200, json!({ "ok": true })),
        Err(error) => json_reply(409, json!({ "error": error })),
    }
}

fn loopback_host(host: &str) -> bool {
    if let Ok(addr) = host.parse::<SocketAddr>() {
        return addr.ip().is_loopback();
    }
    let literal = host
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(host);
    if let Ok(ip) = literal.parse::<IpAddr>() {
        return ip.is_loopback();
    }
    host.eq_ignore_ascii_case("localhost")
        || host.split_once(':').is_some_and(|(name, port)| {
            name.eq_ignore_ascii_case("localhost")
                && port.bytes().all(|byte| byte.is_ascii_digit())
                && port.parse::<u16>().is_ok()
        })
}

fn cursor(query: &str) -> Result<Option<i64>, ()> {
    if query.is_empty() {
        return Ok(None);
    }
    let value = query.strip_prefix("before=").ok_or(())?;
    if !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(());
    }
    value
        .parse::<i64>()
        .ok()
        .filter(|&id| id > 0)
        .map(Some)
        .ok_or(())
}

fn history_reply(history: &History, before: Option<i64>) -> Reply {
    let page = match history.page(before) {
        Ok(page) => page,
        Err(error) => {
            tracing::warn!(%error, "could not read tick history");
            return text(503, "Tick history is temporarily unavailable.");
        }
    };
    match serde_json::to_vec(&page) {
        Ok(body) => reply(200, "application/json", body),
        Err(error) => {
            tracing::error!(%error, "could not encode tick history");
            text(500, "Could not encode tick history.")
        }
    }
}

fn status_reply(history: &History, control: &mut impl Control) -> Reply {
    let engine = match history.runtime_state() {
        Ok(Some(state)) => match serde_json::from_str::<Value>(&state) {
            Ok(value) => value,
            Err(error) => {
                tracing::warn!(%error, "could not decode recorded engine status");
                Value::Null
            }
        },
        Ok(None) => Value::Null,
        Err(error) => {
            tracing::warn!(%error, "could not read recorded engine status");
            Value::Null
        }
    };
    json_reply(
        200,
        json!({ "process": control.status(), "engine": engine }),
    )
}

fn json_reply(code: u16, body: Value) -> Reply {
    reply(code, "application/json", body.to_string().into_bytes())
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("static HTTP header is valid")
}

fn text(code: u16, message: &str) -> Reply {
    reply(code, "text/plain; charset=utf-8", message.as_bytes())
}

fn reply(code: u16, mime: &str, body: impl Into<Vec<u8>>) -> Reply {
    Response::from_data(body)
        .with_status_code(code)
        .with_header(header("Content-Type", mime))
        .with_header(header("Cache-Control", "no-store"))
        .with_header(header("X-Content-Type-Options", "nosniff"))
        .with_header(header("Cross-Origin-Resource-Policy", "same-origin"))
        .with_header(header(
            "Content-Security-Policy",
            "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'",
        ))
}

#[cfg(test)]
mod tests {
    use super::{cursor, loopback_host};

    #[test]
    fn history_cursor_has_one_positive_integer() {
        assert_eq!(cursor(""), Ok(None));
        assert_eq!(cursor("before=42"), Ok(Some(42)));
        for query in [
            "before=",
            "before=0",
            "before=-1",
            "before=+1",
            "before=4&before=5",
            "from=1",
        ] {
            assert!(cursor(query).is_err(), "{query}");
        }
    }

    #[test]
    fn dashboard_host_check_allows_loopback_forwarding() {
        for host in [
            "localhost",
            "LOCALHOST:9401",
            "127.0.0.1:9401",
            "[::1]",
            "[::1]:9401",
        ] {
            assert!(loopback_host(host), "{host}");
        }
        for host in [
            "example.com",
            "0.0.0.0:9400",
            "[::]:9400",
            "localhost.example:9401",
            "localhost:NaN",
            "localhost:",
            "[::1",
        ] {
            assert!(!loopback_host(host), "{host}");
        }
    }
}
