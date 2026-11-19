// @provenance
// source: https://github.com/space-wizards/RobustToolbox
// ref: v294.0.3
// path: Robust.Shared/Physics/Systems/SharedPhysicsSystem.Solver.cs
// modified: yes
// @end-provenance
// @provenance
// source: https://github.com/space-wizards/RobustToolbox
// ref: v294.0.3
// path: Robust.Shared/Physics/Dynamics/Contacts/ContactVelocityConstraint.cs
// modified: yes
// @end-provenance
// @provenance
// source: https://github.com/space-wizards/RobustToolbox
// ref: v294.0.3
// path: Robust.Shared/Physics/Dynamics/Contacts/ContactPositionConstraint.cs
// modified: yes
// @end-provenance
// @provenance
// source: https://github.com/space-wizards/RobustToolbox
// ref: v294.0.3
// path: Robust.Shared/Physics/Vector4Helpers.cs
// modified: yes
// @end-provenance
// @provenance
// source: https://github.com/space-wizards/RobustToolbox
// ref: v294.0.3
// path: Robust.Shared/Physics/Transform.cs
// modified: yes
// @end-provenance
//
// Provenance classification: MIT at the pinned upstream revision.
// Modified in the port: ported from C# to Rust; ResetSolver, GetInvMass, InitializeVelocityConstraints (incl. the 2-point
//   block solver set-up), WarmStart, SolveVelocityConstraints (1 point and the block solver), StoreImpulses,
//   SolvePositionConstraints, InitializeManifold, PositionSolverManifoldInitialize; the 2x2 matrix helpers of
//   Vector4Helpers.Inverse and Transform.Mul(Vector4, Vector2); no parallel ranges, no array pools (buffers are
//   reused by the caller).
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
// Farseer Physics Engine / Box2D (zlib, verbatim from the headers of the source files):
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

//! Box2D v2 contact solver without predictive contacts, adapted from Robust. See the file header.

use crate::body::{Body, BodyType};
use crate::collision::{Manifold, ManifoldType};
use crate::contact::Contact;
use crate::math::{Rot, Transform, Vec2};
use crate::shape::LINEAR_SLOP;

/// Solver parameters for one substep (`SolverData`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct SolverData {
    pub dt_ratio: f32,
    pub warm_starting: bool,
    pub velocity_threshold: f32,
    pub baumgarte: f32,
    pub max_linear_correction: f32,
}

/// 2×2 matrix laid out as Robust's `System.Numerics.Vector4`: (X, Y, Z, W).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Mat22v4 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl Mat22v4 {
    /// `Vector4Helpers.Inverse`.
    fn inverse(self) -> Mat22v4 {
        let (a, b, c, d) = (self.x, self.z, self.y, self.w);
        let mut det = a * d - b * c;
        if det != 0.0 {
            det = 1.0 / det;
        }
        Mat22v4 {
            x: det * d,
            y: -det * c,
            z: -det * b,
            w: det * a,
        }
    }

    /// `Transform.Mul(Vector4, Vector2)`.
    #[inline]
    fn mul(self, v: Vec2) -> Vec2 {
        Vec2::new(self.x * v.x + self.y * v.y, self.z * v.x + self.w * v.y)
    }
}

/// Velocity constraint point (`VelocityConstraintPoint`).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct VelocityConstraintPoint {
    /// `RelativeVelocityA`: vector from body A's center of mass to the point.
    pub r_a: Vec2,
    pub r_b: Vec2,
    pub normal_impulse: f32,
    pub tangent_impulse: f32,
    pub normal_mass: f32,
    pub tangent_mass: f32,
    pub velocity_bias: f32,
}

/// `ContactVelocityConstraint`.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct VelocityConstraint {
    pub points: [VelocityConstraintPoint; 2],
    pub normal: Vec2,
    pub normal_mass: Mat22v4,
    pub k: Mat22v4,
    pub index_a: usize,
    pub index_b: usize,
    pub inv_mass_a: f32,
    pub inv_mass_b: f32,
    pub inv_i_a: f32,
    pub inv_i_b: f32,
    pub friction: f32,
    pub restitution: f32,
    pub tangent_speed: f32,
    pub point_count: usize,
    /// Contact index in the world.
    pub contact_index: usize,
}

