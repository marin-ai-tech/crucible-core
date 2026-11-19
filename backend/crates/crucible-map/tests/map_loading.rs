//! Test map loading and error handling for the SS14 map format.

use std::path::PathBuf;

use crucible_ecs::{Component, EntityUid, World};
use crucible_map::{
    ComponentSpawner, FileEntities, Map, MapGrid, MapLoader, Tile, TileDefinitions, Transform,
    Vec2, Vector2i, anchored_entities, get_tile, register, register_tile_kind,
};
use crucible_proto::{
    ComponentDef, DataRoot, FieldDef, FieldType, IgnoreCategory, KindDef, Node, PrototypeLoader,
    PrototypeSet, Registry,
};

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Entity prototypes used by `fixtures/room.yml`.
const ENTITY_FIXTURE: &str = r#"
- type: entity
  id: TestWall
  components:
  - type: Transform
    anchored: true
  - type: Body
- type: entity
  id: TestHuman
  components:
  - type: Body
- type: entity
  id: TestCrate
  components:
  - type: Body
  - type: Sprite
- type: entity
  id: TestLamp
  components:
  - type: TestLight
    color: '#FFFFFFFF'
    radius: 5
"#;

fn registry() -> Registry {
    let mut r = Registry::new();
    register_tile_kind(&mut r, Vec::new()).unwrap();
    r.register_kind(KindDef::with_components(
        "entity",
        vec![FieldDef::new("name", FieldType::String)],
    ))
    .unwrap();
    r.register_component(ComponentDef {
        name: "Transform".into(),
        fields: vec![FieldDef::new("anchored", FieldType::Bool)],
    })
    .unwrap();
    r.register_component(ComponentDef {
        name: "Body".into(),
        fields: vec![],
    })
    .unwrap();
    r.register_component(ComponentDef {
        name: "TestLight".into(),
        fields: vec![
            FieldDef::new("color", FieldType::String),
            FieldDef::new("radius", FieldType::Float),
        ],
    })
    .unwrap();
    r.ignore_component("Sprite", IgnoreCategory::ClientOnly)
        .unwrap();
    r
}

fn prototypes(registry: &Registry) -> PrototypeSet {
    let mut loader = PrototypeLoader::new(registry);
    let root = loader.add_root(DataRoot::new("map-fixture"));
    let tiles = std::fs::read_to_string(fixture_dir().join("tiles.yml")).unwrap();
    loader.add_source(root, "fixtures/tiles.yml", &tiles);
    loader.add_source(root, "fixtures/entities.yml", ENTITY_FIXTURE);
    let (set, _) = loader
        .finish(None)
        .into_result()
        .unwrap_or_else(|r| panic!("fixture prototypes: {r}"));
    set
}

#[derive(Debug, PartialEq)]
struct Light {
    color: String,
    radius: String,
}
impl Component for Light {}

#[derive(Default)]
struct Body;
impl Component for Body {}

/// Fixture component spawner supporting `Body` and `TestLight`.
#[derive(Default)]
struct FixtureComponents {
    names: Vec<(EntityUid, String)>,
}

impl ComponentSpawner for FixtureComponents {
    fn add_component(
        &mut self,
        world: &mut World,
        uid: EntityUid,
        name: &str,
        data: &Node,
        _entities: &FileEntities,
    ) -> Result<bool, String> {
        self.names.push((uid, name.to_owned()));
        let field = |f: &str| data.get(f).and_then(Node::as_str).unwrap_or("").to_owned();
        match name {
            "Body" => world
                .add_component(uid, Body)
                .map_err(|e| format!("{e:?}"))?,
            "TestLight" => world
                .add_component(
                    uid,
                    Light {
                        color: field("color"),
                        radius: field("radius"),
                    },
                )
                .map_err(|e| format!("{e:?}"))?,
            _ => return Ok(false),
        }
        Ok(true)
    }
}

fn world() -> World {
    let mut builder = World::builder();
    register(&mut builder).unwrap();
    builder.build().unwrap().0
}

