//! Transform hierarchy and tile anchoring.
//! See the world map design for the hierarchy and anchoring requirements.
//!
//! The component caches world position and rotation. Moving or reparenting an entity
//! refreshes the cache throughout its subtree, so reading the world transform takes
//! no hierarchy traversal.

use std::fmt;

use crucible_ecs::{Component, EntityUid, World, networked};

use crate::events::OverVoid;
use crate::grid::{Map, MapGrid, MapId};
use crate::math::{Vec2, Vector2i};
use crate::state::{self, CELL_SIZE};

/// Entity transform storing a parent, local position, and rotation.
///
/// Replicates to all viewers with deltas for `parent`, `local_position`,
/// `local_rotation`, `anchored`, and `teleport_count`. The module's update functions
/// ([`set_parent`], [`set_local_position`], and [`anchor`]) maintain world caches,
/// child lists, the spatial index, and change markers.
#[networked]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Transform {
    pub(crate) parent: Option<EntityUid>,
    pub(crate) local_position: Vec2,
    pub(crate) local_rotation: f64,
    pub(crate) anchored: bool,
    pub(crate) teleport_count: u32,
    #[networked(skip)]
    pub(crate) map_id: MapId,
    #[networked(skip)]
    pub(crate) map_uid: Option<EntityUid>,
    #[networked(skip)]
    pub(crate) grid_uid: Option<EntityUid>,
    #[networked(skip)]
    pub(crate) children: Vec<EntityUid>,
    #[networked(skip)]
    pub(crate) world_position: Vec2,
    #[networked(skip)]
    pub(crate) world_rotation: f64,
    #[networked(skip)]
    pub(crate) over_void: bool,
}

impl Component for Transform {}

impl Transform {
    /// Parent map, grid, or other entity. `None` denotes a root in a map or
    /// nullspace.
    pub fn parent(&self) -> Option<EntityUid> {
        self.parent
    }

    /// Position in the parent's coordinate space.
    pub fn local_position(&self) -> Vec2 {
        self.local_position
    }

    /// Rotation relative to the parent, in radians.
    pub fn local_rotation(&self) -> f64 {
        self.local_rotation
    }

    /// Whether this entity is anchored to a tile.
    pub fn anchored(&self) -> bool {
        self.anchored
    }

    /// Explicit teleport count. A new value breaks interpolation.
    pub fn teleport_count(&self) -> u32 {
        self.teleport_count
    }

    /// Map containing the entity, or [`MapId::NULLSPACE`] when outside every map.
    pub fn map_id(&self) -> MapId {
        self.map_id
    }

    /// The map entity.
    pub fn map_uid(&self) -> Option<EntityUid> {
        self.map_uid
    }

    /// Closest ancestor grid, or this grid itself.
    pub fn grid_uid(&self) -> Option<EntityUid> {
        self.grid_uid
    }

    /// Child entities in attachment order.
    pub fn children(&self) -> &[EntityUid] {
        &self.children
    }

    /// Cached world position.
    pub fn world_position(&self) -> Vec2 {
        self.world_position
    }

    /// Cached world rotation in radians.
    pub fn world_rotation(&self) -> f64 {
        self.world_rotation
    }
}

/// Error updating an entity's transform.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransformError {
    /// The entity does not exist.
    EntityNotFound(EntityUid),
    /// The entity has no transform.
    NoTransform(EntityUid),
    /// The entity already has a transform.
    AlreadyPlaced(EntityUid),
    /// The parent does not exist or has no transform.
    InvalidParent(EntityUid),
    /// The new parent is the entity itself or one of its descendants.
    Cycle {
        entity: EntityUid,
        parent: EntityUid,
    },
    /// A tile anchor requires an entity on a grid.
    NotOnGrid(EntityUid),
}

impl fmt::Display for TransformError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TransformError::EntityNotFound(e) => write!(f, "entity {e} does not exist"),
            TransformError::NoTransform(e) => write!(f, "entity {e} has no transform"),
            TransformError::AlreadyPlaced(e) => write!(f, "entity {e} already has a transform"),
            TransformError::InvalidParent(p) => {
                write!(f, "parent {p} does not exist or has no transform")
            }
            TransformError::Cycle { entity, parent } => write!(
                f,
                "parent {parent} would create a hierarchy cycle for entity {entity}"
            ),
            TransformError::NotOnGrid(e) => write!(f, "entity {e} is not on a grid"),
        }
    }
}

