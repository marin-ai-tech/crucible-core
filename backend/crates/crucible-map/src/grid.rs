//! Maps, tile grids, and chunks.
//!
//! Maps and grids are entities with [`Map`] and [`MapGrid`] components. Each grid
//! stores square tile chunks (16×16 by default) and lists of anchored entities.
//! The data layout matches Robust's `MapComponent`, `MapGridComponent`, and
//! `MapChunk`. The implementation is independent.

use std::collections::{BTreeMap, HashMap};
use std::fmt;

use crucible_ecs::{Component, EntityUid, Tick, World, networked};

use crate::events::TileChanged;
use crate::math::{Vec2, Vector2i};
use crate::state::{self, SpatialIndex};
use crate::tile::Tile;
use crate::transform;

/// Default chunk side length in tiles.
pub const DEFAULT_CHUNK_SIZE: u8 = 16;

/// Map ID. [`MapId::NULLSPACE`] represents a location outside all maps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MapId(pub u32);

impl MapId {
    /// Nullspace for entities outside a map.
    pub const NULLSPACE: MapId = MapId(0);
}

impl fmt::Display for MapId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Map component at the root of a transform hierarchy.
#[networked]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Map {
    pub(crate) map_id: MapId,
    pub(crate) paused: bool,
    lighting_enabled: bool,
}

impl Component for Map {}

impl Map {
    /// The map ID.
    pub fn map_id(&self) -> MapId {
        self.map_id
    }

    /// Whether the map is paused.
    pub fn paused(&self) -> bool {
        self.paused
    }

    /// Whether the client applies lighting when rendering the map.
    pub fn lighting_enabled(&self) -> bool {
        self.lighting_enabled
    }

    /// Enables map lighting so it remains active when light sources are disabled.
    pub fn enable_lighting(world: &mut World, map: MapId) {
        let Some(entity) = map_entity(world, map) else {
            return;
        };
        if world
            .get::<Self>(entity)
            .is_some_and(|map| !map.lighting_enabled)
        {
            Self::set_lighting_enabled(world, entity, true);
        }
    }
}

/// Square chunk of `size × size` tiles in row-major order.
#[derive(Clone, Debug, PartialEq)]
pub struct Chunk {
    size: u8,
    tiles: Box<[Tile]>,
    filled: u32,
    last_modified: Tick,
}

impl Chunk {
    fn new(size: u8) -> Chunk {
        let n = usize::from(size) * usize::from(size);
        Chunk {
            size,
            tiles: vec![Tile::EMPTY; n].into_boxed_slice(),
            filled: 0,
            last_modified: Tick::ZERO,
        }
    }

    fn slot(&self, local: Vector2i) -> usize {
        local.y as usize * usize::from(self.size) + local.x as usize
    }

    /// Tile at the local chunk index (`0..size` on each axis).
    pub fn tile(&self, local: Vector2i) -> Tile {
        self.tiles[self.slot(local)]
    }

    /// All tiles in row-major order.
    pub fn tiles(&self) -> &[Tile] {
        &self.tiles
    }

    /// Number of nonempty tiles.
    pub fn filled(&self) -> u32 {
        self.filled
    }

    /// Tick of the latest tile change, used for replication deltas.
    pub fn last_modified(&self) -> Tick {
        self.last_modified
    }

    fn set(&mut self, local: Vector2i, tile: Tile, tick: Tick) -> Tile {
        let slot = self.slot(local);
        let old = std::mem::replace(&mut self.tiles[slot], tile);
        match (old.is_empty(), tile.is_empty()) {
            (true, false) => self.filled += 1,
            (false, true) => self.filled -= 1,
            _ => {}
        }
        self.last_modified = tick;
        old
    }
}

