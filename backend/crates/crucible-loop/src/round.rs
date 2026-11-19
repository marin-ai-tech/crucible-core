//! Starts and ends rounds, builds summaries, and cleans up the world between rounds.
//! Implements the round start, summary, and world cleanup requirements.
//!
//! [`RoundFlow`] advances the server through [`RoundPhase`] and invokes [`RoundPoints`] at
//! transitions. The points cover map selection, rule selection, spawning, normal end conditions,
//! event direction, and campaign hooks. This crate owns their traits because it calls them.
//! `crucible-ext` assembles them in `ExtensionPoints` with default implementations.

use std::collections::BTreeMap;
use std::time::Duration;

use crucible_ecs::{EntityUid, Tick, World};
use crucible_players::{NetUserId, PlayerManager, Players, SessionStatus};

use crate::campaign::{CampaignPersistence, CampaignPersistenceError, DurableCampaign};
use crate::phase::{CurrentRoundPhase, RoundPhase, RoundPhaseMachine, TransitionError};
use crate::time::SimTime;

// Round flow uses the stable user identifier `NetUserId` from `crucible-players`.
// Player sessions use the same ID, so `PlayerManager` can find a participant and bind the
// session to the spawned character.

/// Default role when the round rule does not assign roles.
pub const DEFAULT_ROLE: &str = "participant";

/// Default round rule identifier, chosen when no registered rule applies.
pub const DEFAULT_RULE_ID: &str = "default";

/// Campaign data lives outside the world and survives a round. Campaign hooks provided by
/// the extension own the meaning and contents of these values.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct CampaignState {
    /// Values indexed by key.
    pub values: BTreeMap<String, String>,
}

/// Data available to extension points during round preparation.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct RoundPreparation {
    /// Round number.
    pub round: u32,
    /// Ready players who will participate in the round.
    pub players: Vec<NetUserId>,
    /// Round parameters supplied by the campaign preparation hooks.
    pub parameters: BTreeMap<String, String>,
}

/// Objective outcome in the round summary.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ObjectiveOutcome {
    /// Player who owns the objective, or `None` for a shared objective.
    pub player: Option<NetUserId>,
    /// Objective description or localization key.
    pub objective: String,
    /// Whether the objective was achieved.
    pub completed: bool,
}

/// Participant in a round.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Participant {
    /// Player.
    pub player: NetUserId,
    /// Role.
    pub role: String,
    /// Character created by the spawn policy, or `None` if it created no character.
    pub entity: Option<EntityUid>,
}

/// Source of the round end.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RoundEndSource {
    /// A condition in the round rule.
    Rule,
    /// The normal end policy, which defaults to the timer from configuration.
    Policy,
    /// An administrator command.
    Admin,
}

/// Reason the round ended.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RoundEndReason {
    /// Source.
    pub source: RoundEndSource,
    /// Explanation.
    pub text: String,
}

/// Round summary with participants, roles, and objective outcomes. Players can access it
/// through [`RoundFlow::last_summary`] and [`RoundEnded`]. Summary hooks also receive it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RoundSummary {
    /// Round number.
    pub round: u32,
    /// Round rule identifier.
    pub rule: String,
    /// Reason the default rule was used, if applicable.
    pub rule_fallback: Option<String>,
    /// Reason the round ended.
    pub end_reason: RoundEndReason,
    /// Round duration in ticks.
    pub duration_ticks: u32,
    /// Participants and their roles.
    pub participants: Vec<Participant>,
    /// Objective outcomes.
    pub objectives: Vec<ObjectiveOutcome>,
}

/// Participants prepared by the trusted input phase before the round update.
#[derive(Default)]
pub struct LateAdmissions(Vec<(u32, Participant)>);

impl LateAdmissions {
    pub fn record(world: &mut World, round: u32, participant: Participant) {
        if world.resource::<Self>().is_none() {
            world.insert_resource(Self::default());
        }
        if let Some(admissions) = world.resource_mut::<Self>() {
            admissions.0.push((round, participant));
        }
    }
}

// ── Round events ────────────────────────────────────────────────────────────────────────

/// The round moves from Lobby to Preparation.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RoundStarting {
    /// Round number.
    pub round: u32,
}

/// Preparation rejected before character bodies are created. Trusted systems flag the current PreRound.
pub struct RoundPreparationFailure {
    round: u32,
    reason: String,
}

impl RoundPreparationFailure {
    pub fn reject(world: &mut World, reason: impl Into<String>) {
        let Some(current) = world
            .resource::<CurrentRoundPhase>()
            .copied()
            .filter(|phase| phase.phase == RoundPhase::PreRound)
        else {
            return;
        };
        if world.resource::<Self>().is_none() {
            world.insert_resource(Self {
                round: current.round,
                reason: reason.into(),
            });
        }
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }
}

