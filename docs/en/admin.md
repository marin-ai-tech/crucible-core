# Administrator client

`crucible-admin` is a local console for a Crucible host. It starts and stops a configured process, shows its combined log, and reads simulation telemetry from SQLite. The console stays available while the host is stopped. It manages the child it launched.

## Build and open the console

From the released `backend` directory, build the client.

```sh
cargo build --locked --release -p crucible-admin
```

Choose an executable host and its working directory. This example uses the binary built in the [extension walkthrough](extensions.md). Adjust the paths for your installation.

```sh
mkdir -p "$HOME/.local/state/crucible"
./target/release/crucible-admin \
  --server "$HOME/src/crucible-demo-host/target/release/crucible-demo-host" \
  --workdir "$HOME/src/crucible-demo-host" \
  --metrics-db "$HOME/.local/state/crucible/ticks.sqlite" \
  --log "$HOME/.local/state/crucible/host.log"
```

Open `http://127.0.0.1:9400/` and choose **Start engine**. Pass optional host arguments after `--` when launching the client. The **Stop engine** action writes `quit` to the child and waits up to five seconds before terminating a host that is still running.

The interface listens on loopback. On a remote machine, forward its port from your workstation and open the forwarded address locally.

```sh
ssh -N -L 9400:127.0.0.1:9400 user@server
```

## Read the dashboard

- **Process and log.** See whether the child is running, its process ID, last exit status, and the most recent 64 KiB of stdout and stderr.
- **Tick history.** Each five-second window shows mean, p95, p99, maximum, and counts for executed or skipped ticks. Page back through the SQLite history to inspect earlier runs. History retains up to seven days.
- **Extensions and mechanics.** An instrumented host reports its active extension and enabled modules. The latest window lists measured ECS updates and named phase handlers with call counts, total time, and maximum time.

The example in the extension walkthrough reports its active extension, the counter module, and the counter system's timings. The first tick history window appears after about five seconds.

## Connect a host to telemetry

The client passes `CRUCIBLE_METRICS_DB` to the managed process. A host can read that path, initialize `crucible_admin::collector::Collector` with its metadata and the current `GameLoop::metrics()`, and attach its observations to the tick and work observer hooks. Call `Collector::poll` after each clock-driven iteration to enqueue complete windows and `Collector::finish` on orderly shutdown.

Each host should use its own database and log paths. The SQLite writer runs separately from the simulation. The client reads the same database while the host is running or stopped. [Metrics API](metrics.md) · [Collector source](../../backend/apps/admin/src/collector.rs)
