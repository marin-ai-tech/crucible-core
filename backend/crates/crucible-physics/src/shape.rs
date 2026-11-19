// @provenance
// source: https://github.com/space-wizards/RobustToolbox
// ref: v294.0.3
// path: Robust.Shared/Physics/PhysicsConstants.cs
// modified: yes
// @end-provenance
//
// Provenance classification: MIT at the pinned upstream revision.
// Modified in the port: only the constants LinearSlop, PolygonRadius, MaxPolygonVertices are taken; the rest of the module is own code.
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

//! Fixture circles and convex polygons, axis-aligned bounds, and density-based mass.
//!
//! Independent implementation except for the constants from `PhysicsConstants.cs`
//! listed in the file header. Mass and inertia use the standard formulas for area,
//! centroid, and moment of inertia. `LINEAR_SLOP` and `POLYGON_RADIUS` match
//! the pinned MIT-licensed `PhysicsConstants.cs`.

use crate::math::{Transform, Vec2};

/// Penetration tolerance (`PhysicsConstants.LinearSlop`).
pub const LINEAR_SLOP: f32 = 0.005;
/// Polygon collision skin (`PhysicsConstants.PolygonRadius` = 2 × LinearSlop).
pub const POLYGON_RADIUS: f32 = 2.0 * LINEAR_SLOP;
/// Maximum polygon vertex count (`PhysicsConstants.MaxPolygonVertices`).
pub const MAX_POLYGON_VERTICES: usize = 8;

/// Axis-aligned bounding box.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Aabb {
    pub lower: Vec2,
    pub upper: Vec2,
}

impl Aabb {
    #[inline]
    pub fn new(lower: Vec2, upper: Vec2) -> Self {
        Self { lower, upper }
    }

    /// Whether the bounds intersect or touch.
    #[inline]
    pub fn overlaps(&self, o: &Aabb) -> bool {
        !(o.upper.x < self.lower.x
            || o.upper.y < self.lower.y
            || self.upper.x < o.lower.x
            || self.upper.y < o.lower.y)
    }

    /// Whether these bounds fully contain `o`.
    #[inline]
    pub fn contains(&self, o: &Aabb) -> bool {
        self.lower.x <= o.lower.x
            && self.lower.y <= o.lower.y
            && o.upper.x <= self.upper.x
            && o.upper.y <= self.upper.y
    }

    #[inline]
    pub fn union(&self, o: &Aabb) -> Aabb {
        Aabb::new(self.lower.min(o.lower), self.upper.max(o.upper))
    }

    #[inline]
    pub fn perimeter(&self) -> f32 {
        2.0 * ((self.upper.x - self.lower.x) + (self.upper.y - self.lower.y))
    }

    #[inline]
    pub fn enlarged(&self, margin: f32) -> Aabb {
        let m = Vec2::new(margin, margin);
        Aabb::new(self.lower - m, self.upper + m)
    }
}

/// Circle (`PhysShapeCircle`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Circle {
    pub radius: f32,
    /// Center in body coordinates.
    pub position: Vec2,
}

/// Counterclockwise convex polygon (`PolygonShape`) with up to eight vertices and no allocations.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Polygon {
    vertices: [Vec2; MAX_POLYGON_VERTICES],
    normals: [Vec2; MAX_POLYGON_VERTICES],
    count: u8,
    /// Polygon collision skin, defaulting to `POLYGON_RADIUS`.
    pub radius: f32,
}

/// Error constructing a shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShapeError {
    /// Fewer than three or more than eight vertices.
    VertexCount,
    /// Normal count differs from the vertex count.
    NormalCount,
    /// Zero-length edge, clockwise vertex order, or non-convex polygon.
    NotConvex,
}

impl Polygon {
    /// Builds a polygon from counterclockwise vertices and computes edge normals.
    pub fn new(vertices: &[Vec2]) -> Result<Self, ShapeError> {
        let n = vertices.len();
        if !(3..=MAX_POLYGON_VERTICES).contains(&n) {
            return Err(ShapeError::VertexCount);
        }
        let mut normals = [Vec2::ZERO; MAX_POLYGON_VERTICES];
        for i in 0..n {
            let edge = vertices[(i + 1) % n] - vertices[i];
            if edge.length_squared() <= f32::EPSILON * f32::EPSILON {
                return Err(ShapeError::NotConvex);
            }
            normals[i] = edge.cross_scalar(1.0).normalized();
        }
        let p = Self::from_parts(vertices, &normals[..n], POLYGON_RADIUS)?;
        p.check_convex()?;
        Ok(p)
    }