/// The round moves from Preparation to In Round.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RoundStarted {
    /// Round number.
    pub round: u32,
    /// Round rule.
    pub rule: String,
}

/// The round ended and the summary is ready. The phase changes from In Round to Results.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RoundEnded {
    /// Round summary.
    pub summary: RoundSummary,
}

/// The phase changes from Results to Lobby. This event precedes removal of round entities.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RoundRestartCleanup {
    /// Number of the completed round.
    pub round: u32,
}

// ── Round extension points ─────────────────────────────────────────────────────────────

/// Selects maps and generates external areas. By default, [`NoMaps`] loads no maps.
pub trait MapSelector: Send {
    /// Loads round maps and returns their root entities.
    fn load_maps(
        &mut self,
        world: &mut World,
        preparation: &RoundPreparation,
    ) -> Result<Vec<EntityUid>, String>;
}

/// Player spawn policy, which creates a character for a participant.
/// [`EmptyEntitySpawn`] creates an empty entity by default.
pub trait SpawnPolicy: Send {
    /// Allows the policy to handle the entire ready roster in one server step.
    fn spawn_prepared(
        &mut self,
        _world: &mut World,
        _roles: &[(NetUserId, String)],
    ) -> Option<Vec<Participant>> {
        None
    }

    /// Creates a character for the player in `role`.
    fn spawn(&mut self, world: &mut World, player: &NetUserId, role: &str) -> Option<EntityUid>;

    /// Spawns a late joiner during an active round. The default calls [`spawn`](SpawnPolicy::spawn).
    /// A policy with dedicated late-join points can override this method.
    fn spawn_late_join(
        &mut self,
        world: &mut World,
        player: &NetUserId,
        role: &str,
    ) -> Option<EntityUid> {
        self.spawn(world, player, role)
    }
}

/// Normal round end policy. [`TimerRoundEnd`] uses the duration in the server configuration
/// ([`RoundConfig::round_duration`]) by default.
pub trait RoundEndPolicy: Send {
    /// Runs each tick. Returning `Some(reason)` ends the round.
    fn check(&mut self, world: &World, round_elapsed: Duration) -> Option<String>;
}

/// Logic for round rule selection, roles, end conditions, and objectives.
///
/// A round rule is data such as a prototype or an entity with components, as in SS14.
/// This trait runs the rule logic. Extensions register it through `RegisterContext::game_rule`
/// in `crucible-ext`. [`id`](GameRule::id) returns the rule identifier.
pub trait GameRule: Send {
    /// Stable identifier.
    fn id(&self) -> &str;

    /// Minimum number of ready players.
    fn min_players(&self) -> usize {
        0
    }

    /// Whether the rule is eligible. Returns `Err(reason)` if it is ineligible.
    fn check_eligible(&self, preparation: &RoundPreparation) -> Result<(), String> {
        let need = self.min_players();
        let have = preparation.players.len();
        if have < need {
            Err(format!(
                "requires at least {need} ready players, got {have}"
            ))
        } else {
            Ok(())
        }
    }

    /// Assigns roles to participants. The default assigns [`DEFAULT_ROLE`] to everyone.
    fn assign_roles(
        &mut self,
        world: &mut World,
        players: &[NetUserId],
    ) -> Vec<(NetUserId, String)> {
        let _ = world;
        players
            .iter()
            .map(|p| (p.clone(), DEFAULT_ROLE.to_owned()))
            .collect()
    }

    /// Called after the round starts and characters are created.
    fn started(&mut self, world: &mut World, participants: &[Participant]) {
        let _ = (world, participants);
    }

    /// Rule-specific end condition, evaluated every round tick.
    fn check_end(&mut self, world: &World) -> Option<String> {
        let _ = world;
        None
    }

    /// Whether the normal end policy may end the round in its current state.
    fn allows_end_policy(&self, _world: &World) -> bool {
        true
    }

    /// Objective outcomes for the summary.
    fn objectives(&self, world: &World, participants: &[Participant]) -> Vec<ObjectiveOutcome> {
        let _ = (world, participants);
        Vec::new()
    }
}

/// Selects an eligible rule. [`FirstEligibleRule`] chooses the first eligible rule
/// in registration order.
pub trait RuleSelector: Send {
    /// Index of a rule in `eligible`, which contains indices into the rule list, or `None`.
    fn select(&mut self, eligible: &[usize], preparation: &RoundPreparation) -> Option<usize>;
}

