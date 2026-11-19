//! Simulation time and gameplay timers.

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use crucible_ecs::{Tick, World};

use crate::rng::{ChaCha8Rng, SimRng};

const NANOS_PER_SEC: u128 = 1_000_000_000;

/// Simulation time is the tick number multiplied by the tick duration. [`GameLoop`](crate::GameLoop)
/// maintains this world resource for gameplay code that needs time independent of the system clock.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SimTime {
    tick: Tick,
    tick_rate: u32,
}

impl SimTime {
    /// Time at tick 0 before the first tick, with the given `tick_rate`.
    pub fn new(tick_rate: u32) -> Self {
        assert!(tick_rate > 0, "tick rate must be greater than zero");
        SimTime {
            tick: Tick::ZERO,
            tick_rate,
        }
    }

    pub(crate) fn set_tick(&mut self, tick: Tick) {
        self.tick = tick;
    }

    /// Current tick.
    pub fn tick(&self) -> Tick {
        self.tick
    }

    /// Tick rate in ticks per second.
    pub fn tick_rate(&self) -> u32 {
        self.tick_rate
    }

    /// Tick duration.
    pub fn tick_duration(&self) -> Duration {
        crate::timestep::tick_duration(self.tick_rate)
    }

    /// Timestep supplied to systems, in seconds.
    pub fn frame_time(&self) -> f32 {
        1.0 / self.tick_rate as f32
    }

    /// Simulation time at the current tick.
    pub fn elapsed(&self) -> Duration {
        self.ticks_to_duration(self.tick.0)
    }

    /// Duration of `ticks` ticks.
    pub fn ticks_to_duration(&self, ticks: u32) -> Duration {
        let nanos = u128::from(ticks) * NANOS_PER_SEC / u128::from(self.tick_rate);
        Duration::from_nanos(nanos as u64)
    }

    /// Number of ticks covering `duration`, rounded up to at least one for nonzero durations.
    /// At 30 ticks per second, 10 seconds cover exactly 300 ticks.
    pub fn duration_to_ticks(&self, duration: Duration) -> u32 {
        let scaled = duration.as_nanos() * u128::from(self.tick_rate);
        u32::try_from(scaled.div_ceil(NANOS_PER_SEC)).unwrap_or(u32::MAX)
    }
}

/// Timer identifier.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct TimerId(u64);

type TimerCallback = Box<dyn FnOnce(&mut World) + Send + Sync>;

/// Gameplay timers based on simulation time. This world resource fires before systems each tick.
#[derive(Default)]
pub struct Timers {
    next_id: u64,
    queue: BTreeMap<(Tick, u64), TimerCallback>,
    due_by_id: HashMap<u64, Tick>,
}

impl Timers {
    /// Schedules a callback for tick `at`.
    pub fn schedule_at(
        &mut self,
        at: Tick,
        callback: impl FnOnce(&mut World) + Send + Sync + 'static,
    ) -> TimerId {
        self.next_id += 1;
        let id = self.next_id;
        self.queue.insert((at, id), Box::new(callback));
        self.due_by_id.insert(id, at);
        TimerId(id)
    }

    /// Cancels the timer. Returns `false` if it has already fired or been cancelled.
    pub fn cancel(&mut self, id: TimerId) -> bool {
        match self.due_by_id.remove(&id.0) {
            Some(at) => self.queue.remove(&(at, id.0)).is_some(),
            None => false,
        }
    }

    /// Tick when the pending timer will fire.
    pub fn due_tick(&self, id: TimerId) -> Option<Tick> {
        self.due_by_id.get(&id.0).copied()
    }

    /// Number of pending timers.
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// Whether there are no pending timers.
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    fn take_due(&mut self, tick: Tick) -> Vec<TimerCallback> {
        let later = self.queue.split_off(&(Tick(tick.0.saturating_add(1)), 0));
        let due = std::mem::replace(&mut self.queue, later);
        due.into_iter()
            .map(|((_, id), callback)| {
                self.due_by_id.remove(&id);
                callback
            })
            .collect()
    }
}

/// Runs timers due by `tick` in due-time order, then in insertion order.
/// A callback can schedule other timers. Timers due on this tick run during the same call.
pub(crate) fn fire_due_timers(world: &mut World, tick: Tick) {
    loop {
        let Some(timers) = world.resource_mut::<Timers>() else {
            return;
        };
        let due = timers.take_due(tick);
        if due.is_empty() {
            return;
        }
        for callback in due {
            callback(world);
        }
    }
}

/// World access to simulation time, timers, and randomness installed by [`GameLoop`](crate::GameLoop).
pub trait WorldTimeExt {
    /// Simulation time. Panics if the world is running without a game loop.
    fn sim_time(&self) -> SimTime;

    /// Starts a simulation timer for `delay`. Its callback runs at the beginning of the current tick
    /// plus the delay in ticks, with the next tick as the earliest possible start.
    fn start_timer(
        &mut self,
        delay: Duration,
        callback: impl FnOnce(&mut World) + Send + Sync + 'static,
    ) -> TimerId;

    /// Cancels a timer.
    fn cancel_timer(&mut self, id: TimerId) -> bool;

    /// Named random stream for the simulation.
    fn rng_stream(&mut self, name: &str) -> &mut ChaCha8Rng;
}

impl WorldTimeExt for World {
    fn sim_time(&self) -> SimTime {
        *self
            .resource::<SimTime>()
            .expect("SimTime requires a world running under a game loop")
    }

    fn start_timer(
        &mut self,
        delay: Duration,
        callback: impl FnOnce(&mut World) + Send + Sync + 'static,
    ) -> TimerId {
        let time = self.sim_time();
        let at = Tick(
            time.tick()
                .0
                .saturating_add(time.duration_to_ticks(delay).max(1)),
        );
        if self.resource::<Timers>().is_none() {
            self.insert_resource(Timers::default());
        }
        self.resource_mut::<Timers>()
            .expect("timer was just inserted")
            .schedule_at(at, callback)
    }

    fn cancel_timer(&mut self, id: TimerId) -> bool {
        self.resource_mut::<Timers>()
            .is_some_and(|timers| timers.cancel(id))
    }

    fn rng_stream(&mut self, name: &str) -> &mut ChaCha8Rng {
        self.resource_mut::<SimRng>()
            .expect("SimRng requires a world running under a game loop")
            .stream(name)
    }
}
