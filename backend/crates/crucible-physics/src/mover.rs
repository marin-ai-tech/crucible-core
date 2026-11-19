// @provenance
// source: https://github.com/space-wizards/space-station-14
// ref: 52cf1558aa0f1e8125231a410c12fd090bbc235c
// path: Content.Shared/Movement/Systems/SharedMoverController.cs
// modified: yes
// @end-provenance
// @provenance
// source: https://github.com/space-wizards/space-station-14
// ref: 52cf1558aa0f1e8125231a410c12fd090bbc235c
// path: Content.Shared/Movement/Systems/SharedMoverController.Input.cs
// modified: yes
// @end-provenance
// @provenance
// source: https://github.com/space-wizards/space-station-14
// ref: 52cf1558aa0f1e8125231a410c12fd090bbc235c
// path: Content.Shared/Movement/Components/InputMoverComponent.cs
// modified: yes
// @end-provenance
// @provenance
// source: https://github.com/space-wizards/space-station-14
// ref: 52cf1558aa0f1e8125231a410c12fd090bbc235c
// path: Content.Shared/Movement/Components/MovementSpeedModifierComponent.cs
// modified: yes
// @end-provenance
//
// Provenance classification: MIT at the pinned upstream revision.
// Modified in the port: ported from C# to Rust; HandleMobMovement keeps only the branch "mob on a tile with gravity"
//   (no weightlessness, relay, relative grid rotation, footsteps); Friction, Accelerate, DirVecForButtons,
//   GetVelocityInput, SetMoveInput (sub-tick accumulation), ResetSubtick, SetSprinting; component fields and
//   defaults. The facing angle uses an own formula (Angle.cs is GPL-risk), see math::facing_angle.
//
// Space Station 14 — LICENSE.TXT (verbatim):
// MIT License
//
// Copyright (c) 2017-2026 Space Wizards Federation
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

//! Intent-driven character movement adapted from the SS14 `SharedMoverController`.
//!
//! Held [`MoveButtons`] represent movement intent. Changes occur at a `sub_tick`
//! from 0 to 65535. Walk and sprint vectors accumulate their proportions of the
//! tick so each intent receives weight based on how long it was held.

use crate::math::Vec2;

/// Held movement buttons as flags in a byte.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct MoveButtons(pub u8);

impl MoveButtons {
    pub const NONE: MoveButtons = MoveButtons(0);
    pub const UP: MoveButtons = MoveButtons(1);
    pub const DOWN: MoveButtons = MoveButtons(2);
    pub const LEFT: MoveButtons = MoveButtons(4);
    pub const RIGHT: MoveButtons = MoveButtons(8);
    /// Walk instead of sprinting.
    pub const WALK: MoveButtons = MoveButtons(16);
    pub const ANY_DIRECTION: MoveButtons = MoveButtons(1 | 2 | 4 | 8);

    #[inline]
    pub fn contains(self, flag: MoveButtons) -> bool {
        (self.0 & flag.0) == flag.0
    }

    #[inline]
    pub fn with(self, flag: MoveButtons, enabled: bool) -> MoveButtons {
        if enabled {
            MoveButtons(self.0 | flag.0)
        } else {
            MoveButtons(self.0 & !flag.0)
        }
    }
}

impl core::ops::BitOr for MoveButtons {
    type Output = MoveButtons;
    fn bitor(self, o: MoveButtons) -> MoveButtons {
        MoveButtons(self.0 | o.0)
    }
}

/// Input position within a tick, from 0 at the start to `u16::MAX` at the end.
pub type SubTick = u16;

/// Entity movement intent (`InputMoverComponent`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InputMover {
    held_move_buttons: MoveButtons,
    cur_tick_walk_movement: Vec2,
    cur_tick_sprint_movement: Vec2,
    last_input_tick: u32,
    last_input_sub_tick: SubTick,
    /// Desired velocity from the last substep (`WishDir`).
    wish_dir: Vec2,
    /// Whether the entity can move (`CanMove`). If false, character movement is skipped.
    pub can_move: bool,
}

impl Default for InputMover {
    fn default() -> Self {
        Self {
            held_move_buttons: MoveButtons::NONE,
            cur_tick_walk_movement: Vec2::ZERO,
            cur_tick_sprint_movement: Vec2::ZERO,
            last_input_tick: 0,
            last_input_sub_tick: 0,
            wish_dir: Vec2::ZERO,
            can_move: true,
        }
    }
}

