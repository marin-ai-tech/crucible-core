// @provenance
// source: https://github.com/space-wizards/RobustToolbox
// ref: v294.0.3
// path: Robust.Shared/Physics/Collision/CollisionManager.PolygonCircle.cs
// modified: yes
// @end-provenance
// @provenance
// source: https://github.com/space-wizards/RobustToolbox
// ref: v294.0.3
// path: Robust.Shared/Physics/Collision/CollisionManager.Polygons.cs
// modified: yes
// @end-provenance
// @provenance
// source: https://github.com/space-wizards/RobustToolbox
// ref: v294.0.3
// path: Robust.Shared/Physics/Collision/CollisionManager.Circles.cs
// modified: yes
// @end-provenance
// @provenance
// source: https://github.com/space-wizards/RobustToolbox
// ref: v294.0.3
// path: Robust.Shared/Physics/Collision/CollisionManager.cs
// modified: yes
// @end-provenance
// @provenance
// source: https://github.com/space-wizards/RobustToolbox
// ref: v294.0.3
// path: Robust.Shared/Physics/Collision/Manifold.cs
// modified: yes
// @end-provenance
// @provenance
// source: https://github.com/space-wizards/RobustToolbox
// ref: v294.0.3
// path: Robust.Shared/Physics/Collision/ClipVertex.cs
// modified: yes
// @end-provenance
//
// Provenance classification: MIT at the pinned upstream revision.
// Modified in the port: ported from C# to Rust; only CollidePolygonAndCircle, CollidePolygons (FindMaxSeparation,
//   FindIncidentEdge), CollideCircles, ClipSegmentToLine and the manifold data are kept; ContactID is a
//   plain u32 key with the same byte layout; no object pools, no debug drawing.
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
// CollisionManager.cs, Manifold.cs, ClipVertex.cs — Farseer Physics Engine / Box2D (zlib, verbatim):
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
//
// CollisionManager.Polygons.cs — Box2D (MIT, verbatim):
// MIT License
//
// Copyright (c) 2019 Erin Catto
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

//! Narrow phase contact manifolds for polygon-circle, polygon-polygon, and circle-circle
//! pairs. Adapted from Robust's `CollisionManager`; see the file header.

use crate::math::{Transform, Vec2};
use crate::shape::{Circle, Polygon, Shape};

/// Manifold type (`ManifoldType`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ManifoldType {
    #[default]
    Invalid,
    Circles,
    FaceA,
    FaceB,
}

/// Contact feature type (`ContactFeatureType`).
const FEATURE_VERTEX: u8 = 0;
const FEATURE_FACE: u8 = 1;

/// Contact point key (`ContactID.Key`). Bytes are `IndexA, IndexB, TypeA, TypeB` in
/// little-endian order, matching the C# `Features` view of `Key`.
#[inline]
fn feature_key(index_a: u8, index_b: u8, type_a: u8, type_b: u8) -> u32 {
    u32::from(index_a)
        | (u32::from(index_b) << 8)
        | (u32::from(type_a) << 16)
        | (u32::from(type_b) << 24)
}

#[inline]
fn swap_features(key: u32) -> u32 {
    let [ia, ib, ta, tb] = key.to_le_bytes();
    feature_key(ib, ia, tb, ta)
}

/// Point in a contact manifold.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ManifoldPoint {
    /// Meaning depends on the manifold type. See Box2D's `b2ManifoldPoint`.
    pub local_point: Vec2,
    /// Accumulated normal impulse for warm starting.
    pub normal_impulse: f32,
    /// Accumulated tangent impulse for warm starting.
    pub tangent_impulse: f32,
    /// Feature key used to transfer impulses across steps.
    pub id: u32,
}

/// Contact manifold with up to two points.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Manifold {
    pub local_normal: Vec2,
    pub local_point: Vec2,
    pub point_count: usize,
    pub points: [ManifoldPoint; 2],
    pub kind: ManifoldType,
}

/// Builds the contact manifold for a pair of shapes. The contact creator orders body A
/// first by the higher shape type number, with polygons before circles.
pub(crate) fn collide(m: &mut Manifold, a: &Shape, xf_a: Transform, b: &Shape, xf_b: Transform) {
    match (a, b) {
        (Shape::Polygon(pa), Shape::Circle(cb)) => {
            collide_polygon_and_circle(m, pa, xf_a, cb, xf_b)
        }
        (Shape::Polygon(pa), Shape::Polygon(pb)) => collide_polygons(m, pa, xf_a, pb, xf_b),
        (Shape::Circle(ca), Shape::Circle(cb)) => collide_circles(m, ca, xf_a, cb, xf_b),
        // Contact creation swaps circle-polygon pairs to polygon-circle order.
        (Shape::Circle(_), Shape::Polygon(_)) => m.point_count = 0,
    }
}

