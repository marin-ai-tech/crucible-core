//! Component lifecycle and safe iteration scenarios for `specs/core/ecs`.

use std::collections::HashMap;

use crucible_ecs::{
    Component, ComponentAdd, ComponentInit, ComponentLifeStage, ComponentRemove, ComponentShutdown,
    ComponentStartup, EntityUid, System, SystemInit, Tick, World,
};

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

#[derive(Default)]
struct Tracked;
impl Component for Tracked {}

struct Other;
impl Component for Other {}

/// Records the lifecycle event and the component stage at dispatch.
fn record<E>(name: &'static str) -> impl Fn(&mut World, EntityUid, &mut E) + Send + Sync + 'static {
    move |world, uid, _| {
        let stage = world
            .component_stage::<Tracked>(uid)
            .expect("component remains accessible");
        log(world, format!("{name} {stage:?}"));
    }
}

struct LifecycleSystem;
impl System for LifecycleSystem {
    fn initialize(&mut self, init: &mut SystemInit<'_>) {
        init.subscribe::<Tracked, ComponentAdd, _>(record("add"));
        init.subscribe::<Tracked, ComponentInit, _>(record("init"));
        init.subscribe::<Tracked, ComponentStartup, _>(record("startup"));
        init.subscribe::<Tracked, ComponentShutdown, _>(record("shutdown"));
        init.subscribe::<Tracked, ComponentRemove, _>(record("remove"));
        // Lifecycle events for other components do not reach this subscription.
        init.subscribe::<Other, ComponentAdd, _>(|world, _, _| log(world, "other add"));
    }
}

fn lifecycle_world() -> World {
    let mut builder = World::builder();
    builder
        .insert_resource(Log::default())
        .add_system(LifecycleSystem);
    builder.build().unwrap().0
}

/// Scenario: lifecycle order during addition and removal.
#[test]
fn order_on_add_and_remove() {
    let mut world = lifecycle_world();
    world.set_current_tick(Tick(1));
    let e = world.spawn();
    world.add_component(e, Tracked).unwrap();
    assert_eq!(
        world.component_stage::<Tracked>(e),
        Some(ComponentLifeStage::Running)
    );

    for tick in 2..5 {
        world.set_current_tick(Tick(tick));
        world.end_tick();
    }
    world.set_current_tick(Tick(5));
    assert!(world.remove_component::<Tracked>(e));
    assert_eq!(world.component_stage::<Tracked>(e), None);
    assert!(!world.remove_component::<Tracked>(e));

    assert_eq!(
        lines(&world),
        vec![
            "add Adding",
            "init Initializing",
            "startup Starting",
            "shutdown Stopping",
            "remove Removing",
        ]
    );
}

#[test]
fn uninitialized_entity_defers_init_and_startup() {
    let mut world = lifecycle_world();
    let e = world.spawn_uninitialized();
    world.add_component(e, Tracked).unwrap();
    world.add_component(e, Other).unwrap();
    assert_eq!(
        world.component_stage::<Tracked>(e),
        Some(ComponentLifeStage::Added)
    );
    assert_eq!(lines(&world), vec!["add Adding", "other add"]);

    world.initialize_entity(e);
    assert_eq!(
        world.component_stage::<Tracked>(e),
        Some(ComponentLifeStage::Running)
    );
    world.delete(e);
    assert_eq!(
        lines(&world),
        vec![
            "add Adding",
            "other add",
            "init Initializing",
            "startup Starting",
            "shutdown Stopping",
            "remove Removing",
        ]
    );
}

#[test]
fn removal_inside_own_lifecycle_handler_is_safe() {
    struct SelfRemoving;
    impl System for SelfRemoving {
        fn initialize(&mut self, init: &mut SystemInit<'_>) {
            // Removing during startup skips shutdown because the component never ran.
            init.subscribe::<Tracked, ComponentStartup, _>(|world, uid, _| {
                assert!(world.remove_component::<Tracked>(uid));
            });
            init.subscribe::<Tracked, ComponentShutdown, _>(|_, _, _| {
                panic!("component never started")
            });
            init.subscribe::<Other, ComponentShutdown, _>(|world, uid, _| {
                // Repeated removal during shutdown is harmless.
                assert!(!world.remove_component::<Other>(uid));
                // Deleting the whole entity from a component handler is harmless too.
                assert!(world.delete(uid));
            });
        }
    }
    let mut builder = World::builder();
    builder.add_system(SelfRemoving);
    let (mut world, _) = builder.build().unwrap();
    let e = world.spawn();
    world.add_component(e, Tracked).unwrap();
    assert!(!world.has::<Tracked>(e));
    assert!(world.exists(e));

    world.add_component(e, Other).unwrap();
    assert!(world.remove_component::<Other>(e));
    assert!(!world.exists(e));
    assert_eq!(world.entity_count(), 0);
    let f = world.spawn();
    assert_eq!(world.component_stage::<Tracked>(f), None);
    assert_eq!(world.component_stage::<Other>(f), None);
}

// ── Removal during iteration ────────────────────────────────────────────────────────────

struct A(u32);
impl Component for A {}

/// Scenario: removal during iteration.
#[test]
fn removal_during_iteration() {
    let mut world = World::new();
    let mut initial = Vec::new();
    for i in 0..100 {
        let e = world.spawn();
        world.add_component(e, A(i)).unwrap();
        initial.push(e);
    }

    let mut visits: HashMap<EntityUid, u32> = HashMap::new();
    let mut query = world.query::<A>();
    let mut n = 0;
    while let Some(uid) = query.next(&world) {
        *visits.entry(uid).or_default() += 1;
        assert!(world.get::<A>(uid).unwrap().0 < 100);
        if n % 2 == 1 {
            assert!(world.remove_component::<A>(uid));
        }
        n += 1;
    }

    assert!(visits.values().all(|&v| v == 1));
    assert!(visits.keys().all(|uid| initial.contains(uid)));
    assert_eq!(visits.len(), 100);
    assert_eq!(world.query::<A>().len(), 50);
}

#[test]
fn iteration_survives_structural_changes() {
    struct B;
    impl Component for B {}

    let mut world = World::new();
    let mut initial = Vec::new();
    for i in 0..40 {
        let e = world.spawn();
        world.add_component(e, A(i)).unwrap();
        if i % 4 == 0 {
            world.add_component(e, B).unwrap();
        }
        initial.push(e);
    }
    let without_b = world.spawn();
    world.add_component(without_b, B).unwrap();

    let mut visited = Vec::new();
    let mut created = Vec::new();
    let mut query = world.query::<(A, B)>();
    assert_eq!(query.len(), 10);
    while let Some(uid) = query.next(&world) {
        visited.push(uid);
        // This query does not visit new entities with the full component set.
        let fresh = world.spawn();
        world.add_component(fresh, A(0)).unwrap();
        world.add_component(fresh, B).unwrap();
        created.push(fresh);
        // An entity gains its missing B component and remains outside the snapshot.
        world.add_component(without_b, A(0)).ok();
        // Remove the next snapshot entity and remove a component from another snapshot entity.
        let i = initial.iter().position(|&e| e == uid).unwrap();
        if let Some(&next) = initial.get(i + 4) {
            world.delete(next);
        }
        if let Some(&later) = initial.get(i + 8) {
            world.remove_component::<B>(later);
        }
    }
    assert!(visited.iter().all(|uid| !created.contains(uid)));
    assert!(!visited.contains(&without_b));
    let unique: std::collections::HashSet<_> = visited.iter().collect();
    assert_eq!(unique.len(), visited.len());
    assert!(!visited.is_empty() && visited.len() < 10);
}