impl InputMover {
    /// Held movement buttons (`HeldMoveButtons`).
    #[inline]
    pub fn held_move_buttons(&self) -> MoveButtons {
        self.held_move_buttons
    }

    /// Whether the entity is sprinting, meaning the walk button is not held.
    #[inline]
    pub fn sprinting(&self) -> bool {
        !self.held_move_buttons.contains(MoveButtons::WALK)
    }

    /// Desired velocity from the last substep (`WishDir`).
    #[inline]
    pub fn wish_dir(&self) -> Vec2 {
        self.wish_dir
    }

    /// Input tick and sub-tick of the most recent command.
    #[inline]
    pub fn last_input(&self) -> (u32, SubTick) {
        (self.last_input_tick, self.last_input_sub_tick)
    }

    /// Replicated input state contains held buttons without accumulated sub-tick values.
    pub fn set_held_move_buttons(&mut self, buttons: MoveButtons) {
        self.held_move_buttons = buttons;
    }

    /// Changes authority over locomotion and clears accumulated input at the boundary.
    pub fn set_can_move(&mut self, can_move: bool) -> bool {
        if self.can_move == can_move {
            return false;
        }
        self.can_move = can_move;
        self.held_move_buttons = MoveButtons::NONE;
        self.last_input_sub_tick = 0;
        self.cur_tick_walk_movement = Vec2::ZERO;
        self.cur_tick_sprint_movement = Vec2::ZERO;
        self.wish_dir = Vec2::ZERO;
        true
    }

    /// Changes `button` at `sub_tick` of `tick`, corresponding to `SetMoveInput`.
    /// Use the current simulation tick, which is also the tick passed to the physics step.
    pub fn set_move_input(
        &mut self,
        tick: u32,
        sub_tick: SubTick,
        button: MoveButtons,
        enabled: bool,
        diagonal: bool,
    ) {
        self.reset_subtick(tick);
        if sub_tick >= self.last_input_sub_tick {
            let fraction = f32::from(sub_tick - self.last_input_sub_tick) / f32::from(u16::MAX);
            let add = dir_vec_for_buttons(self.held_move_buttons, diagonal) * fraction;
            if self.sprinting() {
                self.cur_tick_sprint_movement += add;
            } else {
                self.cur_tick_walk_movement += add;
            }
            self.last_input_sub_tick = sub_tick;
        }
        self.held_move_buttons = self.held_move_buttons.with(button, enabled);
    }

    /// Sets one directional button, corresponding to `SetVelocityDirection`.
    pub fn set_velocity_direction(
        &mut self,
        tick: u32,
        sub_tick: SubTick,
        direction: MoveButtons,
        enabled: bool,
        diagonal: bool,
    ) {
        self.set_move_input(tick, sub_tick, direction, enabled, diagonal);
    }

    /// Sets the walk button to `walking`, corresponding to `SetSprinting`.
    pub fn set_sprinting(&mut self, tick: u32, sub_tick: SubTick, walking: bool, diagonal: bool) {
        self.set_move_input(tick, sub_tick, MoveButtons::WALK, walking, diagonal);
    }

    fn reset_subtick(&mut self, tick: u32) {
        if tick <= self.last_input_tick {
            return;
        }
        self.cur_tick_walk_movement = Vec2::ZERO;
        self.cur_tick_sprint_movement = Vec2::ZERO;
        self.last_input_tick = tick;
        self.last_input_sub_tick = 0;
    }

    /// Computes walk and sprint directions weighted by how long they were held in `tick`.
    pub fn velocity_input(&self, tick: u32, diagonal: bool) -> (Vec2, Vec2) {
        let (mut walk, mut sprint, remaining_fraction) = if tick > self.last_input_tick {
            (Vec2::ZERO, Vec2::ZERO, 1.0f32)
        } else {
            (
                self.cur_tick_walk_movement,
                self.cur_tick_sprint_movement,
                f32::from(u16::MAX - self.last_input_sub_tick) / f32::from(u16::MAX),
            )
        };
        let cur_dir = dir_vec_for_buttons(self.held_move_buttons, diagonal) * remaining_fraction;
        if self.sprinting() {
            sprint += cur_dir;
        } else {
            walk += cur_dir;
        }
        (walk, sprint)
    }
}

