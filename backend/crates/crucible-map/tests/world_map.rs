//! Scenarios for tiles, void behavior, transforms, anchoring, and spatial queries.

use crucible_ecs::{Component, EntityUid, System, SystemInit, World};
use crucible_map::{
    Box2, MapGrid, OverVoid, Tile, TileChanged, TileDefinitions, Transform, Vec2, Vector2i, anchor,
    anchored_entities, create_grid, create_map, entities_in_box, entities_in_radius,
    entities_on_tile, get_tile, register, register_tile_kind, set_local_position, set_parent,
    set_tile, spawn_at, world_position,
};
use crucible_proto::{DataRoot, PrototypeLoader, Registry};

fn world_with(systems: impl FnOnce(&mut crucible_ecs::WorldBuilder)) -> World {
    let mut builder = World::builder();
    register(&mut builder).expect("register the map model");
    systems(&mut builder);
    builder.build().expect("build the world").0
}

fn world() -> World {
    world_with(|_| {})
}

#[test]
fn same_parent_teleport_changes_the_discontinuity_counter() {
    let mut world = world();
    let (map, _) = create_map(&mut world);
    let actor = spawn_at(&mut world, map, Vec2::new(1.5, 1.5)).unwrap();
    set_local_position(&mut world, actor, Vec2::new(2.0, 1.5)).unwrap();
    assert_eq!(world.get::<Transform>(actor).unwrap().teleport_count(), 0);
    let target = Vec2::new(100.0, 70.0);
    crucible_map::teleport_world_position(&mut world, actor, target).unwrap();
    let transform = world.get::<Transform>(actor).unwrap();
    assert_eq!(transform.teleport_count(), 1);
    assert_eq!(transform.parent(), Some(map));
    assert_eq!(transform.world_position(), target);
}

const EPS: f32 = 1e-5;

/// Scenario for setting a tile.
#[test]
fn setting_a_tile() {
    let mut world = world();
    let (_, map_id) = create_map(&mut world);
    let grid = create_grid(&mut world, map_id, Vec2::ZERO).unwrap();
    let tile = Tile {
        type_id: 7,
        flags: 0b101,
        variant: 3,
        rotation_mirroring: 6,
    };
    let at = Vector2i::new(17, -3);
    assert_eq!(set_tile(&mut world, grid, at, tile), Some(Tile::EMPTY));

    let g = world.get::<MapGrid>(grid).unwrap();
    // (17, −3) is in chunk (1, −1) at local coordinates (1, 13).
    assert_eq!(g.chunk_of(at), (Vector2i::new(1, -1), Vector2i::new(1, 13)));
    let chunk = g.chunk(Vector2i::new(1, -1)).expect("created chunk");
    assert_eq!(chunk.tile(Vector2i::new(1, 13)), tile);
    assert_eq!(chunk.filled(), 1);
    assert_eq!(g.chunks().count(), 1);
    assert_eq!(get_tile(&world, grid, at), Some(tile));
    assert_eq!(
        get_tile(&world, grid, Vector2i::new(16, -3)),
        Some(Tile::EMPTY)
    );
    assert_eq!(g.tiles().collect::<Vec<_>>(), vec![(at, tile)]);
}

/// Scenario for deterministic tile IDs.
#[test]
fn deterministic_tile_ids() {
    let first = "- type: tile\n  id: Gamma\n- type: tile\n  id: Alpha\n";
    let second = "- type: tile\n  id: Nothing\n  empty: true\n- type: tile\n  id: Beta\n";
    let load = |files: &[(&str, &str)]| {
        let mut registry = Registry::new();
        register_tile_kind(&mut registry, Vec::new()).unwrap();
        let mut loader = PrototypeLoader::new(&registry);
        let root = loader.add_root(DataRoot::new("core"));
        for (name, text) in files {
            loader.add_source(root, name, text);
        }
        let (set, _) = loader
            .finish(None)
            .into_result()
            .expect("valid tile definitions");
        TileDefinitions::from_prototypes(&set).unwrap()
    };
    let run1 = load(&[("a.yml", first), ("b.yml", second)]);
    let run2 = load(&[("b.yml", second), ("a.yml", first)]);
    assert_eq!(run1, run2);
    let ids: Vec<(i32, &str)> = run1.iter().collect();
    assert_eq!(
        ids,
        vec![(0, "Nothing"), (1, "Alpha"), (2, "Beta"), (3, "Gamma")]
    );
    assert_eq!(run1.id("Nothing"), Some(0));
    assert_eq!(run1.prototype(3), Some("Gamma"));
    assert_eq!(run1.prototype(4), None);
}

