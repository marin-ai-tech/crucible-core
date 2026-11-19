//! `PhysicsWorld` holds one map's bodies and advances them in fixed substeps.
//!
//! The substep loop and tile friction are independent implementations based on
//! observed behavior. They correspond to `SharedPhysicsSystem.SimulateWorld`
//! and `TileFrictionController`. Each substep runs movement and tile friction,
//! updates broad-phase proxies and finds candidate pairs, updates contacts, and
//! solves contact islands.
//!
//! The solver is independent of the ECS world and holds no global mutable state.
//! The ECS integration copies state between physics bodies and components.

use crate::body::{Body, BodyId, BodyType, Fixture};
use crate::broadphase::{DynamicTree, ProxyData};
use crate::contact::{self, Contact, ContactEvent, ContactEventKind};
use crate::island::{IslandParams, IslandSolver};
use crate::math::{Vec2, facing_angle};
use crate::mover::{self, MoveButtons, SubTick};
use crate::solver::SolverData;

/// Step configuration corresponding to Robust and SS14 physics and movement CVars.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PhysicsConfig {
    /// Tick rate in hertz (`net.tickrate`).
    pub tick_rate: u32,
    /// Physics substeps per tick, rounded up from target minimum tick rate divided by tick rate.
    pub substeps: u32,
    /// `physics.veliter`.
    pub velocity_iterations: u32,
    /// `physics.positer`.
    pub position_iterations: u32,
    /// `physics.warmstart`.
    pub warm_starting: bool,
    /// `physics.baumgarte`.
    pub baumgarte: f32,
    /// `physics.maxlinearcorrection`.
    pub max_linear_correction: f32,
    /// `physics.velocitythreshold`.
    pub velocity_threshold: f32,
    /// `physics.maxlinvelocity` in meters per second.
    pub max_linear_velocity: f32,
    /// `physics.maxangvelocity` in revolutions per second.
    pub max_angular_velocity: f32,
    /// `physics.sleepallowed`.
    pub sleep_allowed: bool,
    /// `physics.timetosleep` in seconds.
    pub time_to_sleep: f32,
    /// `physics.linsleeptol`, interpreted as a squared speed (`SetLinearToleranceSqr`).
    pub linear_sleep_tolerance_sqr: f32,
    /// `physics.angsleeptol`, interpreted as a squared speed.
    pub angular_sleep_tolerance_sqr: f32,
    /// `physics.tile_friction`, the tile friction multiplier for dynamic body damping.
    pub tile_friction: f32,
    /// `physics.min_friction`, the minimum tile friction.
    pub min_friction: f32,
    /// `game.diagonalmovement`.
    pub diagonal_movement: bool,
}

impl PhysicsConfig {
    /// CVar values from the SS14 reference trajectory recordings.
    pub fn robust_defaults() -> Self {
        Self {
            tick_rate: 30,
            substeps: 2,
            velocity_iterations: 8,
            position_iterations: 3,
            warm_starting: true,
            baumgarte: 0.2,
            max_linear_correction: 0.2,
            velocity_threshold: 0.5,
            max_linear_velocity: 400.0,
            max_angular_velocity: 15.0,
            sleep_allowed: true,
            time_to_sleep: 0.2,
            linear_sleep_tolerance_sqr: 0.01,
            angular_sleep_tolerance_sqr: 2.0 / 180.0 * core::f32::consts::PI * 0.15,
            tile_friction: 8.0,
            min_friction: 0.0,
            diagonal_movement: true,
        }
    }

    /// Tick duration in seconds.
    #[inline]
    pub fn tick_dt(&self) -> f32 {
        1.0 / self.tick_rate as f32
    }
}

impl Default for PhysicsConfig {
    fn default() -> Self {
        Self::robust_defaults()
    }
}

/// Physics step error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepError {
    /// A previous step panicked, leaving the world in an undefined state. Use
    /// `load_state` to restore a snapshot, or recreate the world.
    Poisoned,
}