/// Event director called every round tick. The default does nothing.
pub trait EventDirector: Send {
    /// One round tick.
    fn update(&mut self, world: &mut World, frame_time: f32);
}

/// Hook that passes a round summary to campaign state or another consumer.
pub trait RoundResultsHook: Send {
    /// Receives the completed round summary.
    fn on_round_results(&mut self, summary: &RoundSummary, campaign: &mut CampaignState);
}

/// Hook that applies campaign state to round preparation.
pub trait RoundPreparationHook: Send {
    /// Extends round preparation using campaign state.
    fn prepare_round(&mut self, campaign: &CampaignState, preparation: &mut RoundPreparation);
}

/// Default map policy, which loads no maps.
pub struct NoMaps;
impl MapSelector for NoMaps {
    fn load_maps(&mut self, _: &mut World, _: &RoundPreparation) -> Result<Vec<EntityUid>, String> {
        Ok(Vec::new())
    }
}

/// Default character spawn, which creates an empty entity until a map module is installed.
pub struct EmptyEntitySpawn;
impl SpawnPolicy for EmptyEntitySpawn {
    fn spawn(&mut self, world: &mut World, _: &NetUserId, _: &str) -> Option<EntityUid> {
        Some(world.spawn())
    }
}

/// Default timer-based end policy. `None` means no time limit.
pub struct TimerRoundEnd {
    /// Round duration.
    pub duration: Option<Duration>,
}
impl RoundEndPolicy for TimerRoundEnd {
    fn check(&mut self, _: &World, round_elapsed: Duration) -> Option<String> {
        match self.duration {
            Some(limit) if round_elapsed >= limit => {
                Some(format!("round timer expired ({} s)", limit.as_secs_f32()))
            }
            _ => None,
        }
    }
}

/// Default rule selector, which chooses the first eligible rule.
pub struct FirstEligibleRule;
impl RuleSelector for FirstEligibleRule {
    fn select(&mut self, eligible: &[usize], _: &RoundPreparation) -> Option<usize> {
        eligible.first().copied()
    }
}

/// Default round rule, with no roles, rule-specific end condition, or objectives.
pub struct DefaultRule;
impl GameRule for DefaultRule {
    fn id(&self) -> &str {
        DEFAULT_RULE_ID
    }
}

/// Round extension points. `None` selects the default implementation.
#[derive(Default)]
pub struct RoundPoints {
    /// Selects maps and generates external areas.
    pub map_selector: Option<Box<dyn MapSelector>>,
    /// Player spawn policy.
    pub spawn_policy: Option<Box<dyn SpawnPolicy>>,
    /// Normal round end policy.
    pub round_end: Option<Box<dyn RoundEndPolicy>>,
    /// Round rule selector.
    pub rule_selector: Option<Box<dyn RuleSelector>>,
    /// Event director.
    pub event_director: Option<Box<dyn EventDirector>>,
    /// Transfers round summaries to campaign state or other consumers.
    pub results_hooks: Vec<Box<dyn RoundResultsHook>>,
    /// Uses campaign state to prepare the round.
    pub preparation_hooks: Vec<Box<dyn RoundPreparationHook>>,
}

/// Round settings from server configuration.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RoundConfig {
    /// Lobby timer before round start (default 60 seconds).
    pub lobby_duration: Duration,
    /// Duration used by the default end policy (default 60 minutes). `None` disables the timer.
    pub round_duration: Option<Duration>,
    /// Time to show results before returning to the lobby (default 15 seconds).
    pub post_round_duration: Duration,
}

impl Default for RoundConfig {
    fn default() -> Self {
        RoundConfig {
            lobby_duration: Duration::from_secs(60),
            round_duration: Some(Duration::from_secs(60 * 60)),
            post_round_duration: Duration::from_secs(15),
        }
    }
}

/// Lobby countdown to round start.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct LobbyCountdown {
    /// Tick when the round will start if the countdown continues or resumes now.
    pub start_tick: Tick,
    /// Whether the countdown is paused.
    pub paused: bool,
}

/// Why a late join request was rejected.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum LateJoinError {
    /// No round is in progress.
    RoundNotStarted,
    /// The player already has a character in this round.
    AlreadyInRound,
    /// The spawn policy created no character.
    SpawnFailed,
}

impl std::fmt::Display for LateJoinError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            LateJoinError::RoundNotStarted => "no round is in progress",
            LateJoinError::AlreadyInRound => "player is already in the round",
            LateJoinError::SpawnFailed => "could not create a character",
        })
    }
}

impl std::error::Error for LateJoinError {}

struct ActiveRound {
    rule: Option<usize>,
    rule_id: String,
    rule_fallback: Option<String>,
    participants: Vec<Participant>,
    maps: Vec<EntityUid>,
    started_at: Tick,
}