/// `ContactPositionConstraint`.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct PositionConstraint {
    pub local_points: [Vec2; 2],
    pub local_normal: Vec2,
    pub local_point: Vec2,
    pub index_a: usize,
    pub index_b: usize,
    pub inv_mass_a: f32,
    pub inv_mass_b: f32,
    pub local_center_a: Vec2,
    pub local_center_b: Vec2,
    pub inv_i_a: f32,
    pub inv_i_b: f32,
    pub kind: ManifoldType,
    pub radius_a: f32,
    pub radius_b: f32,
    pub point_count: usize,
}

/// `GetInvMass` makes controllers immovable to dynamic bodies and dynamic bodies infinitely
/// heavy to controllers. Two controllers do not act on each other.
pub(crate) fn get_inv_mass(a: &Body, b: &Body) -> (f32, f32) {
    use BodyType::*;
    match a.body_type {
        Kinematic | Static => (a.mass.inv_mass, b.mass.inv_mass),
        KinematicController => match b.body_type {
            Kinematic | Static => (a.mass.inv_mass, b.mass.inv_mass),
            Dynamic => (a.mass.inv_mass, 0.0),
            KinematicController => (0.0, 0.0),
        },
        Dynamic => match b.body_type {
            Kinematic | Static | Dynamic => (a.mass.inv_mass, b.mass.inv_mass),
            KinematicController => (0.0, b.mass.inv_mass),
        },
    }
}

/// `ResetSolver` builds constraints for this island's world contact indices in `contact_ids`.
pub(crate) fn reset_solver(
    data: &SolverData,
    contacts: &[Contact],
    bodies: &[Body],
    contact_ids: &[u32],
    vcs: &mut Vec<VelocityConstraint>,
    pcs: &mut Vec<PositionConstraint>,
) {
    vcs.clear();
    pcs.clear();
    for &ci in contact_ids {
        let contact = &contacts[ci as usize];
        let body_a = &bodies[contact.body_a as usize];
        let body_b = &bodies[contact.body_b as usize];
        let radius_a = body_a.fixtures[contact.fixture_a as usize].shape.radius();
        let radius_b = body_b.fixtures[contact.fixture_b as usize].shape.radius();
        let manifold = &contact.manifold;
        let point_count = manifold.point_count;
        let (inv_mass_a, inv_mass_b) = get_inv_mass(body_a, body_b);

        let mut vc = VelocityConstraint {
            friction: contact.friction,
            restitution: contact.restitution,
            tangent_speed: 0.0,
            index_a: body_a.island_index,
            index_b: body_b.island_index,
            inv_mass_a,
            inv_mass_b,
            inv_i_a: body_a.mass.inv_inertia,
            inv_i_b: body_b.mass.inv_inertia,
            contact_index: ci as usize,
            point_count,
            ..VelocityConstraint::default()
        };
        let mut pc = PositionConstraint {
            index_a: body_a.island_index,
            index_b: body_b.island_index,
            inv_mass_a,
            inv_mass_b,
            local_center_a: body_a.mass.local_center,
            local_center_b: body_b.mass.local_center,
            inv_i_a: body_a.mass.inv_inertia,
            inv_i_b: body_b.mass.inv_inertia,
            local_normal: manifold.local_normal,
            local_point: manifold.local_point,
            point_count,
            radius_a,
            radius_b,
            kind: manifold.kind,
            ..PositionConstraint::default()
        };
        for j in 0..point_count {
            let cp = manifold.points[j];
            let p = &mut vc.points[j];
            if data.warm_starting {
                p.normal_impulse = data.dt_ratio * cp.normal_impulse;
                p.tangent_impulse = data.dt_ratio * cp.tangent_impulse;
            }
            pc.local_points[j] = cp.local_point;
        }
        vcs.push(vc);
        pcs.push(pc);
    }
}

/// Island body `index` as a center-of-mass transform.
#[inline]
fn body_transform(center: Vec2, angle: f32, local_center: Vec2) -> Transform {
    let q = Rot::from_angle(angle);
    Transform {
        p: center - q.apply(local_center),
        q,
    }
}