impl core::fmt::Display for StepError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            StepError::Poisoned => {
                f.write_str("the previous physics step panicked; restore the world from a snapshot")
            }
        }
    }
}

impl std::error::Error for StepError {}

/// Read-only contact data.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContactView {
    pub body_a: BodyId,
    pub fixture_a: u32,
    pub body_b: BodyId,
    pub fixture_b: u32,
    pub touching: bool,
    /// Manifold from the last substep with accumulated impulses.
    pub manifold: crate::collision::Manifold,
}

/// Body fields mutated by the step.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct BodyDynamics {
    alive: bool,
    position: Vec2,
    angle: f32,
    linear_velocity: Vec2,
    angular_velocity: f32,
    linear_damping: f32,
    angular_damping: f32,
    awake: bool,
    sleep_time: f32,
    input: Option<mover::InputMover>,
}

/// Snapshot of bodies, contact warm-start impulses, and `inv_dt0` for rollback
/// in a world with the same bodies. Reusing the snapshot avoids allocations.
#[derive(Clone, Debug, Default)]
pub struct PhysicsSnapshot {
    bodies: Vec<BodyDynamics>,
    contacts: Vec<Contact>,
    inv_dt0: f32,
}

impl PhysicsSnapshot {
    /// Number of body slots in the snapshot.
    pub fn body_count(&self) -> usize {
        self.bodies.len()
    }
}

/// Physics bodies and solver for one map.
#[derive(Clone, Debug)]
pub struct PhysicsWorld {
    config: PhysicsConfig,
    bodies: Vec<Body>,
    free_bodies: Vec<u32>,
    contacts: Vec<Contact>,
    tree: DynamicTree,
    /// Candidate pairs from the broad phase: body A, body B, fixture A, fixture B.
    pairs: Vec<(u32, u32, u32, u32)>,
    islands: IslandSolver,
    events: Vec<ContactEvent>,
    /// Inverse duration of the last substep, used for warm-start `dt_ratio`.
    inv_dt0: f32,
    poisoned: bool,
}

impl PhysicsWorld {
    /// Empty world. As for a running server, the initial `inv_dt0` gives `dt_ratio` of 1.
    pub fn new(config: PhysicsConfig) -> Self {
        let substep_dt = config.tick_dt() / config.substeps.max(1) as f32;
        Self {
            config,
            bodies: Vec::new(),
            free_bodies: Vec::new(),
            contacts: Vec::new(),
            tree: DynamicTree::default(),
            pairs: Vec::new(),
            islands: IslandSolver::default(),
            events: Vec::new(),
            inv_dt0: 1.0 / substep_dt,
            poisoned: false,
        }
    }

    #[inline]
    pub fn config(&self) -> &PhysicsConfig {
        &self.config
    }

    /// Adds a body, deriving mass from fixtures unless explicitly set. Frees slots
    /// for reuse when bodies are removed. Visits live bodies in slot order.
    pub fn add_body(&mut self, mut body: Body) -> BodyId {
        body.reset_mass_data();
        body.alive = true;
        body.proxies_dirty = true;
        body.island = false;
        if let Some(index) = self.free_bodies.pop() {
            let generation = self.bodies[index as usize].generation.wrapping_add(1);
            body.generation = generation;
            self.bodies[index as usize] = body;
            self.create_proxies(index);
            BodyId { index, generation }
        } else {
            let index = self.bodies.len() as u32;
            body.generation = 0;
            self.bodies.push(body);
            self.create_proxies(index);
            BodyId {
                index,
                generation: 0,
            }
        }
    }

    /// Removes a body and its contacts. Touching contacts produce end events.
    pub fn remove_body(&mut self, id: BodyId) -> bool {
        if self.try_body(id).is_none() {
            return false;
        }
        self.destroy_contacts_of(id.index);
        self.destroy_proxies(id.index);
        let b = &mut self.bodies[id.index as usize];
        b.alive = false;
        b.mover = None;
        b.fixtures.clear();
        self.free_bodies.push(id.index);
        true
    }