/// Scenario for loading the test map.
#[test]
fn loading_the_test_map() {
    let registry = registry();
    let set = prototypes(&registry);
    let tiles = TileDefinitions::from_prototypes(&set).unwrap();
    let mut world = world();
    let mut components = FixtureComponents::default();
    let loader = MapLoader::new(&tiles)
        .with_prototypes(&set)
        .with_registry(&registry);
    let loaded = loader
        .load_file(&mut world, &mut components, &fixture_dir().join("room.yml"))
        .unwrap_or_else(|e| panic!("{e}"));

    // The map and the grid.
    assert_eq!(
        world.get::<Map>(loaded.map_uid).unwrap().map_id(),
        loaded.map_id
    );
    assert_eq!(loaded.grids.len(), 1);
    let grid = loaded.grids[0];
    assert_eq!(
        world.get::<Transform>(grid).unwrap().parent(),
        Some(loaded.map_uid)
    );

    // The 12×9 room has plating under the walls and flooring inside.
    let floor = Tile::new(tiles.id("TestFloor").unwrap());
    let plating = Tile::new(tiles.id("TestPlating").unwrap());
    let g = world.get::<MapGrid>(grid).unwrap();
    assert_eq!(g.tiles().count(), 12 * 9);
    assert_eq!(get_tile(&world, grid, Vector2i::new(0, 0)), Some(plating));
    assert_eq!(get_tile(&world, grid, Vector2i::new(11, 8)), Some(plating));
    assert_eq!(get_tile(&world, grid, Vector2i::new(5, 4)), Some(floor));
    assert_eq!(
        get_tile(&world, grid, Vector2i::new(12, 0)),
        Some(Tile::EMPTY)
    );

    // All 50 entities use the positions from the file.
    let things: Vec<EntityUid> = loaded
        .entities
        .iter()
        .map(|(_, uid)| uid)
        .filter(|&uid| !world.has::<Map>(uid) && !world.has::<MapGrid>(uid))
        .collect();
    assert_eq!(things.len(), 50);
    assert_eq!(loaded.entities.len(), 52);
    let proto = |uid: EntityUid| {
        world
            .entity_meta(uid)
            .unwrap()
            .prototype()
            .map(str::to_owned)
    };
    let count = |id: &str| {
        things
            .iter()
            .filter(|&&u| proto(u).as_deref() == Some(id))
            .count()
    };
    assert_eq!(
        (
            count("TestWall"),
            count("TestHuman"),
            count("TestCrate"),
            count("TestLamp")
        ),
        (38, 1, 5, 6)
    );
    for &uid in &things {
        let t = world.get::<Transform>(uid).unwrap();
        assert_eq!(t.parent(), Some(grid));
        assert_eq!(t.map_id(), loaded.map_id);
        assert!(world.life_stage(uid) >= Some(crucible_ecs::EntityLifeStage::Initialized));
    }
    let human = loaded.entities.get(41).unwrap();
    assert_eq!(proto(human).as_deref(), Some("TestHuman"));
    assert!(
        world
            .get::<Transform>(human)
            .unwrap()
            .world_position()
            .approx_eq(Vec2::new(5.5, 4.5), 1e-6)
    );
    let lamp = loaded.entities.get(47).unwrap();
    let lt = world.get::<Transform>(lamp).unwrap();
    assert!(lt.world_position().approx_eq(Vec2::new(1.5, 1.5), 1e-6));
    assert!((lt.local_rotation() - std::f64::consts::FRAC_PI_2).abs() < 1e-12);

    // File fields override prototype fields, ignored components are skipped, and walls are anchored.
    assert_eq!(
        world.get::<Light>(lamp).unwrap(),
        &Light {
            color: "#FFAA00FF".into(),
            radius: "5".into()
        }
    );
    let other_lamp = loaded.entities.get(48).unwrap();
    assert_eq!(world.get::<Light>(other_lamp).unwrap().color, "#FFFFFFFF");
    assert!(components.names.iter().all(|(_, n)| n != "Sprite"));
    assert_eq!(world.query::<(Body,)>().len(), 38 + 1 + 5);
    let corner_wall = loaded.entities.get(3).unwrap();
    assert!(world.get::<Transform>(corner_wall).unwrap().anchored());
    assert_eq!(
        anchored_entities(&world, grid, Vector2i::new(0, 0)),
        vec![corner_wall]
    );
    assert!(!world.get::<Transform>(human).unwrap().anchored());
}

