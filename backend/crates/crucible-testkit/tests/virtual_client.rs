use crucible_ecs::Tick;
use crucible_ext::{Extension, LaunchConfig, Launcher, ManifestSource};
use crucible_loop::Clock;
use crucible_map::{MapModule, Vec2, create_grid, create_map};
use crucible_physics::ecs::{MOVE_RIGHT, MOVE_UP, PhysicsModule, tile_fields};
use crucible_replication::{DEFAULT_FORCE_ACK_THRESHOLD, ReplicationModule};
use crucible_testkit::TestServer;

const PLAYER_PROTOTYPE: &str = "TestHuman";
const MANIFEST: &str = r#"
[extension]
id = "testkit"
version = "0.1.0"
core = "^0.1"
id_prefix = "Test"

[modules]
require = ["map", "physics", "replication"]

[content]
roots = ["tests/data"]
"#;

struct TestExtension;

impl Extension for TestExtension {
    fn manifest(&self) -> ManifestSource {
        ManifestSource::new(MANIFEST, env!("CARGO_MANIFEST_DIR"))
    }

    fn register(&self, _ctx: &mut crucible_ext::RegisterContext<'_>) {}
}

fn server() -> TestServer {
    let launcher = Launcher::new()
        .module(MapModule::new().with_tile_fields(tile_fields()))
        .module(PhysicsModule)
        .module(ReplicationModule)
        .extension(TestExtension);
    TestServer::new(launcher, LaunchConfig::for_extension("testkit")).unwrap()
}

fn two_clients() -> (TestServer, usize, usize) {
    let mut server = server();
    let (_, map_id) = create_map(server.runtime_mut().world_mut());
    let grid = create_grid(server.runtime_mut().world_mut(), map_id, Vec2::ZERO).unwrap();
    let a = server
        .connect("A", PLAYER_PROTOTYPE, grid, Vec2::new(5.5, 3.5))
        .unwrap();
    let b = server
        .connect("B", PLAYER_PROTOTYPE, grid, Vec2::new(5.5, 6.5))
        .unwrap();
    (server, a, b)
}

/// Scenario: a virtual client receives replicated movement.
#[test]
fn virtual_client() {
    let (mut server, a, b) = two_clients();
    let a_net = server.client(a).net_entity;
    let b_net = server.client(b).net_entity;
    server.step(1);
    assert!(server.client(a).entities.contains_key(&b_net));
    assert!(server.client(b).entities.contains_key(&a_net));
    let start_a = server.client(b).entities[&a_net].local_position().unwrap()[0];
    let start_b = server.client(a).entities[&b_net].local_position().unwrap()[1];
    assert_eq!(server.client(a).frames[0].from_tick, Tick::ZERO);
    assert_eq!(server.client(b).frames[0].from_tick, Tick::ZERO);

    server.move_button(a, MOVE_RIGHT, true);
    server.move_button(b, MOVE_UP, true);
    // Include starting positions to catch regressions between startup and the first frame.
    let mut a_positions = vec![start_a];
    let mut b_positions = vec![start_b];
    for _ in 0..30 {
        server.step(1);
        a_positions.push(server.client(b).entities[&a_net].local_position().unwrap()[0]);
        b_positions.push(server.client(a).entities[&b_net].local_position().unwrap()[1]);
    }
    assert!(
        a_positions.windows(2).all(|p| p[1] >= p[0])
            && a_positions.last().unwrap() - start_a >= 1.0,
        "B should see A move right: {a_positions:?}"
    );
    assert!(
        b_positions.windows(2).all(|p| p[1] >= p[0])
            && b_positions.last().unwrap() - start_b >= 1.0,
        "A should see B move up: {b_positions:?}"
    );
    assert!(
        server
            .client(a)
            .frames
            .iter()
            .skip(1)
            .all(|f| f.from_tick > Tick::ZERO)
    );
    assert!(
        server
            .client(b)
            .frames
            .iter()
            .skip(1)
            .all(|f| f.from_tick > Tick::ZERO)
    );
}

#[test]
fn lost_acknowledgements_force_reliable_delivery() {
    let (mut server, a, _) = two_clients();
    server.step(1);
    server.set_ack_delay(a, None);
    server.step(DEFAULT_FORCE_ACK_THRESHOLD + 1);
    let reliable = server.client(a).frames.last().unwrap();
    assert!(reliable.reliable);
    assert_eq!(reliable.from_tick, Tick(1));
    server.set_ack_delay(a, Some(0));
    server.step(1);
    server.step(1);
    let normal = server.client(a).frames.last().unwrap();
    assert!(!normal.reliable);
    assert!(normal.from_tick > Tick(1));
}

#[test]
fn full_state_request_restores_client_entities() {
    let (mut server, a, b) = two_clients();
    server.step(2);
    let b_net = server.client(b).net_entity;
    assert!(server.client(a).entities.contains_key(&b_net));
    assert!(server.request_full_state(a));
    assert!(server.client(a).entities.is_empty());
    server.step(1);
    let client = server.client(a);
    assert_eq!(client.frames.last().unwrap().from_tick, Tick::ZERO);
    assert!(client.frames.last().unwrap().reliable);
    assert!(client.entities[&b_net].local_position().is_some());
}

#[test]
fn manual_clock_runs_three_hundred_ticks_without_waiting() {
    let mut server = server();
    let start = std::time::Instant::now();
    server.step(300);
    assert!(start.elapsed() < std::time::Duration::from_secs(2));
    assert_eq!(server.tick(), Tick(300));
    assert!(server.clock().now() >= std::time::Duration::from_secs(9));
}
