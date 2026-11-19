//! Physics scenarios for bodies, fixtures, controlled motion, collisions with static objects,
//! inverse mass, fixed steps, and sleeping.

mod common;

use common::Trajectory;
use crucible_physics::{
    Body, BodyId, BodyType, Fixture, GroundTile, LINEAR_SLOP, ManifoldType, MoveButtons,
    MovementSpeedModifier, Mover, PhysicsConfig, PhysicsWorld, Polygon, Shape, Vec2,
};

/// Collision layer and mask for a solid human fixture (`MobHuman` in the reference).
const MOB_LAYER: u32 = 65;
const MOB_MASK: u32 = 30;
/// Reference wall layer and mask (`WallSolid`).
const WALL_LAYER: u32 = 223;
const WALL_MASK: u32 = 158;
/// Reference crate layer and mask (`CrateGenericSteel`).
const CRATE_LAYER: u32 = 85;
const CRATE_MASK: u32 = 26;

const MOB_RADIUS: f32 = 0.35;

/// Reference human movement: walk 2.5, sprint 4.5, acceleration 20, friction 20.
fn human_mover() -> Mover {
    Mover {
        speed: MovementSpeedModifier {
            base_walk_speed: 2.5,
            base_sprint_speed: 4.5,
            acceleration: 20.0,
            friction: 20.0,
            friction_no_input: 20.0,
            ..MovementSpeedModifier::default()
        },
        ..Mover::default()
    }
}

fn add_mob(world: &mut PhysicsWorld, position: Vec2) -> BodyId {
    world.add_body(
        Body::new(BodyType::KinematicController, position)
            .with_fixed_rotation(true)
            .with_fixture(
                Fixture::new(Shape::circle(MOB_RADIUS, Vec2::ZERO))
                    .with_density(185.0)
                    .with_collision(MOB_LAYER, MOB_MASK),
            )
            .with_mover(human_mover()),
    )
}

/// Static wall on a tile centered at `center`.
fn add_wall(world: &mut PhysicsWorld, center: Vec2, layer: u32, mask: u32) -> BodyId {
    world.add_body(
        Body::new(BodyType::Static, center).with_fixture(
            Fixture::new(Shape::Polygon(Polygon::rect(
                Vec2::new(-0.5, -0.5),
                Vec2::new(0.5, 0.5),
            )))
            .with_collision(layer, mask),
        ),
    )
}

/// Column of walls at tile x = `tile_x` and y in [y0, y1).
fn add_wall_column(world: &mut PhysicsWorld, tile_x: i32, y0: i32, y1: i32) {
    for y in y0..y1 {
        add_wall(
            world,
            Vec2::new(tile_x as f32 + 0.5, y as f32 + 0.5),
            WALL_LAYER,
            WALL_MASK,
        );
    }
}

/// Reference crate with polygon bounds x = −0.4 to 0.4 and y = −0.4 to 0.29, density 50.
fn add_crate(
    world: &mut PhysicsWorld,
    position: Vec2,
    velocity: Vec2,
    ground_friction: f32,
) -> BodyId {
    let mut body = Body::new(BodyType::Dynamic, position)
        .with_fixed_rotation(true)
        .with_fixture(
            Fixture::new(Shape::Polygon(Polygon::rect(
                Vec2::new(-0.4, -0.4),
                Vec2::new(0.4, 0.29),
            )))
            .with_density(50.0)
            .with_collision(CRATE_LAYER, CRATE_MASK),
        );
    body.ground = GroundTile {
        friction: ground_friction,
        ..GroundTile::default()
    };
    body.set_linear_velocity(velocity);
    world.add_body(body)
}

/// Holds `buttons` starting at `tick`, with commands applied at the start of the tick.
fn hold(world: &mut PhysicsWorld, id: BodyId, tick: u32, from: MoveButtons, to: MoveButtons) {
    for flag in [
        MoveButtons::UP,
        MoveButtons::DOWN,
        MoveButtons::LEFT,
        MoveButtons::RIGHT,
        MoveButtons::WALK,
    ] {
        if from.contains(flag) != to.contains(flag) {
            world.set_move_input(id, tick, 0, flag, to.contains(flag));
        }
    }
}

