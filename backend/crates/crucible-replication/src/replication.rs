//! Replication model for recipients, acknowledgments, visibility budgets, and frame assembly.
//! It tracks snapshots and deltas separately for each recipient.
//!
//! Implemented from the behavior of the Robust potential visibility set (`PvsSystem.*`).
//!
//! # How players learn about an entity
//!
//! For each player, the model records visible entities and the tick each entered visibility
//! (`entered`). Until the player acknowledges a tick at or after `entered`, every frame carries
//! the entity's full state, so a lost frame does not interrupt replication. Later frames carry
//! only components changed since the last acknowledged tick (`from_tick`). Component states
//! use absolute values, which makes redelivery of a change safe.
//!
//! Visibility exits and deletions are repeated until the player acknowledges their tick.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use crucible_ecs::{EntityUid, NetEntity, NetworkPolicy, Tick, World};
use crucible_loop::CurrentRoundPhase;
use crucible_players::{ConnectionId, NetUserId, Players, SessionStatus};

use crate::components::{EntryIndex, ReplicatedComponents, StateContext};
use crate::events::{EventTarget, NetEvents, OutgoingEvent};
use crate::frame::{ComponentState, EntityState, PlayerStateFrame};
use crate::input::{ClientEvent, DueInput, InputBuffer, InputControl, InputReceipt, PlayerInput};
use crate::value::{FieldSelection, NeutralValue};
use crate::visibility::{self, AlwaysVisible, Candidate};

/// Default interest radius in meters (Robust `net.pvs_range`).
pub const DEFAULT_VIEW_RADIUS: f32 = 25.0;
/// Default budget for new entities per frame (Robust `net.pvs_budget`).
pub const DEFAULT_NEW_ENTITY_BUDGET: usize = 50;
/// Default budget for all visibility entries per frame, including reentries (Robust `net.pvs_enter_budget`).
pub const DEFAULT_ENTER_BUDGET: usize = 200;
/// Default threshold for acknowledgment lag in ticks (Robust `net.force_ack_threshold`).
pub const DEFAULT_FORCE_ACK_THRESHOLD: u32 = 60;

/// Replication configuration. Defaults follow Robust v294.0.3.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReplicationConfig {
    /// Interest radius around the controlled entity in meters.
    pub view_radius: f32,
    /// Maximum number of entities appearing in a player's view for the first time
    /// in one frame.
    pub new_entity_budget: usize,
    /// Maximum number of new and returning entities that can enter view in one frame.
    pub enter_budget: usize,
    /// Mark the frame for reliable delivery when the current tick exceeds the last
    /// acknowledged tick by this threshold.
    pub force_ack_threshold: u32,
}

impl Default for ReplicationConfig {
    fn default() -> Self {
        ReplicationConfig {
            view_radius: DEFAULT_VIEW_RADIUS,
            new_entity_budget: DEFAULT_NEW_ENTITY_BUDGET,
            enter_budget: DEFAULT_ENTER_BUDGET,
            force_ack_threshold: DEFAULT_FORCE_ACK_THRESHOLD,
        }
    }
}

/// Session assignment at the instant the neutral frame was produced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UpdateAuthority {
    pub connection: ConnectionId,
    pub generation: u64,
    pub round: u32,
}

/// A player's tick output with the state frame and reliable events in send order.
#[derive(Clone, Debug, PartialEq)]
pub struct PlayerUpdate {
    /// Player.
    pub user: NetUserId,
    /// Entity controlled by the player when this frame was built.
    pub controlled: Option<NetEntity>,
    /// State frame.
    pub frame: PlayerStateFrame,
    /// Events queued before this frame.
    pub events: Vec<OutgoingEvent>,
    authority: Option<UpdateAuthority>,
}

impl PlayerUpdate {
    /// Available when a frame is built for a live player who controls this viewer's body.
    pub fn authority(&self) -> Option<UpdateAuthority> {
        self.authority
    }
}

