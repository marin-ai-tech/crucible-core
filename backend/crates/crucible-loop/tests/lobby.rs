//! Lobby countdown and late join behavior, including pauses, deferrals, and return to the
//! lobby between rounds.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use crucible_ecs::{EntityUid, Tick, World};
use crucible_loop::{
    GameLoop, LateJoinError, LoopConfig, RoundConfig, RoundFlow, RoundPhase, RoundPoints,
    SpawnPolicy,
};
use crucible_players::{
    ConnectionId, NetUserId, PlayerManager, Players, PlayersSystem, SessionStatus,
};

const RATE: u32 = 30;

/// Spawn policy that records whether a character joined at round start or joined late.
#[derive(Clone, Default)]
struct Spawns(Arc<Mutex<Vec<(NetUserId, &'static str)>>>);

impl SpawnPolicy for Spawns {
    fn spawn(&mut self, world: &mut World, player: &NetUserId, _role: &str) -> Option<EntityUid> {
        self.0.lock().unwrap().push((player.clone(), "start"));
        let uid = world.spawn();
        let _ = PlayerManager::enter_game(world, player);
        let _ = PlayerManager::attach(world, player, uid);
        Some(uid)
    }

    fn spawn_late_join(
        &mut self,
        world: &mut World,
        player: &NetUserId,
        _role: &str,
    ) -> Option<EntityUid> {
        self.0.lock().unwrap().push((player.clone(), "late"));
        let uid = world.spawn();
        let _ = PlayerManager::enter_game(world, player);
        let _ = PlayerManager::attach(world, player, uid);
        Some(uid)
    }
}

struct Stand {
    game: GameLoop,
    flow: RoundFlow,
    spawns: Spawns,
}

impl Stand {
    fn new(lobby_seconds: u64) -> Self {
        let mut builder = World::builder();
        builder.add_system(PlayersSystem);
        let (world, systems) = builder.build().unwrap();
        let game = GameLoop::new(
            LoopConfig {
                seed: Some(1),
                ..LoopConfig::default()
            },
            world,
            systems,
        );
        let spawns = Spawns::default();
        let points = RoundPoints {
            spawn_policy: Some(Box::new(spawns.clone())),
            ..RoundPoints::default()
        };
        let config = RoundConfig {
            lobby_duration: Duration::from_secs(lobby_seconds),
            round_duration: Some(Duration::from_secs(5)),
            post_round_duration: Duration::from_secs(1),
        };
        Stand {
            game,
            flow: RoundFlow::new(config, points, Vec::new()),
            spawns,
        }
    }

    fn tick(&mut self) -> Tick {
        let flow = &mut self.flow;
        self.game.tick_with(&mut |world, dt| flow.update(world, dt))
    }

    fn ticks(&mut self, n: u32) {
        for _ in 0..n {
            self.tick();
        }
    }

    fn join(&mut self, name: &str) -> NetUserId {
        let user = NetUserId::new(name);
        let world = self.game.world_mut();
        PlayerManager::begin_connect(world, &user, name, ConnectionId(1), Duration::ZERO);
        PlayerManager::finish_connect(world, &user).unwrap();
        self.flow.add_player(user.clone());
        user
    }

    fn status(&self, user: &NetUserId) -> SessionStatus {
        self.game
            .world()
            .resource::<Players>()
            .unwrap()
            .session(user)
            .unwrap()
            .status()
    }
}

/// Scenario: the round starts on the tick specified by the lobby countdown.
#[test]
fn lobby_countdown_names_start_tick() {
    let mut stand = Stand::new(2);
    stand.tick();
    let countdown = stand.flow.lobby_countdown(stand.game.world()).unwrap();
    assert!(!countdown.paused);
    let start = countdown.start_tick;
    while stand.flow.phase() == RoundPhase::Lobby {
        let tick = stand.tick();
        assert!(
            tick.0 <= start.0,
            "the round did not start by tick {start:?}"
        );
    }
    assert_eq!(stand.game.world().current_tick(), start);
    assert!(stand.flow.lobby_countdown(stand.game.world()).is_none());
    assert_eq!(stand.flow.round_started_at(), Some(start));
}

/// Scenario: a paused countdown prevents the round from starting. The paused time and
/// any deferrals move the start tick.
#[test]
fn lobby_pause_and_delay_shift_start() {
    let mut stand = Stand::new(1);
    stand.tick();
    let start = stand
        .flow
        .lobby_countdown(stand.game.world())
        .unwrap()
        .start_tick;
    assert!(stand.flow.set_lobby_paused(stand.game.world(), true));
    assert!(
        !stand.flow.set_lobby_paused(stand.game.world(), true),
        "pausing twice is harmless"
    );
    stand.ticks(3 * RATE);
    assert_eq!(
        stand.flow.phase(),
        RoundPhase::Lobby,
        "the round does not start while the countdown is paused"
    );
    let paused = stand.flow.lobby_countdown(stand.game.world()).unwrap();
    assert!(paused.paused);
    assert_eq!(paused.start_tick.0, start.0 + 3 * RATE);

    assert!(stand.flow.set_lobby_paused(stand.game.world(), false));
    assert!(stand.flow.delay_start(Duration::from_secs(1)));
    let resumed = stand.flow.lobby_countdown(stand.game.world()).unwrap();
    assert!(!resumed.paused);
    assert_eq!(resumed.start_tick.0, start.0 + 3 * RATE + RATE);
    while stand.flow.phase() == RoundPhase::Lobby {
        stand.tick();
    }
    assert_eq!(stand.game.world().current_tick(), resumed.start_tick);
    assert!(
        !stand.flow.delay_start(Duration::from_secs(1)),
        "deferral is unavailable outside the lobby"
    );
}

/// Scenario: during the round, a player joins through the late-join spawn policy.
/// Requests before the round and duplicate joins are rejected.
#[test]
fn late_join_spawns_participant() {
    let mut stand = Stand::new(1);
    let early = stand.join("early");
    stand.flow.set_ready(early.clone(), true);
    let late = stand.join("late");
    assert_eq!(
        stand.flow.late_join(stand.game.world_mut(), &late),
        Err(LateJoinError::RoundNotStarted)
    );
    while stand.flow.phase() != RoundPhase::InRound {
        stand.tick();
    }
    assert_eq!(stand.status(&early), SessionStatus::InGame);
    assert_eq!(
        stand.status(&late),
        SessionStatus::Connected,
        "an unready player remains in the lobby"
    );

    let (world, flow) = (stand.game.world_mut(), &mut stand.flow);
    let body = flow.late_join(world, &late).unwrap();
    assert_eq!(stand.status(&late), SessionStatus::InGame);
    assert!(
        stand
            .flow
            .participants()
            .iter()
            .any(|p| p.player == late && p.entity == Some(body))
    );
    assert_eq!(
        stand.flow.late_join(stand.game.world_mut(), &late),
        Err(LateJoinError::AlreadyInRound)
    );
    assert_eq!(
        *stand.spawns.0.lock().unwrap(),
        vec![(early.clone(), "start"), (late.clone(), "late")]
    );
}

/// Scenario: after round results, players return to the lobby as connected.
#[test]
fn players_return_to_lobby_after_round() {
    let mut stand = Stand::new(1);
    let user = stand.join("player");
    stand.flow.set_ready(user.clone(), true);
    while stand.flow.phase() != RoundPhase::InRound {
        stand.tick();
    }
    assert_eq!(stand.status(&user), SessionStatus::InGame);
    while stand.flow.phase() != RoundPhase::Lobby {
        stand.tick();
    }
    assert_eq!(stand.status(&user), SessionStatus::Connected);
    assert!(stand.flow.lobby_countdown(stand.game.world()).is_some());
}
