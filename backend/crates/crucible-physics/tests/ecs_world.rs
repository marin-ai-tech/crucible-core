//! Exercises the ECS physics system in an assembled server, including contact transitions
//! and replication of velocity changes.

#![cfg(feature = "world")]

use std::sync::{Arc, Mutex};

use crucible_ecs::{EntityUid, System, SystemInit, Tick, World};
use crucible_ext::{LaunchConfig, Launcher, ServerRuntime};
use crucible_map::{MapModule, Transform, create_map, spawn_at};
use crucible_physics::ecs::{
    EndCollide, Fixtures, INPUT_MOVER_HELD_MOVE_BUTTONS, MODULE_ID, Physics, PhysicsModule,
    PhysicsState, StartCollide, set_move_input, tile_fields,
};
use crucible_physics::{
    BodyType, Fixture, InputMover, MoveButtons, MovementSpeedModifier, Polygon, Shape, Vec2,
};
use crucible_testkit::fixtures::{FixtureExtension, fixtures_dir, manifest};

type Hook = Arc<dyn Fn(&mut World) + Send + Sync>;

/// Test system that runs its `hook` each tick to supply scripted player input.
struct Scripted(Hook);
impl System for Scripted {
    fn update(&mut self, world: &mut World, _: f32) {
        (self.0)(world);
    }
}