/// Scenario: collision masks.
#[test]
fn collision_masks() {
    // The wall mask excludes the creature layer, and the human mask excludes the wall layer.
    let foreign_layer = 128;
    let foreign_mask = 16;
    assert_eq!(foreign_mask & MOB_LAYER, 0);
    assert_eq!(MOB_MASK & foreign_layer, 0);

    let run = |layer: u32, mask: u32| {
        let mut world = PhysicsWorld::new(PhysicsConfig::robust_defaults());
        let mob = add_mob(&mut world, Vec2::new(0.5, 0.5));
        add_wall(&mut world, Vec2::new(3.5, 0.5), layer, mask);
        hold(&mut world, mob, 1, MoveButtons::NONE, MoveButtons::RIGHT);
        let mut touched = false;
        for tick in 1..=60 {
            world.step(tick).unwrap();
            touched |= world.contacts().any(|c| c.touching);
        }
        (world.body(mob).position().x, touched)
    };

    let (x, touched) = run(foreign_layer, foreign_mask);
    assert!(
        !touched,
        "bodies without overlapping layers and masks must not touch"
    );
    assert!(x > 4.5, "the mob must pass through the wall, x = {x}");

    // With the reference collision layers, the wall blocks the human.
    let (x, touched) = run(WALL_LAYER, WALL_MASK);
    assert!(touched);
    assert!(
        x < 3.0 - MOB_RADIUS + 0.02,
        "the mob must stop at the wall, x = {x}"
    );
}

/// Independent f64 model of acceleration without collisions. Each substep applies friction first.
struct MobModel {
    v: f64,
    p: f64,
}

impl MobModel {
    fn substep(&mut self, wish: f64, dt: f64) {
        let (accel, friction, min_speed) = (20.0, 20.0, 0.005);
        if self.v.abs() >= min_speed {
            self.v *= (1.0 - dt * friction).clamp(0.0, 1.0);
        }
        if wish > 0.0 {
            let add = wish - self.v;
            if add > 0.0 {
                self.v += (accel * dt * wish).min(add);
            }
        }
        self.p += self.v * dt;
    }

    fn tick(&mut self, wish: f64) {
        for _ in 0..2 {
            self.substep(wish, 1.0 / 60.0);
        }
    }
}

/// Scenario: movement based on input intent.
#[test]
fn movement_by_intent() {
    let mut world = PhysicsWorld::new(PhysicsConfig::robust_defaults());
    let start = Vec2::new(0.5, 0.5);
    let mob = add_mob(&mut world, start);
    hold(&mut world, mob, 1, MoveButtons::NONE, MoveButtons::UP);
    let mut model = MobModel {
        v: 0.0,
        p: f64::from(start.y),
    };
    for tick in 1..=30 {
        world.step(tick).unwrap();
        model.tick(4.5);
        let b = world.body(mob);
        assert!(
            (f64::from(b.position().y) - model.p).abs() < 1e-3,
            "tick {tick}: y {} vs {}",
            b.position().y,
            model.p
        );
        assert!(
            (f64::from(b.linear_velocity().y) - model.v).abs() < 1e-3,
            "tick {tick}: v {} vs {}",
            b.linear_velocity().y,
            model.v
        );
        assert_eq!(b.position().x, start.x);
    }
    let v = world.body(mob).linear_velocity();
    assert!(
        (v.length() - 4.5).abs() <= 0.05,
        "speed after 30 ticks: {}",
        v.length()
    );
    // The body faces its movement direction. Up corresponds to angle π.
    assert!((world.body(mob).angle() - core::f32::consts::PI).abs() < 1e-6);
}

