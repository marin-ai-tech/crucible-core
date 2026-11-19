//! The round spawn policy creates a character for a player in the test lobby. The player
//! becomes a virtual client and gets a new character in the next round after world cleanup.

use std::time::Duration;

use crucible_ecs::{EntityUid, World};
use crucible_ext::{
    Extension, ExtensionPoints, LaunchConfig, Launcher, ManifestSource, SpawnFromPrototype,
};
use crucible_loop::{MapSelector, RoundConfig, RoundPhase, RoundPreparation, SpawnPolicy};
use crucible_map::{MapModule, Vec2, create_grid, create_map, grids_on_map, maps, place};
use crucible_physics::ecs::{PhysicsModule, tile_fields};
use crucible_players::{NetUserId, SessionStatus};
use crucible_replication::ReplicationModule;
use crucible_testkit::TestServer;

const MANIFEST: &str = r#"
[extension]
id = "lobby"
version = "0.1.0"
core = "^0.1"
id_prefix = "Test"

[modules]
require = ["map", "physics", "replication"]

[content]
roots = ["tests/data"]
"#;

/// An empty round map with a single grid.
struct OneGrid;
impl MapSelector for OneGrid {
    fn load_maps(
        &mut self,
        world: &mut World,
        _: &RoundPreparation,
    ) -> Result<Vec<EntityUid>, String> {
        let (map_uid, map_id) = create_map(world);
        create_grid(world, map_id, Vec2::ZERO).ok_or("grid was not created")?;
        Ok(vec![map_uid])
    }
}

/// Spawns prototype characters in a line on the round map's grid.
#[derive(Default)]
struct InARow(f32);
impl SpawnPolicy for InARow {
    fn spawn(&mut self, world: &mut World, player: &NetUserId, role: &str) -> Option<EntityUid> {
        let uid = SpawnFromPrototype {
            prototype: "TestHuman".to_owned(),
        }
        .spawn(world, player, role)?;
        let (map_id, _) = maps(world)[0];
        let grid = grids_on_map(world, map_id)[0];
        self.0 += 1.0;
        place(world, uid, Some(grid), Vec2::new(self.0, 1.0), 0.0).unwrap();
        Some(uid)
    }
}

struct LobbyExtension;
impl Extension for LobbyExtension {
    fn manifest(&self) -> ManifestSource {
        ManifestSource::new(MANIFEST, env!("CARGO_MANIFEST_DIR"))
    }
    fn register(&self, _ctx: &mut crucible_ext::RegisterContext<'_>) {}
    fn configure(&self, points: &mut ExtensionPoints) {
        points
            .set_map_selector(OneGrid)
            .set_spawn_policy(InARow::default());
    }
}

fn server() -> TestServer {
    let launcher = Launcher::new()
        .module(MapModule::new().with_tile_fields(tile_fields()))
        .module(PhysicsModule)
        .module(ReplicationModule)
        .extension(LobbyExtension);
    let mut config = LaunchConfig::for_extension("lobby");
    config.rounds = RoundConfig {
        lobby_duration: Duration::from_secs(600),
        round_duration: None,
        post_round_duration: Duration::from_secs(1),
    };
    TestServer::new(launcher, config).unwrap()
}

fn step_until(server: &mut TestServer, phase: RoundPhase) {
    for _ in 0..300 {
        if server.runtime().rounds().phase() == phase {
            return;
        }
        server.step(1);
    }
    panic!("the round did not reach phase `{phase}`");
}

#[test]
fn lobby_players_become_virtual_clients_when_the_round_spawns_them() {
    let mut server = server();
    let a = server.join_lobby("A").unwrap();
    let b = server.join_lobby("B").unwrap();
    server.step(1);
    for i in [a, b] {
        assert_eq!(
            server.session(i).unwrap().status(),
            SessionStatus::Connected
        );
        assert!(server.try_client(i).is_none(), "no character in the lobby");
    }
    assert_eq!(server.runtime().rounds().phase(), RoundPhase::Lobby);

    server.set_ready(a, true);
    server.set_ready(b, true);
    server.runtime_mut().rounds_mut().request_start();
    server.step(1);
    assert_eq!(server.runtime().rounds().phase(), RoundPhase::InRound);
    let mut first = Vec::new();
    for i in [a, b] {
        let session = server.session(i).unwrap();
        assert_eq!(session.status(), SessionStatus::InGame);
        let client = server.client(i);
        assert_eq!(Some(client.entity), session.entity());
        assert_eq!(client.user, *server.user(i));
        assert_eq!(client.frames.len(), 1, "the spawn tick frame arrived");
        first.push(client.net_entity);
    }
    for i in [a, b] {
        for net in &first {
            assert!(server.client(i).entities.contains_key(net));
        }
    }

    server.runtime_mut().rounds_mut().request_end("end of test");
    step_until(&mut server, RoundPhase::Lobby);
    for i in [a, b] {
        assert_eq!(server.session(i).unwrap().entity(), None);
        assert!(
            server.client(i).entities.is_empty(),
            "entity removals arrived"
        );
    }

    server.runtime_mut().rounds_mut().request_start();
    server.step(1);
    for i in [a, b] {
        let client = server.client(i);
        assert_eq!(Some(client.entity), server.session(i).unwrap().entity());
        assert!(!first.contains(&client.net_entity), "new character");
        assert!(client.entities.contains_key(&client.net_entity));
    }
}
