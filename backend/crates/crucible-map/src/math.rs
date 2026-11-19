//! Map geometry with `f32` vectors, integer indices, rectangles, and rotation.
//!
//! Positions use `f32`, like Robust's `Vector2`. Angles use `f64` radians, like
//! Robust's `Angle`.

use std::ops::{Add, AddAssign, Mul, Neg, Sub, SubAssign};

/// Two-dimensional vector in meters, with one meter per tile.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    /// Zero vector.
    pub const ZERO: Vec2 = Vec2 { x: 0.0, y: 0.0 };

    pub const fn new(x: f32, y: f32) -> Vec2 {
        Vec2 { x, y }
    }

    /// Squared length.
    pub fn length_squared(self) -> f32 {
        self.x * self.x + self.y * self.y
    }

    /// Length.
    pub fn length(self) -> f32 {
        self.length_squared().sqrt()
    }

    /// Rotates counterclockwise by `angle` radians using `f64` arithmetic.
    pub fn rotated(self, angle: f64) -> Vec2 {
        if angle == 0.0 {
            return self;
        }
        let (sin, cos) = angle.sin_cos();
        let (x, y) = (f64::from(self.x), f64::from(self.y));
        Vec2::new((x * cos - y * sin) as f32, (x * sin + y * cos) as f32)
    }

    /// Index of the cell of side length `size` containing the point, rounded down.
    pub fn floor_div(self, size: f32) -> Vector2i {
        Vector2i::new(
            (self.x / size).floor() as i32,
            (self.y / size).floor() as i32,
        )
    }

    /// Tests whether corresponding coordinates are within the given tolerance.
    pub fn approx_eq(self, other: Vec2, eps: f32) -> bool {
        (self.x - other.x).abs() <= eps && (self.y - other.y).abs() <= eps
    }
}

impl Add for Vec2 {
    type Output = Vec2;
    fn add(self, rhs: Vec2) -> Vec2 {
        Vec2::new(self.x + rhs.x, self.y + rhs.y)
    }
}

impl AddAssign for Vec2 {
    fn add_assign(&mut self, rhs: Vec2) {
        *self = *self + rhs;
    }
}

impl Sub for Vec2 {
    type Output = Vec2;
    fn sub(self, rhs: Vec2) -> Vec2 {
        Vec2::new(self.x - rhs.x, self.y - rhs.y)
    }
}

impl SubAssign for Vec2 {
    fn sub_assign(&mut self, rhs: Vec2) {
        *self = *self - rhs;
    }
}

impl Mul<f32> for Vec2 {
    type Output = Vec2;
    fn mul(self, rhs: f32) -> Vec2 {
        Vec2::new(self.x * rhs, self.y * rhs)
    }
}

impl Neg for Vec2 {
    type Output = Vec2;
    fn neg(self) -> Vec2 {
        Vec2::new(-self.x, -self.y)
    }
}

/// Integer pair for a tile or chunk index in a grid.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Vector2i {
    pub x: i32,
    pub y: i32,
}

impl Vector2i {
    pub const fn new(x: i32, y: i32) -> Vector2i {
        Vector2i { x, y }
    }
}

/// Axis-aligned rectangle from lower-left `min` to upper-right `max`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Box2 {
    pub min: Vec2,
    pub max: Vec2,
}

impl Box2 {
    /// Rectangle from two corners given in either order.
    pub fn new(a: Vec2, b: Vec2) -> Box2 {
        Box2 {
            min: Vec2::new(a.x.min(b.x), a.y.min(b.y)),
            max: Vec2::new(a.x.max(b.x), a.y.max(b.y)),
        }
    }

    /// Square with side length `2 * half` around `center`.
    pub fn centered(center: Vec2, half: f32) -> Box2 {
        Box2 {
            min: Vec2::new(center.x - half, center.y - half),
            max: Vec2::new(center.x + half, center.y + half),
        }
    }

    /// Whether the rectangle contains the point or touches it on its boundary.
    pub fn contains(&self, p: Vec2) -> bool {
        p.x >= self.min.x && p.x <= self.max.x && p.y >= self.min.y && p.y <= self.max.y
    }

    /// The four corners.
    pub fn corners(&self) -> [Vec2; 4] {
        [
            self.min,
            Vec2::new(self.max.x, self.min.y),
            self.max,
            Vec2::new(self.min.x, self.max.y),
        ]
    }

    /// Smallest axis-aligned rectangle containing the nonempty set of points.
    pub fn bounding(points: &[Vec2]) -> Box2 {
        let mut b = Box2 {
            min: points[0],
            max: points[0],
        };
        for p in &points[1..] {
            b.min = Vec2::new(b.min.x.min(p.x), b.min.y.min(p.y));
            b.max = Vec2::new(b.max.x.max(p.x), b.max.y.max(p.y));
        }
        b
    }
}