/// Scenario: subtick input.
#[test]
fn sub_tick_input() {
    const T: u32 = 10;
    // Midpoint of the tick at fraction 32768/65535.
    const HALF: u16 = 32768;
    let fraction = f64::from(u16::MAX - HALF) / f64::from(u16::MAX);

    let run = |sub: u16| {
        let mut world = PhysicsWorld::new(PhysicsConfig::robust_defaults());
        let mob = add_mob(&mut world, Vec2::new(0.5, 0.5));
        let mut trace = Vec::new();
        for tick in 1..=T + 20 {
            if tick == T {
                world.set_move_input(mob, tick, sub, MoveButtons::RIGHT, true);
                let input = world.body(mob).mover.unwrap().input;
                let (walk, sprint) = input.velocity_input(tick, true);
                assert_eq!(walk, Vec2::ZERO);
                assert!(
                    (f64::from(sprint.x) - (1.0 - f64::from(sub) / f64::from(u16::MAX))).abs()
                        < 1e-6
                );
            }
            world.step(tick).unwrap();
            let b = world.body(mob);
            trace.push((b.position().x, b.linear_velocity().x));
        }
        trace
    };
    let half = run(HALF);
    let full = run(0);

    // Movement input contributes half a tick's worth of velocity and displacement at tick T.
    let i = (T - 1) as usize;
    let (dx_half, dv_half) = (half[i].0 - 0.5, half[i].1);
    let (dx_full, dv_full) = (full[i].0 - 0.5, full[i].1);
    assert!(
        (f64::from(dv_half) - fraction * f64::from(dv_full)).abs() < 1e-3,
        "{dv_half} vs {dv_full}"
    );
    assert!(
        (f64::from(dx_half) - fraction * f64::from(dx_full)).abs() < 1e-3,
        "{dx_half} vs {dx_full}"
    );

    // The trajectory matches the model with half the desired speed at tick T and full speed
    // thereafter, within the reference tolerance.
    let mut model = MobModel { v: 0.0, p: 0.5 };
    for (k, &(x, v)) in half.iter().enumerate() {
        let tick = k as u32 + 1;
        let wish = if tick < T {
            0.0
        } else if tick == T {
            4.5 * fraction
        } else {
            4.5
        };
        model.tick(wish);
        assert!(
            (f64::from(x) - model.p).abs() <= 1e-3,
            "tick {tick}: x {x} vs {}",
            model.p
        );
        assert!(
            (f64::from(v) - model.v).abs() <= 1e-3,
            "tick {tick}: v {v} vs {}",
            model.v
        );
    }

    // Release halfway through the tick: half the input for this tick, none on the following tick.
    let mut world = PhysicsWorld::new(PhysicsConfig::robust_defaults());
    let mob = add_mob(&mut world, Vec2::new(0.5, 0.5));
    world.set_move_input(mob, 1, 0, MoveButtons::RIGHT, true);
    world.step(1).unwrap();
    world.set_move_input(mob, 2, HALF, MoveButtons::RIGHT, false);
    let input = world.body(mob).mover.unwrap().input;
    let (_, sprint) = input.velocity_input(2, true);
    assert!((f64::from(sprint.x) - f64::from(HALF) / f64::from(u16::MAX)).abs() < 1e-6);
    assert_eq!(input.velocity_input(3, true).1, Vec2::ZERO);
}

/// Scenario: sliding along a wall.
#[test]
fn sliding_along_a_wall() {
    // A controlled circle runs diagonally into the wall column at x = 3 and slides along it.
    let mut world = PhysicsWorld::new(PhysicsConfig::robust_defaults());
    add_wall_column(&mut world, 3, -4, 12);
    let mob = add_mob(&mut world, Vec2::new(0.5, 0.5));
    hold(
        &mut world,
        mob,
        1,
        MoveButtons::NONE,
        MoveButtons::UP | MoveButtons::RIGHT,
    );
    // Box2D v2 has no speculative contacts. An impact can briefly penetrate up to 0.02 m
    // in the reference. The position solver reduces this to LINEAR_SLOP over several ticks.
    let face = 3.0 - 0.01 - MOB_RADIUS;
    let limit = face + 0.03;
    let mut touched = false;
    for tick in 1..=60 {
        world.step(tick).unwrap();
        let b = world.body(mob);
        assert!(
            b.position().x < limit,
            "tick {tick}: the mob went into the wall, x = {}",
            b.position().x
        );
        touched |= world.contacts().any(|c| c.touching);
    }
    assert!(touched);
    let b = world.body(mob);
    // Penetration stays within the position solver's threshold of 3 × LINEAR_SLOP.
    assert!(
        b.position().x <= face + 3.0 * LINEAR_SLOP,
        "resting penetration: x = {}",
        b.position().x
    );
    assert!(
        b.position().y > 0.5 + 3.0,
        "the mob must keep moving along the wall, y = {}",
        b.position().y
    );
    // Wall-parallel speed is about 4.5/√2 = 3.18 m/s before wall friction of 0.4.
    assert!(
        b.linear_velocity().y > 1.0,
        "velocity along the wall: {:?}",
        b.linear_velocity()
    );
    assert!(
        b.linear_velocity().x.abs() < 0.05,
        "velocity into the wall: {:?}",
        b.linear_velocity()
    );

    // A dynamic polygonal crate approaches diagonally, exercising a two-point polygon contact
    // and the block solver.
    let mut world = PhysicsWorld::new(PhysicsConfig::robust_defaults());
    add_wall_column(&mut world, 3, -4, 12);
    let crate_id = add_crate(&mut world, Vec2::new(1.8, 0.75), Vec2::new(4.0, 4.0), 0.0);
    let face = 3.0 - 0.01 - 0.01 - 0.4;
    let limit = face + 0.03;
    let mut two_point = false;
    for tick in 1..=45 {
        world.step(tick).unwrap();
        let b = world.body(crate_id);
        assert!(
            b.position().x < limit,
            "tick {tick}: the crate went into the wall, x = {}",
            b.position().x
        );
        two_point |= world.contacts().any(|c| {
            c.touching
                && c.manifold.point_count == 2
                && matches!(c.manifold.kind, ManifoldType::FaceA | ManifoldType::FaceB)
        });
    }
    assert!(
        two_point,
        "a face-to-face polygon contact must have two points"
    );
    let b = world.body(crate_id);
    assert!(
        b.position().x <= face + 3.0 * LINEAR_SLOP,
        "resting penetration: x = {}",
        b.position().x
    );
    assert!(
        b.position().y > 0.75 + 2.0,
        "the crate must keep sliding along the wall, y = {}",
        b.position().y
    );
    // Wall friction can remove up to 1.6 m/s of tangential speed (0.4 × 4 m/s normal impulse).
    assert!(
        b.linear_velocity().y > 1.0,
        "velocity along the wall: {:?}",
        b.linear_velocity()
    );
    assert!(
        b.linear_velocity().x.abs() < 0.05,
        "velocity into the wall: {:?}",
        b.linear_velocity()
    );
}

