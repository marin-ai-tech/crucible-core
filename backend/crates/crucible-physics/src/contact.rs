//! Contact creation, manifold updates with warm-start impulse transfer, removal,
//! and touch start and end events.
//!
//! Independent implementation based on the behavior of Box2D's `b2Contact::Update`.
//! The Robust `Contact.cs` and `SharedPhysicsSystem.Contacts.cs` implementations carry Ms-PL
//! headers and were excluded from the port.
//! The solver follows these rules:
//! - Both fixtures must be hard, their collision filters must overlap in at least one direction,
//!   and their bodies cannot both be static or kinematic, or both be controllers.
//! - Body A has the higher shape type number. Polygons precede circles.
//! - Contact friction is √(f₁·f₂), and restitution is the higher of the two.
//! - A contact persists while the exact fixture AABBs overlap. Sleeping contacts are not updated.
//! - New manifold points inherit impulses from old points with matching feature keys.
//! - Starting or ending a touch wakes both bodies.

use crate::body::{Body, BodyId, BodyType};
use crate::collision::{Manifold, collide};

/// Contact between two fixtures.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Contact {
    pub body_a: u32,
    pub fixture_a: u32,
    pub body_b: u32,
    pub fixture_b: u32,
    pub manifold: Manifold,
    pub touching: bool,
    pub friction: f32,
    pub restitution: f32,
    /// Whether the contact already belongs to an island in this substep.
    pub island: bool,
}

impl Contact {
    #[inline]
    pub fn is_pair(&self, a: u32, fa: u32, b: u32, fb: u32) -> bool {
        (self.body_a == a && self.fixture_a == fa && self.body_b == b && self.fixture_b == fb)
            || (self.body_a == b
                && self.fixture_a == fb
                && self.body_b == a
                && self.fixture_b == fa)
    }
}

/// Touch start or end during a tick, used for ECS events `StartCollide` and `EndCollide`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContactEvent {
    pub kind: ContactEventKind,
    pub body_a: BodyId,
    pub fixture_a: u32,
    pub body_b: BodyId,
    pub fixture_b: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContactEventKind {
    /// The fixtures began touching.
    Started,
    /// The fixtures stopped touching or their contact was removed while touching.
    Ended,
}

/// Whether fixture `fa` of body `a` and fixture `fb` of body `b` can collide.
pub(crate) fn should_collide(a: &Body, fa: usize, b: &Body, fb: usize) -> bool {
    let (xa, xb) = (&a.fixtures[fa], &b.fixtures[fb]);
    if !(xa.hard && xb.hard) {
        return false;
    }
    if !xa.layers_overlap(xb) {
        return false;
    }
    if a.body_type.is_static_like() && b.body_type.is_static_like() {
        return false;
    }
    if a.body_type == BodyType::KinematicController && b.body_type == BodyType::KinematicController
    {
        return false;
    }
    true
}

/// Whether the exact fixture AABBs overlap.
pub(crate) fn fixtures_overlap(a: &Body, fa: usize, b: &Body, fb: usize) -> bool {
    let aa = a.fixtures[fa].shape.aabb(a.transform());
    let ab = b.fixtures[fb].shape.aabb(b.transform());
    aa.overlaps(&ab)
}

/// Creates a contact with the higher-numbered shape type as body A.
pub(crate) fn new_contact(bodies: &[Body], a: u32, fa: u32, b: u32, fb: u32) -> Contact {
    let ta = bodies[a as usize].fixtures[fa as usize].shape.type_index();
    let tb = bodies[b as usize].fixtures[fb as usize].shape.type_index();
    let ((ca, cfa), (cb, cfb)) = if ta >= tb {
        ((a, fa), (b, fb))
    } else {
        ((b, fb), (a, fa))
    };
    let xa = &bodies[ca as usize].fixtures[cfa as usize];
    let xb = &bodies[cb as usize].fixtures[cfb as usize];
    Contact {
        body_a: ca,
        fixture_a: cfa,
        body_b: cb,
        fixture_b: cfb,
        manifold: Manifold::default(),
        touching: false,
        friction: (xa.friction * xb.friction).sqrt(),
        restitution: xa.restitution.max(xb.restitution),
        island: false,
    }
}

/// Recomputes the manifold, transfers warm-start impulses, and reports whether touching changed.
pub(crate) fn update_contact(c: &mut Contact, bodies: &[Body]) -> bool {
    let ba = &bodies[c.body_a as usize];
    let bb = &bodies[c.body_b as usize];
    let old = c.manifold;
    let mut m = Manifold::default();
    collide(
        &mut m,
        &ba.fixtures[c.fixture_a as usize].shape,
        ba.transform(),
        &bb.fixtures[c.fixture_b as usize].shape,
        bb.transform(),
    );
    for k in 0..m.point_count {
        let id = m.points[k].id;
        m.points[k].normal_impulse = 0.0;
        m.points[k].tangent_impulse = 0.0;
        if let Some(p) = old.points[..old.point_count].iter().find(|p| p.id == id) {
            m.points[k].normal_impulse = p.normal_impulse;
            m.points[k].tangent_impulse = p.tangent_impulse;
        }
    }
    let touching = m.point_count > 0;
    let changed = touching != c.touching;
    c.manifold = m;
    c.touching = touching;
    changed
}