impl std::error::Error for TransformError {}

// Placement

/// Gives the entity a transform with the given parent, local position, and rotation.
/// A `None` parent places the entity at a map or nullspace root.
pub fn place(
    world: &mut World,
    uid: EntityUid,
    parent: Option<EntityUid>,
    local_position: Vec2,
    local_rotation: f64,
) -> Result<(), TransformError> {
    let mut pending = Vec::new();
    place_deferred(
        world,
        uid,
        parent,
        local_position,
        local_rotation,
        &mut pending,
    )?;
    raise_over_void(world, pending);
    Ok(())
}

/// Like [`place`] but queues [`OverVoid`] transitions in `pending` for map loading.
pub(crate) fn place_deferred(
    world: &mut World,
    uid: EntityUid,
    parent: Option<EntityUid>,
    local_position: Vec2,
    local_rotation: f64,
    pending: &mut Vec<EntityUid>,
) -> Result<(), TransformError> {
    if !world.exists(uid) {
        return Err(TransformError::EntityNotFound(uid));
    }
    if world.has::<Transform>(uid) {
        return Err(TransformError::AlreadyPlaced(uid));
    }
    if let Some(p) = parent
        && (p == uid || !world.has::<Transform>(p))
    {
        return Err(TransformError::InvalidParent(p));
    }
    world
        .add_component(
            uid,
            Transform {
                parent,
                local_position,
                local_rotation,
                ..Transform::default()
            },
        )
        .map_err(|_| TransformError::EntityNotFound(uid))?;
    if let Some(p) = parent
        && let Some(pt) = world.get_mut::<Transform>(p)
    {
        pt.children.push(uid);
    }
    refresh_subtree(world, uid, pending);
    Ok(())
}

/// Spawns an initialized entity with a transform.
pub fn spawn_at(
    world: &mut World,
    parent: EntityUid,
    local_position: Vec2,
) -> Result<EntityUid, TransformError> {
    if !world.has::<Transform>(parent) {
        return Err(TransformError::InvalidParent(parent));
    }
    let uid = world.spawn();
    place(world, uid, Some(parent), local_position, 0.0)?;
    Ok(uid)
}

// Queries

/// Entity position in world coordinates.
pub fn world_position(world: &World, uid: EntityUid) -> Option<Vec2> {
    world.get::<Transform>(uid).map(|t| t.world_position)
}

/// Entity rotation in world coordinates, in radians.
pub fn world_rotation(world: &World, uid: EntityUid) -> Option<f64> {
    world.get::<Transform>(uid).map(|t| t.world_rotation)
}

/// Converts a world point to the coordinate space of entity `frame`.
pub fn to_local(world: &World, frame: EntityUid, world_point: Vec2) -> Option<Vec2> {
    let t = world.get::<Transform>(frame)?;
    Some((world_point - t.world_position).rotated(-t.world_rotation))
}

/// Converts a point in entity `frame` coordinates to world coordinates.
pub fn to_world(world: &World, frame: EntityUid, local_point: Vec2) -> Option<Vec2> {
    let t = world.get::<Transform>(frame)?;
    Some(t.world_position + local_point.rotated(t.world_rotation))
}

// Transform updates

/// Changes the parent while preserving world position and rotation, like `SetParent`.
/// Clears any tile anchor.
pub fn set_parent(
    world: &mut World,
    uid: EntityUid,
    parent: EntityUid,
) -> Result<(), TransformError> {
    let t = world
        .get::<Transform>(uid)
        .ok_or(TransformError::NoTransform(uid))?;
    let (wp, wr) = (t.world_position, t.world_rotation);
    let pt = world
        .get::<Transform>(parent)
        .ok_or(TransformError::InvalidParent(parent))?;
    let local = (wp - pt.world_position).rotated(-pt.world_rotation);
    let rotation = wr - pt.world_rotation;
    set_parent_with(world, uid, parent, local, rotation)
}

