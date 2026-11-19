//! 2D physics and movement adapted from MIT-licensed RobustToolbox and SS14 code.
//!
//! The five reference trajectories keep position and velocity within 0.001 m and
//! 0.001 m/s of the recorded SS14 values on every tick, with identical sleep states.
//!
//! # Usage
//!
//! ```
//! use crucible_physics::*;
//!
//! let mut world = PhysicsWorld::new(PhysicsConfig::robust_defaults()); // 30 Hz, 2 substeps, Robust defaults
//! let wall = world.add_body(
//!     Body::new(BodyType::Static, Vec2::new(3.5, 0.5))
//!         .with_fixture(Fixture::new(Shape::Polygon(Polygon::rect(Vec2::new(-0.5, -0.5), Vec2::new(0.5, 0.5))))
//!             .with_collision(1, 2)),
//! );
//! let mob = world.add_body(
//!     Body::new(BodyType::KinematicController, Vec2::new(0.5, 0.5))
//!         .with_fixed_rotation(true)
//!         .with_fixture(Fixture::new(Shape::circle(0.35, Vec2::ZERO)).with_density(185.0).with_collision(2, 1))
//!         .with_mover(Mover::default()),
//! );
//! for tick in 1..=60 {
//!     if tick == 1 {
//!         // Press right at the start of tick 1.
//!         world.set_move_input(mob, tick, 0, MoveButtons::RIGHT, true);
//!     }
//!     world.step(tick).expect("step");
//! }
//! // The mover stops at the wall.
//! assert!(world.body(mob).position().x < 3.0 - 0.35 + 0.02);
//! # let _ = wall;
//! ```
//!
//! The simulation step is independent of the ECS world. With the default `world` feature,
//! the `ecs` module synchronizes `Transform`, `Physics`, `Fixtures`, `InputMover`, and
//! `MovementSpeedModifier` in the physics phase. It raises ECS events for contact
//! starts and ends from [`PhysicsWorld::contact_events`].
//! The core `physics` module is `ecs::PhysicsModule`. The solver also runs on its own for
//! deterministic prediction:
//! - Uses `f32` like Robust and implements transcendental functions with `libm`.
//! - Holds no global mutable state.
//! - Reuses solver, island, and broad-phase buffers to avoid allocations after warm-up.
//! - If the step panics, later steps return [`StepError::Poisoned`] until
//!   [`PhysicsWorld::load_state`] restores a snapshot.
//!
//! # Modules and provenance
//!
//! | Module | Purpose | Provenance |
//! |---|---|---|
//! | `math` | `Vec2`, `Rot`, `Transform`, facing angle | MIT and zlib formulas from `Transform.cs`; independent facing angle implementation |
//! | `shape` | Circles, convex polygons, bounds, and mass | Independent implementation with MIT constants from `PhysicsConstants.cs` |
//! | `body` | Body types, fixtures, and mass data | MIT and zlib behavior for waking and velocities from `SharedPhysicsSystem.Components.cs`; remaining code independently implemented |
//! | `collision` | Polygon and circle contact manifolds | `CollisionManager.*.cs`, `Manifold.cs`, and `ClipVertex.cs`, with MIT and zlib notices |
//! | `broadphase` | Dynamic AABB tree | Independent implementation of the Box2D algorithm |
//! | `contact` | Contact lifecycle, impulse transfer, and events | Independent implementation from behavior |
//! | `solver` | Box2D v2 contact solver and two-point block solver | `SharedPhysicsSystem.Solver.cs`, `Contact*Constraint.cs`, `Vector4Helpers.cs`, and `Transform.cs`, with MIT and zlib notices |
//! | `island` | Islands, integration, damping, and sleep | `SharedPhysicsSystem.Island.cs`, with MIT notices |
//! | `mover` | Character movement and sub-tick input | MIT SS14 code from `SharedMoverController.cs`, `SharedMoverController.Input.cs`, and movement components |
//! | `world` | Physics world, substeps, tile friction, and snapshots | Independent implementation from behavior |
//! | `ecs` | Component synchronization, contact events, and the physics module | Independent implementation |
//!
//! Provenance was verified against RobustToolbox v294.0.3 and SS14 52cf1558.
//! See the `@provenance` blocks in source headers and
//! `backend/THIRD_PARTY_NOTICES.md` for source references and license notices.

mod body;
mod broadphase;
mod collision;
mod contact;
#[cfg(feature = "world")]
pub mod ecs;
mod island;
mod math;
mod mover;
mod shape;
mod solver;
mod world;

pub use body::{Body, BodyId, BodyType, Fixture, MassData};
pub use collision::{Manifold, ManifoldPoint, ManifoldType};
pub use contact::{ContactEvent, ContactEventKind};
pub use math::{Rot, Transform, Vec2, facing_angle};
pub use mover::{
    GroundTile, InputMover, MoveButtons, MovementSpeedModifier, Mover, SubTick, accelerate,
    dir_vec_for_buttons, friction, mob_movement, wish_velocity,
};
pub use shape::{
    Aabb, Circle, LINEAR_SLOP, MAX_POLYGON_VERTICES, POLYGON_RADIUS, Polygon, Shape, ShapeError,
    ShapeMass,
};
pub use world::{ContactView, PhysicsConfig, PhysicsSnapshot, PhysicsWorld, StepError};