/// Tile grid component, corresponding to `MapGridComponent`.
#[networked]
#[derive(Clone, Debug, PartialEq)]
pub struct MapGrid {
    pub(crate) chunk_size: u8,
    pub(crate) chunks: BTreeMap<Vector2i, Chunk>,
    /// Tile-anchored entities in anchoring order.
    #[networked(skip)]
    pub(crate) anchored: HashMap<Vector2i, Vec<EntityUid>>,
}

impl Component for MapGrid {}

impl Default for MapGrid {
    fn default() -> Self {
        MapGrid::new(DEFAULT_CHUNK_SIZE)
    }
}

impl MapGrid {
    /// Empty grid using chunks of `chunk_size × chunk_size`, with a minimum size of 1.
    pub fn new(chunk_size: u8) -> MapGrid {
        MapGrid {
            chunk_size: chunk_size.max(1),
            chunks: BTreeMap::new(),
            anchored: HashMap::new(),
        }
    }

    /// Chunk side length in tiles.
    pub fn chunk_size(&self) -> u8 {
        self.chunk_size
    }

    /// Chunk index and chunk-local index of tile `indices`.
    pub fn chunk_of(&self, indices: Vector2i) -> (Vector2i, Vector2i) {
        let cs = i32::from(self.chunk_size);
        (
            Vector2i::new(indices.x.div_euclid(cs), indices.y.div_euclid(cs)),
            Vector2i::new(indices.x.rem_euclid(cs), indices.y.rem_euclid(cs)),
        )
    }

    /// Tile at the grid indices, or an empty tile when outside allocated chunks.
    pub fn tile(&self, indices: Vector2i) -> Tile {
        let (chunk, local) = self.chunk_of(indices);
        self.chunks
            .get(&chunk)
            .map_or(Tile::EMPTY, |c| c.tile(local))
    }

    /// Chunk at the given chunk index.
    pub fn chunk(&self, index: Vector2i) -> Option<&Chunk> {
        self.chunks.get(&index)
    }

    /// Chunks in ascending index order.
    pub fn chunks(&self) -> impl Iterator<Item = (Vector2i, &Chunk)> {
        self.chunks.iter().map(|(k, v)| (*k, v))
    }

    /// Nonempty tiles and their grid indices, grouped by chunk.
    pub fn tiles(&self) -> impl Iterator<Item = (Vector2i, Tile)> + '_ {
        let cs = i32::from(self.chunk_size);
        self.chunks.iter().flat_map(move |(ci, chunk)| {
            chunk
                .tiles
                .iter()
                .enumerate()
                .filter(|(_, t)| !t.is_empty())
                .map(move |(slot, t)| {
                    let slot = slot as i32;
                    (
                        Vector2i::new(ci.x * cs + slot % cs, ci.y * cs + slot / cs),
                        *t,
                    )
                })
        })
    }

    /// Indices of the tile under a point in grid coordinates.
    pub fn tile_indices(local: Vec2) -> Vector2i {
        local.floor_div(1.0)
    }

    /// Entities anchored to the tile, in anchoring order.
    pub fn anchored(&self, indices: Vector2i) -> &[EntityUid] {
        self.anchored.get(&indices).map_or(&[], Vec::as_slice)
    }

    /// Writes a tile without raising events and returns its previous value. The
    /// first nonempty tile written to a chunk creates it.
    pub(crate) fn put(&mut self, indices: Vector2i, tile: Tile, tick: Tick) -> Tile {
        let size = self.chunk_size;
        let (chunk, local) = self.chunk_of(indices);
        if tile.is_empty() && !self.chunks.contains_key(&chunk) {
            return Tile::EMPTY;
        }
        self.chunks
            .entry(chunk)
            .or_insert_with(|| Chunk::new(size))
            .set(local, tile, tick)
    }
}

/// Creates an entity with a [`Map`] and root [`Transform`](crate::Transform). Returns
/// the entity and its [`MapId`], allocated from 1 without reuse in the world.
pub fn create_map(world: &mut World) -> (EntityUid, MapId) {
    let uid = world.spawn();
    let map_id = add_map(world, uid, false);
    transform::place(world, uid, None, Vec2::ZERO, 0.0).expect("new entity has no transform");
    (uid, map_id)
}