#[test]
fn errors_name_file_and_entity_and_roll_back() {
    let registry = registry();
    let set = prototypes(&registry);
    let tiles = TileDefinitions::from_prototypes(&set).unwrap();
    let mut world = world();
    let before = world.entity_count();
    let text = r#"
meta:
  format: 7
tilemap:
  0: TestEmpty
entities:
- proto: ""
  entities:
  - uid: 1
    components:
    - type: Transform
    - type: Map
- proto: TestHuman
  entities:
  - uid: 2
    components:
    - type: Transform
      parent: 1
      pos: 1,2
    - type: Unheard
"#;
    let err = MapLoader::new(&tiles)
        .with_prototypes(&set)
        .with_registry(&registry)
        .load_str(
            &mut world,
            &mut FixtureComponents::default(),
            "maps/broken.yml",
            text,
        )
        .unwrap_err();
    let shown = err.to_string();
    assert!(
        shown.contains(
            "maps/broken.yml:20: entity uid 2 (prototype `TestHuman`): unknown component `Unheard`"
        ),
        "{shown}"
    );
    assert_eq!(
        world.entity_count(),
        before,
        "the entire load was rolled back"
    );
    assert!(crucible_map::maps(&world).is_empty());

    let missing = text.replace("TestHuman", "NoSuchThing");
    let err = MapLoader::new(&tiles)
        .with_prototypes(&set)
        .load_str(
            &mut world,
            &mut FixtureComponents::default(),
            "maps/broken.yml",
            &missing,
        )
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("entity prototype `NoSuchThing` not found"),
        "{err}"
    );

    let bad_tile = text.replace("0: TestEmpty", "0: TestEmpty\n  5: Unknown");
    let err = MapLoader::new(&tiles)
        .with_prototypes(&set)
        .load_str(
            &mut world,
            &mut FixtureComponents::default(),
            "maps/broken.yml",
            &bad_tile,
        )
        .unwrap_err();
    assert!(
        err.to_string().contains("unknown tile prototype `Unknown`"),
        "{err}"
    );
    assert_eq!(world.entity_count(), before);
}

#[test]
fn grid_file_without_map_gets_a_new_map_and_chunk_version_6() {
    let registry = registry();
    let set = prototypes(&registry);
    let tiles = TileDefinitions::from_prototypes(&set).unwrap();
    let mut world = world();
    // Version 6 chunk of size 2×2 (6 bytes per tile). File tile 1 at (0, 0), empty elsewhere.
    let mut bytes = Vec::new();
    for slot in 0..4 {
        let id: i32 = if slot == 1 { 1 } else { 0 };
        bytes.extend_from_slice(&id.to_le_bytes());
        bytes.extend_from_slice(&[0, 2]);
    }
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    let text = format!(
        "meta:\n  format: 6\ntilemap:\n  0: TestEmpty\n  1: TestFloor\nentities:\n- proto: \"\"\n  entities:\n  - uid: 1\n    components:\n    - type: Transform\n      pos: 0.5,0.5\n      parent: invalid\n    - type: MapGrid\n      chunkSize: 2\n      chunks:\n        -1,0:\n          ind: -1,0\n          tiles: {b64}\n          version: 6\n"
    );
    let loaded = MapLoader::new(&tiles)
        .load_str(
            &mut world,
            &mut FixtureComponents::default(),
            "grid.yml",
            &text,
        )
        .unwrap_or_else(|e| panic!("{e}"));
    let grid = loaded.grids[0];
    assert_eq!(
        world.get::<Transform>(grid).unwrap().parent(),
        Some(loaded.map_uid)
    );
    // Slot 1 of the size-2 chunk (−1, 0) is tile (−1, 0).
    let t = get_tile(&world, grid, Vector2i::new(-1, 0)).unwrap();
    assert_eq!(t.type_id, tiles.id("TestFloor").unwrap());
    assert_eq!(t.variant, 2);
    assert_eq!(world.get::<MapGrid>(grid).unwrap().tiles().count(), 1);
}