#[derive(Clone, Copy, Debug)]
struct VisibleEntity {
    net: NetEntity,
    /// Tick when the entity became visible or started requiring a full state.
    entered: Tick,
    /// Whether the player controlled the entity, which determines owner-only components.
    owner: bool,
}

#[derive(Default)]
struct Viewer {
    controlled: Option<EntityUid>,
    last_acked: Tick,
    /// A full-state request invalidates acknowledgments for ticks at or below this tick.
    ack_floor: Tick,
    full_pending: bool,
    last_sent: Tick,
    visible: HashMap<EntityUid, VisibleEntity>,
    ever_sent: HashMap<EntityUid, NetEntity>,
    pending_leaves: HashMap<EntityUid, (NetEntity, Tick)>,
    pending_deletions: Vec<(NetEntity, Tick)>,
    event_sequence: u64,
}

struct Tracked {
    present: Vec<EntryIndex>,
    last_removed: Tick,
}

/// Protocol-neutral replication model.
///
/// Each recipient is a player ([`NetUserId`]). A protocol adapter or virtual client
/// registers viewers with [`add_viewer`](Self::add_viewer) or
/// [`sync_with_players`](Self::sync_with_players) and sets their controlled entities.
/// It forwards [`receive_input`](Self::receive_input),
/// [`acknowledge`](Self::acknowledge), and full-state requests
///   ([`request_full_state`](Self::request_full_state));
/// On each tick it takes the frames and events from [`update`](Self::update) and sends them.
///
/// The simulation calls [`take_due_inputs`](Self::take_due_inputs) at the start of
/// the tick and builds frames after deleting queued entities at the end.
pub struct Replication {
    config: ReplicationConfig,
    components: ReplicatedComponents,
    inputs: InputBuffer,
    viewers: BTreeMap<NetUserId, Viewer>,
    tracked: HashMap<EntityUid, Tracked>,
    nets: HashMap<EntityUid, NetEntity>,
    last_update: Tick,
}

impl Replication {
    /// Creates a model with the given configuration and component registry.
    pub fn new(config: ReplicationConfig, components: ReplicatedComponents) -> Self {
        Replication {
            config,
            components,
            inputs: InputBuffer::new(),
            viewers: BTreeMap::new(),
            tracked: HashMap::new(),
            nets: HashMap::new(),
            last_update: Tick::ZERO,
        }
    }

    /// Current configuration.
    pub fn config(&self) -> &ReplicationConfig {
        &self.config
    }

    /// Mutable configuration. Changes take effect with the next frame.
    pub fn config_mut(&mut self) -> &mut ReplicationConfig {
        &mut self.config
    }

    /// Replicated component registry.
    pub fn components(&self) -> &ReplicatedComponents {
        &self.components
    }

    /// Mutable registry for registering more replicated components.
    pub fn components_mut(&mut self) -> &mut ReplicatedComponents {
        &mut self.components
    }

    // Viewers

    /// Adds a viewer whose first frame contains full state. Returns `false` if already present.
    pub fn add_viewer(&mut self, user: NetUserId) -> bool {
        if self.viewers.contains_key(&user) {
            return false;
        }
        self.viewers.insert(user, Viewer::default());
        true
    }

    /// Removes a viewer and their input. Returns `false` if absent.
    pub fn remove_viewer(&mut self, user: &NetUserId) -> bool {
        self.inputs.remove_user(user);
        self.viewers.remove(user).is_some()
    }

    /// Returns whether the viewer is registered.
    pub fn has_viewer(&self, user: &NetUserId) -> bool {
        self.viewers.contains_key(user)
    }

    /// Iterates over viewers in ascending ID order.
    pub fn viewers(&self) -> impl Iterator<Item = &NetUserId> {
        self.viewers.keys()
    }