/// `InitializeVelocityConstraints`, with the corresponding Robust signature.
#[allow(clippy::too_many_arguments)]
pub(crate) fn initialize_velocity_constraints(
    data: &SolverData,
    contacts: &[Contact],
    vcs: &mut [VelocityConstraint],
    pcs: &[PositionConstraint],
    positions: &[Vec2],
    angles: &[f32],
    linear_velocities: &[Vec2],
    angular_velocities: &[f32],
) {
    for (vc, pc) in vcs.iter_mut().zip(pcs) {
        let manifold = contacts[vc.contact_index].manifold;
        let (index_a, index_b) = (vc.index_a, vc.index_b);
        let (inv_mass_a, inv_mass_b, inv_i_a, inv_i_b) =
            (vc.inv_mass_a, vc.inv_mass_b, vc.inv_i_a, vc.inv_i_b);
        let center_a = positions[index_a];
        let center_b = positions[index_b];
        let (lin_a, ang_a) = (linear_velocities[index_a], angular_velocities[index_a]);
        let (lin_b, ang_b) = (linear_velocities[index_b], angular_velocities[index_b]);

        let xf_a = body_transform(center_a, angles[index_a], pc.local_center_a);
        let xf_b = body_transform(center_b, angles[index_b], pc.local_center_b);

        let (normal, points) = initialize_manifold(&manifold, xf_a, xf_b, pc.radius_a, pc.radius_b);
        vc.normal = normal;

        for j in 0..vc.point_count {
            let vcp = &mut vc.points[j];
            vcp.r_a = points[j] - center_a;
            vcp.r_b = points[j] - center_b;

            let rn_a = vcp.r_a.cross(normal);
            let rn_b = vcp.r_b.cross(normal);
            let k_normal = inv_mass_a + inv_mass_b + inv_i_a * rn_a * rn_a + inv_i_b * rn_b * rn_b;
            vcp.normal_mass = if k_normal > 0.0 { 1.0 / k_normal } else { 0.0 };

            let tangent = normal.cross_scalar(1.0);
            let rt_a = vcp.r_a.cross(tangent);
            let rt_b = vcp.r_b.cross(tangent);
            let k_tangent = inv_mass_a + inv_mass_b + inv_i_a * rt_a * rt_a + inv_i_b * rt_b * rt_b;
            vcp.tangent_mass = if k_tangent > 0.0 {
                1.0 / k_tangent
            } else {
                0.0
            };

            // Velocity bias for restitution.
            vcp.velocity_bias = 0.0;
            let v_rel = normal.dot(
                lin_b + Vec2::scalar_cross(ang_b, vcp.r_b)
                    - lin_a
                    - Vec2::scalar_cross(ang_a, vcp.r_a),
            );
            if v_rel < -data.velocity_threshold {
                vcp.velocity_bias = -vc.restitution * v_rel;
            }
        }

        // Prepare a block solver for a two-point contact.
        if vc.point_count == 2 {
            let vcp1 = vc.points[0];
            let vcp2 = vc.points[1];
            let rn1_a = vcp1.r_a.cross(normal);
            let rn1_b = vcp1.r_b.cross(normal);
            let rn2_a = vcp2.r_a.cross(normal);
            let rn2_b = vcp2.r_b.cross(normal);

            let k11 = inv_mass_a + inv_mass_b + inv_i_a * rn1_a * rn1_a + inv_i_b * rn1_b * rn1_b;
            let k22 = inv_mass_a + inv_mass_b + inv_i_a * rn2_a * rn2_a + inv_i_b * rn2_b * rn2_b;
            let k12 = inv_mass_a + inv_mass_b + inv_i_a * rn1_a * rn2_a + inv_i_b * rn1_b * rn2_b;

            // Require a reasonable condition number.
            const K_MAX_CONDITION_NUMBER: f32 = 1000.0;
            if k11 * k11 < K_MAX_CONDITION_NUMBER * (k11 * k22 - k12 * k12) {
                vc.k = Mat22v4 {
                    x: k11,
                    y: k12,
                    z: k12,
                    w: k22,
                };
                vc.normal_mass = vc.k.inverse();
            } else {
                // The constraints are redundant. Keep one.
                vc.point_count = 1;
            }
        }
    }
}