// Fixture extensions with different void policies.

/// Events observed by the extension, stored as a world resource.
#[derive(Default)]
struct SeenOverVoid(Vec<OverVoid>);

/// Component that the first extension adds to the entity.
#[derive(Default)]
struct Marked;
impl Component for Marked {}

/// First extension marks entities over the void and leaves them in place.
struct MarkingExtension;
impl System for MarkingExtension {
    fn initialize(&mut self, init: &mut SystemInit<'_>) {
        init.insert_resource(SeenOverVoid::default());
        init.subscribe::<Transform, OverVoid, _>(|world, uid, ev| {
            world.resource_mut::<SeenOverVoid>().unwrap().0.push(*ev);
            world.add_component(uid, Marked).unwrap();
        });
    }
}

/// Second extension removes entities over the void at the end of the tick.
struct RemovingExtension;
impl System for RemovingExtension {
    fn initialize(&mut self, init: &mut SystemInit<'_>) {
        init.insert_resource(SeenOverVoid::default());
        init.subscribe::<Transform, OverVoid, _>(|world, uid, ev| {
            world.resource_mut::<SeenOverVoid>().unwrap().0.push(*ev);
            world.queue_delete(uid);
        });
    }
}

/// Runs the same scenario for both extensions. Clear the tile under one entity,
/// then move another entity off the grid.
fn void_scenario(world: &mut World) -> (EntityUid, EntityUid) {
    let (map_uid, map_id) = create_map(world);
    let grid = create_grid(world, map_id, Vec2::ZERO).unwrap();
    for x in 0..4 {
        set_tile(world, grid, Vector2i::new(x, 0), Tile::new(1));
    }
    let stays = spawn_at(world, grid, Vec2::new(0.5, 0.5)).unwrap();
    let walker = spawn_at(world, grid, Vec2::new(2.5, 0.5)).unwrap();
    assert!(world.resource::<SeenOverVoid>().unwrap().0.is_empty());

    set_tile(world, grid, Vector2i::new(0, 0), Tile::EMPTY);
    set_local_position(world, walker, Vec2::new(9.5, 0.5)).unwrap();
    // Moving while already over the void creates no new transition.
    set_local_position(world, walker, Vec2::new(10.5, 0.5)).unwrap();
    let _ = map_uid;
    (stays, walker)
}

/// Scenario where the extension defines the effect of the void.
#[test]
fn void_meaning_is_set_by_extension() {
    let mut marking = world_with(|b| {
        b.add_system(MarkingExtension);
    });
    let mut removing = world_with(|b| {
        b.add_system(RemovingExtension);
    });
    let (m_stays, m_walker) = void_scenario(&mut marking);
    let (r_stays, r_walker) = void_scenario(&mut removing);

    // The core sends the same event to both extensions.
    let seen_m = &marking.resource::<SeenOverVoid>().unwrap().0;
    let seen_r = &removing.resource::<SeenOverVoid>().unwrap().0;
    assert_eq!(seen_m.len(), 2, "{seen_m:?}");
    assert_eq!(seen_m, seen_r);
    assert_eq!(seen_m[0].entity, m_stays);
    assert!(seen_m[0].position.approx_eq(Vec2::new(0.5, 0.5), EPS));
    assert_eq!(seen_m[1].entity, m_walker);
    assert!(seen_m[1].grid.is_some());

    // The consequences depend on each extension's policy.
    marking.end_tick();
    removing.end_tick();
    assert!(marking.has::<Marked>(m_stays) && marking.has::<Marked>(m_walker));
    assert!(!removing.exists(r_stays) && !removing.exists(r_walker));
}

#[test]
fn entity_over_empty_map_area_is_over_void_and_grid_tile_is_not() {
    let mut world = world_with(|b| {
        b.add_system(MarkingExtension);
    });
    let (map_uid, map_id) = create_map(&mut world);
    let grid = create_grid(&mut world, map_id, Vec2::new(100.0, 0.0)).unwrap();
    set_tile(&mut world, grid, Vector2i::new(0, 0), Tile::new(1));
    // An entity on the map above a grid tile is not over the void.
    let on_tile = spawn_at(&mut world, map_uid, Vec2::new(100.5, 0.5)).unwrap();
    // An entity on the map away from grids is over the void.
    let in_void = spawn_at(&mut world, map_uid, Vec2::new(0.0, 0.0)).unwrap();
    let seen: Vec<EntityUid> = world
        .resource::<SeenOverVoid>()
        .unwrap()
        .0
        .iter()
        .map(|e| e.entity)
        .collect();
    assert_eq!(seen, vec![in_void]);
    assert!(!world.has::<Marked>(on_tile));
}