/// Adds a [`Map`] component and allocates its [`MapId`]. The transform is assigned separately.
pub(crate) fn add_map(world: &mut World, uid: EntityUid, paused: bool) -> MapId {
    let map_id = state::get_mut(world).register_map(uid);
    world
        .add_component(
            uid,
            Map {
                map_id,
                paused,
                lighting_enabled: false,
            },
        )
        .expect("map entity exists and has no map component");
    map_id
}

/// Map entity for the given map ID.
pub fn map_entity(world: &World, map_id: MapId) -> Option<EntityUid> {
    state::get(world)?.map_entity(map_id)
}

/// All maps in ascending map ID order.
pub fn maps(world: &World) -> Vec<(MapId, EntityUid)> {
    state::get(world).map_or_else(Vec::new, |s| s.maps())
}

/// All grids on the map in ascending entity ID order.
pub fn grids_on_map(world: &World, map_id: MapId) -> Vec<EntityUid> {
    state::get(world).map_or_else(Vec::new, |s| s.grids(map_id))
}

/// Spawns a grid at `position` on the map with default 16×16 chunks. Returns
/// `None` if the map does not exist.
pub fn create_grid(world: &mut World, map_id: MapId, position: Vec2) -> Option<EntityUid> {
    let map_uid = map_entity(world, map_id)?;
    let uid = world.spawn();
    world
        .add_component(uid, MapGrid::default())
        .expect("new entity has no grid");
    transform::place(world, uid, Some(map_uid), position, 0.0).expect("map exists");
    Some(uid)
}

/// Tile at the grid indices, or `None` if the entity has no grid.
pub fn get_tile(world: &World, grid: EntityUid, indices: Vector2i) -> Option<Tile> {
    world.get::<MapGrid>(grid).map(|g| g.tile(indices))
}

/// Sets a grid tile. On change, marks `chunks` dirty and raises [`TileChanged`]
/// locally and as a broadcast. If the tile became empty, raises
/// [`OverVoid`](crate::OverVoid) for entities above it. Returns the previous tile,
/// or `None` if the entity has no grid.
pub fn set_tile(world: &mut World, grid: EntityUid, indices: Vector2i, tile: Tile) -> Option<Tile> {
    let tick = world.current_tick();
    let old = world.get_mut::<MapGrid>(grid)?.put(indices, tile, tick);
    if old == tile {
        return Some(old);
    }
    world.dirty_fields::<MapGrid>(grid, MapGrid::FIELD_CHUNKS);
    let mut event = TileChanged {
        grid,
        indices,
        old,
        new: tile,
    };
    world.raise(grid, &mut event);
    world.raise_broadcast(&mut event);
    if old.is_empty() != tile.is_empty() {
        let mut pending = Vec::new();
        for uid in crate::lookup::entities_on_tile(world, grid, indices) {
            transform::refresh_over_void(world, uid, &mut pending);
        }
        transform::raise_over_void(world, pending);
    }
    Some(old)
}

/// Finds a nonempty tile beneath a point on the map, checking grids by entity ID.
/// Returns the grid, tile indices, and tile.
pub fn tile_at(
    world: &World,
    map_id: MapId,
    position: Vec2,
) -> Option<(EntityUid, Vector2i, Tile)> {
    for grid in grids_on_map(world, map_id) {
        let Some(local) = transform::to_local(world, grid, position) else {
            continue;
        };
        let indices = MapGrid::tile_indices(local);
        if let Some(tile) = get_tile(world, grid, indices)
            && !tile.is_empty()
        {
            return Some((grid, indices, tile));
        }
    }
    None
}

/// Looks up the spatial index when the world has a map model.
pub(crate) fn spatial(world: &World) -> Option<&SpatialIndex> {
    state::get(world).map(|s| &s.spatial)
}
