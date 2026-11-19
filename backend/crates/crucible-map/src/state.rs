//! World resource for map and grid registration and the spatial hash.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crucible_ecs::{EntityUid, World};

use crate::grid::MapId;
use crate::math::Vector2i;

/// Spatial hash cell size in meters, equal to the default chunk size.
pub(crate) const CELL_SIZE: f32 = 16.0;

#[derive(Default)]
struct MapEntry {
    uid: Option<EntityUid>,
    grids: BTreeSet<EntityUid>,
}

/// Spatial hash with cells in the local coordinates of each owner map or grid.
/// Grid entities are indexed in the grid's coordinates, so moving the grid leaves
/// the index of its contents intact.
#[derive(Default)]
pub(crate) struct SpatialIndex {
    pub(crate) owners: HashMap<EntityUid, HashMap<Vector2i, Vec<EntityUid>>>,
    entries: HashMap<EntityUid, (EntityUid, Vector2i)>,
}

impl SpatialIndex {
    /// Places the entity in `cell` for `owner`, moving it from its previous cell if needed.
    pub(crate) fn update(&mut self, uid: EntityUid, owner: EntityUid, cell: Vector2i) {
        if self.entries.get(&uid) == Some(&(owner, cell)) {
            return;
        }
        self.remove(uid);
        self.entries.insert(uid, (owner, cell));
        self.owners
            .entry(owner)
            .or_default()
            .entry(cell)
            .or_default()
            .push(uid);
    }

    /// Removes the entity from the index.
    pub(crate) fn remove(&mut self, uid: EntityUid) {
        let Some((owner, cell)) = self.entries.remove(&uid) else {
            return;
        };
        if let Some(cells) = self.owners.get_mut(&owner) {
            if let Some(list) = cells.get_mut(&cell) {
                list.retain(|&e| e != uid);
                if list.is_empty() {
                    cells.remove(&cell);
                }
            }
            if cells.is_empty() {
                self.owners.remove(&owner);
            }
        }
    }

    /// Cells belonging to the owner.
    pub(crate) fn cells(&self, owner: EntityUid) -> Option<&HashMap<Vector2i, Vec<EntityUid>>> {
        self.owners.get(&owner)
    }
}

/// Map model state stored as a world resource.
#[derive(Default)]
pub(crate) struct MapState {
    next_map: u32,
    maps: BTreeMap<MapId, MapEntry>,
    grid_map: HashMap<EntityUid, MapId>,
    pub(crate) spatial: SpatialIndex,
}

impl MapState {
    pub(crate) fn register_map(&mut self, uid: EntityUid) -> MapId {
        self.next_map += 1;
        let id = MapId(self.next_map);
        self.maps.entry(id).or_default().uid = Some(uid);
        id
    }

    pub(crate) fn unregister_map(&mut self, map_id: MapId) {
        if let Some(entry) = self.maps.remove(&map_id) {
            for grid in entry.grids {
                self.grid_map.remove(&grid);
            }
        }
    }

    pub(crate) fn map_entity(&self, map_id: MapId) -> Option<EntityUid> {
        self.maps.get(&map_id)?.uid
    }

    pub(crate) fn maps(&self) -> Vec<(MapId, EntityUid)> {
        self.maps
            .iter()
            .filter_map(|(id, e)| e.uid.map(|u| (*id, u)))
            .collect()
    }

    pub(crate) fn grids(&self, map_id: MapId) -> Vec<EntityUid> {
        self.maps
            .get(&map_id)
            .map_or_else(Vec::new, |e| e.grids.iter().copied().collect())
    }

    /// Associates a grid with a map, or detaches it when given `NULLSPACE`.
    pub(crate) fn set_grid_map(&mut self, grid: EntityUid, map_id: MapId) {
        if self.grid_map.get(&grid) == Some(&map_id) {
            return;
        }
        self.remove_grid(grid);
        if map_id != MapId::NULLSPACE
            && let Some(entry) = self.maps.get_mut(&map_id)
        {
            entry.grids.insert(grid);
            self.grid_map.insert(grid, map_id);
        }
    }

    pub(crate) fn remove_grid(&mut self, grid: EntityUid) {
        if let Some(old) = self.grid_map.remove(&grid)
            && let Some(entry) = self.maps.get_mut(&old)
        {
            entry.grids.remove(&grid);
        }
    }
}

pub(crate) fn get(world: &World) -> Option<&MapState> {
    world.resource::<MapState>()
}

pub(crate) fn get_mut(world: &mut World) -> &mut MapState {
    if world.resource::<MapState>().is_none() {
        world.insert_resource(MapState::default());
    }
    world
        .resource_mut::<MapState>()
        .expect("the resource was just inserted")
}