/// Changes the parent, sets a new local position and rotation, and clears the anchor.
/// The world position may change.
pub fn set_parent_with(
    world: &mut World,
    uid: EntityUid,
    parent: EntityUid,
    local_position: Vec2,
    local_rotation: f64,
) -> Result<(), TransformError> {
    if !world.has::<Transform>(uid) {
        return Err(TransformError::NoTransform(uid));
    }
    if !world.has::<Transform>(parent) {
        return Err(TransformError::InvalidParent(parent));
    }
    if is_self_or_descendant(world, parent, uid) {
        return Err(TransformError::Cycle {
            entity: uid,
            parent,
        });
    }
    unanchor(world, uid);
    let old_parent = world.get::<Transform>(uid).and_then(|t| t.parent);
    if old_parent != Some(parent) {
        if let Some(op) = old_parent
            && let Some(opt) = world.get_mut::<Transform>(op)
        {
            opt.children.retain(|&c| c != uid);
        }
        if let Some(pt) = world.get_mut::<Transform>(parent) {
            pt.children.push(uid);
        }
    }
    if let Some(t) = world.get_mut::<Transform>(uid) {
        t.parent = Some(parent);
        t.local_position = local_position;
        t.local_rotation = local_rotation;
    }
    world.dirty_fields::<Transform>(
        uid,
        Transform::FIELD_PARENT | Transform::FIELD_LOCAL_POSITION | Transform::FIELD_LOCAL_ROTATION,
    );
    let mut pending = Vec::new();
    refresh_subtree(world, uid, &mut pending);
    raise_over_void(world, pending);
    Ok(())
}

/// Sets the local position. An entity keeps its anchor if it stays on the same tile
/// and loses it when leaving the tile.
pub fn set_local_position(
    world: &mut World,
    uid: EntityUid,
    position: Vec2,
) -> Result<(), TransformError> {
    move_local(world, uid, position, false)
}

fn move_local(
    world: &mut World,
    uid: EntityUid,
    position: Vec2,
    teleport: bool,
) -> Result<(), TransformError> {
    let t = world
        .get::<Transform>(uid)
        .ok_or(TransformError::NoTransform(uid))?;
    if t.anchored && MapGrid::tile_indices(t.local_position) != MapGrid::tile_indices(position) {
        unanchor(world, uid);
    }
    let mut dirty = Transform::FIELD_LOCAL_POSITION;
    if let Some(t) = world.get_mut::<Transform>(uid) {
        t.local_position = position;
        if teleport {
            t.teleport_count = t.teleport_count.wrapping_add(1);
            dirty |= Transform::FIELD_TELEPORT_COUNT;
        }
    }
    world.dirty_fields::<Transform>(uid, dirty);
    let mut pending = Vec::new();
    refresh_subtree(world, uid, &mut pending);
    raise_over_void(world, pending);
    Ok(())
}

/// Sets the world position without changing the parent.
pub fn set_world_position(
    world: &mut World,
    uid: EntityUid,
    position: Vec2,
) -> Result<(), TransformError> {
    let local = world_to_local_position(world, uid, position)?;
    set_local_position(world, uid, local)
}

/// Teleports to a world point, retaining the parent and incrementing the teleport count.
pub fn teleport_world_position(
    world: &mut World,
    uid: EntityUid,
    position: Vec2,
) -> Result<(), TransformError> {
    let local = world_to_local_position(world, uid, position)?;
    move_local(world, uid, local, true)
}

fn world_to_local_position(
    world: &World,
    uid: EntityUid,
    position: Vec2,
) -> Result<Vec2, TransformError> {
    let parent = world
        .get::<Transform>(uid)
        .ok_or(TransformError::NoTransform(uid))?
        .parent;
    match parent {
        Some(p) => to_local(world, p, position).ok_or(TransformError::InvalidParent(p)),
        None => Ok(position),
    }
}

