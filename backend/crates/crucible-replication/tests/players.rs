//! Replication recipients from player sessions (`crucible-players`).

use std::time::Duration;

use crucible_ecs::World;
use crucible_players::{
    ConnectionId, DisconnectReason, NetUserId, PlayerManager, Players, PlayersSystem,
};
use crucible_replication::{ReplicatedComponents, Replication, ReplicationConfig};

#[test]
fn viewers_follow_player_sessions() {
    let mut builder = World::builder();
    builder.add_system(PlayersSystem);
    let (mut world, _systems) = builder.build().unwrap();
    let mut replication =
        Replication::new(ReplicationConfig::default(), ReplicatedComponents::new());
    let alice = NetUserId::new("alice");
    let body = world.spawn();

    PlayerManager::begin_connect(&mut world, &alice, "Alice", ConnectionId(1), Duration::ZERO);
    replication.sync_with_players(world.resource::<Players>().unwrap());
    assert!(
        !replication.has_viewer(&alice),
        "handshake is still in progress"
    );

    PlayerManager::finish_connect(&mut world, &alice).unwrap();
    replication.sync_with_players(world.resource::<Players>().unwrap());
    assert!(
        replication.has_viewer(&alice),
        "lobby sessions receive states"
    );
    assert_eq!(replication.controlled_entity(&alice), None);

    PlayerManager::enter_game(&mut world, &alice).unwrap();
    PlayerManager::attach(&mut world, &alice, body).unwrap();
    replication.sync_with_players(world.resource::<Players>().unwrap());
    assert_eq!(replication.controlled_entity(&alice), Some(body));

    PlayerManager::disconnect(
        &mut world,
        &alice,
        ConnectionId(1),
        DisconnectReason::ConnectionLost,
        Duration::from_secs(1),
    );
    replication.sync_with_players(world.resource::<Players>().unwrap());
    assert!(!replication.has_viewer(&alice));
}
