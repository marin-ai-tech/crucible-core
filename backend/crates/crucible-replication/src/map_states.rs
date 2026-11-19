//! Serializers for map components `Transform`, `Map`, and `MapGrid`.
//!
//!
//! Field names come from the `crucible-map` component schemas (`FIELD_NAMES`).
//! The Robust adapter maps them to `TransformComponentState`, `MapComponentState`, and `MapGridComponentState`.

use crucible_ecs::{ComponentRegistry, NetworkedComponent};
use crucible_map::{Map, MapGrid, Transform};

use crate::components::{ReplicatedComponents, ReplicationError, StateContext};
use crate::value::{FieldSelection, NeutralValue, StructWriter};

/// Registers serializers for `Transform`, `Map`, and `MapGrid`. First register the
/// components in the world with `crucible_map::register`.
pub fn register_map_states(
    replicated: &mut ReplicatedComponents,
    registry: &ComponentRegistry,
) -> Result<(), ReplicationError> {
    replicated.register_networked::<Transform>(registry, transform_state)?;
    // Send the complete map state to keep its ID, pause flag, and lighting state together.
    replicated.register::<Map>(registry, map_state)?;
    replicated.register_networked::<MapGrid>(registry, grid_state)?;
    Ok(())
}

/// `Transform` fields: `parent` (entity), `local_position` (`Vec2`), `local_rotation`
/// (`F64`, radians), and `anchored`.
pub fn transform_state(
    ctx: &StateContext<'_>,
    t: &Transform,
    selection: FieldSelection,
) -> NeutralValue {
    let mut w = StructWriter::new(Transform::FIELD_NAMES, selection);
    w.field("parent", || ctx.entity(t.parent()));
    w.field("local_position", || {
        let p = t.local_position();
        NeutralValue::Vec2([p.x, p.y])
    });
    w.field("local_rotation", || NeutralValue::F64(t.local_rotation()));
    w.field("anchored", || NeutralValue::Bool(t.anchored()));
    w.field("teleport_count", || {
        NeutralValue::UInt(t.teleport_count().into())
    });
    w.finish()
}

/// `Map`: `map_id`, `paused`.
pub fn map_state(_ctx: &StateContext<'_>, m: &Map, selection: FieldSelection) -> NeutralValue {
    let mut w = StructWriter::new(Map::FIELD_NAMES, selection);
    w.field("map_id", || NeutralValue::UInt(u64::from(m.map_id().0)));
    w.field("paused", || NeutralValue::Bool(m.paused()));
    w.field("lighting_enabled", || {
        NeutralValue::Bool(m.lighting_enabled())
    });
    w.finish()
}

/// `MapGrid` contains `chunk_size` and chunks changed after [`StateContext::since`].
/// A full state contains every chunk. Each chunk records its `x` and `y` indices
/// and its row-major tiles in parallel `type_ids`, `flags`, `variants`, and
/// `rotation_mirroring` lists, following the Robust `Tile` layout.
pub fn grid_state(ctx: &StateContext<'_>, g: &MapGrid, selection: FieldSelection) -> NeutralValue {
    let mut w = StructWriter::new(MapGrid::FIELD_NAMES, selection);
    w.field("chunk_size", || {
        NeutralValue::UInt(u64::from(g.chunk_size()))
    });
    w.field("chunks", || {
        let since = ctx.since();
        NeutralValue::List(
            g.chunks()
                .filter(|(_, c)| c.last_modified() > since || since.0 == 0)
                .map(|(index, chunk)| {
                    let tiles = chunk.tiles();
                    NeutralValue::Struct(vec![
                        ("x".into(), NeutralValue::Int(i64::from(index.x))),
                        ("y".into(), NeutralValue::Int(i64::from(index.y))),
                        (
                            "type_ids".into(),
                            NeutralValue::List(
                                tiles
                                    .iter()
                                    .map(|t| NeutralValue::Int(i64::from(t.type_id)))
                                    .collect(),
                            ),
                        ),
                        (
                            "flags".into(),
                            NeutralValue::Bytes(tiles.iter().map(|t| t.flags).collect()),
                        ),
                        (
                            "variants".into(),
                            NeutralValue::Bytes(tiles.iter().map(|t| t.variant).collect()),
                        ),
                        (
                            "rotation_mirroring".into(),
                            NeutralValue::Bytes(
                                tiles.iter().map(|t| t.rotation_mirroring).collect(),
                            ),
                        ),
                    ])
                })
                .collect(),
        )
    });
    w.finish()
}
