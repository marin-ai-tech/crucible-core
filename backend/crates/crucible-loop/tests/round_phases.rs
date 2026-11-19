//! Round phase, startup, end, and world cleanup scenarios for `specs/core/simulation-loop`.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crucible_ecs::{Component, System, SystemInit, World};
use crucible_loop::{
    CampaignPersistence, CampaignPersistenceError, CampaignRevision, CampaignRound, CampaignState,
    DEFAULT_ROLE, DEFAULT_RULE_ID, GameLoop, GameRule, LoopConfig, ObjectiveOutcome, Participant,
    RoundConfig, RoundEndSource, RoundEnded, RoundFlow, RoundPhase, RoundPhaseChanged,
    RoundPhaseMachine, RoundPoints, RoundPreparation, RoundPreparationHook, RoundResultsHook,
    RoundStarted, RoundSummary, TransitionError,
};
use crucible_players::NetUserId;

#[derive(Default)]
struct Transitions(Vec<(RoundPhase, RoundPhase)>);

#[derive(Default)]
struct Ended(Vec<RoundSummary>);

/// Records phase transition and summary events.
struct Watcher;
impl System for Watcher {
    fn initialize(&mut self, init: &mut SystemInit<'_>) {
        init.insert_resource(Transitions::default());
        init.insert_resource(Ended::default());
        init.subscribe_broadcast::<RoundPhaseChanged, _>(|world, ev| {
            world
                .resource_mut::<Transitions>()
                .unwrap()
                .0
                .push((ev.from, ev.to));
        });
        init.subscribe_broadcast::<RoundEnded, _>(|world, ev| {
            world
                .resource_mut::<Ended>()
                .unwrap()
                .0
                .push(ev.summary.clone());
        });
        init.subscribe_broadcast::<RoundStarted, _>(|_, ev| assert!(ev.round >= 1));
    }
}

struct RoundsStand {
    game: GameLoop,
    flow: RoundFlow,
}

impl RoundsStand {
    fn new(config: RoundConfig, points: RoundPoints, rules: Vec<Box<dyn GameRule>>) -> Self {
        let mut builder = World::builder();
        builder.add_system(Watcher);
        let (world, systems) = builder.build().unwrap();
        let game = GameLoop::new(
            LoopConfig {
                seed: Some(5),
                ..LoopConfig::default()
            },
            world,
            systems,
        );
        RoundsStand {
            game,
            flow: RoundFlow::new(config, points, rules),
        }
    }

    fn tick(&mut self) {
        let flow = &mut self.flow;
        self.game.tick_with(&mut |world, dt| flow.update(world, dt));
    }

    /// Advances until `state` is reached, for at most `limit` ticks.
    fn run_until(&mut self, state: RoundPhase, limit: u32) -> u32 {
        for i in 0..limit {
            if self.flow.phase() == state {
                return i;
            }
            self.tick();
        }
        panic!("round flow did not reach '{state}' within {limit} ticks");
    }

    fn transitions(&self) -> Vec<(RoundPhase, RoundPhase)> {
        self.game
            .world()
            .resource::<Transitions>()
            .unwrap()
            .0
            .clone()
    }
}

fn config() -> RoundConfig {
    RoundConfig {
        lobby_duration: Duration::from_secs(1),
        round_duration: None,
        post_round_duration: Duration::from_secs(1),
    }
}

/// Marker for an entity created by the rule.
struct RuleMarker;
impl Component for RuleMarker {}

/// The rule assigns the first player `leader` and the rest the default role. It ends after
/// `ticks` ticks and reports the shared `hold` objective as achieved.
struct TimedRule {
    ticks: u32,
    elapsed: u32,
}

impl GameRule for TimedRule {
    fn id(&self) -> &str {
        "timed"
    }
    fn assign_roles(&mut self, _: &mut World, players: &[NetUserId]) -> Vec<(NetUserId, String)> {
        players
            .iter()
            .enumerate()
            .map(|(i, p)| {
                (
                    p.clone(),
                    if i == 0 { "leader" } else { DEFAULT_ROLE }.to_owned(),
                )
            })
            .collect()
    }
    fn started(&mut self, world: &mut World, _: &[Participant]) {
        self.elapsed = 0;
        let uid = world.spawn();
        world.add_component(uid, RuleMarker).unwrap();
    }
    fn check_end(&mut self, _: &World) -> Option<String> {
        self.elapsed += 1;
        (self.elapsed >= self.ticks).then(|| "objective held".to_owned())
    }
    fn objectives(&self, _: &World, participants: &[Participant]) -> Vec<ObjectiveOutcome> {
        let mut out = vec![ObjectiveOutcome {
            player: None,
            objective: "hold".to_owned(),
            completed: true,
        }];
        if let Some(leader) = participants.iter().find(|p| p.role == "leader") {
            out.push(ObjectiveOutcome {
                player: Some(leader.player.clone()),
                objective: "lead".to_owned(),
                completed: false,
            });
        }
        out
    }
}

