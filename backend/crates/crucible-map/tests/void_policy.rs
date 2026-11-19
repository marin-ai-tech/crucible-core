//! Assembled-server scenario from `specs/core/world-map` for extension-defined void behavior.
//! The map module forwards the core [`OverVoid`] event to the void policy registered by the
//! active extension ([`VoidTilePolicy`]).

use crucible_ecs::{Component, EntityUid, World};
use crucible_ext::{
    ComponentSpec, ExtensionPoints, LaunchConfig, Launcher, ServerRuntime, VoidTilePolicy,
};
use crucible_map::{
    MODULE_ID, MapModule, Tile, Vec2, Vector2i, create_grid, create_map, set_local_position,
    set_tile, spawn_at,
};
use crucible_testkit::fixtures::{FixtureExtension, fixtures_dir, manifest};

/// Marks an entity that has been over the void.
#[derive(Default)]
struct Marked;
impl Component for Marked {}

/// First extension policy marks entities over the void and leaves them in place.
struct MarkOverVoid;
impl VoidTilePolicy for MarkOverVoid {
    fn over_void(&self, world: &mut World, entity: EntityUid) {
        world.add_component(entity, Marked).unwrap();
    }
}

/// Second extension policy removes entities over the void at the end of the tick.
struct RemoveOverVoid;
impl VoidTilePolicy for RemoveOverVoid {
    fn over_void(&self, world: &mut World, entity: EntityUid) {
        world.queue_delete(entity);
    }
}

fn launch(
    id: &str,
    configure: impl Fn(&mut ExtensionPoints) + Send + Sync + 'static,
) -> ServerRuntime {
    let ext = FixtureExtension::new(&manifest(id, "^0.1", &[MODULE_ID]), fixtures_dir())
        .on_register(|ctx| {
            ctx.component(ComponentSpec::<Marked>::new("Marked"));
        })
        .on_configure(configure);
    Launcher::new()
        .module(MapModule::new())
        .extension(ext)
        .launch(LaunchConfig::for_extension(id))
        .unwrap_or_else(|e| panic!("{e}"))
}

/// Clears the tile under one entity and moves another off the grid during a server tick.
fn scenario(server: &mut ServerRuntime) -> (EntityUid, EntityUid) {
    let world = server.world_mut();
    let (_, map_id) = create_map(world);
    let grid = create_grid(world, map_id, Vec2::ZERO).unwrap();
    for x in 0..4 {
        set_tile(world, grid, Vector2i::new(x, 0), Tile::new(1));
    }
    let stays = spawn_at(world, grid, Vec2::new(0.5, 0.5)).unwrap();
    let walker = spawn_at(world, grid, Vec2::new(2.5, 0.5)).unwrap();
    set_tile(world, grid, Vector2i::new(0, 0), Tile::EMPTY);
    set_local_position(world, walker, Vec2::new(9.5, 0.5)).unwrap();
    server.tick();
    (stays, walker)
}

/// Scenario where the extension's policy determines the effect of the void.
#[test]
fn void_meaning_comes_from_the_active_extension_point() {
    let mut marking = launch("voidmark", |p| {
        p.set_void_tile(MarkOverVoid);
    });
    let mut removing = launch("voidremove", |p| {
        p.set_void_tile(RemoveOverVoid);
    });
    let mut default = launch("voiddefault", |_| {});

    let (m_stays, m_walker) = scenario(&mut marking);
    let (r_stays, r_walker) = scenario(&mut removing);
    let (d_stays, d_walker) = scenario(&mut default);

    let w = marking.world();
    assert!(w.has::<Marked>(m_stays) && w.has::<Marked>(m_walker));
    let w = removing.world();
    assert!(!w.exists(r_stays) && !w.exists(r_walker));
    // The default void policy leaves entities alone.
    let w = default.world();
    assert!(w.exists(d_stays) && w.exists(d_walker));
    assert!(!w.has::<Marked>(d_stays) && !w.has::<Marked>(d_walker));
}
