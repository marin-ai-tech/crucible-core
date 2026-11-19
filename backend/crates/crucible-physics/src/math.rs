// @provenance
// source: https://github.com/space-wizards/RobustToolbox
// ref: v294.0.3
// path: Robust.Shared/Physics/Transform.cs
// modified: yes
// @end-provenance
//
// Provenance classification: MIT at the pinned upstream revision.
// Modified in the port: ported from C# to Rust: the operation order of Transform.Mul/MulT and Quaternion2D rotation (cos/sin from libm).
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

//! Two-dimensional `f32` math for the solver. Transform formulas follow Robust's `Transform.cs`.
//!
//! Matches `System.Numerics.Vector2` and `Robust.Shared.Physics.Transform` arithmetic order
//! where it affects rounding. The goal is bitwise agreement with SS14 prediction where
//! possible and agreement within the reference tolerances otherwise. `libm` supplies
//! platform-independent transcendental functions, while IEEE 754 defines the
//! correctly rounded square root.

use core::ops::{Add, AddAssign, Mul, MulAssign, Neg, Sub, SubAssign};

/// Two-dimensional vector, corresponding to Robust's `System.Numerics.Vector2`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    pub const ZERO: Vec2 = Vec2 { x: 0.0, y: 0.0 };

    #[inline]
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    #[inline]
    pub fn dot(self, o: Vec2) -> f32 {
        self.x * o.x + self.y * o.y
    }

    #[inline]
    pub fn length_squared(self) -> f32 {
        self.dot(self)
    }

    #[inline]
    pub fn length(self) -> f32 {
        self.length_squared().sqrt()
    }

    /// Unit vector in the same direction. Normalizing zero returns NaN, as in .NET.
    #[inline]
    pub fn normalized(self) -> Vec2 {
        let l = self.length();
        Vec2::new(self.x / l, self.y / l)
    }

    /// Scalar cross product a × b.
    #[inline]
    pub fn cross(self, o: Vec2) -> f32 {
        self.x * o.y - self.y * o.x
    }

    /// a × s = (s·a.y, −s·a.x).
    #[inline]
    pub fn cross_scalar(self, s: f32) -> Vec2 {
        Vec2::new(s * self.y, -s * self.x)
    }

    /// s × a = (−s·a.y, s·a.x).
    #[inline]
    pub fn scalar_cross(s: f32, a: Vec2) -> Vec2 {
        Vec2::new(-s * a.y, s * a.x)
    }

    /// Componentwise minimum.
    #[inline]
    pub fn min(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x.min(o.x), self.y.min(o.y))
    }

    /// Componentwise maximum.
    #[inline]
    pub fn max(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x.max(o.x), self.y.max(o.y))
    }

    /// Whether both components are finite.
    #[inline]
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }
}

impl Add for Vec2 {
    type Output = Vec2;
    #[inline]
    fn add(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x + o.x, self.y + o.y)
    }
}

impl Sub for Vec2 {
    type Output = Vec2;
    #[inline]
    fn sub(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x - o.x, self.y - o.y)
    }
}

impl Neg for Vec2 {
    type Output = Vec2;
    #[inline]
    fn neg(self) -> Vec2 {
        Vec2::new(-self.x, -self.y)
    }
}

impl Mul<f32> for Vec2 {
    type Output = Vec2;
    #[inline]
    fn mul(self, s: f32) -> Vec2 {
        Vec2::new(self.x * s, self.y * s)
    }
}

impl AddAssign for Vec2 {
    #[inline]
    fn add_assign(&mut self, o: Vec2) {
        self.x += o.x;
        self.y += o.y;
    }
}

impl SubAssign for Vec2 {
    #[inline]
    fn sub_assign(&mut self, o: Vec2) {
        self.x -= o.x;
        self.y -= o.y;
    }
}

impl MulAssign<f32> for Vec2 {
    #[inline]
    fn mul_assign(&mut self, s: f32) {
        self.x *= s;
        self.y *= s;
    }
}

/// Rotation represented as (cos, sin), corresponding to Robust's `Quaternion2D`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rot {
    pub c: f32,
    pub s: f32,
}