#[derive(Default)]
struct LobbyReady {
    ready: bool,
    manual: bool,
}

/// Drives round phases, from the lobby timer through extension-based preparation, end-condition
/// checks, summaries, and world cleanup.
///
/// [`update`](RoundFlow::update) runs once per tick after systems through
/// [`GameLoop::tick_with`](crate::GameLoop::tick_with). It reads time from [`SimTime`].
pub struct RoundFlow {
    config: RoundConfig,
    machine: RoundPhaseMachine,
    map_selector: Box<dyn MapSelector>,
    spawn_policy: Box<dyn SpawnPolicy>,
    round_end: Box<dyn RoundEndPolicy>,
    rule_selector: Box<dyn RuleSelector>,
    event_director: Option<Box<dyn EventDirector>>,
    results_hooks: Vec<Box<dyn RoundResultsHook>>,
    preparation_hooks: Vec<Box<dyn RoundPreparationHook>>,
    rules: Vec<Box<dyn GameRule>>,
    default_rule: Box<dyn GameRule>,
    players: BTreeMap<NetUserId, LobbyReady>,
    campaign: CampaignState,
    durable_campaign: Option<DurableCampaign>,
    state_since: Tick,
    active: Option<ActiveRound>,
    last_summary: Option<RoundSummary>,
    start_requested: bool,
    end_requested: Option<String>,
    /// Lobby start delay from deferrals and completed pauses.
    lobby_delay: Duration,
    /// Tick when the lobby countdown was paused.
    lobby_paused_at: Option<Tick>,
}

impl RoundFlow {
    /// Creates the round flow with `config`, extension `points`, and `rules` in registration order.
    /// Starts in the lobby.
    pub fn new(config: RoundConfig, points: RoundPoints, rules: Vec<Box<dyn GameRule>>) -> Self {
        let round_end = points.round_end.unwrap_or_else(|| {
            Box::new(TimerRoundEnd {
                duration: config.round_duration,
            })
        });
        RoundFlow {
            machine: RoundPhaseMachine::new(),
            map_selector: points.map_selector.unwrap_or_else(|| Box::new(NoMaps)),
            spawn_policy: points
                .spawn_policy
                .unwrap_or_else(|| Box::new(EmptyEntitySpawn)),
            round_end,
            rule_selector: points
                .rule_selector
                .unwrap_or_else(|| Box::new(FirstEligibleRule)),
            event_director: points.event_director,
            results_hooks: points.results_hooks,
            preparation_hooks: points.preparation_hooks,
            rules,
            default_rule: Box::new(DefaultRule),
            players: BTreeMap::new(),
            campaign: CampaignState::default(),
            durable_campaign: None,
            state_since: Tick::ZERO,
            active: None,
            last_summary: None,
            start_requested: false,
            end_requested: None,
            lobby_delay: Duration::ZERO,
            lobby_paused_at: None,
            config,
        }
    }

    /// Initial campaign state when using in-memory storage.
    pub fn with_campaign_state(mut self, campaign: CampaignState) -> Self {
        self.campaign = campaign;
        self
    }

    /// Attaches storage once in the lobby. State is read before round preparation.
    pub fn set_campaign_persistence(
        &mut self,
        persistence: Box<dyn CampaignPersistence>,
    ) -> Result<(), CampaignPersistenceError> {
        if self.phase() != RoundPhase::Lobby || self.durable_campaign.is_some() {
            return Err(CampaignPersistenceError::Rejected(
                "campaign storage can only be attached once in the lobby".to_owned(),
            ));
        }
        self.durable_campaign = Some(DurableCampaign::new(persistence));
        Ok(())
    }

    /// Current round phase.
    pub fn phase(&self) -> RoundPhase {
        self.machine.phase()
    }

    /// Current or most recent round number.
    pub fn round(&self) -> u32 {
        self.machine.round()
    }

    /// Round phase machine with rejected transition diagnostics.
    pub fn machine(&self) -> &RoundPhaseMachine {
        &self.machine
    }

    /// Direct transition request, such as from an administrator. Only the next phase in the cycle
    /// is permitted. The transition executes fully, including preparation, summary, or cleanup.
    pub fn request_transition(
        &mut self,
        world: &mut World,
        to: RoundPhase,
    ) -> Result<(), TransitionError> {
        let from = self.machine.phase();
        if !from.can_transition_to(to) {
            // The round phase machine records why the transition was rejected.
            return self.machine.transition(world, to);
        }
        match from {
            RoundPhase::Lobby => self.start_round(world),
            RoundPhase::PreRound => self.machine.transition(world, to),
            RoundPhase::InRound => {
                self.end_round(
                    world,
                    RoundEndReason {
                        source: RoundEndSource::Admin,
                        text: "administrator command".to_owned(),
                    },
                );
                Ok(())
            }
            RoundPhase::PostRound => self.leave_post_round(world),
        }
    }

