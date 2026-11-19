// @provenance
// source: https://github.com/space-wizards/RobustToolbox
// ref: v294.0.3
// path: Robust.Shared/Physics/Systems/SharedPhysicsSystem.Island.cs
// modified: yes
// @end-provenance
//
// Provenance classification: MIT at the pinned upstream revision.
// Modified in the port: ported from C# to Rust; BuildIslands (contacts only, no joints), SolveIsland (integration, damping,
//   velocity clamp, position iterations, sleep including the lone island), FinalisePositions, UpdateBodies,
//   SleepBodies; single-threaded; islands are ranges of reusable flat buffers instead of pooled lists; no
//   gravity, forces, joints, parenting or events (the step works on world coordinates of one map).
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
// Box2D comments in the source file (MIT, verbatim):
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

//! Islands group bodies connected by touching contacts for integration, solving,
//! and sleeping. Adapted from `SharedPhysicsSystem.Island.cs`; see the file header.

use crate::body::{Body, BodyType};
use crate::contact::Contact;
use crate::math::{Rot, Vec2};
use crate::solver::{self, PositionConstraint, SolverData, VelocityConstraint};

/// Island solver parameters, including sleep settings.
#[derive(Clone, Copy, Debug)]
pub(crate) struct IslandParams {
    pub solver: SolverData,
    pub frame_time: f32,
    pub velocity_iterations: u32,
    pub position_iterations: u32,
    pub max_translation: f32,
    pub max_rotation: f32,
    pub sleep_allowed: bool,
    pub time_to_sleep: f32,
    pub lin_tol_sqr: f32,
    pub ang_tol_sqr: f32,
}

/// Island ranges in the flat body and contact buffers.
#[derive(Clone, Copy, Debug)]
struct IslandRange {
    bodies: (usize, usize),
    contacts: (usize, usize),
    lone: bool,
}

/// Reusable island and solver buffers. The step allocates no memory after warm-up.
#[derive(Clone, Debug, Default)]
pub(crate) struct IslandSolver {
    island_bodies: Vec<u32>,
    island_contacts: Vec<u32>,
    lone_bodies: Vec<u32>,
    islands: Vec<IslandRange>,
    stack: Vec<u32>,
    positions: Vec<Vec2>,
    angles: Vec<f32>,
    linear_velocities: Vec<Vec2>,
    angular_velocities: Vec<f32>,
    sleep: Vec<bool>,
    vcs: Vec<VelocityConstraint>,
    pcs: Vec<PositionConstraint>,
}

impl IslandSolver {
    /// Builds and solves islands in seed order, followed by the island of isolated bodies.
    pub fn solve(&mut self, bodies: &mut [Body], contacts: &mut [Contact], p: &IslandParams) {
        self.build_islands(bodies, contacts);
        for k in 0..self.islands.len() {
            let island = self.islands[k];
            self.solve_island(bodies, contacts, island, p);
        }
        for b in bodies.iter_mut() {
            b.island = false;
        }
        for c in contacts.iter_mut() {
            c.island = false;
        }
    }

    fn build_islands(&mut self, bodies: &mut [Body], contacts: &mut [Contact]) {
        self.island_bodies.clear();
        self.island_contacts.clear();
        self.lone_bodies.clear();
        self.islands.clear();

        for seed in 0..bodies.len() {
            let s = &bodies[seed];
            if !s.alive || s.island || !s.awake || !s.can_collide || s.body_type == BodyType::Static
            {
                continue;
            }
            let body_start = self.island_bodies.len();
            let contact_start = self.island_contacts.len();
            self.stack.clear();
            self.stack.push(seed as u32);
            bodies[seed].island = true;

            while let Some(bi) = self.stack.pop() {
                self.island_bodies.push(bi);
                // Static bodies do not connect an island to other bodies.
                if bodies[bi as usize].body_type == BodyType::Static {
                    continue;
                }
                bodies[bi as usize].set_awake(true, false);

                for (ci, c) in contacts.iter_mut().enumerate() {
                    if c.body_a != bi && c.body_b != bi {
                        continue;
                    }
                    // Skip contacts that already belong to an island or are not touching.
                    if c.island || !c.touching {
                        continue;
                    }
                    self.island_contacts.push(ci as u32);
                    c.island = true;
                    let other = if c.body_a == bi { c.body_b } else { c.body_a };
                    if bodies[other as usize].island {
                        continue;
                    }
                    self.stack.push(other);
                    bodies[other as usize].island = true;
                }
            }

            let body_end = self.island_bodies.len();
            let contact_end = self.island_contacts.len();
            if contact_end == contact_start {
                // Put a body with no contacts into the island of isolated bodies.
                let b = self.island_bodies[body_start];
                self.island_bodies.truncate(body_start);
                self.lone_bodies.push(b);
            } else {
                // A static body can belong to more than one island.
                for &b in &self.island_bodies[body_start..body_end] {
                    if bodies[b as usize].body_type == BodyType::Static {
                        bodies[b as usize].island = false;
                    }
                }
                self.islands.push(IslandRange {
                    bodies: (body_start, body_end),
                    contacts: (contact_start, contact_end),
                    lone: false,
                });
            }
        }

        if !self.lone_bodies.is_empty() {
            let start = self.island_bodies.len();
            self.island_bodies.extend_from_slice(&self.lone_bodies);
            let end = self.island_bodies.len();
            let c = self.island_contacts.len();
            self.islands.push(IslandRange {
                bodies: (start, end),
                contacts: (c, c),
                lone: true,
            });
        }
    }