/// Computes a normalized direction from the buttons. Opposite directions cancel.
/// With diagonals disabled, vertical input applies only in the absence of horizontal input.
pub fn dir_vec_for_buttons(buttons: MoveButtons, diagonal: bool) -> Vec2 {
    let mut x = 0i32;
    x -= i32::from(buttons.contains(MoveButtons::LEFT));
    x += i32::from(buttons.contains(MoveButtons::RIGHT));
    let mut y = 0i32;
    if diagonal || x == 0 {
        y -= i32::from(buttons.contains(MoveButtons::DOWN));
        y += i32::from(buttons.contains(MoveButtons::UP));
    }
    let v = Vec2::new(x as f32, y as f32);
    if v.length_squared() > 1.0e-6 {
        v.normalized()
    } else {
        v
    }
}

/// Resolved movement speeds and friction (`MovementSpeedModifierComponent`).
///
/// The ECS supplies resolved `acceleration`, `friction`, and `friction_no_input`.
/// SS14 computes these from base values and modifiers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MovementSpeedModifier {
    pub base_walk_speed: f32,
    pub base_sprint_speed: f32,
    pub walk_speed_modifier: f32,
    pub sprint_speed_modifier: f32,
    pub acceleration: f32,
    pub friction: f32,
    pub friction_no_input: f32,
    /// Speed threshold below which movement friction does not apply.
    pub minimum_friction_speed: f32,
}

impl MovementSpeedModifier {
    pub const DEFAULT_ACCELERATION: f32 = 20.0;
    /// Component base friction (`BaseFriction`): 2.5.
    pub const DEFAULT_BASE_FRICTION: f32 = 2.5;
    /// Movement friction uses the `physics.tile_friction` multiplier of 8. A reference
    /// walk recording without modifiers reports `BaseFriction` of 2.5 and
    /// `Friction` and `FrictionNoInput` both equal to 20.
    pub const BASE_FRICTION_MULTIPLIER: f32 = 8.0;
    /// Effective friction with input: `BaseFriction` × `physics.tile_friction` = 20.
    pub const DEFAULT_FRICTION: f32 = Self::DEFAULT_BASE_FRICTION * Self::BASE_FRICTION_MULTIPLIER;
    /// Effective friction without input: the same value of 20 as with input.
    pub const DEFAULT_FRICTION_NO_INPUT: f32 = Self::DEFAULT_FRICTION;
    pub const DEFAULT_MINIMUM_FRICTION_SPEED: f32 = 0.005;
    pub const DEFAULT_BASE_WALK_SPEED: f32 = 2.5;
    pub const DEFAULT_BASE_SPRINT_SPEED: f32 = 4.5;

    /// `CurrentWalkSpeed`.
    #[inline]
    pub fn current_walk_speed(&self) -> f32 {
        self.walk_speed_modifier * self.base_walk_speed
    }

    /// `CurrentSprintSpeed`.
    #[inline]
    pub fn current_sprint_speed(&self) -> f32 {
        self.sprint_speed_modifier * self.base_sprint_speed
    }
}

impl Default for MovementSpeedModifier {
    fn default() -> Self {
        Self {
            base_walk_speed: Self::DEFAULT_BASE_WALK_SPEED,
            base_sprint_speed: Self::DEFAULT_BASE_SPRINT_SPEED,
            walk_speed_modifier: 1.0,
            sprint_speed_modifier: 1.0,
            acceleration: Self::DEFAULT_ACCELERATION,
            friction: Self::DEFAULT_FRICTION,
            friction_no_input: Self::DEFAULT_FRICTION_NO_INPUT,
            minimum_friction_speed: Self::DEFAULT_MINIMUM_FRICTION_SPEED,
        }
    }
}

/// Ground properties that affect movement (`ContentTileDefinition`: `Friction`, `MobFriction`,
/// `MobAcceleration`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GroundTile {
    pub friction: f32,
    pub mob_friction: Option<f32>,
    pub mob_acceleration: Option<f32>,
}

impl Default for GroundTile {
    fn default() -> Self {
        Self {
            friction: 1.0,
            mob_friction: None,
            mob_acceleration: None,
        }
    }
}

/// Character movement intent and speeds (`InputMover` and `MovementSpeedModifier`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Mover {
    pub input: InputMover,
    pub speed: MovementSpeedModifier,
}

/// Reduces movement velocity during a substep using the island solver damping formula.
pub fn friction(minimum_friction_speed: f32, frame_time: f32, friction: f32, velocity: &mut Vec2) {
    let speed = velocity.length();
    if speed < minimum_friction_speed {
        return;
    }
    *velocity *= (1.0 - frame_time * friction).clamp(0.0, 1.0);
}