    /// Adds an unready player to the lobby.
    pub fn add_player(&mut self, player: NetUserId) {
        self.players.entry(player).or_default();
    }

    /// Removes the player.
    pub fn remove_player(&mut self, player: NetUserId) {
        self.players.remove(&player);
    }

    /// Updates the player's ready state.
    pub fn set_ready(&mut self, player: NetUserId, ready: bool) {
        self.players.insert(
            player,
            LobbyReady {
                ready,
                manual: false,
            },
        );
    }

    /// Accepts normal lobby ready input. The player must confirm readiness again before the next round.
    pub fn set_manual_ready(&mut self, player: NetUserId, ready: bool) {
        self.players.insert(
            player,
            LobbyReady {
                ready,
                manual: true,
            },
        );
    }

    /// Ready players.
    pub fn ready_players(&self) -> Vec<NetUserId> {
        self.players
            .iter()
            .filter(|(_, state)| state.ready)
            .map(|(p, _)| p.clone())
            .collect()
    }

    /// Administrator command to start the round on the next tick without waiting for the lobby timer.
    pub fn request_start(&mut self) {
        self.start_requested = true;
    }

    /// Whether the player is ready.
    pub fn is_ready(&self, player: &NetUserId) -> bool {
        self.players.get(player).is_some_and(|state| state.ready)
    }

    /// Lobby players and their ready states, in identifier order.
    pub fn players(&self) -> impl Iterator<Item = (&NetUserId, bool)> {
        self.players
            .iter()
            .map(|(player, state)| (player, state.ready))
    }

    /// Round settings.
    pub fn config(&self) -> &RoundConfig {
        &self.config
    }

    /// Tick when the current round entered the In Round phase.
    pub fn round_started_at(&self) -> Option<Tick> {
        self.active.as_ref().map(|a| a.started_at)
    }

    /// Countdown to round start, or `None` outside the lobby. Time comes from [`SimTime`].
    pub fn lobby_countdown(&self, world: &World) -> Option<LobbyCountdown> {
        if self.machine.phase() != RoundPhase::Lobby {
            return None;
        }
        let time = current_time(world);
        let mut start = self
            .state_since
            .0
            .saturating_add(time.duration_to_ticks(self.config.lobby_duration + self.lobby_delay));
        if let Some(at) = self.lobby_paused_at {
            start = start.saturating_add(time.tick().0.saturating_sub(at.0));
        }
        Some(LobbyCountdown {
            start_tick: Tick(start),
            paused: self.lobby_paused_at.is_some(),
        })
    }

    /// Pauses or resumes the lobby countdown, as with an administrator command. Returns `true` if the
    /// state changed. Paused time is added to the start delay. Repeats or calls outside the lobby return `false`.
    pub fn set_lobby_paused(&mut self, world: &World, paused: bool) -> bool {
        if self.machine.phase() != RoundPhase::Lobby || self.lobby_paused_at.is_some() == paused {
            return false;
        }
        let time = current_time(world);
        match self.lobby_paused_at.take() {
            Some(at) => {
                self.lobby_delay += time.ticks_to_duration(time.tick().0.saturating_sub(at.0));
            }
            None => self.lobby_paused_at = Some(time.tick()),
        }
        true
    }

    /// Defers round start by `by`, as with an administrator command. Returns `false` outside the lobby.
    pub fn delay_start(&mut self, by: Duration) -> bool {
        if self.machine.phase() != RoundPhase::Lobby {
            return false;
        }
        self.lobby_delay += by;
        true
    }

    /// Adds a late joiner through [`SpawnPolicy::spawn_late_join`] during an active round and
    /// gives the resulting participant a role according to the round rule.
    pub fn late_join(
        &mut self,
        world: &mut World,
        player: &NetUserId,
    ) -> Result<EntityUid, LateJoinError> {
        if self.machine.phase() != RoundPhase::InRound {
            return Err(LateJoinError::RoundNotStarted);
        }
        let Some(active) = self.active.as_ref() else {
            return Err(LateJoinError::RoundNotStarted);
        };
        if active
            .participants
            .iter()
            .any(|p| &p.player == player && p.entity.is_some_and(|e| world.exists(e)))
        {
            return Err(LateJoinError::AlreadyInRound);
        }
        let rule: &mut Box<dyn GameRule> = match active.rule {
            Some(i) => &mut self.rules[i],
            None => &mut self.default_rule,
        };
        let role = rule
            .assign_roles(world, std::slice::from_ref(player))
            .into_iter()
            .next()
            .map_or_else(|| DEFAULT_ROLE.to_owned(), |(_, role)| role);
        let entity = self
            .spawn_policy
            .spawn_late_join(world, player, &role)
            .ok_or(LateJoinError::SpawnFailed)?;
        self.players.entry(player.clone()).or_default();
        if let Some(active) = self.active.as_mut() {
            active.participants.retain(|p| &p.player != player);
            active.participants.push(Participant {
                player: player.clone(),
                role,
                entity: Some(entity),
            });
        }
        Ok(entity)
    }