/// Scenario: a dynamic body pushes a controlled body.
#[test]
fn dynamic_body_pushes_controlled_body_out() {
    let mut world = PhysicsWorld::new(PhysicsConfig::robust_defaults());
    let mob_start = Vec2::new(0.5, 0.5);
    let mob = add_mob(&mut world, mob_start);
    // The crate slides toward a stationary human. Tile friction of 0.25 gives damping of 2/s.
    let crate_id = add_crate(&mut world, Vec2::new(3.0, 0.75), Vec2::new(-6.0, 0.0), 0.25);

    // The crate hits and pushes the human.
    let mut tick = 1;
    let mut pushed = false;
    while tick <= 150 {
        world.step(tick).unwrap();
        tick += 1;
        if world.body(mob).position().x < mob_start.x - 0.05 {
            pushed = true;
        }
        if !world.body(crate_id).awake() {
            break;
        }
    }
    assert!(
        pushed,
        "the moving crate must push the controlled body out, x = {}",
        world.body(mob).position().x
    );
    assert!(
        !world.body(crate_id).awake(),
        "the crate must stop and fall asleep"
    );

    // The human pushes against the stationary crate for 60 ticks, and the crate stays in place.
    let crate_at = world.body(crate_id).position();
    hold(&mut world, mob, tick, MoveButtons::NONE, MoveButtons::RIGHT);
    let mut pressed = false;
    for t in tick..tick + 60 {
        world.step(t).unwrap();
        let c = world.body(crate_id).position();
        assert!(
            (c - crate_at).length() < 1e-5,
            "tick {t}: the controlled body moved the crate: {c:?} vs {crate_at:?}"
        );
        pressed |= world.contacts().any(|c| c.touching);
    }
    assert!(pressed, "the controlled body must press into the crate");
    let m = world.body(mob).position();
    assert!(
        m.x < crate_at.x - 0.4 - MOB_RADIUS + 0.02,
        "the controlled body must not pass the crate: {m:?}"
    );
}

