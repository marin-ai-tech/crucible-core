//! Entity visibility for each player on the current tick.
//!
//!
//! This implementation follows documented Robust PVS behavior in `Robust.Server/GameStates`.
//! - Entities on the same map within the controlled entity's interest radius,
//!   found using `crucible-map` spatial queries.
//! - Entities marked [`AlwaysVisible`], corresponding to a global PVS override.
//! - The controlled entity and its equipment. Its transform descendants
//!   are visible at any radius.
//! - Each visible entity's ancestors, such as its map, grid, or container.
//!   The client needs them to position that entity in the world.
//! - A closed container ([`HidesContents`]) hides contents from all players except
//!   the player carrying or controlling the container.
//!
//! [`Replication`](crate::Replication) applies the new entity budget when building frames.

use std::collections::HashMap;

use crucible_ecs::{
    Component, ComponentRegistration, EntityUid, NetworkPolicy, RegistryError, World, WorldBuilder,
};
use crucible_map::{MapId, Transform, entities_in_radius};

/// Registration source for this crate's components.
pub const SOURCE: &str = "core:replication";

/// Marks an entity visible to all players regardless of distance, even inside a closed
/// container. This is equivalent to a global Robust PVS override.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AlwaysVisible;

impl Component for AlwaysVisible {}

/// Marks a closed container. Its transform descendants are visible only to the
/// player carrying or controlling it. The container module adds this marker when
/// closing a container and removes it on opening.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HidesContents;

impl Component for HidesContents {}

/// Registers visibility markers as server-only components.
pub fn register(builder: &mut WorldBuilder) -> Result<(), RegistryError> {
    builder.register_component(
        ComponentRegistration::new::<AlwaysVisible>("AlwaysVisible", SOURCE)
            .network(NetworkPolicy::ServerOnly),
    )?;
    builder.register_component(
        ComponentRegistration::new::<HidesContents>("HidesContents", SOURCE)
            .network(NetworkPolicy::ServerOnly),
    )?;
    Ok(())
}

/// An entity that may enter a player's view.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Candidate {
    /// Squared distance to the player's eye, used to prioritize the visibility budget.
    pub(crate) dist2: f32,
    /// Exempt from the budget, as for the controlled entity and its ancestors.
    pub(crate) forced: bool,
}

/// Parent entity in the transform hierarchy.
pub(crate) fn parent_of(world: &World, uid: EntityUid) -> Option<EntityUid> {
    world.get::<Transform>(uid).and_then(Transform::parent)
}

/// An entity's ancestors from the root downward, excluding the entity itself.
pub(crate) fn ancestors_root_first(world: &World, uid: EntityUid) -> Vec<EntityUid> {
    let mut chain = Vec::new();
    let mut cur = parent_of(world, uid);
    while let Some(p) = cur {
        if chain.contains(&p) {
            break; // Guard against an invalid hierarchy cycle.
        }
        chain.push(p);
        cur = parent_of(world, p);
    }
    chain.reverse();
    chain
}

/// Checks whether a closed container hides the entity from `viewer`.
pub(crate) fn hidden_from(world: &World, uid: EntityUid, viewer: Option<EntityUid>) -> bool {
    if Some(uid) == viewer {
        return false;
    }
    let mut hidden = false;
    let mut cur = parent_of(world, uid);
    let mut steps = 0;
    while let Some(p) = cur {
        if Some(p) == viewer {
            return false; // Players can see all their own equipment.
        }
        if world.has::<HidesContents>(p) {
            hidden = true;
        }
        steps += 1;
        if steps > 4096 {
            break;
        }
        cur = parent_of(world, p);
    }
    hidden
}

/// Transform descendants of an entity, visited depth first.
fn descendants(world: &World, root: EntityUid) -> Vec<EntityUid> {
    let mut out = Vec::new();
    let mut stack: Vec<EntityUid> = world
        .get::<Transform>(root)
        .map(|t| t.children().iter().rev().copied().collect())
        .unwrap_or_default();
    while let Some(e) = stack.pop() {
        if out.contains(&e) {
            continue;
        }
        out.push(e);
        if let Some(t) = world.get::<Transform>(e) {
            stack.extend(t.children().iter().rev().copied());
        }
    }
    out
}

/// Entities a player can see this tick before the budget is applied.
pub(crate) fn desired_view(
    world: &World,
    controlled: Option<EntityUid>,
    always_visible: &[EntityUid],
    radius: f32,
) -> HashMap<EntityUid, Candidate> {
    let mut out: HashMap<EntityUid, Candidate> = HashMap::new();
    let add = |out: &mut HashMap<EntityUid, Candidate>, uid: EntityUid, c: Candidate| {
        if world.terminating_or_deleted(uid) {
            return;
        }
        out.entry(uid)
            .and_modify(|old| {
                old.forced |= c.forced;
                old.dist2 = old.dist2.min(c.dist2);
            })
            .or_insert(c);
    };

    let controlled = controlled.filter(|&c| !world.terminating_or_deleted(c));
    let eye = controlled
        .and_then(|c| world.get::<Transform>(c))
        .filter(|t| t.map_id() != MapId::NULLSPACE)
        .map(|t| (t.map_id(), t.world_position()));

    let mut seeds: Vec<(EntityUid, f32)> = Vec::new();
    if let Some(c) = controlled {
        let forced = Candidate {
            dist2: 0.0,
            forced: true,
        };
        add(&mut out, c, forced);
        for a in ancestors_root_first(world, c) {
            add(&mut out, a, forced);
        }
        for e in descendants(world, c) {
            seeds.push((e, 0.0));
        }
    }
    if let Some((map, center)) = eye {
        for e in entities_in_radius(world, map, center, radius) {
            if hidden_from(world, e, controlled) {
                continue;
            }
            let p = world
                .get::<Transform>(e)
                .map_or(center, Transform::world_position);
            seeds.push((e, (p - center).length_squared()));
        }
    }
    for &e in always_visible {
        let dist2 = match (eye, world.get::<Transform>(e)) {
            (Some((map, center)), Some(t)) if t.map_id() == map => {
                (t.world_position() - center).length_squared()
            }
            _ => 0.0,
        };
        seeds.push((e, dist2));
    }
    for (e, dist2) in seeds {
        let c = Candidate {
            dist2,
            forced: false,
        };
        add(&mut out, e, c);
        for a in ancestors_root_first(world, e) {
            add(&mut out, a, c);
        }
    }
    out
}
