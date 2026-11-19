//! Advances the world and its systems one tick at a time.

use std::time::{Duration, Instant};

use crucible_ecs::{Systems, Tick, World};

use crate::rng::{SimRng, fresh_seed};
use crate::time::{SimTime, Timers, fire_due_timers};
use crate::timestep::{Clock, FixedTimestep, LoopConfig, LoopMetrics, Steps, SystemClock};

/// Stage outside system updates: input, systems, physics, removal, and replication.
///
/// [`GameLoop::add_phase`] registers phase handlers, including core modules registered through
/// `crucible-ext`. Handlers within a phase run in insertion order.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum TickPhase {
    /// Buffered input before system updates.
    Input,
    /// Physics after systems and the post-system hook, before queued entity removal.
    Physics,
    /// Server snapshot after all physics handlers and before entity removal.
    PostPhysics,
    /// Replication state after queued entities are removed.
    Replication,
}

/// Tick phase handler receiving the world and timestep in seconds.
pub type PhaseHandler = Box<dyn FnMut(&mut World, f32) + Send>;

type TickObserver = Box<dyn FnMut(Tick, Duration) + Send>;
type WorkObserver = Box<dyn FnMut(Option<TickPhase>, &'static str, Duration) + Send>;

/// Game loop that owns the world and systems and runs fixed-duration ticks.
///
/// Tick order:
/// 1. Advance the tick number (starting at 1) and [`SimTime`].
/// 2. Run due timers.
/// 3. Run [`TickPhase::Input`].
/// 4. Update systems in order with a step of one over the tick rate.
/// 5. Run the post-system hook passed to [`tick_with`](GameLoop::tick_with), such as round flow.
/// 6. Run [`TickPhase::Physics`].
/// 7. Run [`TickPhase::PostPhysics`].
/// 8. Finish the world tick by delivering deferred events and removing queued entities.
/// 9. Run [`TickPhase::Replication`].
///
/// World resources installed by the loop are [`SimTime`], [`Timers`], and [`SimRng`].
pub struct GameLoop {
    world: World,
    systems: Systems,
    phases: Vec<(TickPhase, &'static str, PhaseHandler)>,
    timestep: FixedTimestep,
    clock: Box<dyn Clock>,
    next_tick: Tick,
    frame_time: f32,
    metrics: LoopMetrics,
    tick_observer: Option<TickObserver>,
    work_observer: Option<WorkObserver>,
}

impl GameLoop {
    /// Creates a loop for a world and systems with a system clock and logs the random seed.
    pub fn new(config: LoopConfig, mut world: World, systems: Systems) -> Self {
        let seed = config.seed.unwrap_or_else(fresh_seed);
        tracing::info!(
            target: "crucible_loop",
            seed,
            tick_rate = config.tick_rate,
            "simulation random seed"
        );
        world.insert_resource(SimTime::new(config.tick_rate));
        world.insert_resource(Timers::default());
        world.insert_resource(SimRng::new(seed));
        world.set_current_tick(Tick::ZERO);
        let clock: Box<dyn Clock> = Box::new(SystemClock::new());
        let mut timestep = FixedTimestep::new(config.tick_rate, config.max_catch_up_ticks);
        timestep.reset(clock.now());
        GameLoop {
            world,
            systems,
            phases: Vec::new(),
            timestep,
            clock,
            next_tick: Tick(1),
            frame_time: 1.0 / config.tick_rate as f32,
            metrics: LoopMetrics::default(),
            tick_observer: None,
            work_observer: None,
        }
    }

    /// Replaces the clock and uses the new clock's current time as the origin.
    /// Tests can supply [`ManualClock`](crate::ManualClock).
    pub fn with_clock(mut self, clock: Box<dyn Clock>) -> Self {
        self.timestep.reset(clock.now());
        self.clock = clock;
        self
    }

    /// Adds a handler for tick `phase` under `name`.
    pub fn add_phase(
        &mut self,
        phase: TickPhase,
        name: &'static str,
        handler: impl FnMut(&mut World, f32) + Send + 'static,
    ) -> &mut Self {
        self.phases.push((phase, name, Box::new(handler)));
        self
    }

    /// Phase handler names in execution order.
    pub fn phase_handlers(&self, phase: TickPhase) -> Vec<&'static str> {
        self.phases
            .iter()
            .filter(|(p, _, _)| *p == phase)
            .map(|(_, n, _)| *n)
            .collect()
    }

    /// The world.
    pub fn world(&self) -> &World {
        &self.world
    }

    /// Mutable world access between ticks.
    pub fn world_mut(&mut self) -> &mut World {
        &mut self.world
    }

    /// Systems in execution order.
    pub fn systems(&self) -> &Systems {
        &self.systems
    }

    /// The last completed tick number, or 0 before the first tick.
    pub fn current_tick(&self) -> Tick {
        Tick(self.next_tick.0 - 1)
    }

    /// System timestep in seconds.
    pub fn frame_time(&self) -> f32 {
        self.frame_time
    }

    /// Loop metrics.
    pub fn metrics(&self) -> LoopMetrics {
        self.metrics
    }

    /// Measures each tick from the world clock update through replication.
    pub fn set_tick_observer(&mut self, observer: impl FnMut(Tick, Duration) + Send + 'static) {
        self.tick_observer = Some(Box::new(observer));
    }

    /// Disables tick duration measurement.
    pub fn clear_tick_observer(&mut self) {
        self.tick_observer = None;
    }

    /// Measures named phase handlers and regular ECS system updates.
    /// The phase is `None` for a regular system update.
    pub fn set_work_observer(
        &mut self,
        observer: impl FnMut(Option<TickPhase>, &'static str, Duration) + Send + 'static,
    ) {
        self.work_observer = Some(Box::new(observer));
    }

    /// Stops measuring individual systems and phase handlers.
    pub fn clear_work_observer(&mut self) {
        self.work_observer = None;
    }

    /// Runs a tick without a post-system hook.
    pub fn tick(&mut self) -> Tick {
        self.tick_with(&mut |_, _| {})
    }

    /// Runs one tick and calls `after_systems` after system updates, before the world tick ends.
    pub fn tick_with(&mut self, after_systems: &mut dyn FnMut(&mut World, f32)) -> Tick {
        let start = self.tick_observer.as_ref().map(|_| Instant::now());
        let tick = self.next_tick;
        self.next_tick = Tick(tick.0 + 1);
        self.world.set_current_tick(tick);
        if let Some(time) = self.world.resource_mut::<SimTime>() {
            time.set_tick(tick);
        }
        fire_due_timers(&mut self.world, tick);
        self.run_phase(TickPhase::Input);
        if let Some(observe) = &mut self.work_observer {
            self.systems.update_observed(
                &mut self.world,
                self.frame_time,
                Some(&mut |name, work| observe(None, name, work)),
            );
        } else {
            self.systems.update(&mut self.world, self.frame_time);
        }
        after_systems(&mut self.world, self.frame_time);
        self.run_phase(TickPhase::Physics);
        self.run_phase(TickPhase::PostPhysics);
        self.world.end_tick();
        self.run_phase(TickPhase::Replication);
        self.metrics.ticks += 1;
        if let (Some(start), Some(observer)) = (start, &mut self.tick_observer) {
            observer(tick, start.elapsed());
        }
        tick
    }

    fn run_phase(&mut self, phase: TickPhase) {
        for (p, name, handler) in &mut self.phases {
            if *p == phase {
                let start = self.work_observer.as_ref().map(|_| Instant::now());
                handler(&mut self.world, self.frame_time);
                if let (Some(start), Some(observe)) = (start, &mut self.work_observer) {
                    observe(Some(phase), name, start.elapsed());
                }
            }
        }
    }

    /// Runs `n` consecutive ticks without reading the clock, for tests and replays.
    pub fn run_ticks(&mut self, n: u32) {
        for _ in 0..n {
            self.tick();
        }
    }

    /// Runs a clock-driven iteration without a post-system hook.
    pub fn run_iteration(&mut self) -> Steps {
        self.run_iteration_with(&mut |_, _| {})
    }

    /// Runs ticks due by the clock, capped by the catch-up limit.
    /// Excess ticks are discarded, the dropped-tick metric increases, and the loop
    /// logs a warning.
    pub fn run_iteration_with(&mut self, after_systems: &mut dyn FnMut(&mut World, f32)) -> Steps {
        let steps = self.timestep.advance_to(self.clock.now());
        if steps.dropped > 0 {
            self.metrics.skipped_ticks += steps.dropped;
            self.metrics.lagging_iterations += 1;
            tracing::warn!(
                target: "crucible_loop",
                dropped = steps.dropped,
                limit = self.timestep.max_catch_up(),
                total_skipped = self.metrics.skipped_ticks,
                "server exceeded the catch-up limit and dropped ticks"
            );
        }
        for _ in 0..steps.run {
            self.tick_with(after_systems);
        }
        steps
    }

    /// Time to wait until the next tick according to the loop clock.
    pub fn until_next_tick(&self) -> Duration {
        self.timestep.until_next_tick()
    }

    /// Runs in real time while `keep_running` returns `true` after each iteration.
    /// Sleeps until the next tick between iterations.
    pub fn run_realtime(
        &mut self,
        after_systems: &mut dyn FnMut(&mut World, f32),
        keep_running: &mut dyn FnMut(&GameLoop) -> bool,
    ) {
        loop {
            self.run_iteration_with(after_systems);
            if !keep_running(self) {
                return;
            }
            std::thread::sleep(self.until_next_tick());
        }
    }

    /// Consumes the loop and returns its world and systems.
    pub fn into_parts(self) -> (World, Systems) {
        (self.world, self.systems)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    #[test]
    fn tick_observer_runs_after_all_handlers_and_can_be_cleared() {
        let (world, systems) = World::builder().build().unwrap();
        let mut game = GameLoop::new(LoopConfig::default(), world, systems);
        let order = Arc::new(Mutex::new(Vec::new()));
        for (phase, name) in [
            (TickPhase::Input, "input"),
            (TickPhase::Physics, "physics"),
            (TickPhase::PostPhysics, "post_physics"),
            (TickPhase::Replication, "replication"),
            (TickPhase::Replication, "agent_gameplay"),
        ] {
            let order = Arc::clone(&order);
            game.add_phase(phase, name, move |_, _| order.lock().unwrap().push(name));
        }
        let observed_order = Arc::clone(&order);
        game.set_tick_observer(move |tick, _work| {
            assert_eq!(tick, Tick(1));
            observed_order.lock().unwrap().push("observer");
        });

        game.tick_with(&mut |_, _| order.lock().unwrap().push("rounds"));
        game.clear_tick_observer();
        game.tick();

        assert_eq!(
            *order.lock().unwrap(),
            [
                "input",
                "rounds",
                "physics",
                "post_physics",
                "replication",
                "agent_gameplay",
                "observer",
                "input",
                "physics",
                "post_physics",
                "replication",
                "agent_gameplay",
            ]
        );
    }
}
