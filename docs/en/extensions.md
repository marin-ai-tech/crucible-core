# Extend Crucible Core

Use a `CoreModule` for a reusable mechanic, such as a simulation system, a component, or a prototype kind. Use an `Extension` to choose which modules a host activates, register its own behavior, and provide content. A host links both as Rust code and selects one active extension when it starts. Changes to that code require rebuilding the host.

The launcher checks the extension's compatible core version, resolves module dependencies, registers the selected modules and extension, then loads and validates their data. An extension can name YAML directories in `[content].roots`. Those paths resolve relative to the directory provided by its `ManifestSource`.

## Worked example

This example adds a tick counter as a core module. The host loads the `demo` extension, which enables the counter. The same host can run a three-tick check or keep the simulation running in the foreground.

Extract the source archive into `$HOME/src` so that it creates `$HOME/src/crucible-core-0.1.0`. Create the host beside that directory.

```sh
mkdir -p "$HOME/src/crucible-demo-host/src"
cd "$HOME/src/crucible-demo-host"
```

### `Cargo.toml`

```toml
[package]
name = "crucible-demo-host"
version = "0.1.0"
edition = "2024"
publish = false

[dependencies]
crucible-admin = { path = "../crucible-core-0.1.0/backend/apps/admin" }
crucible-ecs = { path = "../crucible-core-0.1.0/backend/crates/crucible-ecs" }
crucible-ext = { path = "../crucible-core-0.1.0/backend/crates/crucible-ext" }
```

### `extension.toml`

```toml
[extension]
id = "demo"
version = "0.1.0"
core = "^0.1"

[modules]
require = ["tick-counter"]
```

### `src/main.rs`

```rust
use std::error::Error;
use std::io;
use std::path::Path;
use std::sync::mpsc;
use std::thread;

use crucible_admin::collector::{Collector, EngineMetadata, ExtensionInfo};
use crucible_ecs::{System, World};
use crucible_ext::{
    CoreModule, Extension, LaunchConfig, Launcher, ManifestSource, RegisterContext,
};

#[derive(Default)]
struct TickCount(u64);

struct CountTicks;

impl System for CountTicks {
    fn update(&mut self, world: &mut World, _frame_time: f32) {
        let count = world.resource_mut::<TickCount>().expect("tick counter registered");
        count.0 += 1;
        if count.0.is_multiple_of(300) {
            println!("completed {} ticks", count.0);
        }
    }
}

struct TickCounter;

impl CoreModule for TickCounter {
    fn id(&self) -> &'static str {
        "tick-counter"
    }

    fn register(&self, ctx: &mut RegisterContext<'_>) {
        ctx.resource(TickCount::default()).system(CountTicks);
    }
}

struct Demo;

impl Extension for Demo {
    fn manifest(&self) -> ManifestSource {
        ManifestSource::new(include_str!("../extension.toml"), env!("CARGO_MANIFEST_DIR"))
    }

    fn register(&self, _ctx: &mut RegisterContext<'_>) {}
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let check = match (args.next().as_deref(), args.next()) {
        (None, None) => false,
        (Some("--check"), None) => true,
        _ => return Err("usage: crucible-demo-host [--check]".into()),
    };

    let launcher = Launcher::new().module(TickCounter).extension(Demo);
    let config = LaunchConfig::for_extension("demo");
    let tick_rate = config.loop_config.tick_rate;
    let mut runtime = launcher.launch(config)?;
    if check {
        runtime.run_ticks(3);
        assert_eq!(runtime.world().resource::<TickCount>().unwrap().0, 3);
        runtime.shutdown();
        println!("check passed: 3 ticks");
        return Ok(());
    }

    let monitoring = std::env::var_os("CRUCIBLE_METRICS_DB")
        .map(|path| {
            Collector::start(
                Path::new(&path),
                EngineMetadata {
                    core_version: crucible_ext::CORE_VERSION.into(),
                    tick_rate,
                    extension: ExtensionInfo {
                        id: runtime.manifest().id.clone(),
                        version: runtime.manifest().version.to_string(),
                    },
                    modules: runtime.modules().into_iter().map(str::to_owned).collect(),
                },
                runtime.game().metrics(),
            )
        })
        .transpose()
        .map_err(io::Error::other)?;
    if let Some(collector) = &monitoring {
        let ticks = collector.observations();
        let work = collector.observations();
        runtime.game_mut().set_tick_observer(move |_, duration| ticks.tick(duration));
        runtime.game_mut().set_work_observer(move |phase, name, duration| {
            work.work(phase, name, duration);
        });
    }

    let (quit, shutdown) = mpsc::channel();
    let console = thread::spawn(move || {
        for line in io::stdin().lines().map_while(Result::ok) {
            if line.trim() == "quit" {
                let _ = quit.send(());
                break;
            }
        }
    });
    println!("demo extension ready");
    loop {
        runtime.run_iteration();
        if let Some(collector) = &monitoring {
            collector.poll(runtime.game().metrics());
        }
        let time = runtime.game().until_next_tick();
        match shutdown.recv_timeout(time) {
            Ok(()) => {
                console.join().expect("console worker");
                break;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => thread::sleep(time),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
    if let Some(collector) = monitoring {
        collector.finish(runtime.game().metrics());
    }
    runtime.shutdown();
    Ok(())
}
```

### Build and connect the module

`Launcher::module` makes the module available to the host. The manifest's `require` entry enables it, and `LaunchConfig::for_extension("demo")` selects the active extension. Run the commands from `$HOME/src/crucible-demo-host`.

```sh
cargo +1.100.0 build --release
./target/release/crucible-demo-host --check
```

The check prints `check passed: 3 ticks` and exits. Cargo can build with `--offline` when the required dependencies are already cached.

```sh
./target/release/crucible-demo-host
```

The foreground run logs a message every 300 ticks. Enter `quit` to stop it and flush the final metrics window. The [administrator client](admin.md) configures the database and sends this same command when you choose **Stop engine**. To install the binary for [running as a user service](running.md), run the following command from the host directory.

```sh
install -D -m 755 target/release/crucible-demo-host "$HOME/.local/bin/crucible-demo-host"
```

## More examples

The [extension tests](../../backend/crates/crucible-ext/tests/extensions.rs) include these short examples.

| Example | What it shows |
| --- | --- |
| `module_dependency_connected_automatically` | A module declares a dependency and the launcher registers it first. |
| `console_command_registered` | An extension registers a command and the host invokes it through the runtime. |
| `default_implementation` | A module supplies default round behavior through an extension point. |
