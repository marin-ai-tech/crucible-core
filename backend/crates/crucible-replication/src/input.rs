//! Per-tick player input buffering.
//!
//! Adapters submit [`PlayerInput`] with a client tick, sub-tick offset, and sequence number.
//! Commands and network events are both [`InputPayload`] variants with a shared sequence.
//! The last processed sequence acknowledges both types, following
//! Robust's `LastProcessedInput = max(LastInput, LastMessage)`.
//!
//! Apply input in client tick and sequence order, never before its client tick. Apply
//! late input during the current tick.
//! Resending an already accepted sequence does not apply the input again.
//! Events from an ordered transport without numbers leave the acknowledgment unchanged.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;

use crucible_ecs::{EntityUid, NetEntity, Tick};
use crucible_players::{ConnectionId, NetUserId};

use crate::value::NeutralValue;

/// Button state in an input command.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ButtonState {
    /// Pressed.
    Down,
    /// Released.
    Up,
}

/// A position relative to a parent entity, such as a map, grid, or other entity.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct NetCoordinates {
    /// Parent entity.
    pub parent: NetEntity,
    /// Position in the parent's coordinate space.
    pub position: [f32; 2],
}

/// Command for an action such as movement or interaction.
///
/// Actions use names such as `"MoveUp"` and `"Use"`. The adapter handles numeric
/// IDs such as Robust's `KeyFunctionId` or CWP message IDs.
#[derive(Clone, PartialEq, Debug)]
pub struct InputCommand {
    /// Input action name.
    pub function: Arc<str>,
    /// Whether the button was pressed or released.
    pub state: ButtonState,
    /// Point at which the player was aiming.
    pub coordinates: Option<NetCoordinates>,
    /// Entity the player is interacting with.
    pub target: Option<NetEntity>,
}

/// Client network event, equivalent to `EntityEventArgs` sent by `RaiseNetworkEvent`.
#[derive(Clone, PartialEq, Debug)]
pub struct ClientEvent {
    /// Event name.
    pub name: Arc<str>,
    /// Event data.
    pub value: NeutralValue,
}

/// Transport-independent player actions.
#[derive(Clone, PartialEq, Debug)]
pub enum InputPayload {
    /// Input command.
    Command(InputCommand),
    /// Client network event.
    Event(ClientEvent),
}

/// An input submitted by a player.
#[derive(Clone, PartialEq, Debug)]
pub struct PlayerInput {
    /// Client tick associated with the input.
    pub client_tick: Tick,
    /// Fraction of the tick (`SubTick`, 0–65535) used by movement.
    pub sub_tick: u16,
    /// Sequence number shared by input commands and client events, starting at 1.
    pub sequence: u32,
    /// Action payload.
    pub payload: InputPayload,
}

/// Control assignment for which the adapter prepared the command.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct InputControl {
    pub entity: EntityUid,
    pub generation: u64,
    pub round: u32,
    /// Transitions for this button belong to the assignment until release or detachment.
    pub track_held: bool,
}

/// Input ready to apply on the current tick.
#[derive(Clone, PartialEq, Debug)]
pub struct DueInput {
    /// Player who sent the input.
    pub user: NetUserId,
    /// Connection on which the network adapter accepted the input.
    pub connection: Option<ConnectionId>,
    /// Optional assignment restriction for prepared input.
    pub control: Option<InputControl>,
    /// Input data.
    pub input: PlayerInput,
    /// Whether the client tick is earlier than the application tick.
    pub late: bool,
}

/// Result of submitting input.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InputReceipt {
    /// Input was buffered.
    Queued,
    /// The sequence was already processed or buffered. A repeat is not applied twice.
    Duplicate,
}

#[derive(Default)]
struct UserInputs {
    connection: Option<ConnectionId>,
    queue: BTreeMap<(Tick, u32), (PlayerInput, Option<InputControl>)>,
    ordered: VecDeque<PlayerInput>,
    last_processed: u32,
}

impl UserInputs {
    fn is_duplicate(&self, sequence: u32) -> bool {
        sequence <= self.last_processed
            || self.queue.keys().any(|&(_, pending)| pending == sequence)
            || self
                .ordered
                .iter()
                .any(|pending| pending.sequence == sequence)
    }
}