    /// Builds a polygon with supplied normals and skin, as in recorded numeric fixtures.
    pub fn with_normals(
        vertices: &[Vec2],
        normals: &[Vec2],
        radius: f32,
    ) -> Result<Self, ShapeError> {
        let p = Self::from_parts(vertices, normals, radius)?;
        p.check_convex()?;
        Ok(p)
    }

    /// Rectangle between `min` and `max` in body coordinates.
    pub fn rect(min: Vec2, max: Vec2) -> Self {
        let verts = [
            Vec2::new(min.x, min.y),
            Vec2::new(max.x, min.y),
            Vec2::new(max.x, max.y),
            Vec2::new(min.x, max.y),
        ];
        let normals = [
            Vec2::new(0.0, -1.0),
            Vec2::new(1.0, 0.0),
            Vec2::new(0.0, 1.0),
            Vec2::new(-1.0, 0.0),
        ];
        Self::from_parts(&verts, &normals, POLYGON_RADIUS).expect("a rectangle has 4 vertices")
    }

    fn from_parts(vertices: &[Vec2], normals: &[Vec2], radius: f32) -> Result<Self, ShapeError> {
        let n = vertices.len();
        if !(3..=MAX_POLYGON_VERTICES).contains(&n) {
            return Err(ShapeError::VertexCount);
        }
        if normals.len() != n {
            return Err(ShapeError::NormalCount);
        }
        let mut p = Polygon {
            vertices: [Vec2::ZERO; MAX_POLYGON_VERTICES],
            normals: [Vec2::ZERO; MAX_POLYGON_VERTICES],
            count: n as u8,
            radius,
        };
        p.vertices[..n].copy_from_slice(vertices);
        p.normals[..n].copy_from_slice(normals);
        Ok(p)
    }

    fn check_convex(&self) -> Result<(), ShapeError> {
        let v = self.vertices();
        let n = v.len();
        for i in 0..n {
            let edge = v[(i + 1) % n] - v[i];
            for (j, w) in v.iter().enumerate() {
                if j == i || j == (i + 1) % n {
                    continue;
                }
                if edge.cross(*w - v[i]) <= 0.0 {
                    return Err(ShapeError::NotConvex);
                }
            }
        }
        Ok(())
    }

    #[inline]
    pub fn vertices(&self) -> &[Vec2] {
        &self.vertices[..self.count as usize]
    }

    #[inline]
    pub fn normals(&self) -> &[Vec2] {
        &self.normals[..self.count as usize]
    }

    /// Area, centroid, and moment of inertia about the body origin at unit density.
    fn unit_mass(&self) -> (f32, Vec2, f32) {
        let v = self.vertices();
        let origin = v[0];
        let mut area = 0.0f32;
        let mut center = Vec2::ZERO;
        let mut inertia = 0.0f32;
        const INV3: f32 = 1.0 / 3.0;
        for i in 1..v.len() - 1 {
            let e1 = v[i] - origin;
            let e2 = v[i + 1] - origin;
            let d = e1.cross(e2);
            let tri = 0.5 * d;
            area += tri;
            center += (e1 + e2) * (tri * INV3);
            let int_x2 = e1.x * e1.x + e2.x * e1.x + e2.x * e2.x;
            let int_y2 = e1.y * e1.y + e2.y * e1.y + e2.y * e2.y;
            inertia += (0.25 * INV3 * d) * (int_x2 + int_y2);
        }
        let c = center * (1.0 / area) + origin;
        // Shift the moment of inertia from the offset origin to the body origin.
        let i_origin = inertia + area * (c.length_squared() - (c - origin).length_squared());
        (area, c, i_origin)
    }
}

/// Fixture shape (`IPhysShape`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shape {
    Circle(Circle),
    Polygon(Polygon),
}

/// Mass, center of mass, and moment of inertia about the body origin at a given density.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ShapeMass {
    pub mass: f32,
    pub center: Vec2,
    pub inertia: f32,
}

impl Shape {
    /// Circle of radius `radius` centered at `position`.
    pub fn circle(radius: f32, position: Vec2) -> Self {
        Shape::Circle(Circle { radius, position })
    }

    /// Circle radius or polygon skin radius.
    #[inline]
    pub fn radius(&self) -> f32 {
        match self {
            Shape::Circle(c) => c.radius,
            Shape::Polygon(p) => p.radius,
        }
    }

