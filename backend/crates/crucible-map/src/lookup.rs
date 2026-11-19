//! Spatial entity queries within a map.
//!
//! The spatial hash uses 16×16 m cells in the coordinate space of each grid and map.
//! Results are sorted by entity ID and exclude maps, entities on other maps, and
//! entities that are being deleted or have been deleted.

use crucible_ecs::{EntityUid, World};

use crate::grid::{self, MapGrid, MapId};
use crate::math::{Box2, Vec2, Vector2i};
use crate::state::CELL_SIZE;
use crate::transform::{self, Transform};

/// Entities on the map within `radius` of `center`, in world coordinates.
pub fn entities_in_radius(
    world: &World,
    map_id: MapId,
    center: Vec2,
    radius: f32,
) -> Vec<EntityUid> {
    let r2 = radius * radius;
    collect(world, map_id, Box2::centered(center, radius), |p| {
        (p - center).length_squared() <= r2
    })
}

/// Entities on the map inside an axis-aligned rectangle, inclusive and in world coordinates.
pub fn entities_in_box(world: &World, map_id: MapId, area: Box2) -> Vec<EntityUid> {
    collect(world, map_id, area, |p| area.contains(p))
}

/// Anchored and unanchored entities whose positions fall on tile `indices` of `grid`,
/// including entities on the same map with different parents.
pub fn entities_on_tile(world: &World, grid: EntityUid, indices: Vector2i) -> Vec<EntityUid> {
    let Some(gt) = world.get::<Transform>(grid) else {
        return Vec::new();
    };
    if !world.has::<MapGrid>(grid) {
        return Vec::new();
    }
    let map_id = gt.map_id;
    let corners = [
        Vec2::new(indices.x as f32, indices.y as f32),
        Vec2::new(indices.x as f32 + 1.0, indices.y as f32),
        Vec2::new(indices.x as f32 + 1.0, indices.y as f32 + 1.0),
        Vec2::new(indices.x as f32, indices.y as f32 + 1.0),
    ]
    .map(|c| transform::to_world(world, grid, c).unwrap_or(c));
    let area = Box2::bounding(&corners);
    let mut found = collect(world, map_id, area, |p| {
        transform::to_local(world, grid, p).map(MapGrid::tile_indices) == Some(indices)
    });
    found.retain(|&e| e != grid);
    found
}

fn collect(
    world: &World,
    map_id: MapId,
    area: Box2,
    accept: impl Fn(Vec2) -> bool,
) -> Vec<EntityUid> {
    let mut out = Vec::new();
    let (Some(index), Some(map_uid)) = (grid::spatial(world), grid::map_entity(world, map_id))
    else {
        return out;
    };
    let mut owners = vec![map_uid];
    owners.extend(grid::grids_on_map(world, map_id));
    for owner in owners {
        let Some(cells) = index.cells(owner) else {
            continue;
        };
        let local = area
            .corners()
            .map(|c| transform::to_local(world, owner, c).unwrap_or(c));
        let local = Box2::bounding(&local);
        let lo = local.min.floor_div(CELL_SIZE);
        let hi = local.max.floor_div(CELL_SIZE);
        let range =
            (i64::from(hi.x) - i64::from(lo.x) + 1) * (i64::from(hi.y) - i64::from(lo.y) + 1);
        let mut visit = |list: &Vec<EntityUid>| {
            for &uid in list {
                if world.terminating_or_deleted(uid) {
                    continue;
                }
                if let Some(t) = world.get::<Transform>(uid)
                    && t.map_id() == map_id
                    && accept(t.world_position())
                {
                    out.push(uid);
                }
            }
        };
        if range > cells.len() as i64 {
            for (cell, list) in cells {
                if cell.x >= lo.x && cell.x <= hi.x && cell.y >= lo.y && cell.y <= hi.y {
                    visit(list);
                }
            }
        } else {
            for y in lo.y..=hi.y {
                for x in lo.x..=hi.x {
                    if let Some(list) = cells.get(&Vector2i::new(x, y)) {
                        visit(list);
                    }
                }
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}