/// Accelerates toward the desired velocity.
pub fn accelerate(current_velocity: &mut Vec2, velocity: Vec2, accel: f32, frame_time: f32) {
    let wish_dir = if velocity != Vec2::ZERO {
        velocity.normalized()
    } else {
        Vec2::ZERO
    };
    let wish_speed = velocity.length();
    let current_speed = current_velocity.dot(wish_dir);
    let add_speed = wish_speed - current_speed;
    if add_speed <= 0.0 {
        return;
    }
    let mut accel_speed = accel * frame_time * wish_speed;
    accel_speed = accel_speed.min(add_speed);
    *current_velocity += wish_dir * accel_speed;
}

/// Desired tick velocity from the walk and sprint directions weighted by their respective speeds.
pub fn wish_velocity(mover: &Mover, tick: u32, diagonal: bool) -> Vec2 {
    let (walk_dir, sprint_dir) = mover.input.velocity_input(tick, diagonal);
    walk_dir * mover.speed.current_walk_speed() + sprint_dir * mover.speed.current_sprint_speed()
}

/// Computes the substep velocity for a character on a tile with gravity.
pub fn mob_movement(
    speed: &MovementSpeedModifier,
    ground: &GroundTile,
    wish_dir: Vec2,
    velocity: Vec2,
    frame_time: f32,
    min_damping: f32,
) -> Vec2 {
    let mut velocity = velocity;
    let mut fr = if wish_dir != Vec2::ZERO {
        speed.friction * ground.mob_friction.unwrap_or(ground.friction)
    } else {
        speed.friction_no_input * ground.friction
    };
    let accel = speed.acceleration * ground.mob_acceleration.unwrap_or(1.0);
    // While there is movement intent, cap friction at the acceleration rate.
    if wish_dir != Vec2::ZERO {
        fr = fr.min(accel);
    }
    fr = fr.max(min_damping);
    friction(speed.minimum_friction_speed, frame_time, fr, &mut velocity);
    accelerate(&mut velocity, wish_dir, accel, frame_time);
    velocity
}

/// Stores the substep's desired velocity (`SetWishDir`).
#[inline]
pub(crate) fn set_wish_dir(input: &mut InputMover, wish_dir: Vec2) {
    input.wish_dir = wish_dir;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Matches reference sprint acceleration without modifiers at 2 substeps of 1/60 s:
    /// 2.5, 3.611, and 4.105 m/s on consecutive ticks, using friction 20.
    #[test]
    fn default_mob_accelerates_like_reference() {
        let speed = MovementSpeedModifier::default();
        assert_eq!(speed.friction, 20.0);
        assert_eq!(speed.friction_no_input, 20.0);
        let ground = GroundTile::default();
        let wish = Vec2::new(speed.current_sprint_speed(), 0.0);
        let mut v = Vec2::ZERO;
        for expected in [2.5, 3.611_11, 4.104_94] {
            for _ in 0..2 {
                v = mob_movement(&speed, &ground, wish, v, 1.0 / 60.0, 0.0);
            }
            assert!(
                (v.x - expected).abs() < 1.0e-4,
                "got {}, expected {expected}",
                v.x
            );
        }
    }

    #[test]
    fn opposite_buttons_cancel() {
        let v = dir_vec_for_buttons(
            MoveButtons::LEFT | MoveButtons::RIGHT | MoveButtons::UP,
            true,
        );
        assert_eq!(v, Vec2::new(0.0, 1.0));
    }

    #[test]
    fn diagonal_disabled_prefers_horizontal() {
        let v = dir_vec_for_buttons(MoveButtons::RIGHT | MoveButtons::UP, false);
        assert_eq!(v, Vec2::new(1.0, 0.0));
    }

    #[test]
    fn input_at_tick_start_counts_whole_tick() {
        let mut m = InputMover::default();
        m.set_move_input(5, 0, MoveButtons::RIGHT, true, true);
        let (walk, sprint) = m.velocity_input(5, true);
        assert_eq!(walk, Vec2::ZERO);
        assert_eq!(sprint, Vec2::new(1.0, 0.0));
        // The input remains held for the entire next tick with no new commands.
        assert_eq!(m.velocity_input(6, true).1, Vec2::new(1.0, 0.0));
    }
}
