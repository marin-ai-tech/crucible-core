// @provenance
// source: https://github.com/space-wizards/RobustToolbox
// ref: v294.0.3
// path: Robust.Shared/Physics/Systems/SharedPhysicsSystem.Components.cs
// modified: yes
// @end-provenance
//
// Provenance classification: MIT at the pinned upstream revision.
// Modified in the port: ported from C# to Rust: the semantics of SetAwake, ResetDynamics, SetLinearVelocity, SetAngularVelocity (no events, no dirtying, no container checks).
//
// RobustToolbox — LICENSE-MIT.TXT (verbatim):
// Copyright (c) 2019 Space Station 14 Contributors
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.
//
// Farseer Physics Engine / Box2D (zlib, verbatim from the header of the source file):
// Farseer Physics Engine:
// Copyright (c) 2012 Ian Qvist
//
// Original source Box2D:
// Copyright (c) 2006-2011 Erin Catto http://www.box2d.org
//
// This software is provided 'as-is', without any express or implied
// warranty.  In no event will the authors be held liable for any damages
// arising from the use of this software.
// Permission is granted to anyone to use this software for any purpose,
// including commercial applications, and to alter it and redistribute it
// freely, subject to the following restrictions:
// 1. The origin of this software must not be misrepresented; you must not
// claim that you wrote the original software. If you use this software
// in a product, an acknowledgment in the product documentation would be
// appreciated but is not required.
// 2. Altered source versions must be plainly marked as such, and must not be
// misrepresented as being the original software.
// 3. This notice may not be removed or altered from any source distribution.
//
// ALTERED SOURCE VERSION: ported to Rust and reduced for Crucible (crucible-physics); not the original software.

//! Physics bodies and fixtures. Their names follow Robust, and their fields cover the subset
//! of `PhysicsComponent`, `FixturesComponent`, and `TransformComponent` needed by the step.
//!
//! The port of `SetAwake`, `SetLinearVelocity`, `SetAngularVelocity`, and `ResetDynamics`
//! follows `Robust.Shared/Physics/Systems/SharedPhysicsSystem.Components.cs` (see the file header).
//! Mass calculation is independently implemented in `shape.rs`.

use crate::math::{Transform, Vec2};
use crate::mover::{GroundTile, Mover};
use crate::shape::Shape;

/// Physics body type (`BodyType`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BodyType {
    /// Static body. It stays in place and is excluded from islands.
    Static,
    /// Kinematic body. It moves at its own velocity and contacts leave it unaffected.
    Kinematic,
    /// Kinematic controller driven by player intent.
    /// Dynamic bodies can push it, but it cannot push them. The inverse mass rules enforce this.
    KinematicController,
    /// Dynamic body affected by contacts and damping.
    Dynamic,
}

impl BodyType {
    /// Whether this body is static or kinematic. Two such bodies do not collide.
    #[inline]
    pub(crate) fn is_static_like(self) -> bool {
        matches!(self, BodyType::Static | BodyType::Kinematic)
    }
}

/// Collision shape and its properties.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fixture {
    pub shape: Shape,
    /// Density in kg/m² for calculating body mass.
    pub density: f32,
    pub friction: f32,
    pub restitution: f32,
    /// Hard fixtures participate in the solver. Non-hard fixtures act as sensors.
    pub hard: bool,
    /// Collision layers occupied by this fixture.
    pub collision_layer: u32,
    /// Collision layers this fixture can collide with.
    pub collision_mask: u32,
}

impl Fixture {
    /// Hard fixture with Robust defaults: density 1, friction 0.4, restitution 0, and no layers or masks.
    pub fn new(shape: Shape) -> Self {
        Self {
            shape,
            density: 1.0,
            friction: 0.4,
            restitution: 0.0,
            hard: true,
            collision_layer: 0,
            collision_mask: 0,
        }
    }

    /// Sets the collision layer and mask.
    pub fn with_collision(mut self, layer: u32, mask: u32) -> Self {
        self.collision_layer = layer;
        self.collision_mask = mask;
        self
    }

    /// Sets density.
    pub fn with_density(mut self, density: f32) -> Self {
        self.density = density;
        self
    }

    /// Sets friction.
    pub fn with_friction(mut self, friction: f32) -> Self {
        self.friction = friction;
        self
    }

    /// Makes the fixture a non-hard sensor.
    pub fn sensor(mut self) -> Self {
        self.hard = false;
        self
    }

