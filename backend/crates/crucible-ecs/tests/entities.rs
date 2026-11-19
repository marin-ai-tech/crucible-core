//! Entity identity, network identifiers, immediate structural changes, and deferred removal
//! scenarios for `specs/core/ecs`.

use std::collections::HashSet;

use crucible_ecs::{
    Component, ComponentError, ComponentRemove, ComponentShutdown, EntityLifeStage, EntityUid,
    NetEntity, System, SystemInit, World,
};

#[derive(Debug, PartialEq)]
struct Health(i32);
impl Component for Health {}

/// Call log for checking ordering.
#[derive(Default)]
struct Log(Vec<String>);

fn log(world: &mut World, line: impl Into<String>) {
    world
        .resource_mut::<Log>()
        .expect("log")
        .0
        .push(line.into());
}

fn lines(world: &World) -> Vec<String> {
    world.resource::<Log>().expect("log").0.clone()
}

/// Scenario: stale identifier after removal.
#[test]
fn stale_id_after_deletion() {
    let mut world = World::new();
    let a = world.spawn();
    world.add_component(a, Health(10)).unwrap();
    world.queue_delete(a);
    world.end_tick();

    let b = world.spawn();
    world.add_component(b, Health(50)).unwrap();
    assert_eq!(a.index(), b.index(), "B reused the same internal index");
    assert_ne!(a, b);

    // An existence check for A returns false.
    assert!(!world.exists(a));
    assert!(world.exists(b));
    assert_eq!(world.net_entity(a), None);

    // Reads and writes through A leave B's components alone.
    assert_eq!(world.get::<Health>(a), None);
    assert!(world.get_mut::<Health>(a).is_none());
    assert!(!world.has::<Health>(a));
    assert_eq!(
        world.add_component(a, Health(-1)),
        Err(ComponentError::EntityNotFound(a))
    );
    assert!(!world.remove_component::<Health>(a));
    assert!(!world.dirty::<Health>(a));
    assert!(!world.queue_delete(a));
    assert!(!world.delete(a));
    world.raise(a, &mut ());
    assert_eq!(world.get::<Health>(b), Some(&Health(50)));
    assert_eq!(world.get::<Health>(b).unwrap().0, 50);
    assert!(world.exists(b));
}

/// Scenario: network identifiers are never reused.
#[test]
fn net_entity_is_never_reused() {
    let mut world = World::new();
    let mut issued = HashSet::new();
    let mut alive = Vec::new();
    for i in 0..10_000 {
        let uid = world.spawn();
        let net = world.net_entity(uid).unwrap();
        assert_ne!(net.raw(), 0, "zero is never assigned");
        assert!(
            issued.insert(net),
            "network identifier {net} was assigned twice"
        );
        assert_eq!(world.entity_by_net(net), Some(uid));
        alive.push(uid);
        // Remove entities in batches both immediately and through the queue to reuse indices.
        if i % 3 == 0 {
            let victim = alive.remove(0);
            world.delete(victim);
        } else if i % 3 == 1 {
            let victim = alive.remove(0);
            world.queue_delete(victim);
            world.end_tick();
        }
    }
    for uid in alive {
        world.delete(uid);
    }
    assert_eq!(world.entity_count(), 0);
    assert_eq!(issued.len(), 10_000);
    // Indices were reused. Network identifiers remained unique.
    let fresh = world.spawn();
    assert!(fresh.index() < 10_000);
    assert!(!issued.contains(&world.net_entity(fresh).unwrap()));
}

#[test]
fn world_can_move_between_threads_and_be_shared() {
    fn assert_send_sync<T: Send + Sync>() {}
    fn assert_send<T: Send>() {}
    assert_send_sync::<World>();
    assert_send::<crucible_ecs::Systems>();
}

/// Scenario: unknown network identifier.
#[test]
fn unknown_net_entity() {
    let mut world = World::new();
    let kept = world.spawn();
    world.add_component(kept, Health(3)).unwrap();
    let gone = world.spawn();
    let gone_net = world.net_entity(gone).unwrap();
    world.delete(gone);

    let before_entities = world.entities();
    let before_count = world.entity_count();

    assert_eq!(world.entity_by_net(NetEntity::from_raw(999_999)), None);
    assert_eq!(world.entity_by_net(NetEntity::from_raw(0)), None);
    assert_eq!(
        world.entity_by_net(gone_net),
        None,
        "the removed entity cannot be found"
    );

    // The world state is unchanged.
    assert_eq!(world.entities(), before_entities);
    assert_eq!(world.entity_count(), before_count);
    assert_eq!(world.get::<Health>(kept), Some(&Health(3)));
    let next = world.spawn();
    assert_eq!(world.net_entity(next).unwrap().raw(), gone_net.raw() + 1);
}

// ── Added components are visible immediately ────────────────────────────────────────────

struct Trigger;
impl Component for Trigger {}

struct Late;
impl Component for Late {}

struct Poke;
struct Touch;