    /// Sets the entity that determines the player's viewpoint, owner-only components, and equipment.
    /// Returns `false` if the viewer is absent.
    pub fn set_controlled_entity(&mut self, user: &NetUserId, entity: Option<EntityUid>) -> bool {
        match self.viewers.get_mut(user) {
            Some(v) => {
                v.controlled = entity;
                true
            }
            None => false,
        }
    }

    /// The player's controlled entity.
    pub fn controlled_entity(&self, user: &NetUserId) -> Option<EntityUid> {
        self.viewers.get(user).and_then(|v| v.controlled)
    }

    /// Syncs viewers with `Connected` or `InGame` sessions and uses each session's attached
    /// entity as the controlled entity. Other viewers are reset so a reconnect begins with
    /// full state.
    pub fn sync_with_players(&mut self, players: &Players) {
        let mut wanted: HashMap<NetUserId, Option<EntityUid>> = HashMap::new();
        for session in players.sessions() {
            if matches!(
                session.status(),
                SessionStatus::Connected | SessionStatus::InGame
            ) {
                self.set_input_connection(session.user(), session.connection());
                wanted.insert(session.user().clone(), session.entity());
            }
        }
        let gone: Vec<NetUserId> = self
            .viewers
            .keys()
            .filter(|u| !wanted.contains_key(*u))
            .cloned()
            .collect();
        for user in gone {
            self.remove_viewer(&user);
        }
        for (user, entity) in wanted {
            self.add_viewer(user.clone());
            self.set_controlled_entity(&user, entity);
        }
    }

    // Acknowledgments

    /// Accepts the player's acknowledgment of a frame that has already been built for `tick`.
    /// The tick must be newer than the last acknowledgment and any full-state request.
    /// Returns whether it accepted the acknowledgment. Repeats are harmless.
    pub fn acknowledge(&mut self, user: &NetUserId, tick: Tick) -> bool {
        let Some(v) = self.viewers.get_mut(user) else {
            return false;
        };
        if tick <= v.last_acked || tick <= v.ack_floor || tick > v.last_sent {
            return false;
        }
        v.last_acked = tick;
        v.full_pending = false;
        true
    }

    /// Last tick acknowledged by the player, or [`Tick::ZERO`].
    pub fn last_acked(&self, user: &NetUserId) -> Option<Tick> {
        self.viewers.get(user).map(|v| v.last_acked)
    }

    /// Requests a reliably delivered full state in the next frame (`from_tick == 0`).
    /// Invalidates acknowledgments for previous frames.
    pub fn request_full_state(&mut self, user: &NetUserId) -> bool {
        let Some(v) = self.viewers.get_mut(user) else {
            return false;
        };
        v.full_pending = true;
        v.ack_floor = v.last_sent;
        v.last_acked = Tick::ZERO;
        v.visible.clear();
        v.pending_leaves.clear();
        true
    }

    // Input

    /// Binds input to a connection authenticated by the adapter. Reconnecting resets
    /// the sequence and clears pending commands from the previous connection.
    pub fn set_input_connection(&mut self, user: &NetUserId, connection: ConnectionId) {
        self.inputs.bind_connection(user, connection);
    }

    /// Buffers player input.
    pub fn receive_input(&mut self, user: &NetUserId, input: PlayerInput) -> InputReceipt {
        self.inputs.push(user, input)
    }

    /// Buffers prepared input whose target is checked during execution.
    pub fn receive_bound_input(
        &mut self,
        user: &NetUserId,
        input: PlayerInput,
        control: InputControl,
    ) -> InputReceipt {
        self.inputs.push_with_control(user, input, Some(control))
    }

    /// Queues an event from a single reliable ordered channel to run in the input phase.
    /// Positive sequences advance the common player input acknowledgement. Sequence zero uses
    /// transport deduplication and leaves the acknowledgement unchanged.
    pub fn receive_ordered_event(
        &mut self,
        user: &NetUserId,
        tick: Tick,
        sequence: u32,
        event: ClientEvent,
    ) -> InputReceipt {
        self.inputs.push_ordered_event(user, tick, sequence, event)
    }