/// Scenario for changing a parent.
#[test]
fn reparenting() {
    let mut world = world();
    let (map_uid, map_id) = create_map(&mut world);
    let grid = create_grid(&mut world, map_id, Vec2::new(3.0, 2.0)).unwrap();
    let thing = spawn_at(&mut world, map_uid, Vec2::new(10.0, 5.0)).unwrap();
    assert_eq!(world_position(&world, thing), Some(Vec2::new(10.0, 5.0)));

    set_parent(&mut world, thing, grid).unwrap();

    let t = world.get::<Transform>(thing).unwrap();
    assert_eq!(t.parent(), Some(grid));
    assert!(t.world_position().approx_eq(Vec2::new(10.0, 5.0), EPS));
    assert!(t.local_position().approx_eq(Vec2::new(7.0, 3.0), EPS));
    assert_eq!(t.grid_uid(), Some(grid));
    assert_eq!(t.map_id(), map_id);
    assert!(
        world
            .get::<Transform>(grid)
            .unwrap()
            .children()
            .contains(&thing)
    );
    assert!(
        !world
            .get::<Transform>(map_uid)
            .unwrap()
            .children()
            .contains(&thing)
    );
}

#[test]
fn world_values_follow_rotated_hierarchy_and_cycles_are_rejected() {
    let mut world = world();
    let (map_uid, map_id) = create_map(&mut world);
    let grid = create_grid(&mut world, map_id, Vec2::new(1.0, 1.0)).unwrap();
    crucible_map::set_local_rotation(&mut world, grid, std::f64::consts::FRAC_PI_2).unwrap();
    let a = spawn_at(&mut world, grid, Vec2::new(2.0, 0.0)).unwrap();
    let b = spawn_at(&mut world, a, Vec2::new(1.0, 0.0)).unwrap();
    // The grid rotates (2, 0) by 90° to (0, 2), then translates it by (1, 1).
    assert!(
        world_position(&world, a)
            .unwrap()
            .approx_eq(Vec2::new(1.0, 3.0), EPS)
    );
    assert!(
        world_position(&world, b)
            .unwrap()
            .approx_eq(Vec2::new(1.0, 4.0), EPS)
    );
    // Moving the grid also moves its descendants.
    crucible_map::set_local_position(&mut world, grid, Vec2::new(0.0, 0.0)).unwrap();
    assert!(
        world_position(&world, b)
            .unwrap()
            .approx_eq(Vec2::new(0.0, 3.0), EPS)
    );
    // A grid cannot be parented to one of its descendants.
    assert!(matches!(
        set_parent(&mut world, grid, b),
        Err(crucible_map::TransformError::Cycle { .. })
    ));
    let _ = map_uid;
}

/// Records all broadcast `TileChanged` events.
#[derive(Default)]
struct TileLog(Vec<TileChanged>);
struct TileLogger;
impl System for TileLogger {
    fn initialize(&mut self, init: &mut SystemInit<'_>) {
        init.insert_resource(TileLog::default());
        init.subscribe_broadcast::<TileChanged, _>(|world, ev| {
            world.resource_mut::<TileLog>().unwrap().0.push(*ev);
        });
    }
}

/// Scenario for querying entities on a tile.
#[test]
fn entities_on_a_tile() {
    let mut world = world_with(|b| {
        b.add_system(TileLogger);
    });
    let (_, map_id) = create_map(&mut world);
    let grid = create_grid(&mut world, map_id, Vec2::ZERO).unwrap();
    let at = Vector2i::new(2, 3);
    set_tile(&mut world, grid, at, Tile::new(1));
    world.resource_mut::<TileLog>().unwrap().0.clear();

    let a = spawn_at(&mut world, grid, Vec2::new(2.2, 3.7)).unwrap();
    let b = spawn_at(&mut world, grid, Vec2::new(2.9, 3.1)).unwrap();
    assert_eq!(anchor(&mut world, a).unwrap(), at);
    assert_eq!(anchor(&mut world, b).unwrap(), at);
    assert!(world.get::<Transform>(a).unwrap().anchored());
    assert!(
        world_position(&world, a)
            .unwrap()
            .approx_eq(Vec2::new(2.5, 3.5), EPS)
    );

    let replaced = Tile::new(2);
    set_tile(&mut world, grid, at, replaced);

    assert_eq!(anchored_entities(&world, grid, at), vec![a, b]);
    assert_eq!(entities_on_tile(&world, grid, at), vec![a, b]);
    let log = &world.resource::<TileLog>().unwrap().0;
    assert_eq!(
        log,
        &vec![TileChanged {
            grid,
            indices: at,
            old: Tile::new(1),
            new: replaced,
        }]
    );

    // Leaving the tile clears the anchor.
    set_local_position(&mut world, b, Vec2::new(5.5, 3.5)).unwrap();
    assert_eq!(anchored_entities(&world, grid, at), vec![a]);
    assert!(!world.get::<Transform>(b).unwrap().anchored());
}