fn timed_rule(ticks: u32) -> Vec<Box<dyn GameRule>> {
    vec![Box::new(TimedRule { ticks, elapsed: 0 })]
}

fn add_ready(flow: &mut RoundFlow, players: &[u64]) {
    for &p in players {
        flow.add_player(NetUserId::new(p.to_string()));
        flow.set_ready(NetUserId::new(p.to_string()), true);
    }
}

/// Scenario: full round lifecycle.
#[test]
fn full_cycle() {
    let mut s = RoundsStand::new(config(), RoundPoints::default(), timed_rule(10));
    add_ready(&mut s.flow, &[1, 2]);
    s.flow.add_player(NetUserId::new("3")); // Unready players do not participate.

    // The lobby timer lasts one second (30 ticks).
    let waited = s.run_until(RoundPhase::InRound, 100);
    assert_eq!(waited, 30, "the round starts when the lobby timer expires");
    assert_eq!(
        s.flow.current_rule(),
        Some("timed"),
        "the rule was selected"
    );
    let roles: Vec<_> = s
        .flow
        .participants()
        .iter()
        .map(|p| (p.player.as_str(), p.role.as_str()))
        .collect();
    assert_eq!(
        roles,
        vec![("1", "leader"), ("2", DEFAULT_ROLE)],
        "roles were assigned"
    );
    assert!(s.flow.participants().iter().all(|p| p.entity.is_some()));

    s.run_until(RoundPhase::PostRound, 100);
    s.run_until(RoundPhase::Lobby, 100);
    use RoundPhase::*;
    assert_eq!(
        s.transitions(),
        vec![
            (Lobby, PreRound),
            (PreRound, InRound),
            (InRound, PostRound),
            (PostRound, Lobby)
        ],
        "each transition raises an event"
    );
    assert_eq!(s.flow.round(), 1);
}

/// Scenario: invalid transition.
#[test]
fn invalid_transition() {
    let mut s = RoundsStand::new(config(), RoundPoints::default(), timed_rule(10));
    let world = s.game.world_mut();
    let error = s
        .flow
        .request_transition(world, RoundPhase::PostRound)
        .unwrap_err();
    assert_eq!(
        error,
        TransitionError {
            from: RoundPhase::Lobby,
            to: RoundPhase::PostRound
        }
    );
    assert_eq!(s.flow.phase(), RoundPhase::Lobby, "the phase is unchanged");
    assert_eq!(
        s.flow.machine().rejected(),
        &[error],
        "the diagnostic was recorded"
    );
    assert!(error.to_string().contains("Lobby -> Results"));
    assert!(s.transitions().is_empty(), "no transition event was raised");

    // The phase machine alone behaves the same way.
    let mut machine = RoundPhaseMachine::new();
    let mut world = World::new();
    assert!(machine.transition(&mut world, RoundPhase::InRound).is_err());
    assert!(machine.transition(&mut world, RoundPhase::PreRound).is_ok());
    assert!(machine.transition(&mut world, RoundPhase::Lobby).is_err());
    assert_eq!(machine.phase(), RoundPhase::PreRound);
    assert_eq!(machine.rejected().len(), 2);
}

/// Rule with a minimum player count.
struct Crowded;
impl GameRule for Crowded {
    fn id(&self) -> &str {
        "crowded"
    }
    fn min_players(&self) -> usize {
        4
    }
}

/// Scenario: starting with no eligible rule.
#[test]
fn start_without_eligible_rules() {
    let mut s = RoundsStand::new(config(), RoundPoints::default(), vec![Box::new(Crowded)]);
    add_ready(&mut s.flow, &[1, 2]);
    s.flow.request_start();
    s.tick();
    assert_eq!(s.flow.phase(), RoundPhase::InRound, "the round starts");
    assert_eq!(
        s.flow.current_rule(),
        Some(DEFAULT_RULE_ID),
        "the default rule was selected"
    );
    let reason = s
        .flow
        .rule_fallback_reason()
        .expect("fallback reason was recorded");
    assert!(
        reason.contains("'crowded'") && reason.contains("at least 4") && reason.contains("got 2"),
        "fallback reason: {reason}"
    );
}

