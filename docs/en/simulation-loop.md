# How the simulation loop works

The core advances an authoritative ECS world at a fixed rate. The host owns the process, and a protocol adapter, when present, converts network messages to core inputs and core updates to outgoing messages. A tick applies the inputs and timers that are due and updates the world. With replication enabled, the tick ends by preparing state for each player.

## Host iterations and simulation ticks

A host calls `ServerRuntime::run_iteration` to advance the simulation with its round flow, or `GameLoop::run_iteration` to drive a world directly. The loop reads its monotonic clock, accumulates elapsed time, and decides how many ticks are due. One host iteration can run zero, one, or several ticks. A networked host can poll its adapter between iterations, submit accepted input and acknowledgments, and drain prepared updates for encoding and transmission.

`LoopConfig` defaults to **30 ticks per second** with a fixed system timestep of `1 / 30` seconds and a maximum of **five catch-up ticks per iteration**. The accumulator carries forward the fractional tick. When more whole ticks are due than the configured limit allows, the loop runs up to the limit and records the excess as dropped. For example, 400 ms of elapsed time from a fresh clock gives 12 due ticks at 30 Hz. The default catch-up limit runs five and drops seven. Simulation time advances by the five executed ticks.

`GameLoop::until_next_tick` tells the host how long the accumulator has left until the next step. The [host example](extensions.md#build-and-connect-the-module) sleeps between iterations. `ManualClock` lets a test advance elapsed time explicitly, while `run_ticks(n)` directly advances a fixed number of ticks for a replay or a test. [Clock and accumulator](../../backend/crates/crucible-loop/src/timestep.rs) · [Fixed-step tests](../../backend/crates/crucible-loop/tests/timing.rs) · [Catch-up test](../../backend/crates/crucible-loop/tests/heavy_lag.rs)

## What runs on each tick

[`GameLoop::tick_with`](../../backend/crates/crucible-loop/src/game_loop.rs) owns this order. Steps 3, 6, 7, and 9 have named registration points. See the [tick diagram](architecture.md#simulation-ticks) for the sequence at a glance.

| Step | Where the work runs | What it reads, calculates, or writes |
| --- | --- | --- |
| 1. Advance simulation time | `GameLoop` and `SimTime` | Sets the current tick on the world and its time resource. Changes can now be recorded against this tick, and systems receive the fixed timestep. |
| 2. Fire due timers | `crucible-loop` timer queue | Runs callbacks due by the current tick, in deadline order and then insertion order. A callback can modify the world. |
| 3. Apply input | `TickPhase::Input` | With replication enabled, reads due commands and events from the input buffer, checks the current player session and any bound connection or control, and dispatches accepted inputs to the controlled entity. |
| 4. Update systems | `crucible-ecs::Systems::update` | Runs registered systems sequentially in the order resolved when building the world. Systems read or modify components and resources and can raise events or queue work for the end of the tick. |
| 5. Advance the round | Post-system hook in `ServerRuntime` | Updates `RoundFlow` with the current world and fixed step. The configured round flow can advance the lobby countdown, transition phases, and run extension-provided rules. A host driving `GameLoop` directly chooses its own hook. |
| 6. Calculate physics | `TickPhase::Physics` | With the physics module enabled, synchronizes bodies, fixtures, transforms, and movement inputs into the per-map physics worlds. Steps those worlds, writes positions, rotations, and velocities back to ECS, marks changed state, and raises contact events. |
| 7. Read the physics result | `TickPhase::PostPhysics` | Runs registered handlers against the updated world. This is the point for rules that need final positions from the physics step while queued entities still exist. |
| 8. Finish the world tick | `World::end_tick` | Delivers deferred events and removes queued entities. It continues processing work added by those handlers until both queues are empty. |
| 9. Prepare player updates | `TickPhase::Replication` | With replication enabled, syncs viewers with current sessions and builds per-player state and events. Visibility, component policy, entry budgets, acknowledgments, and change ticks determine the result. The adapter can then drain these updates from `OutgoingUpdates`. |

[Timer execution](../../backend/crates/crucible-loop/src/time.rs) · [World cleanup](../../backend/crates/crucible-ecs/src/world.rs) · [Runtime and round flow wiring](../../backend/crates/crucible-ext/src/launch.rs)

## Follow a movement input

1. The adapter submits a movement command with a client tick, sequence number, and position within the tick to `ReplicationState`.
2. At the start of the due tick, the replication input handler raises `InputCommandReceived`. The physics module's subscriber maps the movement function to a button and records it in the entity's `InputMover`. Future input stays in the buffer. Late input that is accepted is applied on the current tick.
3. The physics phase reads that movement intent with the body's configuration and the tile under it. The solver applies acceleration, friction, and collision handling. Updated position and velocity are written back to ECS before post-physics rules run.
4. After deferred events and deletions are processed, replication builds a full frame or a delta from the viewer's acknowledged tick. The host picks up the resulting update for its adapter and client.

[Input and replication wiring](../../backend/crates/crucible-replication/src/module.rs) · [Movement processing](../../backend/crates/crucible-physics/src/ecs.rs) · [Input and deletion phase test](../../backend/crates/crucible-replication/tests/module.rs)

## Where to add behavior

Use `RegisterContext::system` for normal ECS updates and `RegisterContext::tick_phase` for work that needs one of the four named tick phases. The launcher registers modules in dependency order, then the active extension. Handlers in a phase run in registration order. Ordinary systems have their own ordering, which the world builder resolves from their `updates_before` and `updates_after` constraints. [Registration API](../../backend/crates/crucible-ext/src/register.rs) · [Extend the engine](extensions.md)

For work duration, dropped ticks, and host instrumentation, see [Loop metrics](metrics.md).