    /// Administrator command to end the round on the next tick.
    pub fn request_end(&mut self, reason: impl Into<String>) {
        self.end_requested = Some(reason.into());
    }

    /// Campaign state is outside the world. Changes to stored campaigns are visible after confirmation.
    pub fn campaign_state(&self) -> &CampaignState {
        &self.campaign
    }

    /// Last revision read from or confirmed by the store.
    pub fn campaign_revision(&self) -> Option<u64> {
        self.durable_campaign
            .as_ref()
            .and_then(|campaign| campaign.revision)
    }

    /// Last read or write failure, until the operation is confirmed.
    pub fn campaign_persistence_error(&self) -> Option<&CampaignPersistenceError> {
        self.durable_campaign
            .as_ref()
            .and_then(|campaign| campaign.error.as_ref())
    }

    /// Summary of the most recently completed round.
    pub fn last_summary(&self) -> Option<&RoundSummary> {
        self.last_summary.as_ref()
    }

    /// Rule identifier for the current round.
    pub fn current_rule(&self) -> Option<&str> {
        self.active.as_ref().map(|a| a.rule_id.as_str())
    }

    /// Reason the current round uses the default rule.
    pub fn rule_fallback_reason(&self) -> Option<&str> {
        self.active
            .as_ref()
            .and_then(|a| a.rule_fallback.as_deref())
    }

    /// Current round participants.
    pub fn participants(&self) -> &[Participant] {
        self.active
            .as_ref()
            .map(|a| a.participants.as_slice())
            .unwrap_or(&[])
    }

    /// Root map entities for the current round.
    pub fn maps(&self) -> &[EntityUid] {
        self.active
            .as_ref()
            .map(|a| a.maps.as_slice())
            .unwrap_or(&[])
    }

    /// Updates phase timers, checks end conditions, and runs the event director.
    pub fn update(&mut self, world: &mut World, frame_time: f32) {
        self.consume_late_admissions(world);
        let time = current_time(world);
        let in_state = time.ticks_to_duration(time.tick().0.saturating_sub(self.state_since.0));
        match self.machine.phase() {
            RoundPhase::Lobby => {
                let due = self.lobby_paused_at.is_none()
                    && in_state >= self.config.lobby_duration + self.lobby_delay
                    && self
                        .durable_campaign
                        .as_ref()
                        .is_none_or(|campaign| campaign.can_auto_attempt(time.tick()));
                if self.start_requested || due {
                    self.start_requested = false;
                    // The lobby can always advance to preparation.
                    let _ = self.start_round(world);
                }
            }
            // Preparation runs entirely inside the transition.
            RoundPhase::PreRound => {}
            RoundPhase::InRound => {
                if let Some(director) = self.event_director.as_mut() {
                    director.update(world, frame_time);
                }
                if let Some(reason) = self.check_end(world, in_state) {
                    self.end_round(world, reason);
                }
            }
            RoundPhase::PostRound => {
                self.commit_campaign(time);
                if in_state >= self.config.post_round_duration {
                    let _ = self.leave_post_round(world);
                }
            }
        }
    }

    fn consume_late_admissions(&mut self, world: &mut World) {
        let Some(admissions) = world.resource_mut::<LateAdmissions>() else {
            return;
        };
        let pending = std::mem::take(&mut admissions.0);
        if self.machine.phase() != RoundPhase::InRound {
            return;
        }
        let round = self.machine.round();
        let Some(active) = self.active.as_mut() else {
            return;
        };
        for (incoming_round, participant) in pending {
            let linked = world
                .resource::<Players>()
                .and_then(|players| players.session(&participant.player))
                .and_then(|session| session.entity());
            if incoming_round != round
                || linked != participant.entity
                || participant
                    .entity
                    .is_none_or(|body| world.terminating_or_deleted(body))
            {
                continue;
            }
            self.players.entry(participant.player.clone()).or_default();
            active
                .participants
                .retain(|entry| entry.player != participant.player);
            active.participants.push(participant);
        }
    }