/// `WarmStart`.
pub(crate) fn warm_start(
    vcs: &[VelocityConstraint],
    linear_velocities: &mut [Vec2],
    angular_velocities: &mut [f32],
) {
    for vc in vcs {
        let normal = vc.normal;
        let tangent = normal.cross_scalar(1.0);
        for cp in &vc.points[..vc.point_count] {
            let p = normal * cp.normal_impulse + tangent * cp.tangent_impulse;
            angular_velocities[vc.index_a] -= vc.inv_i_a * cp.r_a.cross(p);
            linear_velocities[vc.index_a] -= p * vc.inv_mass_a;
            angular_velocities[vc.index_b] += vc.inv_i_b * cp.r_b.cross(p);
            linear_velocities[vc.index_b] += p * vc.inv_mass_b;
        }
    }
}

/// Applies normal impulse increments `d` for both points.
#[inline]
#[allow(clippy::too_many_arguments)]
fn apply_block_impulse(
    vc: &VelocityConstraint,
    d: Vec2,
    va: &mut Vec2,
    wa: &mut f32,
    vb: &mut Vec2,
    wb: &mut f32,
    normal: Vec2,
) {
    let (cp1, cp2) = (&vc.points[0], &vc.points[1]);
    let p1 = normal * d.x;
    let p2 = normal * d.y;
    *va -= (p1 + p2) * vc.inv_mass_a;
    *wa -= vc.inv_i_a * (cp1.r_a.cross(p1) + cp2.r_a.cross(p2));
    *vb += (p1 + p2) * vc.inv_mass_b;
    *wb += vc.inv_i_b * (cp1.r_b.cross(p1) + cp2.r_b.cross(p2));
}

/// `SolveVelocityConstraints`.
pub(crate) fn solve_velocity_constraints(
    vcs: &mut [VelocityConstraint],
    linear_velocities: &mut [Vec2],
    angular_velocities: &mut [f32],
) {
    for vc in vcs.iter_mut() {
        let (index_a, index_b) = (vc.index_a, vc.index_b);
        let (m_a, i_a, m_b, i_b) = (vc.inv_mass_a, vc.inv_i_a, vc.inv_mass_b, vc.inv_i_b);
        let point_count = vc.point_count;

        let mut va = linear_velocities[index_a];
        let mut wa = angular_velocities[index_a];
        let mut vb = linear_velocities[index_b];
        let mut wb = angular_velocities[index_b];

        let normal = vc.normal;
        let tangent = normal.cross_scalar(1.0);
        let friction = vc.friction;

        // Solve tangential constraints first so the normal solve has priority.
        for j in 0..point_count {
            let p = &mut vc.points[j];
            let dv = vb + Vec2::scalar_cross(wb, p.r_b) - va - Vec2::scalar_cross(wa, p.r_a);
            let vt = dv.dot(tangent) - vc.tangent_speed;
            let mut lambda = p.tangent_mass * (-vt);
            let max_friction = friction * p.normal_impulse;
            let new_impulse = (p.tangent_impulse + lambda).clamp(-max_friction, max_friction);
            lambda = new_impulse - p.tangent_impulse;
            p.tangent_impulse = new_impulse;
            let imp = tangent * lambda;
            va -= imp * m_a;
            wa -= i_a * p.r_a.cross(imp);
            vb += imp * m_b;
            wb += i_b * p.r_b.cross(imp);
        }

        if vc.point_count == 1 {
            let p = &mut vc.points[0];
            let dv = vb + Vec2::scalar_cross(wb, p.r_b) - va - Vec2::scalar_cross(wa, p.r_a);
            let vn = dv.dot(normal);
            let mut lambda = -p.normal_mass * (vn - p.velocity_bias);
            let new_impulse = (p.normal_impulse + lambda).max(0.0);
            lambda = new_impulse - p.normal_impulse;
            p.normal_impulse = new_impulse;
            let imp = normal * lambda;
            va -= imp * m_a;
            wa -= i_a * p.r_a.cross(imp);
            vb += imp * m_b;
            wb += i_b * p.r_b.cross(imp);
        } else {
            // Box2D block solver (with Dirk Gregorius). Solve the two-point LCP by
            // enumerating cases, with x = a + d and b' = b − A·a.
            let cp1 = vc.points[0];
            let cp2 = vc.points[1];
            let a = Vec2::new(cp1.normal_impulse, cp2.normal_impulse);

            let dv1 = vb + Vec2::scalar_cross(wb, cp1.r_b) - va - Vec2::scalar_cross(wa, cp1.r_a);
            let dv2 = vb + Vec2::scalar_cross(wb, cp2.r_b) - va - Vec2::scalar_cross(wa, cp2.r_a);
            let vn1 = dv1.dot(normal);
            let vn2 = dv2.dot(normal);
            let mut b = Vec2::new(vn1 - cp1.velocity_bias, vn2 - cp2.velocity_bias);
            b -= vc.k.mul(a);

            let mut solved = None;
            // Case 1: vn = 0, so x = −A⁻¹·b'.
            let x = -vc.normal_mass.mul(b);
            if x.x >= 0.0 && x.y >= 0.0 {
                solved = Some(x);
            }
            // Case 2: vn1 = 0 and x2 = 0.
            if solved.is_none() {
                let x = Vec2::new(-cp1.normal_mass * b.x, 0.0);
                let vn2 = vc.k.y * x.x + b.y;
                if x.x >= 0.0 && vn2 >= 0.0 {
                    solved = Some(x);
                }
            }
            // Case 3: vn2 = 0 and x1 = 0.
            if solved.is_none() {
                let x = Vec2::new(0.0, -cp2.normal_mass * b.y);
                let vn1 = vc.k.z * x.y + b.x;
                if x.y >= 0.0 && vn1 >= 0.0 {
                    solved = Some(x);
                }
            }
            // Case 4: x1 = 0 and x2 = 0, so vn = b'.
            if solved.is_none() && b.x >= 0.0 && b.y >= 0.0 {
                solved = Some(Vec2::ZERO);
            }
            // Give up if no case yields a solution, as in Box2D.
            if let Some(x) = solved {
                let d = x - a;
                apply_block_impulse(vc, d, &mut va, &mut wa, &mut vb, &mut wb, normal);
                vc.points[0].normal_impulse = x.x;
                vc.points[1].normal_impulse = x.y;
            }
        }

        linear_velocities[index_a] = va;
        angular_velocities[index_a] = wa;
        linear_velocities[index_b] = vb;
        angular_velocities[index_b] = wb;
    }
}

