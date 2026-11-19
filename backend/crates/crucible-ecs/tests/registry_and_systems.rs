//! Component registry and sequential system ordering scenarios for `specs/core/ecs`.

use crucible_ecs::{
    BuildError, Component, ComponentRegistration, NetworkPolicy, RegistryError, System, SystemInit,
    World, WorldBuilder,
};

struct CoreDoor;
impl Component for CoreDoor {}

struct ExtensionDoor;
impl Component for ExtensionDoor {}

/// Scenario: conflicting component names.
#[test]
fn component_name_conflict() {
    let mut builder = World::builder();
    builder
        .register_component(
            ComponentRegistration::new::<CoreDoor>("Door", "core:doors")
                .field("open", "bool")
                .network(NetworkPolicy::AllViewers)
                .persisted(true)
                .debug(true),
        )
        .unwrap();

    let error = builder
        .register_component(ComponentRegistration::new::<ExtensionDoor>(
            "Door",
            "demo-source",
        ))
        .err()
        .expect("construction fails");

    assert_eq!(
        error,
        RegistryError::DuplicateName {
            name: "Door".into(),
            first_source: "core:doors".into(),
            second_source: "demo-source".into(),
        }
    );
    let message = error.to_string();
    assert!(
        message.contains("core:doors") && message.contains("demo-source"),
        "{message}"
    );
}

#[test]
fn registry_lookups_and_duplicate_type() {
    let mut builder = World::builder();
    builder
        .register_component(
            ComponentRegistration::new::<CoreDoor>("Door", "core:doors")
                .field("open", "bool")
                .network(NetworkPolicy::OwnerOnly)
                .persisted(true),
        )
        .unwrap();
    let error = builder
        .register_component(ComponentRegistration::new::<CoreDoor>(
            "Airlock",
            "demo-source",
        ))
        .err()
        .unwrap();
    assert!(matches!(error, RegistryError::DuplicateType { .. }));
    assert!(error.to_string().contains("core:doors") && error.to_string().contains("demo-source"));

    let (world, _) = builder.build().unwrap();
    let registry = world.registry();
    assert_eq!(registry.len(), 1);
    let door = registry.by_name("Door").unwrap();
    assert_eq!(door.source(), "core:doors");
    assert_eq!(door.network_policy(), NetworkPolicy::OwnerOnly);
    assert!(door.is_persisted() && !door.is_debug());
    assert_eq!(door.fields()[0].name, "open");
    assert_eq!(registry.by_type::<CoreDoor>().unwrap().name(), "Door");
    assert!(registry.by_type::<ExtensionDoor>().is_none());
    assert!(registry.by_name("Airlock").is_none());
}

// ── Stable system order ─────────────────────────────────────────────────────────────────

#[derive(Default)]
struct Trace(Vec<&'static str>);

macro_rules! traced_system {
    ($name:ident $(, $method:ident :: <$other:ident>)*) => {
        struct $name;
        impl System for $name {
            fn initialize(&mut self, init: &mut SystemInit<'_>) {
                $( init.$method::<$other>(); )*
                let _ = init;
            }
            fn update(&mut self, world: &mut World, _: f32) {
                world.resource_mut::<Trace>().unwrap().0.push(stringify!($name));
            }
        }
    };
}

traced_system!(Input);
traced_system!(Movement, updates_after::<Input>);
traced_system!(Physics, updates_after::<Movement>);
traced_system!(Atmos);
traced_system!(Cleanup, updates_after::<Physics>, updates_after::<Atmos>);
traced_system!(Ai, updates_before::<Movement>);

fn add_all(builder: &mut WorldBuilder, reversed: bool) {
    let mut adders: Vec<fn(&mut WorldBuilder)> = vec![
        |b| {
            b.add_system(Input);
        },
        |b| {
            b.add_system(Movement);
        },
        |b| {
            b.add_system(Physics);
        },
        |b| {
            b.add_system(Atmos);
        },
        |b| {
            b.add_system(Cleanup);
        },
        |b| {
            b.add_system(Ai);
        },
    ];
    if reversed {
        adders.reverse();
    }
    for add in adders {
        add(builder);
    }
}

fn run(reversed: bool) -> (Vec<&'static str>, Vec<&'static str>) {
    let mut builder = World::builder();
    builder.insert_resource(Trace::default());
    add_all(&mut builder, reversed);
    let (mut world, mut systems) = builder.build().unwrap();
    for _ in 0..3 {
        systems.update(&mut world, 1.0 / 30.0);
    }
    let names = systems
        .names()
        .into_iter()
        .map(|n| n.rsplit("::").next().unwrap())
        .collect();
    (names, world.remove_resource::<Trace>().unwrap().0)
}

/// Scenario: stable system order.
#[test]
fn stable_system_order() {
    let (first_order, first_trace) = run(false);
    let (second_order, second_trace) = run(true);

    assert_eq!(first_order, second_order, "both runs use the same order");
    assert_eq!(first_trace, second_trace);
    // The order is stable across ticks.
    let per_tick: Vec<_> = first_trace.chunks(first_order.len()).collect();
    assert_eq!(per_tick.len(), 3);
    assert!(per_tick.iter().all(|tick| *tick == first_order.as_slice()));
    // The order satisfies the constraints.
    let pos = |name: &str| first_order.iter().position(|n| *n == name).unwrap();
    assert!(pos("Input") < pos("Movement"));
    assert!(pos("Ai") < pos("Movement"));
    assert!(pos("Movement") < pos("Physics"));
    assert!(pos("Physics") < pos("Cleanup") && pos("Atmos") < pos("Cleanup"));
    assert_eq!(
        first_order,
        vec!["Ai", "Atmos", "Input", "Movement", "Physics", "Cleanup"]
    );
}

#[test]
fn system_order_cycle_and_duplicate_system() {
    traced_system!(Left, updates_after::<Right>);
    traced_system!(Right, updates_after::<Left>);
    let mut builder = World::builder();
    builder.add_system(Left).add_system(Right);
    let Err(BuildError::SystemOrderCycle { systems }) = builder.build() else {
        panic!("expected a system ordering cycle");
    };
    assert_eq!(systems.len(), 2);

    let mut builder = World::builder();
    builder.add_system(Input).add_system(Input);
    assert!(matches!(
        builder.build(),
        Err(BuildError::DuplicateSystem { .. })
    ));
}
