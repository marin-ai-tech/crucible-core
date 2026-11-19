//! Map component registration and hierarchy maintenance during deletion.

use crucible_ecs::{
    ComponentRegistration, ComponentRemove, EntityTerminating, EntityUid, NetworkPolicy,
    RegistryError, System, SystemInit, World, WorldBuilder,
};

use crate::grid::{Map, MapGrid};
use crate::state;
use crate::transform::{self, Transform};

/// Source identifier for map component registrations.
pub const SOURCE: &str = "core:map";

/// Registers `Transform`, `Map`, and `MapGrid` for all viewers and adds [`MapSystem`].
pub fn register(builder: &mut WorldBuilder) -> Result<(), RegistryError> {
    builder.register_component(
        ComponentRegistration::new::<Transform>("Transform", SOURCE)
            .networked_fields::<Transform>()
            .network(NetworkPolicy::AllViewers),
    )?;
    register_without_transform(builder)
}

/// Like [`register`] without `Transform`. The map module registers that component
/// and its prototype factory ([`MapModule`](crate::MapModule)).
pub fn register_without_transform(builder: &mut WorldBuilder) -> Result<(), RegistryError> {
    builder.register_component(
        ComponentRegistration::new::<Map>("Map", SOURCE)
            .networked_fields::<Map>()
            .network(NetworkPolicy::AllViewers),
    )?;
    builder.register_component(
        ComponentRegistration::new::<MapGrid>("MapGrid", SOURCE)
            .networked_fields::<MapGrid>()
            .network(NetworkPolicy::AllViewers),
    )?;
    builder.add_system(MapSystem);
    Ok(())
}

/// When an entity is deleted, removes its children, disconnects it from the parent,
/// spatial index and tile, and removes it from the map and grid tables.
pub struct MapSystem;

impl System for MapSystem {
    fn initialize(&mut self, init: &mut SystemInit<'_>) {
        init.subscribe::<Transform, EntityTerminating, _>(on_terminating);
        init.subscribe::<Transform, ComponentRemove, _>(|world, uid, _| {
            transform::detach(world, uid)
        });
        init.subscribe::<Map, ComponentRemove, _>(on_map_remove);
        init.subscribe::<MapGrid, ComponentRemove, _>(on_grid_remove);
    }
}

fn on_terminating(world: &mut World, uid: EntityUid, _: &mut EntityTerminating) {
    let children = world
        .get::<Transform>(uid)
        .map(|t| t.children().to_vec())
        .unwrap_or_default();
    for child in children {
        world.delete(child);
    }
}

fn on_map_remove(world: &mut World, uid: EntityUid, _: &mut ComponentRemove) {
    if let Some(map_id) = world.get::<Map>(uid).map(Map::map_id) {
        state::get_mut(world).unregister_map(map_id);
    }
}

fn on_grid_remove(world: &mut World, uid: EntityUid, _: &mut ComponentRemove) {
    state::get_mut(world).remove_grid(uid);
}