/// Summary hook that collects summaries and counts rounds in campaign state.
struct ResultsProbe(Arc<Mutex<Vec<RoundSummary>>>);
impl RoundResultsHook for ResultsProbe {
    fn on_round_results(&mut self, summary: &RoundSummary, campaign: &mut CampaignState) {
        self.0.lock().unwrap().push(summary.clone());
        campaign
            .values
            .insert("rounds".to_owned(), summary.round.to_string());
    }
}

/// Scenario: round summary.
#[test]
fn round_summary() {
    let received = Arc::new(Mutex::new(Vec::new()));
    let points = RoundPoints {
        results_hooks: vec![Box::new(ResultsProbe(received.clone()))],
        ..RoundPoints::default()
    };
    let mut s = RoundsStand::new(config(), points, timed_rule(5));
    add_ready(&mut s.flow, &[10, 20]);
    s.flow.request_start();
    s.run_until(RoundPhase::PostRound, 50);

    let summary = s
        .flow
        .last_summary()
        .expect("round summary is available")
        .clone();
    assert_eq!(summary.round, 1);
    assert_eq!(summary.rule, "timed");
    assert_eq!(
        summary.end_reason.source,
        RoundEndSource::Rule,
        "the rule's end condition ended the round"
    );
    let roles: Vec<_> = summary
        .participants
        .iter()
        .map(|p| (p.player.clone(), p.role.clone()))
        .collect();
    assert_eq!(
        roles,
        vec![
            (NetUserId::new("10"), "leader".to_owned()),
            (NetUserId::new("20"), DEFAULT_ROLE.to_owned())
        ]
    );
    assert_eq!(summary.objectives.len(), 2, "objective outcomes");
    assert!(summary.objectives[0].completed && !summary.objectives[1].completed);

    // Registered summary hooks and players receive the summary through RoundEnded.
    assert_eq!(*received.lock().unwrap(), vec![summary.clone()]);
    assert_eq!(s.game.world().resource::<Ended>().unwrap().0, vec![summary]);
    assert_eq!(s.flow.campaign_state().values["rounds"], "1");
}

/// Data outside round entities: player profiles are a world resource.
#[derive(Clone, PartialEq, Debug)]
struct ProfileStore(BTreeMap<u64, String>);

/// Scenario: a new round starts with a clean world.
#[test]
fn new_round_with_clean_world() {
    let campaign = CampaignState {
        values: BTreeMap::from([("progress".to_owned(), "3".to_owned())]),
    };
    let mut s = RoundsStand::new(config(), RoundPoints::default(), timed_rule(5));
    s.flow = RoundFlow::new(config(), RoundPoints::default(), timed_rule(5))
        .with_campaign_state(campaign.clone());
    let profiles = ProfileStore(BTreeMap::from([
        (1, "first".to_owned()),
        (2, "second".to_owned()),
    ]));
    s.game.world_mut().insert_resource(profiles.clone());
    add_ready(&mut s.flow, &[1, 2]);

    s.flow.request_start();
    s.run_until(RoundPhase::PostRound, 50);
    // The two players' characters and the rule entity, plus an entity queued for removal.
    let world = s.game.world_mut();
    let queued = world.spawn();
    world.queue_delete(queued);
    let leftover = world.spawn();
    assert!(world.entity_count() >= 4);
    let round_entities = world.entities();

    s.run_until(RoundPhase::Lobby, 100);
    let world = s.game.world();
    assert_eq!(
        world.entity_count(),
        0,
        "the world contains no entities from the previous round"
    );
    assert!(round_entities.iter().all(|&e| !world.exists(e)));
    assert!(!world.exists(leftover));
    assert_eq!(
        world.resource::<ProfileStore>(),
        Some(&profiles),
        "profiles are unchanged"
    );
    assert_eq!(
        s.flow.campaign_state(),
        &campaign,
        "campaign state is unchanged"
    );

    // The next round starts with a clean world.
    s.flow.request_start();
    s.tick();
    assert_eq!(s.flow.round(), 2);
    assert_eq!(
        s.game.world().entity_count(),
        3,
        "only the new round's entities are present"
    );
}

struct CampaignStoreProbe {
    snapshot: CampaignRevision,
    preparation_error: Option<CampaignPersistenceError>,
    commit_error: Option<CampaignPersistenceError>,
    lose_next_confirmation: bool,
    last_committed_id: Option<String>,
    begins: usize,
    attempts: Vec<(CampaignRound, CampaignState)>,
}