/// Sets the local rotation in radians.
pub fn set_local_rotation(
    world: &mut World,
    uid: EntityUid,
    rotation: f64,
) -> Result<(), TransformError> {
    let t = world
        .get_mut::<Transform>(uid)
        .ok_or(TransformError::NoTransform(uid))?;
    t.local_rotation = rotation;
    world.dirty_fields::<Transform>(uid, Transform::FIELD_LOCAL_ROTATION);
    let mut pending = Vec::new();
    refresh_subtree(world, uid, &mut pending);
    raise_over_void(world, pending);
    Ok(())
}

// Tile anchoring

/// Anchors the entity to the grid tile beneath it, like `AnchorEntity`. The grid
/// becomes its parent and the tile center its position. Returns the tile indices.
pub fn anchor(world: &mut World, uid: EntityUid) -> Result<Vector2i, TransformError> {
    let t = world
        .get::<Transform>(uid)
        .ok_or(TransformError::NoTransform(uid))?;
    let grid = match t.grid_uid {
        Some(g) if g != uid => g,
        _ => return Err(TransformError::NotOnGrid(uid)),
    };
    if t.anchored {
        return Ok(MapGrid::tile_indices(t.local_position));
    }
    if t.parent != Some(grid) {
        set_parent(world, uid, grid)?;
    }
    let local = world
        .get::<Transform>(uid)
        .map(|t| t.local_position)
        .unwrap_or_default();
    let indices = MapGrid::tile_indices(local);
    let center = Vec2::new(indices.x as f32 + 0.5, indices.y as f32 + 0.5);
    if center != local {
        set_local_position(world, uid, center)?;
    }
    if let Some(t) = world.get_mut::<Transform>(uid) {
        t.anchored = true;
    }
    world.dirty_fields::<Transform>(uid, Transform::FIELD_ANCHORED);
    if let Some(g) = world.get_mut::<MapGrid>(grid) {
        g.anchored.entry(indices).or_default().push(uid);
    }
    Ok(indices)
}

/// Clears the tile anchor and returns whether the entity was anchored.
pub fn unanchor(world: &mut World, uid: EntityUid) -> bool {
    let Some(t) = world.get_mut::<Transform>(uid) else {
        return false;
    };
    if !t.anchored {
        return false;
    }
    t.anchored = false;
    let (grid, indices) = (t.parent, MapGrid::tile_indices(t.local_position));
    world.dirty_fields::<Transform>(uid, Transform::FIELD_ANCHORED);
    if let Some(g) = grid.and_then(|g| world.get_mut::<MapGrid>(g))
        && let Some(list) = g.anchored.get_mut(&indices)
    {
        list.retain(|&e| e != uid);
        if list.is_empty() {
            g.anchored.remove(&indices);
        }
    }
    true
}

/// Entities anchored to the tile, in attachment order, like `GetAnchoredEntities`.
pub fn anchored_entities(world: &World, grid: EntityUid, indices: Vector2i) -> Vec<EntityUid> {
    world
        .get::<MapGrid>(grid)
        .map_or_else(Vec::new, |g| g.anchored(indices).to_vec())
}

// Internal helpers

fn is_self_or_descendant(world: &World, candidate: EntityUid, root: EntityUid) -> bool {
    let mut cur = Some(candidate);
    while let Some(c) = cur {
        if c == root {
            return true;
        }
        cur = world.get::<Transform>(c).and_then(|t| t.parent);
    }
    false
}

/// Updates the world transform cache, map, grid, spatial index, and void flag for
/// the entity and all of its descendants.
pub(crate) fn refresh_subtree(world: &mut World, root: EntityUid, pending: &mut Vec<EntityUid>) {
    let mut stack = vec![root];
    while let Some(uid) = stack.pop() {
        refresh_one(world, uid, pending);
        if let Some(t) = world.get::<Transform>(uid) {
            stack.extend(t.children.iter().rev().copied());
        }
    }
}