    /// Confirms an input rejected by the protocol adapter without applying it.
    pub fn reject_input(&mut self, user: &NetUserId, sequence: u32) {
        self.inputs.reject(user, sequence);
    }

    /// Takes input due for `tick`, before other tick processing.
    pub fn take_due_inputs(&mut self, tick: Tick) -> Vec<DueInput> {
        self.inputs.take_due(tick)
    }

    /// Input buffer.
    pub fn inputs(&self) -> &InputBuffer {
        &self.inputs
    }

    // Visibility

    /// Whether the player saw the entity in their most recent frame.
    pub fn is_visible(&self, user: &NetUserId, uid: EntityUid) -> bool {
        self.viewers
            .get(user)
            .is_some_and(|v| v.visible.contains_key(&uid))
    }

    /// Entities visible to the player in their most recent frame.
    pub fn visible_entities(&self, user: &NetUserId) -> Vec<EntityUid> {
        let mut out: Vec<EntityUid> = self
            .viewers
            .get(user)
            .map(|v| v.visible.keys().copied().collect())
            .unwrap_or_default();
        out.sort();
        out
    }

    /// Complete neutral component states from this tick's admitted player view.
    ///
    /// Unlike a transport delta, this doesn’t depend on acknowledgement history. Callers
    /// can narrow visibility before serialization. Closed foreign contents stay hidden.
    pub fn visible_snapshot(
        &self,
        world: &World,
        user: &NetUserId,
        mut include: impl FnMut(EntityUid) -> bool,
        include_component: impl Fn(&str) -> bool,
    ) -> Option<Vec<(EntityUid, EntityState)>> {
        if self.last_update != world.current_tick() {
            return None;
        }
        let viewer = self.viewers.get(user)?;
        if viewer.last_sent != self.last_update {
            return None;
        }
        let mut cache = HashMap::new();
        let mut states = Vec::new();
        for uid in self.visible_entities(user) {
            if world.terminating_or_deleted(uid)
                || visibility::hidden_from(world, uid, viewer.controlled)
                || !include(uid)
            {
                continue;
            }
            let entry = viewer.visible[&uid];
            let owner = viewer.controlled == Some(uid);
            if let Some(state) = self.entity_state(
                world,
                uid,
                entry,
                owner,
                true,
                Tick::ZERO,
                &mut cache,
                &include_component,
            ) {
                states.push((uid, state));
            }
        }
        Some(states)
    }

    // Frame assembly

    /// Builds a frame for each viewer on the current world tick and distributes queued
    /// [`NetEvents`]. Call at the end of the tick.
    pub fn update(&mut self, world: &mut World) -> Vec<PlayerUpdate> {
        let events = world
            .resource_mut::<NetEvents>()
            .map(NetEvents::drain)
            .unwrap_or_default();
        let frames = self.build_frames(world);
        let mut out: Vec<PlayerUpdate> = frames
            .into_iter()
            .map(|(user, frame)| {
                let controlled = self.controlled_entity(&user);
                let authority = world.resource::<CurrentRoundPhase>().and_then(|phase| {
                    let session = world.resource::<Players>()?.session(&user)?;
                    if session.status() != SessionStatus::InGame
                        || controlled.is_none()
                        || session.entity() != controlled
                    {
                        return None;
                    }
                    Some(UpdateAuthority {
                        connection: session.connection(),
                        generation: session.control_generation(),
                        round: phase.round,
                    })
                });
                PlayerUpdate {
                    controlled: controlled.and_then(|entity| world.net_entity(entity)),
                    user,
                    frame,
                    events: Vec::new(),
                    authority,
                }
            })
            .collect();
        let index: HashMap<NetUserId, usize> = out
            .iter()
            .enumerate()
            .map(|(i, u)| (u.user.clone(), i))
            .collect();
        for (target, event) in events {
            let recipients: Vec<NetUserId> = match &target {
                EventTarget::Player(user) => {
                    if self.viewers.contains_key(user) {
                        vec![user.clone()]
                    } else {
                        tracing::debug!(%user, "discarded event for an unknown recipient");
                        Vec::new()
                    }
                }
                EventTarget::ViewersOf(uid) => self
                    .viewers
                    .iter()
                    .filter(|(_, v)| v.visible.contains_key(uid))
                    .map(|(u, _)| u.clone())
                    .collect(),
                EventTarget::AllPlayers => self.viewers.keys().cloned().collect(),
            };
            let audience_entity = match target {
                EventTarget::ViewersOf(uid) => world.net_entity(uid),
                EventTarget::Player(_) | EventTarget::AllPlayers => None,
            };
            for user in recipients {
                let v = self.viewers.get_mut(&user).expect("viewer exists");
                v.event_sequence += 1;
                out[index[&user]].events.push(OutgoingEvent {
                    sequence: v.event_sequence,
                    event: Arc::clone(&event),
                    audience_entity,
                });
            }
        }
        out
    }