/// Input buffer for all players.
#[derive(Default)]
pub struct InputBuffer {
    users: HashMap<NetUserId, UserInputs>,
}

impl InputBuffer {
    /// Creates an empty buffer.
    pub fn new() -> Self {
        InputBuffer::default()
    }

    pub(crate) fn bind_connection(&mut self, user: &NetUserId, connection: ConnectionId) {
        let inputs = self.users.entry(user.clone()).or_default();
        if inputs
            .connection
            .is_some_and(|previous| previous != connection)
        {
            *inputs = UserInputs::default();
        }
        inputs.connection = Some(connection);
    }

    /// Accepts player input. A sequence already processed or buffered leaves the buffer unchanged
    /// and returns [`InputReceipt::Duplicate`].
    pub fn push(&mut self, user: &NetUserId, input: PlayerInput) -> InputReceipt {
        self.push_with_control(user, input, None)
    }

    pub(crate) fn push_with_control(
        &mut self,
        user: &NetUserId,
        input: PlayerInput,
        control: Option<InputControl>,
    ) -> InputReceipt {
        let inputs = self.users.entry(user.clone()).or_default();
        if inputs.is_duplicate(input.sequence) {
            return InputReceipt::Duplicate;
        }
        inputs
            .queue
            .insert((input.client_tick, input.sequence), (input, control));
        InputReceipt::Queued
    }

    pub(crate) fn push_ordered_event(
        &mut self,
        user: &NetUserId,
        tick: Tick,
        sequence: u32,
        event: ClientEvent,
    ) -> InputReceipt {
        let inputs = self.users.entry(user.clone()).or_default();
        if sequence != 0 && inputs.is_duplicate(sequence) {
            return InputReceipt::Duplicate;
        }
        inputs.ordered.push_back(PlayerInput {
            client_tick: tick,
            sub_tick: 0,
            sequence,
            payload: InputPayload::Event(event),
        });
        InputReceipt::Queued
    }

    /// Drains input due at `tick`, including late input with an earlier client tick.
    /// Players are sorted by ID. For each player, sequenced input is sorted by client tick and
    /// sequence number, followed by ordered-channel events in delivery order.
    /// Drained input is marked processed.
    pub fn take_due(&mut self, tick: Tick) -> Vec<DueInput> {
        let mut users: Vec<&NetUserId> = self.users.keys().collect();
        users.sort();
        let users: Vec<NetUserId> = users.into_iter().cloned().collect();
        let mut out = Vec::new();
        for user in users {
            let inputs = self
                .users
                .get_mut(&user)
                .expect("user key came from the buffer");
            let rest = inputs.queue.split_off(&(Tick(tick.0.saturating_add(1)), 0));
            let due = std::mem::replace(&mut inputs.queue, rest);
            let ready_ordered = inputs
                .ordered
                .iter()
                .take_while(|event| event.client_tick <= tick)
                .count();
            for (input, control) in due.into_values().chain(
                inputs
                    .ordered
                    .drain(..ready_ordered)
                    .map(|input| (input, None)),
            ) {
                inputs.last_processed = inputs.last_processed.max(input.sequence);
                out.push(DueInput {
                    user: user.clone(),
                    connection: inputs.connection,
                    control,
                    late: input.client_tick < tick,
                    input,
                });
            }
        }
        out
    }

    /// Sequence of the last processed input for a player, or zero if none was processed.
    pub fn last_processed(&self, user: &NetUserId) -> u32 {
        self.users.get(user).map_or(0, |u| u.last_processed)
    }

    /// Confirms a rejected input without placing it in the simulation queue.
    pub fn reject(&mut self, user: &NetUserId, sequence: u32) {
        if sequence != 0 {
            let inputs = self.users.entry(user.clone()).or_default();
            inputs.last_processed = inputs.last_processed.max(sequence);
        }
    }

    /// Number of inputs waiting for the player's tick.
    pub fn pending(&self, user: &NetUserId) -> usize {
        self.users
            .get(user)
            .map_or(0, |u| u.queue.len() + u.ordered.len())
    }

    /// Removes buffered input for a disconnected player.
    pub fn remove_user(&mut self, user: &NetUserId) {
        self.users.remove(user);
    }
}
