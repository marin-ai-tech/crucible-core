//! Fixed timestep with accumulated time, bounded catch-up, and a monotonic clock.
//!
//! This code is an independent implementation and does not adapt Robust's `GameTiming` or
//! `GameLoop`.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Default tick rate in ticks per second.
pub const DEFAULT_TICK_RATE: u32 = 30;

/// Default limit on catch-up ticks per loop iteration.
pub const DEFAULT_MAX_CATCH_UP_TICKS: u32 = 5;

const NANOS_PER_SEC: u128 = 1_000_000_000;

/// Game loop settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoopConfig {
    /// Tick rate in ticks per second (default 30).
    pub tick_rate: u32,
    /// Maximum ticks the loop runs in one iteration to catch up (default 5).
    /// Further skipped ticks are dropped and counted in the dropped-tick metric.
    pub max_catch_up_ticks: u32,
    /// Simulation random seed. With `None`, the loop chooses and logs a seed at startup.
    pub seed: Option<u64>,
}

impl Default for LoopConfig {
    fn default() -> Self {
        LoopConfig {
            tick_rate: DEFAULT_TICK_RATE,
            max_catch_up_ticks: DEFAULT_MAX_CATCH_UP_TICKS,
            seed: None,
        }
    }
}

impl LoopConfig {
    /// Sets `tick_rate` and keeps all other settings at their defaults.
    pub fn with_tick_rate(tick_rate: u32) -> Self {
        LoopConfig {
            tick_rate,
            ..LoopConfig::default()
        }
    }

    /// Tick duration rounded to the nearest nanosecond. [`SimTime`](crate::SimTime) tracks exact tick time.
    pub fn tick_duration(&self) -> Duration {
        tick_duration(self.tick_rate)
    }
}

pub(crate) fn tick_duration(tick_rate: u32) -> Duration {
    Duration::from_nanos((NANOS_PER_SEC / u128::from(tick_rate.max(1))) as u64)
}

/// Monotonic time source for the loop. Tests supply [`ManualClock`] to avoid sleeping.
pub trait Clock: Send {
    /// Elapsed time since an arbitrary origin. Never decreases.
    fn now(&self) -> Duration;
}

/// System monotonic clock.
pub struct SystemClock {
    start: Instant,
}

impl SystemClock {
    /// Starts the clock at the current instant.
    pub fn new() -> Self {
        SystemClock {
            start: Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        SystemClock::new()
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Duration {
        self.start.elapsed()
    }
}

/// Manual clock that moves only when [`advance`](ManualClock::advance) is called.
/// Clones share time, so tests can advance the loop clock even from inside a system.
#[derive(Clone, Default)]
pub struct ManualClock {
    nanos: Arc<AtomicU64>,
}

impl ManualClock {
    /// Starts the clock at zero.
    pub fn new() -> Self {
        ManualClock::default()
    }

    /// Advances the clock.
    pub fn advance(&self, by: Duration) {
        self.nanos.fetch_add(by.as_nanos() as u64, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Duration {
        Duration::from_nanos(self.nanos.load(Ordering::SeqCst))
    }
}

/// Ticks to run in the iteration, and ticks dropped.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Steps {
    /// Ticks to run, capped by the catch-up limit.
    pub run: u32,
    /// Excess ticks discarded above the catch-up limit.
    pub dropped: u64,
}

/// Fixed-step time accumulator.
///
/// Accumulates nanoseconds multiplied by tick rate. The reciprocal rate is exact,
/// so at 30 ticks per second, 10 seconds produce exactly 300 ticks.
#[derive(Clone, Debug)]
pub struct FixedTimestep {
    tick_rate: u32,
    max_catch_up: u32,
    accumulated: u128,
    last: Option<Duration>,
}

impl FixedTimestep {
    /// Sets `tick_rate` and the catch-up limit `max_catch_up` (minimum 1).
    pub fn new(tick_rate: u32, max_catch_up: u32) -> Self {
        assert!(tick_rate > 0, "tick rate must be greater than zero");
        FixedTimestep {
            tick_rate,
            max_catch_up: max_catch_up.max(1),
            accumulated: 0,
            last: None,
        }
    }

    /// Tick rate.
    pub fn tick_rate(&self) -> u32 {
        self.tick_rate
    }

    /// Maximum catch-up ticks per iteration.
    pub fn max_catch_up(&self) -> u32 {
        self.max_catch_up
    }

    /// Sets the origin for the next [`advance_to`](FixedTimestep::advance_to) call to `now`.
    pub fn reset(&mut self, now: Duration) {
        self.last = Some(now);
        self.accumulated = 0;
    }

    /// Accounts for elapsed time through `now` on the same clock and returns the ticks to run.
    /// The first call without [`reset`](FixedTimestep::reset) only records the origin.
    pub fn advance_to(&mut self, now: Duration) -> Steps {
        let elapsed = match self.last {
            None => Duration::ZERO,
            Some(last) => now.saturating_sub(last),
        };
        self.last = Some(now);
        self.advance(elapsed)
    }

    /// Accounts for `elapsed` time and returns the ticks to run.
    pub fn advance(&mut self, elapsed: Duration) -> Steps {
        self.accumulated += elapsed.as_nanos() * u128::from(self.tick_rate);
        let due = self.accumulated / NANOS_PER_SEC;
        self.accumulated %= NANOS_PER_SEC;
        let run = due.min(u128::from(self.max_catch_up)) as u32;
        let dropped = u64::try_from(due - u128::from(run)).unwrap_or(u64::MAX);
        Steps { run, dropped }
    }

    /// Time remaining until the next tick.
    pub fn until_next_tick(&self) -> Duration {
        let left = NANOS_PER_SEC - self.accumulated;
        Duration::from_nanos(left.div_ceil(u128::from(self.tick_rate)) as u64)
    }
}

/// Loop metrics.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct LoopMetrics {
    /// Ticks executed.
    pub ticks: u64,
    /// Ticks discarded above the catch-up limit.
    pub skipped_ticks: u64,
    /// Iterations that discarded ticks.
    pub lagging_iterations: u64,
}
