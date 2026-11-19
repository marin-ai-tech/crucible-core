//! Session, entity control, and reconnection scenarios.

mod common;

use common::{Logged, join, secs, take, user, world_with_log};
use crucible_players::{
    ConnectionId, DetachReason, DisconnectReason, PlayerControlled, PlayerDisconnected,
    PlayerError, PlayerManager, Players, SessionStatus,
};

fn status_of(world: &crucible_ecs::World, u: &crucible_players::NetUserId) -> SessionStatus {
    world
        .resource::<Players>()
        .unwrap()
        .session(u)
        .unwrap()
        .status()
}

/// Scenario: session status changes.
#[test]
fn status_changes() {
    let (mut world, log) = world_with_log();
    let alice = user("alice");

    PlayerManager::begin_connect(&mut world, &alice, "Alice", ConnectionId(1), secs(0));
    PlayerManager::finish_connect(&mut world, &alice).unwrap();
    PlayerManager::enter_game(&mut world, &alice).unwrap();
    assert!(PlayerManager::disconnect(
        &mut world,
        &alice,
        ConnectionId(1),
        DisconnectReason::ConnectionLost,
        secs(5),
    ));

    assert_eq!(
        take(&log),
        vec![
            Logged::Status(SessionStatus::Connecting),
            Logged::Status(SessionStatus::Connected),
            Logged::Status(SessionStatus::InGame),
            Logged::Status(SessionStatus::Disconnected),
        ]
    );
}

/// Scenario: moving control to another entity.
#[test]
fn rebinding() {
    let (mut world, log) = world_with_log();
    let alice = user("alice");
    let (a, b) = (world.spawn(), world.spawn());
    join(&mut world, &alice, 1, secs(0));
    PlayerManager::enter_game(&mut world, &alice).unwrap();
    PlayerManager::attach(&mut world, &alice, a).unwrap();
    take(&log);

    PlayerManager::attach(&mut world, &alice, b).unwrap();

    assert_eq!(
        take(&log),
        vec![
            Logged::Detached(a, alice.clone(), DetachReason::Rebound),
            Logged::Attached(b, alice.clone()),
        ]
    );
    let players = world.resource::<Players>().unwrap();
    assert_eq!(players.session(&alice).unwrap().entity(), Some(b));
    assert_eq!(players.user_of(a), None);
    assert_eq!(players.user_of(b), Some(&alice));
    assert!(!world.has::<PlayerControlled>(a));
    assert!(world.has::<PlayerControlled>(b));
}

/// Scenario: reconnecting after a lost connection.
#[test]
fn return_after_connection_loss() {
    let (mut world, log) = world_with_log();
    let alice = user("alice");
    let body = world.spawn();
    join(&mut world, &alice, 1, secs(0));
    PlayerManager::enter_game(&mut world, &alice).unwrap();
    PlayerManager::attach(&mut world, &alice, body).unwrap();
    world.add_component(body, Health(42)).unwrap();

    PlayerManager::disconnect(
        &mut world,
        &alice,
        ConnectionId(1),
        DisconnectReason::ConnectionLost,
        secs(100),
    );
    // The entity is uncontrolled and marked as disconnected.
    assert!(world.exists(body));
    assert!(world.has::<PlayerDisconnected>(body));
    let session = world
        .resource::<Players>()
        .unwrap()
        .session(&alice)
        .unwrap();
    assert!(!session.is_controlling());
    assert_eq!(session.disconnected_at(), Some(secs(100)));
    take(&log);

    // One minute later the same user reconnects with a new connection.
    PlayerManager::begin_connect(&mut world, &alice, "Alice", ConnectionId(2), secs(160));
    PlayerManager::finish_connect(&mut world, &alice).unwrap();

    assert_eq!(
        take(&log),
        vec![
            Logged::Status(SessionStatus::Connecting),
            Logged::Status(SessionStatus::Connected),
            Logged::Attached(body, alice.clone()),
            Logged::Status(SessionStatus::InGame),
        ]
    );
    let session = world
        .resource::<Players>()
        .unwrap()
        .session(&alice)
        .unwrap();
    assert!(session.is_controlling());
    assert_eq!(session.entity(), Some(body));
    assert_eq!(session.connection(), ConnectionId(2));
    assert!(!world.has::<PlayerDisconnected>(body));
    assert_eq!(world.get::<Health>(body).unwrap().0, 42); // The entity kept its state.
}