/// Contact event entries record the receiving entity, the event kind, and the other entity.
#[derive(Default)]
struct ContactLog(Vec<(EntityUid, &'static str, EntityUid)>);

struct ContactRecorder;
impl System for ContactRecorder {
    fn initialize(&mut self, init: &mut SystemInit<'_>) {
        init.insert_resource(ContactLog::default());
        init.subscribe::<Physics, StartCollide, _>(|world, uid, ev| {
            world
                .resource_mut::<ContactLog>()
                .unwrap()
                .0
                .push((uid, "start", ev.other_entity));
        });
        init.subscribe::<Physics, EndCollide, _>(|world, uid, ev| {
            world
                .resource_mut::<ContactLog>()
                .unwrap()
                .0
                .push((uid, "end", ev.other_entity));
        });
    }
}

/// Server with a fixture that requires the physics module. The map module is loaded as a
/// dependency.
fn launch(hook: Hook) -> ServerRuntime {
    let ext = FixtureExtension::new(&manifest("physicsfx", "^0.1", &[MODULE_ID]), fixtures_dir())
        .on_register(move |ctx| {
            ctx.system(ContactRecorder)
                .system(Scripted(Arc::clone(&hook)));
        });
    let server = Launcher::new()
        .module(MapModule::new().with_tile_fields(tile_fields()))
        .module(PhysicsModule)
        .extension(ext)
        .launch(LaunchConfig::for_extension("physicsfx"))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(server.modules(), vec![crucible_map::MODULE_ID, MODULE_ID]);
    server
}

fn square(half: f32, layer: u32, mask: u32) -> Fixtures {
    Fixtures::single(
        "fix1",
        Fixture::new(Shape::Polygon(Polygon::rect(
            Vec2::new(-half, -half),
            Vec2::new(half, half),
        )))
        .with_collision(layer, mask),
    )
}

fn body(
    world: &mut World,
    parent: EntityUid,
    at: (f32, f32),
    physics: Physics,
    fixtures: Fixtures,
) -> EntityUid {
    let uid = spawn_at(world, parent, crucible_map::Vec2::new(at.0, at.1)).unwrap();
    world.add_component(uid, physics).unwrap();
    world.add_component(uid, fixtures).unwrap();
    uid
}

/// Scenario: contact begins and ends.
#[test]
fn contact_start_and_end() {
    let mut server = launch(Arc::new(|_| {}));
    let world = server.world_mut();
    let (map_uid, _) = create_map(world);
    let wall = body(
        world,
        map_uid,
        (1.1, 0.0),
        Physics::new(BodyType::Static),
        square(0.5, 1, 1),
    );
    let mut moving = Physics::new(BodyType::Dynamic);
    moving.linear_velocity = Vec2::new(4.0, 0.0);
    let thing = body(world, map_uid, (0.0, 0.0), moving, square(0.5, 1, 1));

    // The dynamic body hits and rests against the static body.
    server.run_ticks(15);
    let log = &server.world().resource::<ContactLog>().unwrap().0;
    assert_eq!(
        log,
        &vec![(thing, "start", wall), (wall, "start", thing)],
        "contact started on both entities"
    );

    // Set velocity through the component to move the body away on the next step.
    Physics::set_linear_velocity(server.world_mut(), thing, Vec2::new(-4.0, 0.0));
    server.run_ticks(15);
    let log = &server.world().resource::<ContactLog>().unwrap().0;
    let count = |uid, kind| {
        log.iter()
            .filter(|(u, k, _)| *u == uid && *k == kind)
            .count()
    };
    for uid in [thing, wall] {
        assert_eq!((count(uid, "start"), count(uid, "end")), (1, 1), "{log:?}");
    }
    assert_eq!(&log[2..], &[(thing, "end", wall), (wall, "end", thing)]);
    let x = server
        .world()
        .get::<Transform>(thing)
        .unwrap()
        .world_position()
        .x;
    assert!(x < 0.0, "body moved away from the wall: x = {x}");
}

/// Returns the server, both touching bodies, and the number of recorded events.
fn touching_pair() -> (ServerRuntime, EntityUid, EntityUid) {
    let mut server = launch(Arc::new(|_| {}));
    let world = server.world_mut();
    let (map_uid, _) = create_map(world);
    let wall = body(
        world,
        map_uid,
        (1.1, 0.0),
        Physics::new(BodyType::Static),
        square(0.5, 1, 1),
    );
    let mut moving = Physics::new(BodyType::Dynamic);
    moving.linear_velocity = Vec2::new(4.0, 0.0);
    let thing = body(world, map_uid, (0.0, 0.0), moving, square(0.5, 1, 1));
    server.run_ticks(15);
    let log = &server.world().resource::<ContactLog>().unwrap().0;
    assert_eq!(log.len(), 2, "contact started: {log:?}");
    (server, thing, wall)
}

/// Scenario: contact ends when a body is deleted.
#[test]
fn contact_ends_when_a_body_is_deleted() {
    let (mut server, thing, wall) = touching_pair();
    server.world_mut().delete(thing);
    let log = &server.world().resource::<ContactLog>().unwrap().0;
    assert_eq!(
        &log[2..],
        &[(thing, "end", wall), (wall, "end", thing)],
        "end contact reaches the deleted entity before deletion and the surviving entity"
    );
    server.run_ticks(2);
    let log = &server.world().resource::<ContactLog>().unwrap().0;
    assert_eq!(log.len(), 4, "end contact is not repeated: {log:?}");
}

/// Scenario: contact ends when a body is queued for deletion at the end of a tick.
#[test]
fn contact_ends_when_a_body_is_queued_for_deletion() {
    let (mut server, thing, wall) = touching_pair();
    server.world_mut().queue_delete(thing);
    server.run_ticks(1);
    let log = &server.world().resource::<ContactLog>().unwrap().0;
    assert!(
        log[2..].contains(&(wall, "end", thing)),
        "the surviving body received end contact: {log:?}"
    );
    assert!(!server.world().exists(thing));
}

/// Disabling `can_collide` ends contact on both bodies.
#[test]
fn contact_ends_when_collision_is_switched_off() {
    let (mut server, thing, wall) = touching_pair();
    Physics::set_can_collide(server.world_mut(), thing, false);
    server.run_ticks(1);
    let log = &server.world().resource::<ContactLog>().unwrap().0;
    assert_eq!(&log[2..], &[(thing, "end", wall), (wall, "end", thing)]);
}

/// Scenario: velocity changes appear in component state.
#[test]
fn velocity_change_reaches_the_state() {
    // Press up on tick 1 and release it on tick 50.
    let mob = Arc::new(Mutex::new(None::<EntityUid>));
    let input_mob = Arc::clone(&mob);
    let mut server = launch(Arc::new(move |world: &mut World| {
        let Some(uid) = *input_mob.lock().unwrap() else {
            return;
        };
        match world.current_tick().0 {
            1 => assert!(set_move_input(world, uid, 0, MoveButtons::UP, true)),
            50 => assert!(set_move_input(world, uid, 0, MoveButtons::UP, false)),
            _ => {}
        }
    }));
    let world = server.world_mut();
    let (map_uid, _) = create_map(world);
    let uid = body(
        world,
        map_uid,
        (0.5, 0.5),
        Physics {
            fixed_rotation: true,
            ..Physics::new(BodyType::KinematicController)
        },
        Fixtures::single(
            "fix1",
            Fixture::new(Shape::circle(0.35, Vec2::ZERO))
                .with_density(185.0)
                .with_collision(2, 1),
        ),
    );
    world.add_component(uid, InputMover::default()).unwrap();
    world
        .add_component(uid, MovementSpeedModifier::default())
        .unwrap();
    *mob.lock().unwrap() = Some(uid);

    let mut previous = (Vec2::ZERO, crucible_map::Vec2::new(0.5, 0.5));
    let mut velocity_at_49 = Vec2::ZERO;
    let mut changes = 0;
    for tick in 1..=150u32 {
        server.tick();
        let world = server.world();
        let velocity = world.get::<Physics>(uid).unwrap().linear_velocity;
        let position = world.get::<Transform>(uid).unwrap().world_position();
        // Each change to velocity and transform is recorded on the same tick.
        if velocity != previous.0 {
            changes += 1;
            assert_eq!(
                world.component_last_modified::<Physics>(uid),
                Some(Tick(tick)),
                "tick {tick}"
            );
            assert!(
                world
                    .fields_changed_since::<Physics>(uid, Tick(tick - 1))
                    .contains(Physics::FIELD_LINEAR_VELOCITY)
            );
        }
        if position != previous.1 {
            assert_eq!(
                world.component_last_modified::<Transform>(uid),
                Some(Tick(tick)),
                "tick {tick}"
            );
        }
        // The component value matches the physics body value.
        let state = world.resource::<PhysicsState>().unwrap();
        assert_eq!(state.body(uid).unwrap().linear_velocity(), velocity);
        if tick == 49 {
            velocity_at_49 = velocity;
        }
        if tick == 50 {
            // The release changes the body and control state on tick 50. The components
            // contain the new velocity and intent.
            assert_eq!(
                world.component_last_modified::<Physics>(uid),
                Some(Tick(50))
            );
            assert_eq!(
                world.component_last_modified::<InputMover>(uid),
                Some(Tick(50))
            );
            assert!(
                world
                    .fields_changed_since::<InputMover>(uid, Tick(49))
                    .contains(INPUT_MOVER_HELD_MOVE_BUTTONS)
            );
            assert_ne!(velocity, velocity_at_49);
            assert!(velocity.y < velocity_at_49.y, "friction slows the body");
            assert_eq!(
                world.get::<InputMover>(uid).unwrap().held_move_buttons(),
                MoveButtons::NONE
            );
        }
        previous = (velocity, position);
    }
    assert!(
        (velocity_at_49.y - 4.5).abs() < 0.05,
        "sprint speed: {velocity_at_49:?}"
    );
    // With reference friction of 20, acceleration and stopping take 30 ticks, fewer than at 2.5.
    assert!(
        changes >= 25,
        "velocity changed during acceleration and braking for {changes} ticks"
    );
    // When the body stops, it sleeps and stops being marked as changed.
    let world = server.world();
    assert_eq!(
        world.get::<Physics>(uid).unwrap().linear_velocity,
        Vec2::ZERO
    );
    assert!(world.component_last_modified::<Physics>(uid) < Some(Tick(150)));
    assert!(
        !world.get::<Physics>(uid).unwrap().awake,
        "body is sleeping"
    );
}

/// Controlled body at (0.5, 0.5) on map `map_uid`.
fn spawn_mob(world: &mut World, map_uid: EntityUid) -> EntityUid {
    let uid = body(
        world,
        map_uid,
        (0.5, 0.5),
        Physics {
            fixed_rotation: true,
            ..Physics::new(BodyType::KinematicController)
        },
        Fixtures::single(
            "fix1",
            Fixture::new(Shape::circle(0.35, Vec2::ZERO))
                .with_density(185.0)
                .with_collision(2, 1),
        ),
    );
    world.add_component(uid, InputMover::default()).unwrap();
    world
        .add_component(uid, MovementSpeedModifier::default())
        .unwrap();
    uid
}

/// Delivers rightward input at `sub_tick` on tick 45, five ticks after its client tick.
/// Returns the server and body after tick 45 and its velocity after tick 44.
fn late_input_run(sub_tick: u16) -> (ServerRuntime, EntityUid, Vec2) {
    let mob = Arc::new(Mutex::new(None::<EntityUid>));
    let input_mob = Arc::clone(&mob);
    let mut server = launch(Arc::new(move |world: &mut World| {
        let Some(uid) = *input_mob.lock().unwrap() else {
            return;
        };
        // The input buffer emits the late command for the current simulation tick.
        if world.current_tick().0 == 45 {
            assert!(set_move_input(
                world,
                uid,
                sub_tick,
                MoveButtons::RIGHT,
                true
            ));
        }
    }));
    let world = server.world_mut();
    let (map_uid, _) = create_map(world);
    let uid = spawn_mob(world, map_uid);
    *mob.lock().unwrap() = Some(uid);
    server.run_ticks(44);
    let before = server.world().get::<Physics>(uid).unwrap().linear_velocity;
    server.tick();
    (server, uid, before)
}

/// Scenario: late input is applied in the current tick.
#[test]
fn late_input_is_applied_in_the_current_tick() {
    let (late, uid, before) = late_input_run(u16::MAX / 2);
    let world = late.world();
    assert_eq!(
        before,
        Vec2::ZERO,
        "the body stands still before input arrives"
    );
    let input = world.get::<InputMover>(uid).unwrap();
    assert_eq!(
        input.last_input(),
        (45, u16::MAX / 2),
        "input was applied on tick 45 rather than 40"
    );
    assert_eq!(
        world.component_last_modified::<InputMover>(uid),
        Some(Tick(45))
    );
    let late_v = world.get::<Physics>(uid).unwrap().linear_velocity;
    assert!(
        late_v.x > 0.0,
        "movement starts on the delivery tick: {late_v:?}"
    );

    // Applying the same command at the start of tick 45 contributes more movement.
    let (full, uid_full, _) = late_input_run(0);
    let full_v = full
        .world()
        .get::<Physics>(uid_full)
        .unwrap()
        .linear_velocity;
    assert!(
        late_v.x < full_v.x,
        "half a tick contributes less than a full tick: {late_v:?} < {full_v:?}"
    );
    assert_eq!(
        full.world().resource::<PhysicsState>().unwrap().last_step(),
        45
    );
}

/// Input ticks must increase monotonically. In debug builds, an out-of-order command
/// triggers an error rather than contributing movement to an earlier tick.
#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "input ticks must not decrease")]
fn input_tick_never_goes_back() {
    let mut server = launch(Arc::new(|_| {}));
    let world = server.world_mut();
    let (map_uid, _) = create_map(world);
    let uid = spawn_mob(world, map_uid);
    server.run_ticks(5);
    let world = server.world_mut();
    assert!(set_move_input(world, uid, 0, MoveButtons::UP, true));
    world.set_current_tick(Tick(3));
    set_move_input(world, uid, 0, MoveButtons::UP, false);
}
