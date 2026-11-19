//! Directed and broadcast event scenarios for `specs/core/ecs` covering ordering
//! and cancellation.

use crucible_ecs::{BuildError, Cancellable, Component, System, SystemInit, World};

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

struct X;
impl Component for X {}
struct Y;
impl Component for Y {}
struct Z;
impl Component for Z {}

#[derive(Default)]
struct E {
    value: i32,
}

// ── Delivery by component ───────────────────────────────────────────────────────────────

struct ComponentRouting;
impl System for ComponentRouting {
    fn initialize(&mut self, init: &mut SystemInit<'_>) {
        init.subscribe::<X, E, _>(|world, _, _| log(world, "(X, E)"));
        init.subscribe::<Z, E, _>(|world, _, _| log(world, "(Z, E)"));
        init.subscribe::<Y, (), _>(|world, _, _| log(world, "(Y, ())"));
    }
}

/// Scenario: delivery by component.
#[test]
fn delivery_by_component() {
    let mut builder = World::builder();
    builder
        .insert_resource(Log::default())
        .add_system(ComponentRouting);
    let (mut world, _) = builder.build().unwrap();
    let e = world.spawn();
    world.add_component(e, X).unwrap();
    world.add_component(e, Y).unwrap();

    world.raise(e, &mut E::default());

    assert_eq!(lines(&world), vec!["(X, E)"]);
}

// ── Returning a result through an event ─────────────────────────────────────────────────

struct Writer;
impl System for Writer {
    fn initialize(&mut self, init: &mut SystemInit<'_>) {
        init.subscribe::<X, E, _>(|_, _, ev| ev.value = 42);
        init.subscribe_broadcast::<E, _>(|_, ev| ev.value += 1);
    }
}

/// Scenario: returning a result through an event.
#[test]
fn result_via_event() {
    let mut builder = World::builder();
    builder.add_system(Writer);
    let (mut world, _) = builder.build().unwrap();
    let e = world.spawn();
    world.add_component(e, X).unwrap();

    let mut ev = E::default();
    world.raise(e, &mut ev);
    assert_eq!(ev.value, 42);

    world.raise_broadcast(&mut ev);
    assert_eq!(ev.value, 43);
}

// ── Delivery to all subscribers ────────────────────────────────────────────────────────

#[derive(Default)]
struct Counts([u32; 3]);

struct Broadcast;

macro_rules! counting_system {
    ($name:ident, $slot:expr) => {
        struct $name;
        impl System for $name {
            fn initialize(&mut self, init: &mut SystemInit<'_>) {
                init.subscribe_broadcast::<Broadcast, _>(|world, _| {
                    world.resource_mut::<Counts>().unwrap().0[$slot] += 1;
                });
            }
        }
    };
}
counting_system!(First, 0);
counting_system!(Second, 1);
counting_system!(Third, 2);

/// Scenario: delivery to all subscribers.
#[test]
fn delivery_to_all_subscribers() {
    let mut builder = World::builder();
    builder
        .insert_resource(Counts::default())
        .add_system(First)
        .add_system(Second)
        .add_system(Third);
    let (mut world, _) = builder.build().unwrap();
    // Entities with components do not affect broadcast delivery.
    let e = world.spawn();
    world.add_component(e, X).unwrap();

    world.raise_broadcast(&mut Broadcast);

    assert_eq!(world.resource::<Counts>().unwrap().0, [1, 1, 1]);
}

// ── After constraint ───────────────────────────────────────────────────────────────────

/// System A sorts after B by name to distinguish the constrained order from the default.
struct ZetaA;
impl System for ZetaA {
    fn initialize(&mut self, init: &mut SystemInit<'_>) {
        init.subscribe::<X, E, _>(|world, _, _| log(world, "A"));
    }
}

struct AlphaB;
impl System for AlphaB {
    fn initialize(&mut self, init: &mut SystemInit<'_>) {
        init.subscribe::<Y, E, _>(|world, _, _| log(world, "B"))
            .after::<ZetaA>();
    }
}

/// Scenario: after constraint.
#[test]
fn after_constraint() {
    let mut builder = World::builder();
    builder
        .insert_resource(Log::default())
        .add_system(AlphaB)
        .add_system(ZetaA);
    let (mut world, _) = builder.build().unwrap();
    let e = world.spawn();
    world.add_component(e, Y).unwrap();
    world.add_component(e, X).unwrap();

    for _ in 0..3 {
        world.raise(e, &mut E::default());
    }

    assert_eq!(lines(&world), vec!["A", "B", "A", "B", "A", "B"]);
}

