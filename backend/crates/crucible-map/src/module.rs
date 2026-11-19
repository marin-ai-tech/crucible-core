//! Core `map` module for an assembled `crucible-ext` server.
//!
//! An extension enables the module by listing `map` in its manifest's `[modules] require`:
//! - Registers `Transform`, `Map`, `MapGrid`, and [`MapSystem`](crate::MapSystem)
//!   ([`register`](crate::register)), the [`TILE_KIND`](crate::TILE_KIND) prototype kind
//!   ([`register_tile_kind`]), and [`VoidTileSystem`].
//! - Builds the world resource [`TileDefinitions`] from loaded tile prototypes.
//! - Loads maps with [`load_map`] using entity prototypes and the `crucible-ext`
//!   [`ComponentFactories`] implementation of [`ComponentSpawner`].
//!
//! The core raises [`OverVoid`] and [`VoidTileSystem`] forwards it to the active
//! policy in the [`VoidTile`] resource, which the launcher sets from
//! [`ExtensionPoints::set_void_tile`](crucible_ext::ExtensionPoints::set_void_tile)).

use std::path::Path;
use std::sync::Arc;

use crucible_ecs::{EntityUid, NetworkPolicy, System, SystemInit, World};
use crucible_ext::{
    ComponentData, ComponentFactories, ComponentSpec, CoreModule, FieldError, Prototypes,
    RegisterContext, VoidTile,
};
use crucible_proto::{FieldDef, FieldType, Node, PrototypeSet};

use crate::events::OverVoid;
use crate::loader::{
    ComponentSpawner, FileEntities, LoadedMap, MapDiagnostic, MapLoadError, MapLoader,
};
use crate::tile::{TileDefinitions, register_tile_kind};
use crate::transform::Transform;

/// Map module ID for `[modules] require`.
pub const MODULE_ID: &str = "map";

/// Core map module.
#[derive(Clone, Default)]
pub struct MapModule {
    tile_fields: Vec<FieldDef>,
}

impl MapModule {
    /// Creates the module with its built-in tile prototype fields.
    pub fn new() -> Self {
        MapModule::default()
    }

    /// Adds tile prototype fields consumed by other modules, such as the friction
    /// field from `crucible_physics::tile_fields`.
    pub fn with_tile_fields(mut self, fields: impl IntoIterator<Item = FieldDef>) -> Self {
        self.tile_fields.extend(fields);
        self
    }
}

impl CoreModule for MapModule {
    fn id(&self) -> &'static str {
        MODULE_ID
    }

    fn register(&self, ctx: &mut RegisterContext<'_>) {
        let fields = self.tile_fields.clone();
        // Prototype `Transform` fields (`anchored`, `noRot`) do not set position or parent.
        // Spawning or map loading sets both. The loader anchors to a tile after parents
        // are set, since anchoring requires a grid. The model does not store `noRot`.
        ctx.component(
            ComponentSpec::<Transform>::new("Transform")
                .field("anchored", FieldType::Bool)
                .field("noRot", FieldType::Bool)
                .network(NetworkPolicy::AllViewers)
                .networked_fields()
                .from_prototype(read_transform),
        )
        .world_builder(crate::register_without_transform)
        .prototype_registry(|r| register_tile_kind(r, fields))
        .system(VoidTileSystem);
    }

    /// At most one prototype can define the empty tile. Report violations during
    /// `validate-content` so they are caught before launch.
    fn validate(&self, set: &PrototypeSet) -> Vec<String> {
        match TileDefinitions::from_prototypes(set) {
            Ok(_) => Vec::new(),
            Err(e) => vec![e.to_string()],
        }
    }

    fn ready(&self, world: &mut World) -> Result<(), String> {
        let set = world
            .resource::<Prototypes>()
            .map(|p| Arc::clone(&p.set))
            .ok_or("world has no prototype set")?;
        let tiles = TileDefinitions::from_prototypes(&set).map_err(|e| e.to_string())?;
        world.insert_resource(tiles);
        Ok(())
    }
}

/// Validates the prototype's `anchored` and `noRot` boolean fields and creates
/// an empty transform. An entity must be placed on a grid before anchoring.
fn read_transform(d: &ComponentData<'_>) -> Result<Transform, FieldError> {
    d.bool("anchored")?;
    d.bool("noRot")?;
    Ok(Transform::default())
}

/// Forwards [`OverVoid`] to the active extension's policy in the [`VoidTile`] resource.
/// Without that resource, the event is handled by ordinary core subscribers.
pub struct VoidTileSystem;

impl System for VoidTileSystem {
    fn initialize(&mut self, init: &mut SystemInit<'_>) {
        init.subscribe::<Transform, OverVoid, _>(on_over_void);
    }
}

fn on_over_void(world: &mut World, uid: EntityUid, _: &mut OverVoid) {
    let Some(VoidTile(policy)) = world.resource::<VoidTile>().cloned() else {
        return;
    };
    policy.over_void(world, uid);
}

impl ComponentSpawner for ComponentFactories {
    fn add_component(
        &mut self,
        world: &mut World,
        uid: EntityUid,
        name: &str,
        data: &Node,
        _entities: &FileEntities,
    ) -> Result<bool, String> {
        ComponentFactories::add_component(self, world, uid, name, data).map_err(|e| e.to_string())
    }
}

/// Loads an SS14 map into an assembled server world. The [`Prototypes`] resource
/// supplies the prototype set, component factories, and ignored component registry.
/// The map module's [`TileDefinitions`] translates tile IDs.
pub fn load_map(world: &mut World, path: &Path) -> Result<LoadedMap, MapLoadError> {
    let file = path.to_string_lossy().replace('\\', "/");
    let fail = |message: &str| MapLoadError {
        file: file.clone(),
        diagnostics: vec![MapDiagnostic {
            line: None,
            entity: None,
            prototype: None,
            message: message.to_owned(),
        }],
    };
    let Prototypes {
        set,
        factories,
        registry,
    } = world
        .resource::<Prototypes>()
        .cloned()
        .ok_or_else(|| fail("world has no prototype set"))?;
    let tiles = world
        .resource::<TileDefinitions>()
        .cloned()
        .ok_or_else(|| fail("world has no tile definitions; enable the map module"))?;
    let mut components = (*factories).clone();
    MapLoader::new(&tiles)
        .with_prototypes(&set)
        .with_registry(&registry)
        .load_file(world, &mut components, path)
}
