//! Fixed ticks, simulation time, randomness, and slower updates for `specs/core/simulation-loop`.
//! A manual clock avoids sleeping.

use std::time::Duration;

use crucible_ecs::{System, World};
use crucible_loop::{EveryN, GameLoop, LoopConfig, ManualClock, RngCore, WorldTimeExt};

#[derive(Default)]
struct Log(Vec<(u32, f32)>);

/// Records (tick, step) and simulates work at `slow_tick` by advancing the clock by `slow_by`.
struct Recorder {
    clock: ManualClock,
    slow_tick: u32,
    slow_by: Duration,
}

impl System for Recorder {
    fn update(&mut self, world: &mut World, frame_time: f32) {
        let tick = world.current_tick().0;
        world
            .resource_mut::<Log>()
            .unwrap()
            .0
            .push((tick, frame_time));
        if tick == self.slow_tick {
            self.clock.advance(self.slow_by);
        }
    }
}

fn game_with(system: impl System, config: LoopConfig, clock: &ManualClock) -> GameLoop {
    let mut builder = World::builder();
    builder.insert_resource(Log::default()).add_system(system);
    let (world, systems) = builder.build().unwrap();
    GameLoop::new(config, world, systems).with_clock(Box::new(clock.clone()))
}

/// Scenario: constant step under load.
#[test]
fn constant_step_under_load() {
    let clock = ManualClock::new();
    let config = LoopConfig {
        tick_rate: 30,
        max_catch_up_ticks: 5,
        seed: Some(1),
    };
    // Tick 3 takes four tick budgets (133 ms at a 33 ms budget).
    let recorder = Recorder {
        clock: clock.clone(),
        slow_tick: 3,
        slow_by: Duration::from_nanos(4 * 1_000_000_000 / 30 + 1),
    };
    let mut game = game_with(recorder, config.clone(), &clock);

    clock.advance(Duration::from_millis(100));
    let first = game.run_iteration();
    assert_eq!((first.run, first.dropped), (3, 0));

    // A backlog of four fits within the catch-up limit of five in one iteration.
    let second = game.run_iteration();
    assert_eq!((second.run, second.dropped), (4, 0));
    assert!(second.run <= config.max_catch_up_ticks);
    assert_eq!(game.metrics().skipped_ticks, 0);

    let log = &game.world().resource::<Log>().unwrap().0;
    assert_eq!(
        log.iter().map(|(t, _)| *t).collect::<Vec<_>>(),
        vec![1, 2, 3, 4, 5, 6, 7],
        "consecutive ticks start at 1"
    );
    let step = 1.0 / 30.0;
    for (tick, frame_time) in log {
        assert_eq!(
            *frame_time, step,
            "tick {tick}: timestep is independent of load"
        );
    }
}

#[derive(Default)]
struct Fired(Vec<u32>);

struct StartTimerAt(u32);
impl System for StartTimerAt {
    fn update(&mut self, world: &mut World, _: f32) {
        if world.current_tick().0 == self.0 {
            world.start_timer(Duration::from_secs(10), |world| {
                let tick = world.current_tick().0;
                world.resource_mut::<Fired>().unwrap().0.push(tick);
            });
        }
    }
}

/// Scenario: simulation-time timer.
#[test]
fn timer_by_simulation_time() {
    let mut builder = World::builder();
    builder
        .insert_resource(Fired::default())
        .add_system(StartTimerAt(5));
    let (world, systems) = builder.build().unwrap();
    // Ticks advance without consulting the system clock.
    let mut game = GameLoop::new(LoopConfig::with_tick_rate(30), world, systems);

    game.run_ticks(304);
    assert!(game.world().resource::<Fired>().unwrap().0.is_empty());
    game.run_ticks(10);
    assert_eq!(
        game.world().resource::<Fired>().unwrap().0,
        vec![5 + 300],
        "a 10-second timer at 30 Hz fires exactly 300 ticks after it starts"
    );
    assert_eq!(
        game.world().sim_time().elapsed(),
        Duration::from_secs(314) / 30
    );
}

#[derive(Default)]
struct Draws(Vec<u64>);

/// One mechanic reads its stream every tick. Commands specify the ticks on which another mechanic
/// also reads its own stream.
struct Mechanic {
    commands: Vec<u32>,
}
impl System for Mechanic {
    fn update(&mut self, world: &mut World, _: f32) {
        let tick = world.current_tick().0;
        if self.commands.contains(&tick) {
            world.rng_stream("other").next_u64();
        }
        let value = world.rng_stream("mechanic").next_u64();
        world.resource_mut::<Draws>().unwrap().0.push(value);
    }
}

fn run_simulation(seed: u64, commands: &[u32]) -> Vec<u64> {
    let mut builder = World::builder();
    builder
        .insert_resource(Draws::default())
        .add_system(Mechanic {
            commands: commands.to_vec(),
        });
    let (world, systems) = builder.build().unwrap();
    let config = LoopConfig {
        seed: Some(seed),
        ..LoopConfig::default()
    };
    let mut game = GameLoop::new(config, world, systems);
    game.run_ticks(50);
    std::mem::take(&mut game.world_mut().resource_mut::<Draws>().unwrap().0)
}

/// Scenario: replay with the same random seed.
#[test]
fn replay_with_same_seed() {
    let commands = [3, 7, 20];
    let first = run_simulation(42, &commands);
    let second = run_simulation(42, &commands);
    assert_eq!(first.len(), 50);
    assert_eq!(
        first, second,
        "the same seed and commands reproduce the sequence"
    );
    assert_ne!(
        first,
        run_simulation(43, &commands),
        "a different seed produces a different sequence"
    );
    assert_eq!(
        first,
        run_simulation(42, &[1, 2]),
        "the mechanic's stream is independent of calls to other streams"
    );
}

#[derive(Default)]
struct Calls(Vec<(u32, f32)>);

struct Slow;
impl System for Slow {
    fn update(&mut self, world: &mut World, frame_time: f32) {
        let tick = world.current_tick().0;
        world
            .resource_mut::<Calls>()
            .unwrap()
            .0
            .push((tick, frame_time));
    }
}

/// Scenario: subsystem with reduced update frequency.
#[test]
fn subsystem_at_reduced_rate() {
    let mut builder = World::builder();
    builder
        .insert_resource(Calls::default())
        .add_system(EveryN::new(3, Slow));
    let (world, systems) = builder.build().unwrap();
    let mut game = GameLoop::new(LoopConfig::with_tick_rate(30), world, systems);
    game.run_ticks(30);

    let calls = &game.world().resource::<Calls>().unwrap().0;
    assert_eq!(calls.len(), 10, "30 ticks produce 10 calls");
    let expected_step = 3.0 * game.frame_time();
    for (i, (tick, frame_time)) in calls.iter().enumerate() {
        assert_eq!(*tick, 3 * (i as u32 + 1));
        assert!(
            (frame_time - expected_step).abs() < 1e-6,
            "the step spans three tick durations"
        );
    }
}