struct ImmediateSystem;
impl System for ImmediateSystem {
    fn initialize(&mut self, init: &mut SystemInit<'_>) {
        init.subscribe::<Trigger, Poke, _>(|world, uid, _| {
            world.add_component(uid, Late).unwrap();
            world.raise(uid, &mut Touch);
            log(world, "after raising Touch");
        });
        init.subscribe::<Late, Touch, _>(|world, _, _| log(world, "(Late, Touch)"));
    }
}

/// Scenario: added components are visible immediately.
#[test]
fn added_component_is_visible_immediately() {
    let mut builder = World::builder();
    builder
        .insert_resource(Log::default())
        .add_system(ImmediateSystem);
    let (mut world, _) = builder.build().unwrap();
    let e = world.spawn();
    world.add_component(e, Trigger).unwrap();

    world.raise(e, &mut Poke);

    assert_eq!(lines(&world), vec!["(Late, Touch)", "after raising Touch"]);
}

// ── Removal at the end of the tick ──────────────────────────────────────────────────────

struct Target(EntityUid);

struct Deleter;
impl System for Deleter {
    fn update(&mut self, world: &mut World, _: f32) {
        let target = world.resource::<Target>().unwrap().0;
        assert!(world.queue_delete(target));
        assert!(world.queue_delete(target), "repeated queueing is harmless");
    }
}

struct Observer;
impl System for Observer {
    fn initialize(&mut self, init: &mut SystemInit<'_>) {
        init.updates_after::<Deleter>();
        init.subscribe::<Health, ComponentShutdown, _>(|world, uid, _| {
            let stage = world.life_stage(uid);
            log(world, format!("shutdown {stage:?}"));
        });
        init.subscribe::<Health, ComponentRemove, _>(|world, uid, _| {
            let present = world.has::<Health>(uid);
            log(world, format!("remove, component accessible: {present}"));
        });
    }

    fn update(&mut self, world: &mut World, _: f32) {
        let target = world.resource::<Target>().unwrap().0;
        let line = format!(
            "exists: {}, removing: {}",
            world.exists(target),
            world.is_queued_for_deletion(target)
        );
        log(world, line);
        assert!(world.terminating_or_deleted(target));
        assert_eq!(world.get::<Health>(target), Some(&Health(1)));
    }
}

/// Scenario: removal at the end of the tick.
#[test]
fn deletion_at_end_of_tick() {
    let mut builder = World::builder();
    // Add the systems in reverse order. The after constraint determines update order.
    builder
        .add_system(Observer)
        .add_system(Deleter)
        .insert_resource(Log::default());
    let (mut world, mut systems) = builder.build().unwrap();
    let target = world.spawn();
    world.add_component(target, Health(1)).unwrap();
    world.insert_resource(Target(target));

    systems.update(&mut world, 1.0 / 30.0);
    assert!(
        world.exists(target),
        "the entity exists until the end of the tick"
    );
    world.end_tick();

    assert!(!world.exists(target), "the entity is gone after the tick");
    assert!(!world.has::<Health>(target));
    assert_eq!(
        lines(&world),
        vec![
            "exists: true, removing: true",
            "shutdown Some(Terminating)",
            "remove, component accessible: true",
        ]
    );
    // A second end_tick call leaves previously removed entities alone.
    world.end_tick();
    assert_eq!(world.entity_count(), 0);
}

#[test]
fn queued_events_are_delivered_at_end_of_tick() {
    struct Ping(u32);
    struct Pinger;
    impl System for Pinger {
        fn initialize(&mut self, init: &mut SystemInit<'_>) {
            init.subscribe_broadcast::<Ping, _>(|world, ping| {
                log(world, format!("ping {}", ping.0))
            });
            init.subscribe::<Health, Ping, _>(|world, _, ping| {
                log(world, format!("directed {}", ping.0))
            });
        }
    }
    let mut builder = World::builder();
    builder.add_system(Pinger).insert_resource(Log::default());
    let (mut world, _) = builder.build().unwrap();
    let e = world.spawn();
    world.add_component(e, Health(1)).unwrap();
    world.queue_broadcast(Ping(1));
    world.queue_raise(e, Ping(2));
    assert!(
        lines(&world).is_empty(),
        "delivery is deferred until the end of the tick"
    );
    world.end_tick();
    assert_eq!(lines(&world), vec!["ping 1", "directed 2"]);
}

#[test]
fn immediate_delete_and_entity_stages() {
    let mut world = World::new();
    let e = world.spawn_uninitialized();
    assert_eq!(world.life_stage(e), Some(EntityLifeStage::PreInit));
    assert!(world.initialize_entity(e));
    assert!(
        !world.initialize_entity(e),
        "repeated initialization has no effect"
    );
    assert_eq!(world.life_stage(e), Some(EntityLifeStage::Initialized));
    world.queue_delete(e);
    assert!(
        world.delete(e),
        "immediate deletion clears the queued deletion too"
    );
    world.end_tick();
    assert_eq!(world.entity_count(), 0);
}
