//! Map model events.

use crucible_ecs::EntityUid;

use crate::grid::MapId;
use crate::math::{Vec2, Vector2i};
use crate::tile::Tile;

/// A grid tile changed, corresponding to Robust's `TileChangedEvent`.
///
/// Raised locally on the grid entity and then broadcast, as with Robust's
/// `RaiseLocalEvent(uid, ref ev, broadcast: true)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileChanged {
    /// The grid.
    pub grid: EntityUid,
    /// Tile indices on the grid.
    pub indices: Vector2i,
    /// Previous tile.
    pub old: Tile,
    /// New tile.
    pub new: Tile,
}

/// An entity moved over the void, with no nonempty tile beneath it on any grid of the map.
///
/// The event describes the transition. The active extension supplies the void policy.
/// Subscribe with `subscribe::<Transform, OverVoid>` to receive it on the entity
/// after movement, reparenting, or clearing the tile beneath it.
/// The entity must have a map or grid parent. Entities contained by other
/// entities are outside the scope of this event.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OverVoid {
    /// Entity over the void.
    pub entity: EntityUid,
    /// The map.
    pub map_id: MapId,
    /// Parent grid, if the entity is on a grid.
    pub grid: Option<EntityUid>,
    /// Entity position in world coordinates.
    pub position: Vec2,
}
