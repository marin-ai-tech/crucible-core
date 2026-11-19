//! Unbounded event recursion scenario for `specs/core/ecs`.
//!
//! This binary contains a single test with its own `tracing` subscriber. The process shares
//! the tracing call-site interest cache. A concurrent test raising the same error without
//! a subscriber could prevent the counter from seeing the record.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crucible_ecs::{Component, DEFAULT_MAX_EVENT_DEPTH, System, SystemInit, World};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Metadata, Subscriber};

/// A `tracing` subscriber that counts error-level records.
struct ErrorCounter(Arc<AtomicUsize>);

impl Subscriber for ErrorCounter {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }
    fn record(&self, _: &Id, _: &Record<'_>) {}
    fn record_follows_from(&self, _: &Id, _: &Id) {}
    fn event(&self, event: &Event<'_>) {
        if *event.metadata().level() == Level::ERROR {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    fn enter(&self, _: &Id) {}
    fn exit(&self, _: &Id) {}
}

struct Echo;
impl Component for Echo {}

struct Ping;
struct Pong;

#[derive(Default)]
struct Calls {
    ping: u32,
    pong: u32,
}

struct Recursive;
impl System for Recursive {
    fn initialize(&mut self, init: &mut SystemInit<'_>) {
        // Each invocation raises the same event on the same entity with no exit condition.
        init.subscribe::<Echo, Ping, _>(|world, uid, ping| {
            world.resource_mut::<Calls>().unwrap().ping += 1;
            world.raise(uid, ping);
        });
        init.subscribe::<Echo, Pong, _>(|world, _, _| {
            world.resource_mut::<Calls>().unwrap().pong += 1
        });
    }

    fn update(&mut self, world: &mut World, _: f32) {
        let uid = world.resource::<Source>().unwrap().0;
        world.raise(uid, &mut Ping);
        // The tick continues to dispatch later events normally.
        world.raise(uid, &mut Pong);
    }
}

struct Source(crucible_ecs::EntityUid);

/// Scenario: unbounded event recursion.
#[test]
fn infinite_event_recursion() {
    let errors = Arc::new(AtomicUsize::new(0));
    tracing::subscriber::with_default(ErrorCounter(errors.clone()), || {
        let mut builder = World::builder();
        builder
            .insert_resource(Calls::default())
            .add_system(Recursive);
        let (mut world, mut systems) = builder.build().unwrap();
        assert_eq!(world.max_event_depth(), DEFAULT_MAX_EVENT_DEPTH);
        assert_eq!(DEFAULT_MAX_EVENT_DEPTH, 64);
        let e = world.spawn();
        world.add_component(e, Echo).unwrap();
        world.insert_resource(Source(e));

        systems.update(&mut world, 1.0 / 30.0);
        world.end_tick();

        let calls = world.resource::<Calls>().unwrap();
        assert_eq!(
            calls.ping, 64,
            "only the deepest dispatch (the 65th) is stopped"
        );
        assert_eq!(calls.pong, 1, "the server continues the tick");
        assert_eq!(world.event_depth_overflows(), 1);
        assert_eq!(
            errors.load(Ordering::SeqCst),
            1,
            "one error-level record was logged"
        );

        // The depth counter returns to zero, so the next tick behaves the same.
        systems.update(&mut world, 1.0 / 30.0);
        assert_eq!(world.resource::<Calls>().unwrap().ping, 128);
        assert_eq!(world.event_depth_overflows(), 2);

        // The limit can be set at construction or changed at runtime.
        event_depth_limit_is_configurable();
        assert_eq!(errors.load(Ordering::SeqCst), 4);
    });
}

fn event_depth_limit_is_configurable() {
    let mut builder = World::builder();
    builder
        .insert_resource(Calls::default())
        .add_system(Recursive)
        .max_event_depth(8);
    let (mut world, mut systems) = builder.build().unwrap();
    let e = world.spawn();
    world.add_component(e, Echo).unwrap();
    world.insert_resource(Source(e));
    systems.update(&mut world, 1.0 / 30.0);
    assert_eq!(world.resource::<Calls>().unwrap().ping, 8);
    world.set_max_event_depth(3);
    systems.update(&mut world, 1.0 / 30.0);
    assert_eq!(world.resource::<Calls>().unwrap().ping, 11);
}
