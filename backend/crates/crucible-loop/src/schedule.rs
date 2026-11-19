//! Runs selected systems at a lower frequency.

use crucible_ecs::{System, SystemInit, World};

/// Wraps a system that updates every `n` ticks with a timestep of `n` tick durations.
///
/// Calls the system on tick numbers divisible by `n`. Numbering starts at 1, so the cadence is
/// independent of registration time and remains the same in replays.
///
/// The wrapper owns the inner system's subscriptions and ordering constraints. Other systems
/// refer to it as `EveryN<S>`, for example `updates_after::<EveryN<S>>()`.
///
/// ```
/// use crucible_ecs::{System, World};
/// use crucible_loop::EveryN;
///
/// struct Slow;
/// impl System for Slow {
///     fn update(&mut self, _world: &mut World, frame_time: f32) {
///         assert!((frame_time - 0.1).abs() < 1e-6);
///     }
/// }
///
/// let mut builder = World::builder();
/// builder.add_system(EveryN::new(3, Slow));
/// ```
pub struct EveryN<S> {
    n: u32,
    inner: S,
}

impl<S: System> EveryN<S> {
    /// Wraps `inner` so it runs every `n` ticks, where `n` must be at least 1.
    pub fn new(n: u32, inner: S) -> Self {
        assert!(n >= 1, "system update interval must be at least one tick");
        EveryN { n, inner }
    }

    /// Period in ticks.
    pub fn period(&self) -> u32 {
        self.n
    }

    /// Inner system.
    pub fn inner(&self) -> &S {
        &self.inner
    }
}

impl<S: System> System for EveryN<S> {
    fn initialize(&mut self, init: &mut SystemInit<'_>) {
        self.inner.initialize(init);
    }

    fn update(&mut self, world: &mut World, frame_time: f32) {
        if world.current_tick().0.is_multiple_of(self.n) {
            self.inner.update(world, frame_time * self.n as f32);
        }
    }
}