/// `CollideCircles`.
pub(crate) fn collide_circles(
    m: &mut Manifold,
    circle_a: &Circle,
    xf_a: Transform,
    circle_b: &Circle,
    xf_b: Transform,
) {
    m.point_count = 0;
    let p_a = xf_a.apply(circle_a.position);
    let p_b = xf_b.apply(circle_b.position);
    let d = p_b - p_a;
    let dist_sqr = d.dot(d);
    let radius = circle_a.radius + circle_b.radius;
    if dist_sqr > radius * radius {
        return;
    }
    m.kind = ManifoldType::Circles;
    m.local_point = circle_a.position;
    m.local_normal = Vec2::ZERO;
    m.point_count = 1;
    // As in Robust, point B is body B's origin rather than circle B's center.
    m.points[0].local_point = Vec2::ZERO;
    m.points[0].id = 0;
}

/// `CollidePolygonAndCircle`.
pub(crate) fn collide_polygon_and_circle(
    m: &mut Manifold,
    polygon_a: &Polygon,
    xf_a: Transform,
    circle_b: &Circle,
    xf_b: Transform,
) {
    m.point_count = 0;
    // Circle center in polygon coordinates.
    let c = xf_b.apply(circle_b.position);
    let c_local = xf_a.apply_inv(c);

    // Edge with the least separation.
    let mut normal_index = 0usize;
    let mut separation = f32::MIN;
    let radius = polygon_a.radius + circle_b.radius;
    let vertices = polygon_a.vertices();
    let normals = polygon_a.normals();
    let vertex_count = vertices.len();
    for i in 0..vertex_count {
        let s = normals[i].dot(c_local - vertices[i]);
        if s > radius {
            return;
        }
        if s > separation {
            separation = s;
            normal_index = i;
        }
    }

    let vert_index1 = normal_index;
    let vert_index2 = if vert_index1 + 1 < vertex_count {
        vert_index1 + 1
    } else {
        0
    };
    let v1 = vertices[vert_index1];
    let v2 = vertices[vert_index2];

    // The center is inside the polygon.
    if separation < f32::EPSILON {
        m.point_count = 1;
        m.kind = ManifoldType::FaceA;
        m.local_normal = normals[normal_index];
        m.local_point = (v1 + v2) * 0.5;
        m.points[0].local_point = circle_b.position;
        m.points[0].id = 0;
        return;
    }

    // Barycentric coordinates.
    let u1 = (c_local - v1).dot(v2 - v1);
    let u2 = (c_local - v2).dot(v1 - v2);
    if u1 <= 0.0 {
        if (c_local - v1).length_squared() > radius * radius {
            return;
        }
        m.point_count = 1;
        m.kind = ManifoldType::FaceA;
        m.local_normal = (c_local - v1).normalized();
        m.local_point = v1;
        m.points[0].local_point = circle_b.position;
        m.points[0].id = 0;
    } else if u2 <= 0.0 {
        if (c_local - v2).length_squared() > radius * radius {
            return;
        }
        m.point_count = 1;
        m.kind = ManifoldType::FaceA;
        m.local_normal = (c_local - v2).normalized();
        m.local_point = v2;
        m.points[0].local_point = circle_b.position;
        m.points[0].id = 0;
    } else {
        let face_center = (v1 + v2) * 0.5;
        let s = (c_local - face_center).dot(normals[vert_index1]);
        if s > radius {
            return;
        }
        m.point_count = 1;
        m.kind = ManifoldType::FaceA;
        m.local_normal = normals[vert_index1];
        m.local_point = face_center;
        m.points[0].local_point = circle_b.position;
        m.points[0].id = 0;
    }
}

/// `FindMaxSeparation` computes the largest separation along `poly1` edge normals.
fn find_max_separation(
    poly1: &Polygon,
    xf1: Transform,
    poly2: &Polygon,
    xf2: Transform,
) -> (usize, f32) {
    let n1s = poly1.normals();
    let v1s = poly1.vertices();
    let v2s = poly2.vertices();
    let xf = xf2.inv_mul(xf1);

    let mut best_index = 0usize;
    let mut max_separation = f32::MIN;
    for (i, (n1, v1)) in n1s.iter().zip(v1s).enumerate() {
        // Normal of poly1 in poly2 coordinates.
        let n = xf.q.apply(*n1);
        let v1 = xf.apply(*v1);
        // Deepest point for normal i.
        let mut si = f32::MAX;
        for v2 in v2s {
            let sij = n.dot(*v2 - v1);
            if sij < si {
                si = sij;
            }
        }
        if si > max_separation {
            max_separation = si;
            best_index = i;
        }
    }
    (best_index, max_separation)
}

