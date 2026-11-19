//! Change ticks and `#[networked]` field tracking for the `specs/core/ecs` change-marking scenario.

use crucible_ecs::{
    Component, ComponentRegistration, FieldMask, NetworkPolicy, NetworkedComponent, System, Tick,
    World, networked,
};

/// Replicated component with per-field deltas.
#[networked]
#[derive(Debug, Default, PartialEq)]
pub struct Mover {
    pub speed: f32,
    pub held_buttons: u16,
    /// Server-side cache excluded from replication.
    #[networked(skip)]
    pub cache: u32,
}
impl Component for Mover {}

/// Replicated component without per-field deltas.
#[derive(Default)]
struct Body {
    velocity: f32,
}
impl Component for Body {}

struct MoveSystem;
impl System for MoveSystem {
    fn update(&mut self, world: &mut World, _: f32) {
        let mut query = world.query::<Mover>();
        while let Some(uid) = query.next(world) {
            Mover::set_speed(world, uid, 2.5);
        }
    }
}

/// Scenario: marking a field as changed.
#[test]
fn change_mark() {
    let mut builder = World::builder();
    builder
        .register_component(
            ComponentRegistration::new::<Mover>("InputMover", "core:movement")
                .network(NetworkPolicy::AllViewers)
                .networked_fields::<Mover>(),
        )
        .unwrap()
        .add_system(MoveSystem);
    let (mut world, mut systems) = builder.build().unwrap();

    world.set_current_tick(Tick(100));
    let e = world.spawn();
    world.add_component(e, Mover::default()).unwrap();
    assert_eq!(world.component_last_modified::<Mover>(e), Some(Tick(100)));

    world.set_current_tick(Tick(120));
    systems.update(&mut world, 1.0 / 30.0);

    assert_eq!(world.get::<Mover>(e).unwrap().speed, 2.5);
    assert_eq!(world.component_last_modified::<Mover>(e), Some(Tick(120)));
    assert_eq!(world.entity_last_modified(e), Some(Tick(120)));
    assert_eq!(world.entity_meta(e).unwrap().last_modified(), Tick(120));
    let changed = world.fields_changed_since::<Mover>(e, Tick(119));
    assert_eq!(changed, Mover::FIELD_SPEED, "only this field is marked");
    assert!(!changed.contains(Mover::FIELD_HELD_BUTTONS));
    // Adding a component counts every field as changed.
    assert_eq!(
        world.fields_changed_since::<Mover>(e, Tick(99)),
        Mover::all_fields()
    );
    assert_eq!(
        world.fields_changed_since::<Mover>(e, Tick(120)),
        FieldMask::EMPTY
    );
}

#[test]
fn networked_macro_generates_field_list_and_masks() {
    assert_eq!(Mover::FIELD_NAMES, &["speed", "held_buttons"]);
    assert_eq!(Mover::FIELD_SPEED, FieldMask::bit(0));
    assert_eq!(Mover::FIELD_HELD_BUTTONS, FieldMask::bit(1));
    assert_eq!(Mover::all_fields().bits(), 0b11);

    let mut world = World::new();
    let ghost = world.spawn();
    assert!(
        !Mover::set_speed(&mut world, ghost, 1.0),
        "the setter has no effect without the component"
    );
    world.delete(ghost);
    assert!(!Mover::set_held_buttons(&mut world, ghost, 1));
}

#[test]
fn whole_component_dirty_and_plain_components() {
    let mut world = World::new();
    world.set_current_tick(Tick(1));
    let e = world.spawn();
    world.add_component(e, Mover::default()).unwrap();
    world.add_component(e, Body::default()).unwrap();

    world.set_current_tick(Tick(10));
    Mover::set_held_buttons(&mut world, e, 0b101);
    world.set_current_tick(Tick(11));
    world.get_mut::<Body>(e).unwrap().velocity = 3.0;
    assert_eq!(
        world.component_last_modified::<Body>(e),
        Some(Tick(1)),
        "get_mut leaves change flags untouched"
    );
    assert!(world.dirty::<Body>(e));
    assert_eq!(world.get::<Body>(e).unwrap().velocity, 3.0);
    assert_eq!(world.component_last_modified::<Body>(e), Some(Tick(11)));
    assert_eq!(world.component_last_modified::<Mover>(e), Some(Tick(10)));
    assert_eq!(world.entity_last_modified(e), Some(Tick(11)));

    // `dirty` marks every field of a component with per-field deltas.
    world.set_current_tick(Tick(12));
    world.dirty::<Mover>(e);
    assert_eq!(
        world.fields_changed_since::<Mover>(e, Tick(11)),
        Mover::all_fields()
    );

    // Removing a component also updates the entity's change tick.
    world.set_current_tick(Tick(13));
    world.remove_component::<Body>(e);
    assert_eq!(world.entity_last_modified(e), Some(Tick(13)));
}