    /// Replaces a body's fixtures and recalculates mass unless explicitly set.
    pub fn set_fixtures(&mut self, id: BodyId, fixtures: &[Fixture]) {
        assert!(self.try_body(id).is_some(), "stale body id {id:?}");
        self.destroy_contacts_of(id.index);
        self.destroy_proxies(id.index);
        let b = &mut self.bodies[id.index as usize];
        b.fixtures.clear();
        b.fixtures.extend_from_slice(fixtures);
        b.reset_mass_data();
        self.create_proxies(id.index);
    }

    /// Toggles `CanCollide`. Disabling collisions puts the body to sleep and removes its contacts.
    pub fn set_can_collide(&mut self, id: BodyId, value: bool) {
        assert!(self.try_body(id).is_some(), "stale body id {id:?}");
        let b = &mut self.bodies[id.index as usize];
        if b.can_collide == value {
            return;
        }
        if value {
            b.can_collide = true;
            b.proxies_dirty = true;
            self.create_proxies(id.index);
        } else {
            b.set_awake(false, true);
            b.can_collide = false;
            self.destroy_contacts_of(id.index);
            self.destroy_proxies(id.index);
        }
    }

    pub fn try_body(&self, id: BodyId) -> Option<&Body> {
        self.bodies
            .get(id.index as usize)
            .filter(|b| b.alive && b.generation == id.generation)
    }

    pub fn try_body_mut(&mut self, id: BodyId) -> Option<&mut Body> {
        self.bodies
            .get_mut(id.index as usize)
            .filter(|b| b.alive && b.generation == id.generation)
    }

    /// Body by ID. Panics if the ID is stale.
    pub fn body(&self, id: BodyId) -> &Body {
        self.try_body(id)
            .unwrap_or_else(|| panic!("stale body id {id:?}"))
    }

    /// Mutable body by ID. Panics if the ID is stale.
    pub fn body_mut(&mut self, id: BodyId) -> &mut Body {
        self.try_body_mut(id)
            .unwrap_or_else(|| panic!("stale body id {id:?}"))
    }

    /// Live bodies in ascending slot order.
    pub fn bodies(&self) -> impl Iterator<Item = (BodyId, &Body)> {
        self.bodies
            .iter()
            .enumerate()
            .filter(|(_, b)| b.alive)
            .map(|(i, b)| {
                (
                    BodyId {
                        index: i as u32,
                        generation: b.generation,
                    },
                    b,
                )
            })
    }

    /// Changes a movement button for body `id` at `sub_tick` of `tick`. Pass the same
    /// `tick` to the step. Bodies without input-driven movement ignore the command.
    pub fn set_move_input(
        &mut self,
        id: BodyId,
        tick: u32,
        sub_tick: SubTick,
        button: MoveButtons,
        enabled: bool,
    ) {
        let diagonal = self.config.diagonal_movement;
        if let Some(m) = self.body_mut(id).mover.as_mut() {
            m.input
                .set_move_input(tick, sub_tick, button, enabled, diagonal);
        }
    }

    /// Touch start and end events from the last step.
    #[inline]
    pub fn contact_events(&self) -> &[ContactEvent] {
        &self.events
    }