    /// `SolveIsland` + `FinalisePositions` + `UpdateBodies` + `SleepBodies`.
    fn solve_island(
        &mut self,
        bodies: &mut [Body],
        contacts: &mut [Contact],
        island: IslandRange,
        p: &IslandParams,
    ) {
        let dt = p.frame_time;
        let island_bodies = &self.island_bodies[island.bodies.0..island.bodies.1];
        let contact_ids = &self.island_contacts[island.contacts.0..island.contacts.1];
        let count = island_bodies.len();

        self.positions.clear();
        self.angles.clear();
        self.linear_velocities.clear();
        self.angular_velocities.clear();
        for (k, &bi) in island_bodies.iter().enumerate() {
            let b = &mut bodies[bi as usize];
            b.island_index = k;
            let xf = b.transform();
            let mut v = b.linear_velocity;
            let mut w = b.angular_velocity;
            if b.body_type == BodyType::Dynamic {
                // This step has no map gravity or external forces.
                v *= (1.0 - dt * b.linear_damping).clamp(0.0, 1.0);
                w *= (1.0 - dt * b.angular_damping).clamp(0.0, 1.0);
            }
            self.positions.push(xf.apply(b.mass.local_center));
            self.angles.push(b.angle);
            self.linear_velocities.push(v);
            self.angular_velocities.push(w);
        }

        solver::reset_solver(
            &p.solver,
            contacts,
            bodies,
            contact_ids,
            &mut self.vcs,
            &mut self.pcs,
        );
        solver::initialize_velocity_constraints(
            &p.solver,
            contacts,
            &mut self.vcs,
            &self.pcs,
            &self.positions,
            &self.angles,
            &self.linear_velocities,
            &self.angular_velocities,
        );
        if p.solver.warm_starting {
            solver::warm_start(
                &self.vcs,
                &mut self.linear_velocities,
                &mut self.angular_velocities,
            );
        }
        for _ in 0..p.velocity_iterations {
            solver::solve_velocity_constraints(
                &mut self.vcs,
                &mut self.linear_velocities,
                &mut self.angular_velocities,
            );
        }
        // Save impulses for warm starting.
        solver::store_impulses(&self.vcs, contacts);

        let max_vel = p.max_translation / dt;
        let max_vel_sq = max_vel * max_vel;
        let max_ang_vel = p.max_rotation / dt;
        let max_ang_vel_sq = max_ang_vel * max_ang_vel;

        // Integrate positions.
        for k in 0..count {
            let mut v = self.linear_velocities[k];
            let mut w = self.angular_velocities[k];
            let vel_sqr = v.length_squared();
            if vel_sqr > max_vel_sq {
                v *= max_vel / vel_sqr.sqrt();
                self.linear_velocities[k] = v;
            }
            if w * w > max_ang_vel_sq {
                w *= max_ang_vel / w.abs();
                self.angular_velocities[k] = w;
            }
            self.positions[k] += v * dt;
            self.angles[k] += w * dt;
        }

        let mut position_solved = false;
        for _ in 0..p.position_iterations {
            if solver::solve_position_constraints(
                &p.solver,
                &self.pcs,
                &mut self.positions,
                &mut self.angles,
            ) {
                position_solved = true;
                break;
            }
        }

        // Check sleep against the body velocity before writing the solved values.
        self.sleep.clear();
        self.sleep.resize(count, false);
        if p.sleep_allowed {
            let below = |b: &Body| {
                b.sleeping_allowed
                    && b.angular_velocity * b.angular_velocity <= p.ang_tol_sqr
                    && b.linear_velocity.dot(b.linear_velocity) <= p.lin_tol_sqr
            };
            if island.lone {
                for (k, &bi) in island_bodies.iter().enumerate() {
                    let b = &mut bodies[bi as usize];
                    if b.body_type == BodyType::Static {
                        continue;
                    }
                    if below(b) {
                        b.sleep_time += dt;
                    } else {
                        b.sleep_time = 0.0;
                    }
                    if b.sleep_time >= p.time_to_sleep && position_solved {
                        self.sleep[k] = true;
                    }
                }
            } else {
                let mut min_sleep_time = f32::MAX;
                for &bi in island_bodies {
                    let b = &mut bodies[bi as usize];
                    if b.body_type == BodyType::Static {
                        continue;
                    }
                    if below(b) {
                        b.sleep_time += dt;
                        min_sleep_time = min_sleep_time.min(b.sleep_time);
                    } else {
                        b.sleep_time = 0.0;
                        min_sleep_time = 0.0;
                    }
                }
                if min_sleep_time >= p.time_to_sleep && position_solved {
                    self.sleep.iter_mut().for_each(|s| *s = true);
                }
            }
        }

        // Finalize positions and update bodies. With a fixed map frame, apply the
        // displacement (solved position minus old position) to the old position, as Robust does.
        for (k, &bi) in island_bodies.iter().enumerate() {
            let b = &mut bodies[bi as usize];
            if b.body_type == BodyType::Static {
                continue;
            }
            let angle = self.angles[k];
            let q = Rot::from_angle(angle);
            let adjusted = self.positions[k] - q.apply(b.mass.local_center);
            let v = self.linear_velocities[k];
            if v.is_finite() {
                b.set_linear_velocity(v);
            }
            let w = self.angular_velocities[k];
            if w.is_finite() {
                b.set_angular_velocity(w);
            }
            if adjusted.is_finite() {
                let delta = adjusted - b.position;
                b.position += delta;
                b.angle += angle - b.angle;
            }
        }

        // SleepBodies.
        for (k, &bi) in island_bodies.iter().enumerate() {
            if self.sleep[k] {
                bodies[bi as usize].set_awake(false, true);
            }
        }
    }
}