/// Vertex produced or consumed by clipping (`ClipVertex`).
#[derive(Clone, Copy, Debug, Default)]
struct ClipVertex {
    v: Vec2,
    id: u32,
}

/// `FindIncidentEdge`.
fn find_incident_edge(
    poly1: &Polygon,
    xf1: Transform,
    edge1: usize,
    poly2: &Polygon,
    xf2: Transform,
) -> [ClipVertex; 2] {
    let normals1 = poly1.normals();
    let vertices2 = poly2.vertices();
    let normals2 = poly2.normals();
    let count2 = vertices2.len();

    // Reference edge normal in poly2 coordinates.
    let normal1 = xf2.q.apply_inv(xf1.q.apply(normals1[edge1]));

    // Incident edge of poly2.
    let mut index = 0usize;
    let mut min_dot = f32::MAX;
    for (i, n2) in normals2.iter().enumerate() {
        let dot = normal1.dot(*n2);
        if dot < min_dot {
            min_dot = dot;
            index = i;
        }
    }

    let i1 = index;
    let i2 = if i1 + 1 < count2 { i1 + 1 } else { 0 };
    [
        ClipVertex {
            v: xf2.apply(vertices2[i1]),
            id: feature_key(edge1 as u8, i1 as u8, FEATURE_FACE, FEATURE_VERTEX),
        },
        ClipVertex {
            v: xf2.apply(vertices2[i2]),
            id: feature_key(edge1 as u8, i2 as u8, FEATURE_FACE, FEATURE_VERTEX),
        },
    ]
}

/// `ClipSegmentToLine` clips a segment against a half-plane and counts the output points.
fn clip_segment_to_line(
    v_out: &mut [ClipVertex; 2],
    v_in: &[ClipVertex; 2],
    normal: Vec2,
    offset: f32,
    vertex_index_a: usize,
) -> usize {
    let v0 = v_in[0];
    let v1 = v_in[1];
    let mut num_out = 0usize;

    // Signed endpoint distances to the line.
    let distance0 = normal.x * v0.v.x + normal.y * v0.v.y - offset;
    let distance1 = normal.x * v1.v.x + normal.y * v1.v.y - offset;

    // Retain points on the kept side of the plane.
    if distance0 <= 0.0 {
        v_out[num_out] = v0;
        num_out += 1;
    }
    if distance1 <= 0.0 {
        v_out[num_out] = v1;
        num_out += 1;
    }

    // Endpoints on opposite sides produce an intersection.
    if distance0 * distance1 < 0.0 {
        let interp = distance0 / (distance0 - distance1);
        let [_, ib0, _, _] = v0.id.to_le_bytes();
        // Robust writes at numOut without a check. Both retained points would be on the same side.
        v_out[num_out] = ClipVertex {
            v: Vec2::new(
                v0.v.x + interp * (v1.v.x - v0.v.x),
                v0.v.y + interp * (v1.v.y - v0.v.y),
            ),
            // Vertex A lies on edge B.
            id: feature_key(vertex_index_a as u8, ib0, FEATURE_VERTEX, FEATURE_FACE),
        };
        num_out += 1;
    }
    num_out
}