impl Rot {
    pub const IDENTITY: Rot = Rot { c: 1.0, s: 0.0 };

    /// Rotation by an angle in radians, using `cosf` and `sinf` from `libm`.
    #[inline]
    pub fn from_angle(angle: f32) -> Self {
        Self {
            c: libm::cosf(angle),
            s: libm::sinf(angle),
        }
    }

    /// q · v.
    #[inline]
    pub fn apply(self, v: Vec2) -> Vec2 {
        Vec2::new(self.c * v.x - self.s * v.y, self.s * v.x + self.c * v.y)
    }

    /// qᵀ · v.
    #[inline]
    pub fn apply_inv(self, v: Vec2) -> Vec2 {
        Vec2::new(self.c * v.x + self.s * v.y, -self.s * v.x + self.c * v.y)
    }

    /// qᵀ · r.
    #[inline]
    pub fn inv_mul(self, r: Rot) -> Rot {
        Rot {
            s: self.c * r.s - self.s * r.c,
            c: self.c * r.c + self.s * r.s,
        }
    }
}

/// Body-to-world transform with position and rotation, corresponding to `Robust.Shared.Physics.Transform`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub p: Vec2,
    pub q: Rot,
}

impl Transform {
    pub const IDENTITY: Transform = Transform {
        p: Vec2::ZERO,
        q: Rot::IDENTITY,
    };

    #[inline]
    pub fn new(p: Vec2, angle: f32) -> Self {
        Self {
            p,
            q: Rot::from_angle(angle),
        }
    }

    /// T · v converts a body-local point to world coordinates.
    #[inline]
    pub fn apply(self, v: Vec2) -> Vec2 {
        let x = (self.q.c * v.x - self.q.s * v.y) + self.p.x;
        let y = (self.q.s * v.x + self.q.c * v.y) + self.p.y;
        Vec2::new(x, y)
    }

    /// Tᵀ · v converts a world point to body-local coordinates.
    #[inline]
    pub fn apply_inv(self, v: Vec2) -> Vec2 {
        let px = v.x - self.p.x;
        let py = v.y - self.p.y;
        Vec2::new(
            self.q.c * px + self.q.s * py,
            -self.q.s * px + self.q.c * py,
        )
    }

    /// Aᵀ · B converts coordinates from frame B to frame A.
    #[inline]
    pub fn inv_mul(self, b: Transform) -> Transform {
        Transform {
            q: self.q.inv_mul(b.q),
            p: self.q.apply_inv(b.p - self.p),
        }
    }
}

/// Facing angle for the desired movement, counterclockwise from south.
///
/// Independently implements the behavior of Robust's `ToWorldAngle`.
/// East is π/2, north is π, and southwest is −π/4. Uses `f64` and `libm`'s `atan2`.
pub fn facing_angle(dir: Vec2) -> f32 {
    use core::f64::consts::{FRAC_PI_2, PI, TAU};
    let a = libm::atan2(f64::from(dir.y), f64::from(dir.x)) + FRAC_PI_2;
    let a = if a > PI { a - TAU } else { a };
    a as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn facing_angle_matches_observed_directions() {
        let east = facing_angle(Vec2::new(1.0, 0.0));
        let north = facing_angle(Vec2::new(0.0, 1.0));
        let south_west = facing_angle(Vec2::new(-1.0, -1.0));
        assert!((east - core::f32::consts::FRAC_PI_2).abs() < 1e-6);
        assert!((north - core::f32::consts::PI).abs() < 1e-6);
        assert!((south_west + core::f32::consts::FRAC_PI_4).abs() < 1e-6);
    }

    #[test]
    fn transform_round_trip() {
        let t = Transform::new(Vec2::new(1.0, 2.0), 0.7);
        let v = Vec2::new(0.3, -0.4);
        let back = t.apply_inv(t.apply(v));
        assert!((back - v).length() < 1e-6);
        let a = Transform::new(Vec2::new(-1.0, 0.5), 0.2);
        let rel = a.inv_mul(t);
        let w = a.apply(rel.apply(v));
        assert!((w - t.apply(v)).length() < 1e-5);
    }
}