/// `StoreImpulses` stores constraint impulses in contact manifolds for the next substep's warm start.
pub(crate) fn store_impulses(vcs: &[VelocityConstraint], contacts: &mut [Contact]) {
    for vc in vcs {
        let m = &mut contacts[vc.contact_index].manifold;
        for j in 0..vc.point_count {
            m.points[j].normal_impulse = vc.points[j].normal_impulse;
            m.points[j].tangent_impulse = vc.points[j].tangent_impulse;
        }
    }
}

/// `SolvePositionConstraints` returns true when all positions satisfy the constraints.
pub(crate) fn solve_position_constraints(
    data: &SolverData,
    pcs: &[PositionConstraint],
    positions: &mut [Vec2],
    angles: &mut [f32],
) -> bool {
    let mut min_separation = 0.0f32;
    for pc in pcs {
        let (index_a, index_b) = (pc.index_a, pc.index_b);
        let (m_a, i_a, m_b, i_b) = (pc.inv_mass_a, pc.inv_i_a, pc.inv_mass_b, pc.inv_i_b);
        for j in 0..pc.point_count {
            let xf_a = body_transform(positions[index_a], angles[index_a], pc.local_center_a);
            let xf_b = body_transform(positions[index_b], angles[index_b], pc.local_center_b);
            let (normal, point, separation) = position_solver_manifold(pc, j, xf_a, xf_b);
            let r_a = point - positions[index_a];
            let r_b = point - positions[index_b];

            // Track the largest constraint error.
            min_separation = min_separation.min(separation);

            // Limit corrections and allow a small penetration tolerance.
            let c = (data.baumgarte * (separation + LINEAR_SLOP))
                .clamp(-data.max_linear_correction, 0.0);

            let rn_a = r_a.cross(normal);
            let rn_b = r_b.cross(normal);
            let k = m_a + m_b + i_a * rn_a * rn_a + i_b * rn_b * rn_b;
            let impulse = if k > 0.0 { -c / k } else { 0.0 };
            let p = normal * impulse;

            positions[index_a] -= p * m_a;
            angles[index_a] -= i_a * r_a.cross(p);
            positions[index_b] += p * m_b;
            angles[index_b] += i_b * r_b.cross(p);
        }
    }
    // The correction stops at −LinearSlop. Allow −3·LinearSlop in the convergence check.
    min_separation >= -3.0 * LINEAR_SLOP
}