    /// Robust shape number: 0 for a circle and 2 for a polygon. Body A has the higher number.
    #[inline]
    pub(crate) fn type_index(&self) -> u8 {
        match self {
            Shape::Circle(_) => 0,
            Shape::Polygon(_) => 2,
        }
    }

    /// World-space bounds including the collision skin.
    pub fn aabb(&self, xf: Transform) -> Aabb {
        match self {
            Shape::Circle(c) => {
                let p = xf.apply(c.position);
                let r = Vec2::new(c.radius, c.radius);
                Aabb::new(p - r, p + r)
            }
            Shape::Polygon(poly) => {
                let v = poly.vertices();
                let mut lo = xf.apply(v[0]);
                let mut hi = lo;
                for w in &v[1..] {
                    let w = xf.apply(*w);
                    lo = lo.min(w);
                    hi = hi.max(w);
                }
                let r = Vec2::new(poly.radius, poly.radius);
                Aabb::new(lo - r, hi + r)
            }
        }
    }

    /// Mass properties at `density`, computed by the independent implementation.
    pub fn mass(&self, density: f32) -> ShapeMass {
        match self {
            Shape::Circle(c) => {
                let mass = density * core::f32::consts::PI * c.radius * c.radius;
                let inertia = mass * (0.5 * c.radius * c.radius + c.position.length_squared());
                ShapeMass {
                    mass,
                    center: c.position,
                    inertia,
                }
            }
            Shape::Polygon(p) => {
                let (area, center, inertia) = p.unit_mass();
                ShapeMass {
                    mass: density * area,
                    center,
                    inertia: density * inertia,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polygon_new_computes_unit_normals() {
        let p = Polygon::new(&[
            Vec2::new(0.0, 0.0),
            Vec2::new(2.0, 0.0),
            Vec2::new(0.0, 2.0),
        ])
        .unwrap();
        assert_eq!(p.normals()[0], Vec2::new(0.0, -1.0));
        let n = p.normals()[1];
        assert!(
            (n.x - core::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6
                && (n.y - core::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6
        );
        assert_eq!(p.radius, POLYGON_RADIUS);
    }

    #[test]
    fn polygon_rejects_clockwise_and_bad_counts() {
        let cw = [
            Vec2::new(0.0, 0.0),
            Vec2::new(0.0, 1.0),
            Vec2::new(1.0, 0.0),
        ];
        assert_eq!(Polygon::new(&cw), Err(ShapeError::NotConvex));
        assert_eq!(Polygon::new(&cw[..2]), Err(ShapeError::VertexCount));
    }

    #[test]
    fn rect_mass_and_center() {
        // Reference crate: x −0.4 to 0.4, y −0.4 to 0.29, density 50, mass 27.6, center (0, −0.055).
        let s = Shape::Polygon(Polygon::rect(Vec2::new(-0.4, -0.4), Vec2::new(0.4, 0.29)));
        let m = s.mass(50.0);
        assert!((m.mass - 27.6).abs() < 1e-4, "{m:?}");
        assert!(
            m.center.x.abs() < 1e-6 && (m.center.y + 0.055).abs() < 1e-6,
            "{m:?}"
        );
        // Rectangle inertia about the center is m(w² + h²)/12. Apply the parallel-axis offset.
        let expected = 27.6 * (0.8f32 * 0.8 + 0.69 * 0.69) / 12.0 + 27.6 * 0.055 * 0.055;
        assert!(
            (m.inertia - expected).abs() < 1e-3,
            "{} vs {expected}",
            m.inertia
        );
    }

    #[test]
    fn circle_mass() {
        // Reference character: radius 0.35 and density 185 give mass 71.196.
        let m = Shape::circle(0.35, Vec2::ZERO).mass(185.0);
        assert!((m.mass - 71.196_34).abs() < 1e-3, "{m:?}");
    }

    #[test]
    fn aabb_includes_skin() {
        let s = Shape::Polygon(Polygon::rect(Vec2::new(-0.5, -0.5), Vec2::new(0.5, 0.5)));
        let a = s.aabb(Transform::new(Vec2::new(3.0, 0.0), 0.0));
        assert!((a.lower - Vec2::new(2.49, -0.51)).length() < 1e-6, "{a:?}");
        assert!((a.upper - Vec2::new(3.51, 0.51)).length() < 1e-6, "{a:?}");
    }
}