    /// Whether either fixture's collision mask intersects the other's layers.
    #[inline]
    pub fn layers_overlap(&self, other: &Fixture) -> bool {
        (self.collision_mask & other.collision_layer) != 0
            || (other.collision_mask & self.collision_layer) != 0
    }
}

/// Mass properties of a body.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MassData {
    pub mass: f32,
    pub inv_mass: f32,
    pub inv_inertia: f32,
    /// Center of mass in body coordinates.
    pub local_center: Vec2,
}

/// Body ID in `PhysicsWorld` with a slot index and generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BodyId {
    pub(crate) index: u32,
    pub(crate) generation: u32,
}

impl BodyId {
    /// Slot index. Bodies are visited in ascending slot order.
    #[inline]
    pub fn index(self) -> u32 {
        self.index
    }
}

/// Physics body.
#[derive(Clone, Debug)]
pub struct Body {
    pub(crate) body_type: BodyType,
    /// World position of the body's local origin (`TransformComponent`).
    pub(crate) position: Vec2,
    /// World angle in radians.
    pub(crate) angle: f32,
    pub(crate) linear_velocity: Vec2,
    pub(crate) angular_velocity: f32,
    /// Linear damping in inverse seconds. Tile friction sets it for dynamic bodies on a tile.
    pub linear_damping: f32,
    /// Angular damping in inverse seconds.
    pub angular_damping: f32,
    /// Whether rotation is locked, giving the body infinite inertia.
    pub fixed_rotation: bool,
    /// Whether the body is allowed to sleep.
    pub sleeping_allowed: bool,
    pub(crate) can_collide: bool,
    pub(crate) awake: bool,
    /// Seconds the body has remained below the sleep velocity threshold.
    pub sleep_time: f32,
    pub(crate) fixtures: Vec<Fixture>,
    pub(crate) mass: MassData,
    pub(crate) explicit_mass: bool,
    /// Intent-driven movement (`InputMover` and `MovementSpeedModifier`).
    pub mover: Option<Mover>,
    /// Friction and movement acceleration from the tile beneath the body, set before the step.
    pub ground: GroundTile,
    // Internal state.
    pub(crate) alive: bool,
    pub(crate) generation: u32,
    pub(crate) island: bool,
    pub(crate) island_index: usize,
    pub(crate) used_mob_movement: bool,
    /// External changes to position or fixtures require updating the broad-phase proxies.
    pub(crate) proxies_dirty: bool,
    /// Broad-phase nodes for the fixtures. Empty if the body is outside the tree.
    pub(crate) proxies: Vec<u32>,
}

impl Body {
    /// Creates a body at `position` without fixtures. Static bodies start asleep; others start awake.
    pub fn new(body_type: BodyType, position: Vec2) -> Self {
        Self {
            body_type,
            position,
            angle: 0.0,
            linear_velocity: Vec2::ZERO,
            angular_velocity: 0.0,
            linear_damping: 0.0,
            angular_damping: 0.0,
            fixed_rotation: false,
            sleeping_allowed: true,
            can_collide: true,
            awake: body_type != BodyType::Static,
            sleep_time: 0.0,
            fixtures: Vec::new(),
            mass: MassData::default(),
            explicit_mass: false,
            mover: None,
            ground: GroundTile::default(),
            alive: true,
            generation: 0,
            island: false,
            island_index: 0,
            used_mob_movement: false,
            proxies_dirty: true,
            proxies: Vec::new(),
        }
    }

    /// Adds a fixture before inserting the body into the world. Use `PhysicsWorld::set_fixtures` after insertion.
    pub fn with_fixture(mut self, fixture: Fixture) -> Self {
        self.fixtures.push(fixture);
        self
    }

    /// Specifies mass properties explicitly, for example from a reference trace. Otherwise uses fixture density.
    pub fn with_mass(mut self, mass: MassData) -> Self {
        self.mass = mass;
        self.explicit_mass = true;
        self
    }

    pub fn with_fixed_rotation(mut self, fixed: bool) -> Self {
        self.fixed_rotation = fixed;
        self
    }

    pub fn with_angle(mut self, angle: f32) -> Self {
        self.angle = angle;
        self
    }

    pub fn with_mover(mut self, mover: Mover) -> Self {
        self.mover = Some(mover);
        self
    }

    #[inline]
    pub fn body_type(&self) -> BodyType {
        self.body_type
    }