    fn check_end(&mut self, world: &World, in_state: Duration) -> Option<RoundEndReason> {
        if let Some(text) = self.end_requested.take() {
            return Some(RoundEndReason {
                source: RoundEndSource::Admin,
                text,
            });
        }
        let active = self.active.as_ref()?;
        let rule = match active.rule {
            Some(i) => &mut self.rules[i],
            None => &mut self.default_rule,
        };
        if let Some(text) = rule.check_end(world) {
            return Some(RoundEndReason {
                source: RoundEndSource::Rule,
                text,
            });
        }
        if !rule.allows_end_policy(world) {
            return None;
        }
        self.round_end
            .check(world, in_state)
            .map(|text| RoundEndReason {
                source: RoundEndSource::Policy,
                text,
            })
    }

    fn enter(&mut self, world: &mut World, to: RoundPhase) -> Result<(), TransitionError> {
        self.machine.transition(world, to)?;
        self.state_since = current_time(world).tick();
        Ok(())
    }

    /// Moves from Lobby through Preparation to In Round by setting up maps, rules, roles, and characters.
    fn start_round(&mut self, world: &mut World) -> Result<(), TransitionError> {
        if let Some(campaign) = &mut self.durable_campaign {
            let Some(state) = campaign.begin_round(current_time(world)) else {
                return Err(TransitionError {
                    from: RoundPhase::Lobby,
                    to: RoundPhase::PreRound,
                });
            };
            self.campaign = state;
        }
        self.enter(world, RoundPhase::PreRound)?;
        self.lobby_delay = Duration::ZERO;
        self.lobby_paused_at = None;
        let round = self.machine.round();
        world.raise_broadcast(&mut RoundStarting { round });
        if self.abort_failed_preparation(world, round) {
            return Ok(());
        }

        let mut preparation = RoundPreparation {
            round,
            players: self.ready_players(),
            parameters: BTreeMap::new(),
        };
        for hook in &mut self.preparation_hooks {
            hook.prepare_round(&self.campaign, &mut preparation);
        }

        // 1. Maps.
        let maps = match self.map_selector.load_maps(world, &preparation) {
            Ok(maps) => maps,
            Err(error) => {
                tracing::error!(target: "crucible_loop::round", round, %error, "could not load round maps");
                Vec::new()
            }
        };

        if self.abort_failed_preparation(world, round) {
            return Ok(());
        }

        // 2. Rule.
        let mut eligible = Vec::new();
        let mut refusals = Vec::new();
        for (i, rule) in self.rules.iter().enumerate() {
            match rule.check_eligible(&preparation) {
                Ok(()) => eligible.push(i),
                Err(reason) => refusals.push(format!("'{}': {reason}", rule.id())),
            }
        }
        let chosen = self
            .rule_selector
            .select(&eligible, &preparation)
            .filter(|i| eligible.contains(i));
        let rule_fallback = match chosen {
            Some(_) => None,
            None => {
                let reason = if self.rules.is_empty() {
                    "no round rules are registered".to_owned()
                } else if refusals.is_empty() {
                    "the rule selector did not choose a rule".to_owned()
                } else {
                    format!("no eligible round rule: {}", refusals.join("; "))
                };
                tracing::warn!(
                    target: "crucible_loop::round",
                    round,
                    "{reason}; using the default rule '{}'",
                    self.default_rule.id()
                );
                Some(reason)
            }
        };
        let rule: &mut Box<dyn GameRule> = match chosen {
            Some(i) => &mut self.rules[i],
            None => &mut self.default_rule,
        };
        let rule_id = rule.id().to_owned();

        // 3. Roles.
        let roles = rule.assign_roles(world, &preparation.players);

        // 4. Characters.
        let participants = self
            .spawn_policy
            .spawn_prepared(world, &roles)
            .unwrap_or_else(|| {
                roles
                    .into_iter()
                    .map(|(player, role)| {
                        let entity = self.spawn_policy.spawn(world, &player, &role);
                        Participant {
                            player,
                            role,
                            entity,
                        }
                    })
                    .collect()
            });
        rule.started(world, &participants);

        self.active = Some(ActiveRound {
            rule: chosen,
            rule_id: rule_id.clone(),
            rule_fallback,
            participants,
            maps,
            started_at: current_time(world).tick(),
        });
        self.enter(world, RoundPhase::InRound)?;
        world.raise_broadcast(&mut RoundStarted {
            round,
            rule: rule_id,
        });
        Ok(())
    }