impl CampaignStoreProbe {
    fn new() -> Self {
        Self {
            snapshot: CampaignRevision {
                revision: 8,
                state: CampaignState {
                    values: BTreeMap::from([("rounds".to_owned(), "0".to_owned())]),
                },
            },
            preparation_error: None,
            commit_error: None,
            lose_next_confirmation: false,
            last_committed_id: None,
            begins: 0,
            attempts: Vec::new(),
        }
    }
}

struct StoredProbe(Arc<Mutex<CampaignStoreProbe>>);

impl CampaignPersistence for StoredProbe {
    fn begin_round(&mut self) -> Result<CampaignRound, CampaignPersistenceError> {
        let mut store = self.0.lock().unwrap();
        store.begins += 1;
        if let Some(error) = &store.preparation_error {
            return Err(error.clone());
        }
        Ok(CampaignRound {
            result_id: format!("stable-{}", store.begins),
            base: store.snapshot.clone(),
        })
    }

    fn commit_round(
        &mut self,
        round: &CampaignRound,
        staged: &CampaignState,
    ) -> Result<CampaignRevision, CampaignPersistenceError> {
        let mut store = self.0.lock().unwrap();
        store.attempts.push((round.clone(), staged.clone()));
        if let Some(error) = &store.commit_error {
            return Err(error.clone());
        }
        if store.last_committed_id.as_deref() != Some(round.result_id.as_str()) {
            assert_eq!(round.base, store.snapshot);
            store.snapshot = CampaignRevision {
                revision: store.snapshot.revision + 1,
                state: staged.clone(),
            };
            store.last_committed_id = Some(round.result_id.clone());
        }
        if std::mem::take(&mut store.lose_next_confirmation) {
            return Err(CampaignPersistenceError::Retryable(
                "write acknowledgement was lost".to_owned(),
            ));
        }
        Ok(store.snapshot.clone())
    }
}

struct PreparationProbe(Arc<Mutex<Vec<CampaignState>>>);

impl RoundPreparationHook for PreparationProbe {
    fn prepare_round(&mut self, campaign: &CampaignState, _: &mut RoundPreparation) {
        self.0.lock().unwrap().push(campaign.clone());
    }
}

#[test]
fn campaign_retry_keeps_same_result_and_guards_both_post_round_exits() {
    let store = Arc::new(Mutex::new(CampaignStoreProbe::new()));
    store.lock().unwrap().lose_next_confirmation = true;
    let prepared = Arc::new(Mutex::new(Vec::new()));
    let outcomes = Arc::new(Mutex::new(Vec::new()));
    let mut s = RoundsStand::new(
        RoundConfig {
            post_round_duration: Duration::ZERO,
            ..config()
        },
        RoundPoints {
            results_hooks: vec![Box::new(ResultsProbe(outcomes.clone()))],
            preparation_hooks: vec![Box::new(PreparationProbe(prepared.clone()))],
            ..RoundPoints::default()
        },
        timed_rule(1),
    );
    s.flow
        .set_campaign_persistence(Box::new(StoredProbe(store.clone())))
        .unwrap();

    s.flow.request_start();
    s.tick();
    assert_eq!(s.flow.campaign_revision(), Some(8));
    assert_eq!(prepared.lock().unwrap()[0].values["rounds"], "0");

    s.tick();
    assert_eq!(s.flow.phase(), RoundPhase::PostRound);
    assert!(matches!(
        s.flow.campaign_persistence_error(),
        Some(CampaignPersistenceError::Retryable(_))
    ));
    assert_eq!(store.lock().unwrap().snapshot.revision, 9);
    assert_eq!(s.flow.campaign_revision(), Some(8));
    assert_eq!(s.flow.campaign_state().values["rounds"], "0");
    let entity = s.game.world().entities()[0];
    let error = s
        .flow
        .request_transition(s.game.world_mut(), RoundPhase::Lobby)
        .unwrap_err();
    assert_eq!(error.from, RoundPhase::PostRound);
    for _ in 0..29 {
        s.tick();
    }
    assert_eq!(s.flow.phase(), RoundPhase::PostRound);
    assert!(s.game.world().exists(entity));
    assert_eq!(store.lock().unwrap().attempts.len(), 1);

    s.tick();
    assert_eq!(s.flow.phase(), RoundPhase::Lobby);
    assert!(!s.game.world().exists(entity));
    assert_eq!(s.flow.campaign_revision(), Some(9));
    assert_eq!(s.flow.campaign_state().values["rounds"], "1");
    assert_eq!(s.flow.campaign_persistence_error(), None);
    {
        let store = store.lock().unwrap();
        assert_eq!(store.attempts.len(), 2);
        assert_eq!(store.attempts[0], store.attempts[1]);
        assert_eq!(store.snapshot.revision, 9);
    }
    assert_eq!(outcomes.lock().unwrap().len(), 1);
    assert_eq!(s.game.world().resource::<Ended>().unwrap().0.len(), 1);

    s.flow.request_start();
    s.tick();
    assert_eq!(s.flow.round(), 2);
    assert_eq!(prepared.lock().unwrap()[1].values["rounds"], "1");
}

