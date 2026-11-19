//! Severe lag scenario for `specs/core/simulation-loop`.
//!
//! This separate test binary has one test with its own `tracing` subscriber because the process
//! shares the call-site interest cache, as in `crucible-ecs/tests/event_depth.rs`.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use crucible_ecs::World;
use crucible_loop::{GameLoop, LoopConfig, ManualClock};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Metadata, Subscriber};

/// Counts loop warnings.
struct WarnCounter(Arc<AtomicUsize>);

impl Subscriber for WarnCounter {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }
    fn record(&self, _: &Id, _: &Record<'_>) {}
    fn record_follows_from(&self, _: &Id, _: &Id) {}
    fn event(&self, event: &Event<'_>) {
        if *event.metadata().level() == Level::WARN && event.metadata().target() == "crucible_loop"
        {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    fn enter(&self, _: &Id) {}
    fn exit(&self, _: &Id) {}
}

/// Scenario: severe lag.
#[test]
fn heavy_lag() {
    let warnings = Arc::new(AtomicUsize::new(0));
    tracing::subscriber::with_default(WarnCounter(warnings.clone()), || {
        let (world, systems) = World::builder().build().unwrap();
        let clock = ManualClock::new();
        let config = LoopConfig {
            tick_rate: 30,
            max_catch_up_ticks: 5,
            seed: Some(7),
        };
        let mut game = GameLoop::new(config, world, systems).with_clock(Box::new(clock.clone()));

        // The server stalls for one second: 30 ticks at a catch-up limit of five.
        clock.advance(Duration::from_secs(1));
        let steps = game.run_iteration();
        assert_eq!(steps.run, 5, "no more than the catch-up limit runs");
        assert_eq!(steps.dropped, 25, "excess ticks are dropped");
        assert_eq!(
            game.metrics().skipped_ticks,
            25,
            "the dropped-tick metric increases"
        );
        assert_eq!(game.current_tick().0, 5);
        assert_eq!(
            warnings.load(Ordering::SeqCst),
            1,
            "the logger receives one warning"
        );

        // The excess is discarded, so another iteration without elapsed time runs no ticks.
        assert_eq!(game.run_iteration().run, 0);
    });
}