    fn abort_failed_preparation(&mut self, world: &mut World, round: u32) -> bool {
        let Some(failure) = world
            .remove_resource::<RoundPreparationFailure>()
            .filter(|failure| failure.round == round)
        else {
            return false;
        };
        tracing::error!(target: "crucible_loop::round", round, reason = %failure.reason, "round preparation was cancelled");
        self.return_to_lobby(world);
        true
    }

    /// Moves from In Round to Results, building the summary and calling summary hooks.
    fn end_round(&mut self, world: &mut World, end_reason: RoundEndReason) {
        if self.enter(world, RoundPhase::PostRound).is_err() {
            return;
        }
        let round = self.machine.round();
        let now = current_time(world).tick();
        let summary = match self.active.as_ref() {
            Some(active) => {
                let rule = match active.rule {
                    Some(i) => &self.rules[i],
                    None => &self.default_rule,
                };
                RoundSummary {
                    round,
                    rule: active.rule_id.clone(),
                    rule_fallback: active.rule_fallback.clone(),
                    end_reason,
                    duration_ticks: now.0.saturating_sub(active.started_at.0),
                    objectives: rule.objectives(world, &active.participants),
                    participants: active.participants.clone(),
                }
            }
            None => RoundSummary {
                round,
                rule: String::new(),
                rule_fallback: None,
                end_reason,
                duration_ticks: 0,
                participants: Vec::new(),
                objectives: Vec::new(),
            },
        };
        tracing::info!(
            target: "crucible_loop::round",
            round,
            rule = %summary.rule,
            reason = %summary.end_reason.text,
            "round ended"
        );
        let mut staged = self
            .durable_campaign
            .as_ref()
            .map(|_| self.campaign.clone());
        for hook in &mut self.results_hooks {
            hook.on_round_results(&summary, staged.as_mut().unwrap_or(&mut self.campaign));
        }
        if let (Some(campaign), Some(staged)) = (&mut self.durable_campaign, staged) {
            let eligible = matches!(
                summary.end_reason.source,
                RoundEndSource::Rule | RoundEndSource::Policy
            );
            let changed = (eligible && staged != self.campaign).then_some(staged);
            campaign.stage_result(changed);
            self.commit_campaign(current_time(world));
        }
        self.last_summary = Some(summary.clone());
        world.raise_broadcast(&mut RoundEnded { summary });
    }

    fn commit_campaign(&mut self, time: SimTime) {
        if let Some(campaign) = &mut self.durable_campaign
            && let Some(confirmed) = campaign.commit_pending(time)
        {
            self.campaign = confirmed;
        }
    }

    fn leave_post_round(&mut self, world: &mut World) -> Result<(), TransitionError> {
        if self
            .durable_campaign
            .as_ref()
            .is_some_and(DurableCampaign::is_commit_pending)
        {
            return Err(TransitionError {
                from: RoundPhase::PostRound,
                to: RoundPhase::Lobby,
            });
        }
        self.return_to_lobby(world);
        Ok(())
    }

    /// Returns from Results to Lobby and clears the round world.
    fn return_to_lobby(&mut self, world: &mut World) {
        let round = self.machine.round();
        world.raise_broadcast(&mut RoundRestartCleanup { round });
        clear_world(world);
        self.active = None;
        if let Some(campaign) = &mut self.durable_campaign {
            campaign.discard_round();
        }
        // Round players return to the lobby by transitioning from in-game to connected.
        let in_game: Vec<NetUserId> = world
            .resource::<Players>()
            .map(|players| {
                players
                    .sessions()
                    .filter(|s| s.status() == SessionStatus::InGame)
                    .map(|s| s.user().clone())
                    .collect()
            })
            .unwrap_or_default();
        for user in in_game {
            if let Err(error) = PlayerManager::leave_game(world, &user) {
                tracing::warn!(target: "crucible_loop::round", %user, %error, "player could not return to the lobby");
            }
        }
        for state in self.players.values_mut().filter(|state| state.manual) {
            state.ready = false;
        }
        self.lobby_delay = Duration::ZERO;
        self.lobby_paused_at = None;
        if self.machine.phase() == RoundPhase::PreRound {
            self.machine.cancel_preparation(world);
            self.state_since = current_time(world).tick();
        } else {
            let _ = self.enter(world, RoundPhase::Lobby);
        }
    }
}

/// Removes every entity in the world, including those already queued for removal.
/// World resources and data outside the world remain available.
pub fn clear_world(world: &mut World) {
    for uid in world.entities() {
        world.delete(uid);
    }
    world.end_tick();
    // Removal handlers may spawn entities. Remove those too.
    for uid in world.entities() {
        world.delete(uid);
    }
}

fn current_time(world: &World) -> SimTime {
    *world
        .resource::<SimTime>()
        .expect("RoundFlow requires the SimTime resource installed by GameLoop")
}