/// Scenario: frame-processing delays do not change the simulation.
#[test]
fn independence_from_load() {
    let traj = Trajectory::load("wall");
    let run = |delay: bool| {
        let mut scene = traj.build();
        let mut states = Vec::new();
        for k in 0..traj.tick_count() {
            if delay && k % 4 == 0 {
                // Introduce an artificial delay while processing the tick.
                std::thread::sleep(std::time::Duration::from_millis(3));
            }
            scene.step_reference_tick(&traj, k);
            states.push(scene.state(0));
        }
        states
    };
    let slow = run(true);
    let fast = run(false);
    for (k, (a, b)) in slow.iter().zip(&fast).enumerate() {
        assert_eq!(a.p.x.to_bits(), b.p.x.to_bits(), "tick {k}");
        assert_eq!(a.p.y.to_bits(), b.p.y.to_bits(), "tick {k}");
        assert_eq!(a.v.x.to_bits(), b.v.x.to_bits(), "tick {k}");
        assert_eq!(a.v.y.to_bits(), b.v.y.to_bits(), "tick {k}");
        assert_eq!(a.awake, b.awake, "tick {k}");
    }
}

/// Scenario: a body goes to sleep.
#[test]
fn body_falls_asleep() {
    // In the walk reference, the human releases input at tick 120 and slows down due to friction.
    let traj = Trajectory::load("walk");
    let moving = |b: &MoveButtons| b.0 & MoveButtons::ANY_DIRECTION.0 != 0;
    let release = traj.inputs.last().unwrap();
    assert!(!moving(&release.1), "the last input releases the movement");
    let release = release.0;
    // Last movement before the release. The body may have slept after earlier releases.
    let last_move = traj.inputs.iter().rev().find(|(_, b)| moving(b)).unwrap().0;
    let reference_sleep = (release as usize + 1..=traj.tick_count())
        .find(|&k| !traj.reference(k, "human").awake)
        .expect("the reference body falls asleep");

    let config = traj.config();
    let mut scene = traj.build();
    let mut below_since: Option<usize> = None;
    for k in 0..traj.tick_count() {
        scene.step_reference_tick(&traj, k);
        let s = scene.state(0);
        let tick = k + 1;
        if tick <= release as usize {
            if tick > last_move as usize {
                assert!(s.awake, "tick {tick}: the moving body must be awake");
            }
            continue;
        }
        if s.awake {
            if s.v.length_squared() <= config.linear_sleep_tolerance_sqr {
                below_since.get_or_insert(tick);
            } else {
                below_since = None;
            }
            continue;
        }
        // The body sleeps on the reference tick at zero speed, after 0.2 s below the threshold.
        assert_eq!(
            tick, reference_sleep,
            "the body must fall asleep on the reference tick"
        );
        assert_eq!(s.v, Vec2::ZERO);
        let since = below_since.expect("speed was below the threshold before sleeping");
        let below_time = (tick - since + 1) as f32 * config.tick_dt();
        assert!(
            below_time >= config.time_to_sleep - 1e-6,
            "below the threshold for {below_time} s"
        );
        // The body remains asleep until woken.
        for k2 in k + 1..traj.tick_count() {
            scene.step_reference_tick(&traj, k2);
            assert!(!scene.state(0).awake);
            assert_eq!(scene.state(0).v, Vec2::ZERO);
        }
        return;
    }
    panic!("the body did not fall asleep");
}

/// Substep count is configurable and defaults to two.
#[test]
fn substeps_are_configurable() {
    assert_eq!(PhysicsConfig::default().substeps, 2);
    let run = |substeps: u32| {
        let mut world = PhysicsWorld::new(PhysicsConfig {
            substeps,
            ..PhysicsConfig::robust_defaults()
        });
        let mob = add_mob(&mut world, Vec2::new(0.5, 0.5));
        world.set_move_input(mob, 1, 0, MoveButtons::RIGHT, true);
        world.step(1).unwrap();
        world.body(mob).linear_velocity().x
    };
    // Acceleration per tick depends on substeps: two give 2.5 m/s, one gives 3.0 m/s.
    assert!((run(2) - 2.5).abs() < 1e-5, "{}", run(2));
    assert!((run(1) - 3.0).abs() < 1e-5, "{}", run(1));
}

/// Controlled bodies ignore one another, as do static bodies, matching the reference behavior.
#[test]
fn controlled_bodies_do_not_collide_with_each_other() {
    let mut world = PhysicsWorld::new(PhysicsConfig::robust_defaults());
    let a = add_mob(&mut world, Vec2::new(0.5, 0.5));
    let _b = add_mob(&mut world, Vec2::new(1.0, 0.5));
    world.set_move_input(a, 1, 0, MoveButtons::RIGHT, true);
    for tick in 1..=30 {
        world.step(tick).unwrap();
        assert_eq!(world.contact_count(), 0);
    }
}