fn refresh_one(world: &mut World, uid: EntityUid, pending: &mut Vec<EntityUid>) {
    let Some(t) = world.get::<Transform>(uid) else {
        return;
    };
    let (parent, local_position, local_rotation) = (t.parent, t.local_position, t.local_rotation);
    let (pp, pr, p_map, p_map_uid, p_grid) = match parent.and_then(|p| world.get::<Transform>(p)) {
        Some(pt) => (
            pt.world_position,
            pt.world_rotation,
            pt.map_id,
            pt.map_uid,
            pt.grid_uid,
        ),
        None => (Vec2::ZERO, 0.0, MapId::NULLSPACE, None, None),
    };
    let own_map = world.get::<Map>(uid).map(Map::map_id);
    let is_grid = world.has::<MapGrid>(uid);
    let (map_id, map_uid) = match own_map {
        Some(id) => (id, Some(uid)),
        None => (p_map, p_map_uid),
    };
    let grid_uid = if is_grid { Some(uid) } else { p_grid };
    let world_position = pp + local_position.rotated(pr);
    let world_rotation = pr + local_rotation;

    if let Some(t) = world.get_mut::<Transform>(uid) {
        t.map_id = map_id;
        t.map_uid = map_uid;
        t.grid_uid = grid_uid;
        t.world_position = world_position;
        t.world_rotation = world_rotation;
    }

    // Index the entity in grid cells when on a grid and map cells otherwise.
    let owner = match (own_map, map_uid) {
        (Some(_), _) | (_, None) => None,
        (None, Some(m)) => Some(if is_grid { m } else { grid_uid.unwrap_or(m) }),
    };
    if is_grid {
        state::get_mut(world).set_grid_map(uid, map_id);
    }
    match owner {
        Some(owner) => {
            let in_owner = if parent == Some(owner) {
                local_position
            } else {
                to_local(world, owner, world_position).unwrap_or(world_position)
            };
            state::get_mut(world)
                .spatial
                .update(uid, owner, in_owner.floor_div(CELL_SIZE));
        }
        None => {
            if let Some(s) = world.resource_mut::<state::MapState>() {
                s.spatial.remove(uid);
            }
        }
    }

    refresh_over_void(world, uid, pending);
}

/// Recomputes the void flag and adds the entity to `pending` when it enters the void.
pub(crate) fn refresh_over_void(world: &mut World, uid: EntityUid, pending: &mut Vec<EntityUid>) {
    let void = is_over_void(world, uid);
    if let Some(t) = world.get_mut::<Transform>(uid) {
        if void && !t.over_void {
            pending.push(uid);
        }
        t.over_void = void;
    }
}

/// Tests whether an entity is above the void. It must have a map or grid parent.
fn is_over_void(world: &World, uid: EntityUid) -> bool {
    if world.has::<Map>(uid) || world.has::<MapGrid>(uid) {
        return false;
    }
    let Some(t) = world.get::<Transform>(uid) else {
        return false;
    };
    let Some(parent) = t.parent else {
        return false;
    };
    if let Some(grid) = world.get::<MapGrid>(parent) {
        if !grid
            .tile(MapGrid::tile_indices(t.local_position))
            .is_empty()
        {
            return false;
        }
        // If the tile is empty, check the same point on the map's other grids.
        return crate::grid::tile_at(world, t.map_id, t.world_position).is_none();
    }
    if world.has::<Map>(parent) {
        return crate::grid::tile_at(world, t.map_id, t.world_position).is_none();
    }
    false
}

/// Raises [`OverVoid`] for queued transitions whose entities still exist and
/// remain over the void.
pub(crate) fn raise_over_void(world: &mut World, pending: Vec<EntityUid>) {
    for uid in pending {
        let Some(t) = world.get::<Transform>(uid) else {
            continue;
        };
        if !t.over_void || world.terminating_or_deleted(uid) {
            continue;
        }
        let mut event = OverVoid {
            entity: uid,
            map_id: t.map_id,
            grid: t.parent.filter(|p| world.has::<MapGrid>(*p)),
            position: t.world_position,
        };
        world.raise(uid, &mut event);
    }
}

/// Removes a deleted transform from its parent, the spatial index, and its tile.
pub(crate) fn detach(world: &mut World, uid: EntityUid) {
    unanchor(world, uid);
    let parent = world.get::<Transform>(uid).and_then(|t| t.parent);
    if let Some(p) = parent
        && let Some(pt) = world.get_mut::<Transform>(p)
    {
        pt.children.retain(|&c| c != uid);
    }
    if let Some(s) = world.resource_mut::<state::MapState>() {
        s.spatial.remove(uid);
    }
}