    #[inline]
    pub fn position(&self) -> Vec2 {
        self.position
    }

    #[inline]
    pub fn angle(&self) -> f32 {
        self.angle
    }

    /// Teleports the body. Broad-phase proxies refresh at the start of the next substep.
    pub fn set_position(&mut self, position: Vec2) {
        self.position = position;
        self.proxies_dirty = true;
    }

    /// Rotates the body.
    pub fn set_angle(&mut self, angle: f32) {
        self.angle = angle;
        self.proxies_dirty = true;
    }

    #[inline]
    pub fn linear_velocity(&self) -> Vec2 {
        self.linear_velocity
    }

    #[inline]
    pub fn angular_velocity(&self) -> f32 {
        self.angular_velocity
    }

    #[inline]
    pub fn awake(&self) -> bool {
        self.awake
    }

    #[inline]
    pub fn can_collide(&self) -> bool {
        self.can_collide
    }

    #[inline]
    pub fn fixtures(&self) -> &[Fixture] {
        &self.fixtures
    }

    #[inline]
    pub fn mass_data(&self) -> MassData {
        self.mass
    }

    /// Transform from body to world coordinates.
    #[inline]
    pub fn transform(&self) -> Transform {
        Transform::new(self.position, self.angle)
    }

    /// Implements `SetAwake`. Static bodies and bodies without collisions cannot wake.
    /// Sleeping resets velocities (`ResetDynamics`), and `update_sleep_time` resets the sleep timer.
    pub fn set_awake(&mut self, value: bool, update_sleep_time: bool) {
        let can_wake = self.body_type != BodyType::Static && self.can_collide;
        if self.awake == value {
            return;
        }
        if value && !can_wake {
            return;
        }
        self.awake = value;
        if !value {
            self.linear_velocity = Vec2::ZERO;
            self.angular_velocity = 0.0;
        }
        if update_sleep_time {
            self.sleep_time = 0.0;
        }
    }

    /// Implements `SetLinearVelocity`. A nonzero velocity wakes the body. Ignores changes within 1e-7.
    /// Returns whether the velocity changed.
    pub fn set_linear_velocity(&mut self, velocity: Vec2) -> bool {
        if self.body_type == BodyType::Static {
            return false;
        }
        if velocity.dot(velocity) > 0.0 {
            self.set_awake(true, true);
            if !self.awake {
                return false;
            }
        }
        if (self.linear_velocity.x - velocity.x).abs() <= 1e-7
            && (self.linear_velocity.y - velocity.y).abs() <= 1e-7
        {
            return false;
        }
        self.linear_velocity = velocity;
        true
    }

    /// Implements `SetAngularVelocity`. A nonzero angular velocity wakes the body.
    pub fn set_angular_velocity(&mut self, velocity: f32) -> bool {
        if self.body_type == BodyType::Static {
            return false;
        }
        if velocity * velocity > 0.0 {
            self.set_awake(true, true);
            if !self.awake {
                return false;
            }
        }
        if self.angular_velocity == velocity {
            return false;
        }
        self.angular_velocity = velocity;
        true
    }

    /// Recomputes mass from fixture density. Static and kinematic bodies have zero
    /// inverse mass. A dynamic body without mass defaults to 1, as in Box2D.
    pub fn reset_mass_data(&mut self) {
        if self.explicit_mass {
            return;
        }
        let mut md = MassData::default();
        if matches!(self.body_type, BodyType::Static | BodyType::Kinematic) {
            self.mass = md;
            return;
        }
        let mut center = Vec2::ZERO;
        let mut inertia = 0.0f32;
        for f in &self.fixtures {
            if f.density <= 0.0 {
                continue;
            }
            let m = f.shape.mass(f.density);
            md.mass += m.mass;
            center += m.center * m.mass;
            inertia += m.inertia;
        }
        if md.mass > 0.0 {
            md.inv_mass = 1.0 / md.mass;
            center *= md.inv_mass;
        } else if self.body_type == BodyType::Dynamic {
            md.mass = 1.0;
            md.inv_mass = 1.0;
        }
        if inertia > 0.0 && !self.fixed_rotation {
            let i_center = inertia - md.mass * center.length_squared();
            if i_center > 0.0 {
                md.inv_inertia = 1.0 / i_center;
            }
        }
        md.local_center = center;
        self.mass = md;
    }
}