#[test]
fn campaign_incompatible_revision_blocks_preparation() {
    let store = Arc::new(Mutex::new(CampaignStoreProbe::new()));
    let diagnostic = CampaignPersistenceError::Rejected("migration failed".to_owned());
    store.lock().unwrap().preparation_error = Some(diagnostic.clone());
    let prepared = Arc::new(Mutex::new(Vec::new()));
    let mut s = RoundsStand::new(
        config(),
        RoundPoints {
            preparation_hooks: vec![Box::new(PreparationProbe(prepared.clone()))],
            ..RoundPoints::default()
        },
        timed_rule(1),
    );
    s.flow
        .set_campaign_persistence(Box::new(StoredProbe(store.clone())))
        .unwrap();
    assert!(
        s.flow
            .request_transition(s.game.world_mut(), RoundPhase::PreRound)
            .is_err()
    );
    for _ in 0..60 {
        s.tick();
    }
    assert_eq!(s.flow.phase(), RoundPhase::Lobby);
    assert_eq!(s.flow.round(), 0);
    assert_eq!(s.flow.campaign_revision(), None);
    assert_eq!(s.flow.campaign_persistence_error(), Some(&diagnostic));
    assert!(s.transitions().is_empty());
    assert!(prepared.lock().unwrap().is_empty());
    assert_eq!(store.lock().unwrap().begins, 1);
}

#[test]
fn campaign_admin_end_does_not_commit_staged_changes() {
    let store = Arc::new(Mutex::new(CampaignStoreProbe::new()));
    let outcomes = Arc::new(Mutex::new(Vec::new()));
    let mut s = RoundsStand::new(
        config(),
        RoundPoints {
            results_hooks: vec![Box::new(ResultsProbe(outcomes.clone()))],
            ..RoundPoints::default()
        },
        timed_rule(10),
    );
    s.flow
        .set_campaign_persistence(Box::new(StoredProbe(store.clone())))
        .unwrap();
    s.flow.request_start();
    s.tick();
    s.flow
        .request_transition(s.game.world_mut(), RoundPhase::PostRound)
        .unwrap();
    assert_eq!(s.flow.campaign_revision(), Some(8));
    assert_eq!(s.flow.campaign_state().values["rounds"], "0");
    assert!(store.lock().unwrap().attempts.is_empty());
    assert_eq!(
        outcomes.lock().unwrap()[0].end_reason.source,
        RoundEndSource::Admin
    );
    s.flow
        .request_transition(s.game.world_mut(), RoundPhase::Lobby)
        .unwrap();
}

#[test]
fn campaign_permanent_store_rejection_holds_post_round() {
    let store = Arc::new(Mutex::new(CampaignStoreProbe::new()));
    let diagnostic = CampaignPersistenceError::Rejected("revision conflict".to_owned());
    store.lock().unwrap().commit_error = Some(diagnostic.clone());
    let mut s = RoundsStand::new(
        RoundConfig {
            post_round_duration: Duration::ZERO,
            ..config()
        },
        RoundPoints {
            results_hooks: vec![Box::new(ResultsProbe(Arc::default()))],
            ..RoundPoints::default()
        },
        timed_rule(1),
    );
    s.flow
        .set_campaign_persistence(Box::new(StoredProbe(store.clone())))
        .unwrap();
    s.flow.request_start();
    s.run_until(RoundPhase::PostRound, 10);
    for _ in 0..60 {
        s.tick();
    }
    assert!(
        s.flow
            .request_transition(s.game.world_mut(), RoundPhase::Lobby)
            .is_err()
    );
    assert_eq!(s.flow.phase(), RoundPhase::PostRound);
    assert_eq!(s.flow.campaign_revision(), Some(8));
    assert_eq!(s.flow.campaign_persistence_error(), Some(&diagnostic));
    assert_eq!(store.lock().unwrap().attempts.len(), 1);
}