#[test]
fn before_constraint_and_default_order_by_system_name() {
    struct Mid;
    impl System for Mid {
        fn initialize(&mut self, init: &mut SystemInit<'_>) {
            init.subscribe_broadcast::<E, _>(|world, _| log(world, "Mid"));
        }
    }
    struct Last;
    impl System for Last {
        fn initialize(&mut self, init: &mut SystemInit<'_>) {
            init.subscribe_broadcast::<E, _>(|world, _| log(world, "Last"))
                .before::<Mid>();
        }
    }
    let mut builder = World::builder();
    builder
        .insert_resource(Log::default())
        .add_system(Mid)
        .add_system(Last);
    let (mut world, _) = builder.build().unwrap();
    world.raise_broadcast(&mut E::default());
    assert_eq!(lines(&world), vec!["Last", "Mid"]);
}

// ── Constraint cycle ────────────────────────────────────────────────────────────────────

struct CycleA;
impl System for CycleA {
    fn initialize(&mut self, init: &mut SystemInit<'_>) {
        init.subscribe::<X, E, _>(|_, _, _| {}).after::<CycleB>();
    }
}

struct CycleB;
impl System for CycleB {
    fn initialize(&mut self, init: &mut SystemInit<'_>) {
        init.subscribe::<Y, E, _>(|_, _, _| {}).after::<CycleA>();
    }
}

struct Bystander;
impl System for Bystander {
    fn initialize(&mut self, init: &mut SystemInit<'_>) {
        init.subscribe::<Z, E, _>(|_, _, _| {}).after::<CycleA>();
    }
}

/// Scenario: constraint cycle.
#[test]
fn constraint_cycle() {
    let mut builder = World::builder();
    builder
        .add_system(Bystander)
        .add_system(CycleA)
        .add_system(CycleB);
    let error = builder.build().err().expect("registration fails");

    let BuildError::EventOrderCycle { event, systems } = &error else {
        panic!("expected a handler ordering cycle, got {error:?}");
    };
    assert!(event.ends_with("::E"), "{event}");
    assert_eq!(
        systems.len(),
        2,
        "only A and B belong to the cycle: {systems:?}"
    );
    assert!(systems.iter().any(|s| s.ends_with("::CycleA")));
    assert!(systems.iter().any(|s| s.ends_with("::CycleB")));
    let message = error.to_string();
    assert!(
        message.contains("CycleA") && message.contains("CycleB"),
        "{message}"
    );
    assert!(!message.contains("Bystander"), "{message}");
}

// ── Checking whether an action is allowed through cancellation ──────────────────────────

#[derive(Default)]
struct OpenAttempt {
    cancelled: bool,
}
impl Cancellable for OpenAttempt {
    fn cancel(&mut self) {
        self.cancelled = true;
    }
    fn is_cancelled(&self) -> bool {
        self.cancelled
    }
}

struct Locked;
impl Component for Locked {}

#[derive(Default)]
struct Opened(bool);
impl Component for Opened {}

struct Use;

struct DoorLogic;
impl System for DoorLogic {
    fn initialize(&mut self, init: &mut SystemInit<'_>) {
        init.subscribe::<Opened, Use, _>(|world, uid, _| {
            let mut attempt = OpenAttempt::default();
            world.raise(uid, &mut attempt);
            if attempt.is_cancelled() {
                log(world, "action did not run");
                return;
            }
            world.get_mut::<Opened>(uid).unwrap().0 = true;
        });
        init.subscribe::<Opened, OpenAttempt, _>(|world, _, attempt| {
            let seen = attempt.is_cancelled();
            log(world, format!("cancellation observed: {seen}"));
        })
        .after::<LockLogic>();
    }
}

struct LockLogic;
impl System for LockLogic {
    fn initialize(&mut self, init: &mut SystemInit<'_>) {
        init.subscribe::<Locked, OpenAttempt, _>(|_, _, attempt| attempt.cancel());
    }
}

/// Scenario: checking whether an action is allowed through cancellation.
#[test]
fn can_do_check_via_cancellation() {
    let mut builder = World::builder();
    builder
        .insert_resource(Log::default())
        .add_system(DoorLogic)
        .add_system(LockLogic);
    let (mut world, _) = builder.build().unwrap();
    let door = world.spawn();
    world.add_component(door, Opened::default()).unwrap();
    world.add_component(door, Locked).unwrap();

    world.raise(door, &mut Use);
    assert!(!world.get::<Opened>(door).unwrap().0, "action did not run");
    assert_eq!(
        lines(&world),
        vec!["cancellation observed: true", "action did not run"]
    );

    world.remove_component::<Locked>(door);
    world.raise(door, &mut Use);
    assert!(
        world.get::<Opened>(door).unwrap().0,
        "the action runs without a lock"
    );
}
