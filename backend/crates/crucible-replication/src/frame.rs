//! State frames for individual players, including snapshots, deltas, and acknowledgments.
//!

use std::sync::Arc;

use crucible_ecs::{FieldMask, NetEntity, Tick};

use crate::value::NeutralValue;

/// Component name, transport-independent value, and field selection in a frame.
#[derive(Clone, Debug, PartialEq)]
pub struct ComponentState {
    /// Component name as registered in the ECS and YAML.
    pub name: Arc<str>,
    /// Tick of the component's last change.
    pub last_modified: Tick,
    /// `None` includes the entire component, such as in full state or for a component
    /// without field deltas. `Some(mask)` includes only the selected fields.
    pub fields: Option<FieldMask>,
    /// The value is usually a [`NeutralValue::Struct`] with named fields.
    /// Player frames share it through an `Arc`.
    pub value: Arc<NeutralValue>,
}

/// State of an entity in a frame.
#[derive(Clone, Debug, PartialEq)]
pub struct EntityState {
    /// Network entity ID.
    pub entity: NetEntity,
    /// Tick of the last change to this entity or any of its components.
    pub last_modified: Tick,
    /// The player has not acknowledged the entity entering view or requested full state.
    /// Include the complete state of every component visible to that player.
    pub full: bool,
    /// The prototype appears only in full state so a client can create the entity.
    pub prototype: Option<Arc<str>>,
    /// Changed components, or every visible component in full state.
    pub components: Vec<ComponentState>,
    /// Complete list of components visible to the player. Included in full states and in deltas
    /// where a component was removed or became hidden after the acknowledged tick.
    /// The client removes any of its components absent from this list.
    pub component_set: Option<Vec<Arc<str>>>,
}

impl EntityState {
    /// Looks up the component state by name.
    pub fn component(&self, name: &str) -> Option<&ComponentState> {
        self.components.iter().find(|c| c.name.as_ref() == name)
    }
}

/// State frame for one player on one tick.
///
/// An adapter encodes the frame for its transport. Robust uses `GameState`, mapping
/// `from_tick` to `FromSequence`, `to_tick` to `ToSequence`, and `left_view` to `MsgStateLeavePvs`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlayerStateFrame {
    /// Delta baseline: the last tick acknowledged by the player.
    /// [`Tick::ZERO`] indicates full state.
    pub from_tick: Tick,
    /// Frame tick, which is the current server tick.
    pub to_tick: Tick,
    /// Sequence number of the last processed player input. Commands and client events
    /// share one sequence. Zero means that no input has been processed.
    pub last_processed_input: u32,
    /// Changed entities and entities entering view, ordered by increasing network ID.
    pub entities: Vec<EntityState>,
    /// Previously sent entities that were deleted. Repeat until the player acknowledges
    /// the removal tick.
    pub deletions: Vec<NetEntity>,
    /// Entities that left view and are no longer being replicated to this player.
    /// Repeat until the player acknowledges the exit tick. Repeated notices are harmless.
    pub left_view: Vec<NetEntity>,
    /// Requests reliable delivery when acknowledgments lag past the threshold or full state is requested.
    /// An adapter can acknowledge the tick as soon as it delivers a reliable frame
    /// (see [`Replication::acknowledge`](crate::Replication::acknowledge)), as Robust does.
    pub reliable: bool,
}

impl PlayerStateFrame {
    /// Whether this frame contains full state (`from_tick == 0`).
    pub fn is_full(&self) -> bool {
        self.from_tick == Tick::ZERO
    }

    /// Looks up an entity state by network ID.
    pub fn entity(&self, net: NetEntity) -> Option<&EntityState> {
        self.entities.iter().find(|e| e.entity == net)
    }

    /// Whether the frame has no entity changes, removals, or departures from view.
    pub fn is_empty(&self) -> bool {
        self.entities.is_empty() && self.deletions.is_empty() && self.left_view.is_empty()
    }
}