    /// Takes pending contact events. Call before stepping to retain end events
    /// generated by removal or disabled collisions, as stepping clears the list.
    pub fn drain_contact_events(&mut self) -> std::vec::Drain<'_, ContactEvent> {
        self.events.drain(..)
    }

    /// Number of contacts, whether touching or not.
    #[inline]
    pub fn contact_count(&self) -> usize {
        self.contacts.len()
    }

    /// Contacts in solver order, including fixture pairs whose AABBs overlap without touching.
    pub fn contacts(&self) -> impl Iterator<Item = ContactView> + '_ {
        self.contacts.iter().map(|c| ContactView {
            body_a: self.id_of(c.body_a),
            fixture_a: c.fixture_a,
            body_b: self.id_of(c.body_b),
            fixture_b: c.fixture_b,
            touching: c.touching,
            manifold: c.manifold,
        })
    }

    #[inline]
    fn id_of(&self, index: u32) -> BodyId {
        BodyId {
            index,
            generation: self.bodies[index as usize].generation,
        }
    }

    /// Advances one `tick` with `substeps` substeps of `tick_dt / substeps` each.
    /// Results depend on world state and input, independently of elapsed wall-clock time.
    pub fn step(&mut self, tick: u32) -> Result<(), StepError> {
        if self.poisoned {
            return Err(StepError::Poisoned);
        }
        // Leave the flag set if any substep panics.
        self.poisoned = true;
        self.events.clear();
        let substeps = self.config.substeps.max(1);
        let dt = self.config.tick_dt() / substeps as f32;
        for _ in 0..substeps {
            self.mover_controller(tick, dt);
            self.tile_friction_controller();
            self.update_proxies(dt);
            self.find_new_contacts();
            self.update_contacts();
            self.solve(dt);
        }
        self.poisoned = false;
        Ok(())
    }

    /// Whether the step panicked and the world remains unrestored.
    #[inline]
    pub fn is_poisoned(&self) -> bool {
        self.poisoned
    }

    /// Runs movement for a character on the ground, corresponding to `SharedMoverController.HandleMobMovement`.
    fn mover_controller(&mut self, tick: u32, dt: f32) {
        let diagonal = self.config.diagonal_movement;
        let min_damping = self.config.min_friction;
        for b in self.bodies.iter_mut() {
            b.used_mob_movement = false;
            if !b.alive {
                continue;
            }
            let Some(mut m) = b.mover else { continue };
            if !m.input.can_move {
                continue;
            }
            b.used_mob_movement = true;
            let wish = mover::wish_velocity(&m, tick, diagonal);
            let v = mover::mob_movement(
                &m.speed,
                &b.ground,
                wish,
                b.linear_velocity,
                dt,
                min_damping,
            );
            mover::set_wish_dir(&mut m.input, wish);
            b.mover = Some(m);
            b.set_linear_velocity(v);
            b.set_angular_velocity(0.0);
            if wish != Vec2::ZERO {
                b.angle = facing_angle(wish);
                b.proxies_dirty = true;
            }
        }
    }

    /// Implements observed tile damping behavior from `TileFrictionController`.
    /// A moving, awake body without a character mover gets damping equal to the tile
    /// friction times `tile_friction`, with a floor of `min_friction`.
    fn tile_friction_controller(&mut self) {
        let tile_friction = self.config.tile_friction;
        let min_damping = self.config.min_friction;
        for b in self.bodies.iter_mut() {
            if !b.alive || !b.awake || b.body_type == BodyType::Static || b.used_mob_movement {
                continue;
            }
            if b.linear_velocity == Vec2::ZERO && b.angular_velocity == 0.0 {
                continue;
            }
            let damping = (tile_friction * b.ground.friction).max(min_damping);
            b.linear_damping = damping;
            b.angular_damping = damping;
        }
    }

    // Broad phase

    fn create_proxies(&mut self, index: u32) {
        let b = &mut self.bodies[index as usize];
        b.proxies.clear();
        if !b.can_collide {
            return;
        }
        let xf = b.transform();
        for (fi, f) in b.fixtures.iter().enumerate() {
            let id = self.tree.create_proxy(
                f.shape.aabb(xf),
                ProxyData {
                    body: index,
                    fixture: fi as u32,
                },
            );
            b.proxies.push(id);
        }
        b.proxies_dirty = false;
    }

    fn destroy_proxies(&mut self, index: u32) {
        let b = &mut self.bodies[index as usize];
        for &node in &b.proxies {
            self.tree.destroy_proxy(node);
        }
        b.proxies.clear();
    }

    /// Updates leaves for bodies that may have moved, including awake and externally updated bodies.
    fn update_proxies(&mut self, dt: f32) {
        for (index, b) in self.bodies.iter_mut().enumerate() {
            if !b.alive || b.proxies.is_empty() {
                continue;
            }
            let moving = b.awake && b.body_type != BodyType::Static;
            if !moving && !b.proxies_dirty {
                continue;
            }
            let xf = b.transform();
            let displacement = b.linear_velocity * dt;
            for (fi, (f, &node)) in b.fixtures.iter().zip(&b.proxies).enumerate() {
                debug_assert_eq!(
                    self.tree.data(node),
                    ProxyData {
                        body: index as u32,
                        fixture: fi as u32
                    }
                );
                self.tree.move_proxy(node, f.shape.aabb(xf), displacement);
            }
            b.proxies_dirty = false;
        }
    }

    /// Finds new contacts by querying overlapping tree leaves for each fixture of
    /// an awake, moving body. Sorts pairs by body and fixture IDs so contact order
    /// does not depend on tree shape.
    fn find_new_contacts(&mut self) {
        self.pairs.clear();
        for a in 0..self.bodies.len() {
            let ba = &self.bodies[a];
            if !ba.alive
                || !ba.awake
                || !ba.can_collide
                || ba.body_type == BodyType::Static
                || ba.proxies.is_empty()
            {
                continue;
            }
            let xf = ba.transform();
            for (fa, fixture) in ba.fixtures.iter().enumerate() {
                let aabb = fixture.shape.aabb(xf);
                let pairs = &mut self.pairs;
                self.tree.query(&aabb, |_, d| {
                    if d.body as usize != a {
                        pairs.push((a as u32, d.body, fa as u32, d.fixture));
                    }
                });
            }
        }
        self.pairs.sort_unstable();
        for k in 0..self.pairs.len() {
            let (a, b, fa, fb) = self.pairs[k];
            let (ba, bb) = (&self.bodies[a as usize], &self.bodies[b as usize]);
            if !contact::should_collide(ba, fa as usize, bb, fb as usize)
                || !contact::fixtures_overlap(ba, fa as usize, bb, fb as usize)
            {
                continue;
            }
            if self.contacts.iter().any(|c| c.is_pair(a, fa, b, fb)) {
                continue;
            }
            self.contacts
                .push(contact::new_contact(&self.bodies, a, fa, b, fb));
        }
    }

    /// Removes separated contacts, updates manifolds, and emits touch start and end events.
    fn update_contacts(&mut self) {
        let mut i = 0;
        while i < self.contacts.len() {
            let c = self.contacts[i];
            let (ba, bb) = (
                &self.bodies[c.body_a as usize],
                &self.bodies[c.body_b as usize],
            );
            let active_a = ba.awake && ba.body_type != BodyType::Static;
            let active_b = bb.awake && bb.body_type != BodyType::Static;
            if !active_a && !active_b {
                i += 1;
                continue;
            }
            if !contact::fixtures_overlap(ba, c.fixture_a as usize, bb, c.fixture_b as usize) {
                self.destroy_contact(i);
                continue;
            }
            self.contacts[i].island = false;
            let changed = contact::update_contact(&mut self.contacts[i], &self.bodies);
            if changed {
                let touching = self.contacts[i].touching;
                self.bodies[c.body_a as usize].set_awake(true, true);
                self.bodies[c.body_b as usize].set_awake(true, true);
                let kind = if touching {
                    ContactEventKind::Started
                } else {
                    ContactEventKind::Ended
                };
                self.push_event(kind, &c);
            }
            i += 1;
        }
    }

    fn push_event(&mut self, kind: ContactEventKind, c: &Contact) {
        let e = ContactEvent {
            kind,
            body_a: self.id_of(c.body_a),
            fixture_a: c.fixture_a,
            body_b: self.id_of(c.body_b),
            fixture_b: c.fixture_b,
        };
        self.events.push(e);
    }

    fn destroy_contact(&mut self, i: usize) {
        let c = self.contacts.remove(i);
        if c.touching {
            self.push_event(ContactEventKind::Ended, &c);
        }
    }

    fn destroy_contacts_of(&mut self, index: u32) {
        let mut i = 0;
        while i < self.contacts.len() {
            let c = self.contacts[i];
            if c.body_a == index || c.body_b == index {
                self.destroy_contact(i);
            } else {
                i += 1;
            }
        }
    }

    // Solver

    fn solve(&mut self, frame_time: f32) {
        let inv_dt = if frame_time > 0.0 {
            1.0 / frame_time
        } else {
            0.0
        };
        let dt_ratio = self.inv_dt0 * frame_time;
        let c = &self.config;
        let tick_rate = c.tick_rate as f32;
        let params = IslandParams {
            solver: SolverData {
                dt_ratio,
                warm_starting: c.warm_starting,
                velocity_threshold: c.velocity_threshold,
                baumgarte: c.baumgarte,
                max_linear_correction: c.max_linear_correction,
            },
            frame_time,
            velocity_iterations: c.velocity_iterations,
            position_iterations: c.position_iterations,
            max_translation: c.max_linear_velocity / tick_rate,
            max_rotation: core::f32::consts::TAU * c.max_angular_velocity / tick_rate,
            sleep_allowed: c.sleep_allowed,
            time_to_sleep: c.time_to_sleep,
            lin_tol_sqr: c.linear_sleep_tolerance_sqr,
            ang_tol_sqr: c.angular_sleep_tolerance_sqr,
        };
        self.islands
            .solve(&mut self.bodies, &mut self.contacts, &params);
        self.inv_dt0 = inv_dt;
    }

    // Snapshots

    /// Saves dynamic state in `out`, reusing its buffers.
    pub fn save_state(&self, out: &mut PhysicsSnapshot) {
        out.bodies.clear();
        out.bodies.extend(self.bodies.iter().map(|b| BodyDynamics {
            alive: b.alive,
            position: b.position,
            angle: b.angle,
            linear_velocity: b.linear_velocity,
            angular_velocity: b.angular_velocity,
            linear_damping: b.linear_damping,
            angular_damping: b.angular_damping,
            awake: b.awake,
            sleep_time: b.sleep_time,
            input: b.mover.map(|m| m.input),
        }));
        out.contacts.clear();
        out.contacts.extend_from_slice(&self.contacts);
        out.inv_dt0 = self.inv_dt0;
    }

    /// Restores a snapshot with the same set of bodies and clears the interrupted-step flag.
    /// If the body sets differ, returns `false` and leaves the world unchanged.
    pub fn load_state(&mut self, s: &PhysicsSnapshot) -> bool {
        if s.bodies.len() != self.bodies.len()
            || self
                .bodies
                .iter()
                .zip(&s.bodies)
                .any(|(b, d)| b.alive != d.alive)
        {
            return false;
        }
        for (b, d) in self.bodies.iter_mut().zip(&s.bodies) {
            b.position = d.position;
            b.angle = d.angle;
            b.linear_velocity = d.linear_velocity;
            b.angular_velocity = d.angular_velocity;
            b.linear_damping = d.linear_damping;
            b.angular_damping = d.angular_damping;
            b.awake = d.awake;
            b.sleep_time = d.sleep_time;
            if let (Some(m), Some(input)) = (b.mover.as_mut(), d.input) {
                m.input = input;
            }
            b.proxies_dirty = true;
        }
        self.contacts.clear();
        self.contacts.extend_from_slice(&s.contacts);
        self.inv_dt0 = s.inv_dt0;
        self.poisoned = false;
        true
    }
}