/// Scenario: the same user logs in again.
#[test]
fn repeated_login_of_same_user() {
    let (mut world, log) = world_with_log();
    let alice = user("alice");
    let body = world.spawn();
    join(&mut world, &alice, 1, secs(0));
    PlayerManager::enter_game(&mut world, &alice).unwrap();
    PlayerManager::attach(&mut world, &alice, body).unwrap();
    take(&log);

    // The second connection opens before the first closes.
    let outcome =
        PlayerManager::begin_connect(&mut world, &alice, "Alice", ConnectionId(2), secs(10));
    PlayerManager::finish_connect(&mut world, &alice).unwrap();

    assert_eq!(outcome.replaced, Some(ConnectionId(1)));
    let events = take(&log);
    assert_eq!(
        events[0],
        Logged::Drop(ConnectionId(1), DisconnectReason::Replaced)
    );
    assert!(events.contains(&Logged::Attached(body, alice.clone())));
    // Only one session is active, and the new connection controls it.
    let players = world.resource::<Players>().unwrap();
    assert_eq!(players.sessions().count(), 1);
    let session = players.session(&alice).unwrap();
    assert_eq!(session.connection(), ConnectionId(2));
    assert_eq!(session.status(), SessionStatus::InGame);
    assert_eq!(session.entity(), Some(body));

    // Late closure of the first connection leaves the new session alone.
    assert!(!PlayerManager::disconnect(
        &mut world,
        &alice,
        ConnectionId(1),
        DisconnectReason::ConnectionLost,
        secs(11),
    ));
    assert_eq!(status_of(&world, &alice), SessionStatus::InGame);
    assert!(!world.has::<PlayerDisconnected>(body));
}

#[derive(Debug)]
struct Health(u32);
impl crucible_ecs::Component for Health {}

#[test]
fn deleting_entity_detaches_player() {
    let (mut world, log) = world_with_log();
    let alice = user("alice");
    let body = world.spawn();
    join(&mut world, &alice, 1, secs(0));
    PlayerManager::attach(&mut world, &alice, body).unwrap();
    take(&log);

    world.delete(body);

    assert_eq!(
        take(&log),
        vec![Logged::Detached(
            body,
            alice.clone(),
            DetachReason::EntityDeleted
        )]
    );
    let players = world.resource::<Players>().unwrap();
    assert_eq!(players.session(&alice).unwrap().entity(), None);
    assert_eq!(players.user_of(body), None);
}

#[test]
fn entity_serves_one_session_only() {
    let (mut world, _log) = world_with_log();
    let (alice, bob) = (user("alice"), user("bob"));
    let body = world.spawn();
    join(&mut world, &alice, 1, secs(0));
    join(&mut world, &bob, 2, secs(0));
    PlayerManager::attach(&mut world, &alice, body).unwrap();

    assert_eq!(
        PlayerManager::attach(&mut world, &bob, body),
        Err(PlayerError::EntityOccupied {
            entity: body,
            by: alice.clone()
        })
    );
    // Alice's entity is reserved while she is disconnected.
    PlayerManager::disconnect(
        &mut world,
        &alice,
        ConnectionId(1),
        DisconnectReason::ConnectionLost,
        secs(1),
    );
    assert!(matches!(
        PlayerManager::attach(&mut world, &bob, body),
        Err(PlayerError::EntityOccupied { .. })
    ));
}

#[test]
fn attach_requires_live_entity_and_active_session() {
    let (mut world, _log) = world_with_log();
    let alice = user("alice");
    let body = world.spawn();
    assert_eq!(
        PlayerManager::attach(&mut world, &alice, body),
        Err(PlayerError::UnknownUser(alice.clone()))
    );
    PlayerManager::begin_connect(&mut world, &alice, "Alice", ConnectionId(1), secs(0));
    assert!(matches!(
        PlayerManager::attach(&mut world, &alice, body),
        Err(PlayerError::WrongStatus { .. })
    ));
    PlayerManager::finish_connect(&mut world, &alice).unwrap();
    world.delete(body);
    assert_eq!(
        PlayerManager::attach(&mut world, &alice, body),
        Err(PlayerError::EntityNotFound(body))
    );
}

/// Scenario: returning to the lobby between rounds moves the session from in-game to connected
/// and detaches the entity. A player already in the lobby cannot return to it.
#[test]
fn leave_game_returns_to_lobby() {
    let (mut world, log) = world_with_log();
    let alice = user("alice");
    let body = world.spawn();
    join(&mut world, &alice, 1, secs(0));
    PlayerManager::enter_game(&mut world, &alice).unwrap();
    PlayerManager::attach(&mut world, &alice, body).unwrap();
    take(&log);

    PlayerManager::leave_game(&mut world, &alice).unwrap();
    assert_eq!(status_of(&world, &alice), SessionStatus::Connected);
    assert_eq!(
        take(&log),
        vec![
            Logged::Detached(body, alice.clone(), DetachReason::Requested),
            Logged::Status(SessionStatus::Connected),
        ]
    );
    assert!(!world.has::<PlayerControlled>(body));
    assert!(matches!(
        PlayerManager::leave_game(&mut world, &alice),
        Err(PlayerError::WrongStatus { .. })
    ));
    // Re-enter the game through the same path as at round start.
    PlayerManager::enter_game(&mut world, &alice).unwrap();
    assert_eq!(status_of(&world, &alice), SessionStatus::InGame);
}

#[test]
fn explicit_detach_removes_binding() {
    let (mut world, log) = world_with_log();
    let alice = user("alice");
    let body = world.spawn();
    join(&mut world, &alice, 1, secs(0));
    PlayerManager::attach(&mut world, &alice, body).unwrap();
    take(&log);

    assert!(PlayerManager::detach(&mut world, &alice));
    assert!(!PlayerManager::detach(&mut world, &alice));
    assert_eq!(
        take(&log),
        vec![Logged::Detached(
            body,
            alice.clone(),
            DetachReason::Requested
        )]
    );
    assert!(!world.has::<PlayerControlled>(body));
}