/// Scenario for searching within a radius.
#[test]
fn radius_search() {
    let mut world = world();
    let (map_uid, map_id) = create_map(&mut world);
    let (other_uid, _) = create_map(&mut world);
    let center = Vec2::new(40.0, -12.0);
    let near = [
        spawn_at(&mut world, map_uid, center + Vec2::new(4.9, 0.0)).unwrap(),
        spawn_at(&mut world, map_uid, center + Vec2::new(-3.0, -3.0)).unwrap(),
        spawn_at(&mut world, map_uid, center + Vec2::new(0.0, 5.0)).unwrap(),
    ];
    let far = spawn_at(&mut world, map_uid, center + Vec2::new(6.0, 0.0)).unwrap();
    // The same location on another map is outside the result.
    spawn_at(&mut world, other_uid, center).unwrap();

    let mut expected = near.to_vec();
    expected.sort();
    assert_eq!(entities_in_radius(&world, map_id, center, 5.0), expected);
    assert!(!entities_in_radius(&world, map_id, center, 5.0).contains(&far));
    assert_eq!(entities_in_radius(&world, map_id, center, 6.0).len(), 4);

    // The query omits deleted entities and entities being deleted.
    world.queue_delete(near[0]);
    assert_eq!(entities_in_radius(&world, map_id, center, 5.0).len(), 2);
    world.end_tick();
    world.delete(near[1]);
    assert_eq!(
        entities_in_radius(&world, map_id, center, 5.0),
        vec![near[2]]
    );
}

#[test]
fn box_and_tile_search_cover_grids_and_map() {
    let mut world = world();
    let (map_uid, map_id) = create_map(&mut world);
    let grid = create_grid(&mut world, map_id, Vec2::new(32.0, 32.0)).unwrap();
    let on_grid = spawn_at(&mut world, grid, Vec2::new(0.5, 0.5)).unwrap();
    let on_map = spawn_at(&mut world, map_uid, Vec2::new(32.7, 32.2)).unwrap();
    let elsewhere = spawn_at(&mut world, map_uid, Vec2::new(-40.0, 0.0)).unwrap();

    let found = entities_in_box(
        &world,
        map_id,
        Box2::new(Vec2::new(31.0, 31.0), Vec2::new(34.0, 34.0)),
    );
    let mut expected = vec![grid, on_grid, on_map];
    expected.sort();
    assert_eq!(found, expected);
    assert!(!found.contains(&elsewhere));

    let mut on_tile = vec![on_grid, on_map];
    on_tile.sort();
    assert_eq!(entities_on_tile(&world, grid, Vector2i::new(0, 0)), on_tile);
}

#[test]
fn deleting_a_parent_deletes_children_and_cleans_index() {
    let mut world = world();
    let (map_uid, map_id) = create_map(&mut world);
    let grid = create_grid(&mut world, map_id, Vec2::ZERO).unwrap();
    let box_ = spawn_at(&mut world, grid, Vec2::new(1.5, 1.5)).unwrap();
    let inside = spawn_at(&mut world, box_, Vec2::ZERO).unwrap();
    anchor(&mut world, box_).unwrap();

    world.delete(grid);
    assert!(!world.exists(box_) && !world.exists(inside));
    assert!(crucible_map::grids_on_map(&world, map_id).is_empty());
    assert!(entities_in_radius(&world, map_id, Vec2::ZERO, 100.0).is_empty());
    assert!(
        world
            .get::<Transform>(map_uid)
            .unwrap()
            .children()
            .is_empty()
    );

    world.delete(map_uid);
    assert_eq!(crucible_map::map_entity(&world, map_id), None);
}
