# Simulation metrics

## Built-in counters

Read `GameLoop::metrics()` to get a snapshot of the counters for the current loop. A host using `ServerRuntime` can read them through `runtime.game().metrics()`.

| Counter | What it measures |
| --- | --- |
| `ticks` | Completed simulation ticks, including manually executed ticks. |
| `skipped_ticks` | Tick intervals discarded when a clock-driven iteration exceeds the catch-up limit. |
| `lagging_iterations` | Outer iterations that discarded at least one tick interval. |

Counters accumulate for the lifetime of the loop. Take differences between samples to measure a time window. When the loop drops ticks, it also emits a `tracing` warning with `dropped`, `limit`, and `total_skipped`. Install a tracing subscriber in the host to see these warnings.

## Measure time spent inside a tick

`GameLoop::set_tick_observer` installs one callback that receives the tick number and its elapsed wall time. The interval starts before the tick and simulation clock advance and includes timers, input, system updates, physics, deferred work, and all replication-phase handlers. Measure socket work, wire encoding, the wait between iterations, and end-to-end latency separately in the host.

The observer uses a real monotonic clock even when the simulation uses `ManualClock`. Installing another observer replaces the existing callback. Keep the callback lightweight as it runs on the simulation thread. Use `clear_tick_observer` when the measurement window is over.

### Example

Use this as `src/main.rs` in a Rust host with `crucible-ecs` and `crucible-loop` as dependencies. The manual clock schedules 12 due tick intervals. The default catch-up limit allows the loop to execute five, so the remaining seven are counted as skipped. The reported work duration is the real time spent in each of the five executed ticks.

```rust
use std::time::Duration;

use crucible_ecs::World;
use crucible_loop::{GameLoop, LoopConfig, ManualClock};

fn main() {
    let (world, systems) = World::builder().build().expect("build a test world");
    let clock = ManualClock::new();
    let mut game = GameLoop::new(LoopConfig::default(), world, systems)
        .with_clock(Box::new(clock.clone()));

    game.set_tick_observer(|tick, work| {
        println!("tick={} work_us={}", tick.0, work.as_micros());
    });
    clock.advance(Duration::from_millis(400));
    game.run_iteration();
    game.clear_tick_observer();

    let metrics = game.metrics();
    assert_eq!(metrics.ticks, 5);
    assert_eq!(metrics.skipped_ticks, 7);
    assert_eq!(metrics.lagging_iterations, 1);
    println!(
        "ticks={} skipped_ticks={} lagging_iterations={}",
        metrics.ticks, metrics.skipped_ticks, metrics.lagging_iterations
    );
}
```

## Monitoring a running host

The core exposes the counters, tick observer, and tracing events. `GameLoop::set_work_observer` adds measurements of individual ECS updates and named phase handlers. The [administrator client](admin.md) includes a collector that records five-second windows to SQLite and a dashboard that reads this history. The host attaches the collector to its running loop.

At 30 Hz, one scheduled interval is about 33.3 ms. If simulation work consistently approaches that interval, the host has less time for network processing. A rising `skipped_ticks` counter shows that the catch-up limit is being exceeded.

[Counter and warning implementation](../../backend/crates/crucible-loop/src/game_loop.rs) · [Counter definitions](../../backend/crates/crucible-loop/src/timestep.rs) · [Lag tests](../../backend/crates/crucible-loop/tests/heavy_lag.rs) · [Simulation loop](simulation-loop.md) · [Running a host](running.md)
