//! Reliable network events and recipient selection.
//!
//! Game logic adds an event to [`NetEvents`] for one player, everyone viewing an entity,
//! or all players. When [`Replication::update`](crate::Replication::update) builds frames,
//! the queue is routed in send order using visibility for that same tick.
//! Each player receives events in order, numbered consecutively.

use std::sync::Arc;

use crucible_ecs::{EntityUid, Tick, World};
use crucible_players::NetUserId;

use crate::value::NeutralValue;

/// Event recipient selection.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum EventTarget {
    /// A single player.
    Player(NetUserId),
    /// Every player who can see the entity on this tick.
    ViewersOf(EntityUid),
    /// All players.
    AllPlayers,
}

/// A transport-independent server event.
#[derive(Clone, PartialEq, Debug)]
pub struct NetworkEvent {
    /// Event name.
    pub name: Arc<str>,
    /// Event data.
    pub value: NeutralValue,
    /// The event's tick. An adapter aligns it with the state frame for that tick.
    /// The Robust client waits until it has applied the relevant state.
    /// `None` indicates that the event is independent of a tick.
    pub tick: Option<Tick>,
}

impl NetworkEvent {
    /// Creates an event independent of any tick.
    pub fn new(name: impl Into<Arc<str>>, value: NeutralValue) -> Self {
        NetworkEvent {
            name: name.into(),
            value,
            tick: None,
        }
    }

    /// Ties the event to one tick.
    pub fn at_tick(mut self, tick: Tick) -> Self {
        self.tick = Some(tick);
        self
    }
}

/// Event addressed to one player.
#[derive(Clone, PartialEq, Debug)]
pub struct OutgoingEvent {
    /// Per-player sequence number, starting at 1 without gaps. Adapters drop repeats by sequence.
    pub sequence: u64,
    /// Event shared among all recipients.
    pub event: Arc<NetworkEvent>,
    /// The relevant entity for events routed to viewers. This allows a narrower visual view to
    /// honor the same routing rule.
    pub audience_entity: Option<crucible_ecs::NetEntity>,
}

/// Server event queue, stored as a world resource.
#[derive(Default)]
pub struct NetEvents {
    queue: Vec<(EventTarget, Arc<NetworkEvent>)>,
}

impl NetEvents {
    /// Queues an event.
    pub fn send(&mut self, target: EventTarget, event: NetworkEvent) {
        self.queue.push((target, Arc::new(event)));
    }

    /// Number of queued events.
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// Whether the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    pub(crate) fn drain(&mut self) -> Vec<(EventTarget, Arc<NetworkEvent>)> {
        std::mem::take(&mut self.queue)
    }
}

/// Queues an event for the current tick, inserting the [`NetEvents`] resource if
/// the world does not already have it.
pub fn send_network_event(
    world: &mut World,
    target: EventTarget,
    name: impl Into<Arc<str>>,
    value: NeutralValue,
) {
    let tick = world.current_tick();
    if world.resource::<NetEvents>().is_none() {
        world.insert_resource(NetEvents::default());
    }
    world
        .resource_mut::<NetEvents>()
        .expect("the event resource was inserted above")
        .send(target, NetworkEvent::new(name, value).at_tick(tick));
}
