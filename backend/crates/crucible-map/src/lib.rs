//! Maps, grids, tiles, transforms, and spatial queries.
//!
//! A world contains maps, and each map can contain grids of square tile chunks,
//! 16×16 by default. Maps and grids are entities with [`Map`] and [`MapGrid`].
//! Each [`Transform`] has a parent map, grid, or entity, plus local position and
//! rotation. The hierarchy maintains a cache of world transforms.
//!
//! The map primitives and transform component are implemented independently.
//! The adapter can represent Robust tile data with the corresponding [`Tile`] fields:
//! `type_id`, `flags`, `variant`, and `rotation_mirroring`. No conversion of the
//! field values is needed.
//!
//! # Robust to Crucible types
//!
//! | Robust (C#) | Crucible (Rust) |
//! |---|---|
//! | `MapId`, `MapComponent`, `MapGridComponent`, `MapChunk`, `Tile` | [`MapId`], [`Map`], [`MapGrid`], [`Chunk`], [`Tile`] |
//! | `MapSystem.CreateMap()`, `MapManager.CreateGridEntity(mapId)` | [`create_map`], [`create_grid`] |
//! | `SetTile`, `GetTileRef`, `TileChangedEvent` | [`set_tile`], [`get_tile`], [`TileChanged`] |
//! | `ITileDefinitionManager` (ID 0 is empty; remaining IDs ordered by prototype) | [`TileDefinitions`] |
//! | `TransformComponent`, `SetParent`, `SetCoordinates`, `GetWorldPosition` | [`Transform`], [`set_parent`], [`set_local_position`], [`world_position`] |
//! | `AnchorEntity`, `Unanchor`, `GetAnchoredEntities` | [`anchor`], [`unanchor`], [`anchored_entities`] |
//! | `EntityLookupSystem.GetEntitiesInRange` / `…Intersecting` / `GetEntitiesInTile` | [`entities_in_radius`], [`entities_in_box`], [`entities_on_tile`] |
//! | `MapLoaderSystem.TryLoad` | [`MapLoader::load_file`] |
//!
//! [`OverVoid`] reports a transition to a point with no tile beneath the entity.
//! The active extension decides what the event means for the simulation.
//!
//! # Integration
//!
//! [`register`] adds the components with replication to all viewers and installs
//! [`MapSystem`], which removes descendants and clears hierarchy, index, and anchor
//! entries during deletion. Build [`TileDefinitions`] from [`TILE_KIND`] prototypes.
//!
//! An assembled `crucible-ext` server uses [`MapModule`] (`map` in `[modules] require`).
//! It registers the model, builds the tile table after loading data, and lets
//! [`VoidTileSystem`] route [`OverVoid`] to the void policy. Use [`load_map`] to load
//! maps and build entities from prototypes.
//!
//! ```
//! use crucible_ecs::World;
//! use crucible_map::{Tile, Vec2, Vector2i, create_grid, create_map, entities_in_radius, get_tile, set_tile, spawn_at};
//!
//! let mut builder = World::builder();
//! crucible_map::register(&mut builder).unwrap();
//! let (mut world, _systems) = builder.build().unwrap();
//!
//! let (map_uid, map_id) = create_map(&mut world);
//! let grid = create_grid(&mut world, map_id, Vec2::new(3.0, 2.0)).unwrap();
//! set_tile(&mut world, grid, Vector2i::new(17, -3), Tile::new(1));
//! assert_eq!(get_tile(&world, grid, Vector2i::new(17, -3)), Some(Tile::new(1)));
//!
//! let thing = spawn_at(&mut world, map_uid, Vec2::new(1.0, 1.0)).unwrap();
//! assert_eq!(entities_in_radius(&world, map_id, Vec2::ZERO, 2.0), vec![thing]);
//! ```

mod events;
mod grid;
mod loader;
mod lookup;
mod math;
mod module;
mod state;
mod system;
mod tile;
mod transform;

pub use events::{OverVoid, TileChanged};
pub use grid::{
    Chunk, DEFAULT_CHUNK_SIZE, Map, MapGrid, MapId, create_grid, create_map, get_tile,
    grids_on_map, map_entity, maps, set_tile, tile_at,
};
pub use loader::{
    ComponentSpawner, FileEntities, LoadedMap, MapDiagnostic, MapLoadError, MapLoader,
    MissingPrototypes,
};
pub use lookup::{entities_in_box, entities_in_radius, entities_on_tile};
pub use math::{Box2, Vec2, Vector2i};
pub use module::{MODULE_ID, MapModule, VoidTileSystem, load_map};
pub use system::{MapSystem, SOURCE, register, register_without_transform};
pub use tile::{
    EMPTY_FIELD, TILE_KIND, Tile, TileDefinitions, TileDefinitionsError, register_tile_kind,
};
pub use transform::{
    Transform, TransformError, anchor, anchored_entities, place, set_local_position,
    set_local_rotation, set_parent, set_parent_with, set_world_position, spawn_at,
    teleport_world_position, to_local, to_world, unanchor, world_position, world_rotation,
};