    /// Builds a frame for each viewer on the current world tick, without events.
    pub fn build_frames(&mut self, world: &World) -> Vec<(NetUserId, PlayerStateFrame)> {
        let tick = world.current_tick();
        let deleted = self.track_entities(world, tick);

        let mut always: Vec<EntityUid> = world.query::<AlwaysVisible>().into_snapshot();
        always.sort();

        let mut cache: HashMap<(EntityUid, EntryIndex, FieldSelection, Tick), Arc<NeutralValue>> =
            HashMap::new();
        let users: Vec<NetUserId> = self.viewers.keys().cloned().collect();
        let mut out = Vec::with_capacity(users.len());
        for user in users {
            let last_input = self.inputs.last_processed(&user);
            let mut viewer = self.viewers.remove(&user).expect("viewer key from list");
            let frame = self.build_frame(
                world,
                tick,
                &mut viewer,
                &deleted,
                &always,
                last_input,
                &mut cache,
            );
            self.viewers.insert(user.clone(), viewer);
            out.push((user, frame));
        }
        self.last_update = tick;
        out
    }

    /// Updates component tracking for removals and returns the entities deleted since
    /// the previous frame.
    fn track_entities(&mut self, world: &World, tick: Tick) -> Vec<(EntityUid, NetEntity)> {
        let mut deleted = Vec::new();
        self.tracked.retain(|&uid, _| world.exists(uid));
        self.nets.retain(|&uid, &mut net| {
            let alive = world.exists(uid);
            if !alive {
                deleted.push((uid, net));
            }
            alive
        });
        for uid in world.entities() {
            let modified = world.entity_last_modified(uid).unwrap_or(Tick::ZERO);
            match self.tracked.get_mut(&uid) {
                Some(_) if modified <= self.last_update => {}
                Some(t) => {
                    let present = self.components.present_on(world, uid);
                    if t.present.iter().any(|i| present.binary_search(i).is_err()) {
                        t.last_removed = tick;
                    }
                    t.present = present;
                }
                None => {
                    let present = self.components.present_on(world, uid);
                    self.tracked.insert(
                        uid,
                        Tracked {
                            present,
                            last_removed: Tick::ZERO,
                        },
                    );
                }
            }
        }
        deleted.sort_by_key(|&(_, net)| net);
        deleted
    }