/// `InitializeManifold` computes the normal and up to two points in world coordinates.
pub(crate) fn initialize_manifold(
    m: &Manifold,
    xf_a: Transform,
    xf_b: Transform,
    radius_a: f32,
    radius_b: f32,
) -> (Vec2, [Vec2; 2]) {
    let mut points = [Vec2::ZERO; 2];
    if m.point_count == 0 {
        return (Vec2::ZERO, points);
    }
    match m.kind {
        ManifoldType::Circles => {
            let mut normal = Vec2::new(1.0, 0.0);
            let point_a = xf_a.apply(m.local_point);
            let point_b = xf_b.apply(m.points[0].local_point);
            if (point_a - point_b).length_squared() > f32::EPSILON * f32::EPSILON {
                normal = (point_b - point_a).normalized();
            }
            let c_a = point_a + normal * radius_a;
            let c_b = point_b - normal * radius_b;
            points[0] = (c_a + c_b) * 0.5;
            (normal, points)
        }
        ManifoldType::FaceA => {
            let normal = xf_a.q.apply(m.local_normal);
            let plane_point = xf_a.apply(m.local_point);
            for (i, out) in points.iter_mut().enumerate().take(m.point_count) {
                let clip_point = xf_b.apply(m.points[i].local_point);
                let c_a = clip_point + normal * (radius_a - (clip_point - plane_point).dot(normal));
                let c_b = clip_point - normal * radius_b;
                *out = (c_a + c_b) * 0.5;
            }
            (normal, points)
        }
        ManifoldType::FaceB => {
            let normal = xf_b.q.apply(m.local_normal);
            let plane_point = xf_b.apply(m.local_point);
            for (i, out) in points.iter_mut().enumerate().take(m.point_count) {
                let clip_point = xf_a.apply(m.points[i].local_point);
                let c_b = clip_point + normal * (radius_b - (clip_point - plane_point).dot(normal));
                let c_a = clip_point - normal * radius_a;
                *out = (c_a + c_b) * 0.5;
            }
            // Point the normal from A to B.
            (-normal, points)
        }
        // A nonempty manifold always has a type. Robust throws on this invalid case.
        ManifoldType::Invalid => (Vec2::ZERO, points),
    }
}

/// `PositionSolverManifoldInitialize` computes normal, point, and separation for `index`.
fn position_solver_manifold(
    pc: &PositionConstraint,
    index: usize,
    xf_a: Transform,
    xf_b: Transform,
) -> (Vec2, Vec2, f32) {
    match pc.kind {
        ManifoldType::Circles => {
            let point_a = xf_a.apply(pc.local_point);
            let point_b = xf_b.apply(pc.local_points[0]);
            let mut normal = point_b - point_a;
            // Farseer leaves a zero-length normal unnormalized.
            if normal != Vec2::ZERO {
                normal = normal.normalized();
            }
            let point = (point_a + point_b) * 0.5;
            let separation = (point_b - point_a).dot(normal) - pc.radius_a - pc.radius_b;
            (normal, point, separation)
        }
        ManifoldType::FaceA => {
            let normal = xf_a.q.apply(pc.local_normal);
            let plane_point = xf_a.apply(pc.local_point);
            let clip_point = xf_b.apply(pc.local_points[index]);
            let separation = (clip_point - plane_point).dot(normal) - pc.radius_a - pc.radius_b;
            (normal, clip_point, separation)
        }
        ManifoldType::FaceB => {
            let normal = xf_b.q.apply(pc.local_normal);
            let plane_point = xf_b.apply(pc.local_point);
            let clip_point = xf_a.apply(pc.local_points[index]);
            let separation = (clip_point - plane_point).dot(normal) - pc.radius_a - pc.radius_b;
            // Point the normal from A to B.
            (-normal, clip_point, separation)
        }
        ManifoldType::Invalid => (Vec2::ZERO, Vec2::ZERO, 0.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inverse_of_symmetric_matrix() {
        let k = Mat22v4 {
            x: 2.0,
            y: 1.0,
            z: 1.0,
            w: 3.0,
        };
        let inv = k.inverse();
        let v = Vec2::new(0.7, -0.2);
        let back = inv.mul(k.mul(v));
        assert!((back - v).length() < 1e-6);
    }
}