/// `CollidePolygons`.
pub(crate) fn collide_polygons(
    m: &mut Manifold,
    poly_a: &Polygon,
    xf_a: Transform,
    poly_b: &Polygon,
    xf_b: Transform,
) {
    m.point_count = 0;
    let total_radius = poly_a.radius + poly_b.radius;

    let (edge_a, separation_a) = find_max_separation(poly_a, xf_a, poly_b, xf_b);
    if separation_a > total_radius {
        return;
    }
    let (edge_b, separation_b) = find_max_separation(poly_b, xf_b, poly_a, xf_a);
    if separation_b > total_radius {
        return;
    }

    const K_RELATIVE_TOL: f32 = 0.98;
    const K_ABSOLUTE_TOL: f32 = 0.001;
    // poly1 is the reference polygon, and poly2 is the incident polygon.
    let (poly1, poly2, xf1, xf2, edge1, flip) =
        if separation_b > K_RELATIVE_TOL * separation_a + K_ABSOLUTE_TOL {
            m.kind = ManifoldType::FaceB;
            (poly_b, poly_a, xf_b, xf_a, edge_b, true)
        } else {
            m.kind = ManifoldType::FaceA;
            (poly_a, poly_b, xf_a, xf_b, edge_a, false)
        };

    let incident_edge = find_incident_edge(poly1, xf1, edge1, poly2, xf2);

    let vertices1 = poly1.vertices();
    let count1 = vertices1.len();
    let iv1 = edge1;
    let iv2 = if edge1 + 1 < count1 { edge1 + 1 } else { 0 };
    let mut v11 = vertices1[iv1];
    let mut v12 = vertices1[iv2];

    let local_tangent = (v12 - v11).normalized();
    let local_normal = Vec2::new(local_tangent.y, -local_tangent.x);
    let plane_point = (v11 + v12) * 0.5;

    let tangent = xf1.q.apply(local_tangent);
    let normal_x = tangent.y;
    let normal_y = -tangent.x;

    v11 = xf1.apply(v11);
    v12 = xf1.apply(v12);

    // Face offset.
    let front_offset = normal_x * v11.x + normal_y * v11.y;

    // Side offsets including the collision skin.
    let side_offset1 = -(tangent.x * v11.x + tangent.y * v11.y) + total_radius;
    let side_offset2 = tangent.x * v12.x + tangent.y * v12.y + total_radius;

    // Clip the incident edge against the sides of the reference edge.
    let mut clip_points1 = [ClipVertex::default(); 2];
    let np = clip_segment_to_line(
        &mut clip_points1,
        &incident_edge,
        -tangent,
        side_offset1,
        iv1,
    );
    if np < 2 {
        return;
    }
    let mut clip_points2 = [ClipVertex::default(); 2];
    let np = clip_segment_to_line(&mut clip_points2, &clip_points1, tangent, side_offset2, iv2);
    if np < 2 {
        return;
    }

    m.local_normal = local_normal;
    m.local_point = plane_point;

    let mut point_count = 0usize;
    for cp in &clip_points2 {
        let value = cp.v;
        let separation = normal_x * value.x + normal_y * value.y - front_offset;
        if separation <= total_radius {
            let p = &mut m.points[point_count];
            p.local_point = xf2.apply_inv(cp.v);
            p.id = if flip { swap_features(cp.id) } else { cp.id };
            point_count += 1;
        }
    }
    m.point_count = point_count;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape::POLYGON_RADIUS;

    fn box_at(p: Vec2) -> (Polygon, Transform) {
        (
            Polygon::rect(Vec2::new(-0.5, -0.5), Vec2::new(0.5, 0.5)),
            Transform::new(p, 0.0),
        )
    }

    #[test]
    fn face_to_face_boxes_give_two_points() {
        let (a, xa) = box_at(Vec2::new(0.0, 0.0));
        let (b, xb) = box_at(Vec2::new(0.995, 0.2));
        let mut m = Manifold::default();
        collide_polygons(&mut m, &a, xa, &b, xb);
        assert_eq!(m.point_count, 2);
        assert_eq!(m.kind, ManifoldType::FaceA);
        assert_eq!(m.local_normal, Vec2::new(1.0, 0.0));
        assert_ne!(m.points[0].id, m.points[1].id);
    }

    #[test]
    fn separated_boxes_give_no_points() {
        let (a, xa) = box_at(Vec2::new(0.0, 0.0));
        let (b, xb) = box_at(Vec2::new(1.0 + 2.0 * POLYGON_RADIUS + 0.01, 0.0));
        let mut m = Manifold::default();
        collide_polygons(&mut m, &a, xa, &b, xb);
        assert_eq!(m.point_count, 0);
    }

    #[test]
    fn circle_against_box_face() {
        let (a, xa) = box_at(Vec2::new(0.0, 0.0));
        let c = Circle {
            radius: 0.35,
            position: Vec2::ZERO,
        };
        let mut m = Manifold::default();
        collide_polygon_and_circle(
            &mut m,
            &a,
            xa,
            &c,
            Transform::new(Vec2::new(0.0, 0.85), 0.0),
        );
        assert_eq!(m.point_count, 1);
        assert_eq!(m.local_normal, Vec2::new(0.0, 1.0));
    }

    #[test]
    fn circles_touching() {
        let c = Circle {
            radius: 0.5,
            position: Vec2::ZERO,
        };
        let mut m = Manifold::default();
        collide_circles(
            &mut m,
            &c,
            Transform::IDENTITY,
            &c,
            Transform::new(Vec2::new(0.9, 0.0), 0.0),
        );
        assert_eq!(m.point_count, 1);
        assert_eq!(m.kind, ManifoldType::Circles);
        collide_circles(
            &mut m,
            &c,
            Transform::IDENTITY,
            &c,
            Transform::new(Vec2::new(1.1, 0.0), 0.0),
        );
        assert_eq!(m.point_count, 0);
    }

    #[test]
    fn swapped_feature_key() {
        assert_eq!(
            swap_features(feature_key(1, 2, 3, 4)),
            feature_key(2, 1, 4, 3)
        );
    }
}