    #[allow(clippy::too_many_arguments)]
    fn build_frame(
        &mut self,
        world: &World,
        tick: Tick,
        v: &mut Viewer,
        deleted: &[(EntityUid, NetEntity)],
        always: &[EntityUid],
        last_input: u32,
        cache: &mut HashMap<(EntityUid, EntryIndex, FieldSelection, Tick), Arc<NeutralValue>>,
    ) -> PlayerStateFrame {
        // Send deletions only for entities that were sent to this player.
        for &(uid, net) in deleted {
            if v.ever_sent.remove(&uid).is_some() {
                v.visible.remove(&uid);
                v.pending_leaves.remove(&uid);
                v.pending_deletions.push((net, tick));
            }
        }

        // Visibility and budget.
        let desired =
            visibility::desired_view(world, v.controlled, always, self.config.view_radius);
        let mut next: HashMap<EntityUid, VisibleEntity> = HashMap::with_capacity(desired.len());
        for (uid, entry) in &v.visible {
            if desired.contains_key(uid) {
                next.insert(*uid, *entry);
            }
        }
        let mut order: Vec<(EntityUid, Candidate, NetEntity)> = desired
            .iter()
            .filter(|(uid, _)| !next.contains_key(uid))
            .filter_map(|(&uid, &c)| world.net_entity(uid).map(|n| (uid, c, n)))
            .collect();
        order.sort_by(|a, b| {
            b.1.forced
                .cmp(&a.1.forced)
                .then(a.1.dist2.total_cmp(&b.1.dist2))
                .then(a.2.cmp(&b.2))
        });
        let (mut entered, mut entered_new) = (0usize, 0usize);
        for (uid, cand, _) in order {
            if next.contains_key(&uid) {
                continue; // Already included as an ancestor of another entity.
            }
            let mut chain: Vec<EntityUid> = visibility::ancestors_root_first(world, uid)
                .into_iter()
                .filter(|a| !next.contains_key(a))
                .collect();
            chain.push(uid);
            let fresh = chain
                .iter()
                .filter(|e| !v.ever_sent.contains_key(e))
                .count();
            if !cand.forced
                && (entered + chain.len() > self.config.enter_budget
                    || entered_new + fresh > self.config.new_entity_budget)
            {
                continue; // Include this entity in a later frame.
            }
            // The controlled entity and its ancestors always enter view and still use the budget.
            entered += chain.len();
            entered_new += fresh;
            for e in chain {
                let Some(net) = world.net_entity(e) else {
                    continue;
                };
                v.pending_leaves.remove(&e);
                v.ever_sent.insert(e, net);
                self.nets.insert(e, net);
                next.insert(
                    e,
                    VisibleEntity {
                        net,
                        entered: tick,
                        owner: v.controlled == Some(e),
                    },
                );
            }
        }

        // Entities leaving visibility.
        for (uid, entry) in &v.visible {
            if !next.contains_key(uid) {
                v.pending_leaves.insert(*uid, (entry.net, tick));
            }
        }
        v.visible = next;

        // Stop repeating acknowledged entries.
        let acked = v.last_acked;
        v.pending_leaves.retain(|_, (_, t)| *t > acked);
        v.pending_deletions.retain(|(_, t)| *t > acked);

        let from = if v.full_pending {
            Tick::ZERO
        } else {
            v.last_acked
        };
        let mut entities = Vec::new();
        let mut visible: Vec<(EntityUid, VisibleEntity)> =
            v.visible.iter().map(|(&u, &e)| (u, e)).collect();
        visible.sort_by_key(|(_, e)| e.net);
        for (uid, mut entry) in visible {
            let owner = v.controlled == Some(uid);
            if entry.owner != owner {
                // Ownership changes which owner-only components are present, so send full state.
                entry.owner = owner;
                entry.entered = tick;
                v.visible.insert(uid, entry);
            }
            let full = v.full_pending || entry.entered > v.last_acked;
            if let Some(state) =
                self.entity_state(world, uid, entry, owner, full, from, cache, |_| true)
            {
                entities.push(state);
            }
        }

        let mut left_view: Vec<NetEntity> = v.pending_leaves.values().map(|(n, _)| *n).collect();
        left_view.sort();
        let mut deletions: Vec<NetEntity> = v.pending_deletions.iter().map(|(n, _)| *n).collect();
        deletions.sort();
        deletions.dedup();

        let reliable = v.full_pending
            || tick.0
                > v.last_acked
                    .0
                    .saturating_add(self.config.force_ack_threshold);
        v.last_sent = tick;
        PlayerStateFrame {
            from_tick: from,
            to_tick: tick,
            last_processed_input: last_input,
            entities,
            deletions,
            left_view,
            reliable,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn entity_state(
        &self,
        world: &World,
        uid: EntityUid,
        entry: VisibleEntity,
        owner: bool,
        full: bool,
        from: Tick,
        cache: &mut HashMap<(EntityUid, EntryIndex, FieldSelection, Tick), Arc<NeutralValue>>,
        include_component: impl Fn(&str) -> bool,
    ) -> Option<EntityState> {
        let last_modified = world.entity_last_modified(uid)?;
        if !full && last_modified <= from {
            return None;
        }
        let present = self
            .tracked
            .get(&uid)
            .map(|t| t.present.clone())
            .unwrap_or_else(|| self.components.present_on(world, uid));
        let allowed = |index: EntryIndex| match self.components.entry(index).policy {
            NetworkPolicy::AllViewers => true,
            NetworkPolicy::OwnerOnly => owner,
            NetworkPolicy::ServerOnly => false,
        };
        let since = if full { Tick::ZERO } else { from };
        let mut components = Vec::new();
        let mut set = Vec::new();
        for index in present.into_iter().filter(|&i| allowed(i)) {
            let rep = self.components.entry(index);
            if !include_component(&rep.name) {
                continue;
            }
            set.push(Arc::clone(&rep.name));
            let Some(lm) = rep.last_modified(world, uid) else {
                continue;
            };
            let selection = if full {
                FieldSelection::All
            } else if lm > from {
                rep.delta_selection(world, uid, from)
            } else {
                continue;
            };
            if let FieldSelection::Fields(mask) = selection
                && mask.is_empty()
            {
                continue;
            }
            let key = (uid, index, selection, since);
            let value = match cache.get(&key) {
                Some(v) => Arc::clone(v),
                None => {
                    let ctx = StateContext::new(world, uid, since);
                    let Some(value) = rep.serialize(&ctx, uid, selection) else {
                        continue;
                    };
                    let value = Arc::new(value);
                    cache.insert(key, Arc::clone(&value));
                    value
                }
            };
            components.push(ComponentState {
                name: Arc::clone(&rep.name),
                last_modified: lm,
                fields: match selection {
                    FieldSelection::Fields(mask) if rep.has_field_deltas() => Some(mask),
                    _ => None,
                },
                value,
            });
        }
        let removed_since = self
            .tracked
            .get(&uid)
            .is_some_and(|t| t.last_removed > from);
        let component_set = (full || removed_since).then_some(set);
        if !full && components.is_empty() && component_set.is_none() {
            return None;
        }
        Some(EntityState {
            entity: entry.net,
            last_modified,
            full,
            prototype: if full {
                world
                    .entity_meta(uid)
                    .and_then(|m| m.prototype())
                    .map(Arc::from)
            } else {
                None
            },
            components,
            component_set,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_robust() {
        let c = ReplicationConfig::default();
        assert_eq!(c.view_radius, 25.0);
        assert_eq!(c.new_entity_budget, 50);
        assert_eq!(c.enter_budget, 200);
        assert_eq!(c.force_ack_threshold, 60);
    }

    #[test]
    fn unknown_viewer_operations_are_rejected() {
        let mut r = Replication::new(ReplicationConfig::default(), ReplicatedComponents::new());
        let u = NetUserId::new("nobody");
        assert!(!r.acknowledge(&u, Tick(1)));
        assert!(!r.request_full_state(&u));
        assert!(!r.set_controlled_entity(&u, None));
        assert!(!r.remove_viewer(&u));
        assert!(r.add_viewer(u.clone()));
        assert!(!r.add_viewer(u.clone()));
        assert!(
            !r.acknowledge(&u, Tick(1)),
            "tick 1 frame has not been built"
        );
    }
}
