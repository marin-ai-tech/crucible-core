//! Replicated component names, visibility policies, and transport-independent serializers.
//!
//!
//! The ECS component registry ([`ComponentRegistration`]) provides names and policies.
//! Components are registered once in the world. This registry adds only serializers, so
//! each adapter sees the same component names.

use std::any::TypeId;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use crucible_ecs::{
    Component, ComponentRegistration, ComponentRegistry, EntityUid, FieldMask, NetEntity,
    NetworkPolicy, NetworkedComponent, Tick, World,
};

use crate::value::{FieldSelection, NeutralValue};

/// Context available to serializers alongside the component.
pub struct StateContext<'w> {
    world: &'w World,
    uid: EntityUid,
    since: Tick,
}

impl<'w> StateContext<'w> {
    pub(crate) fn new(world: &'w World, uid: EntityUid, since: Tick) -> Self {
        StateContext { world, uid, since }
    }

    /// Read-only access to the world.
    pub fn world(&self) -> &'w World {
        self.world
    }

    /// Entity whose component is being serialized.
    pub fn uid(&self) -> EntityUid {
        self.uid
    }

    /// Baseline tick for the state; [`Tick::ZERO`] requests full state. Components with
    /// their own deltas inside a field, such as grid chunks, may use `since`
    /// to include only changes newer than the baseline.
    pub fn since(&self) -> Tick {
        self.since
    }

    /// Network entity ID.
    pub fn net(&self, uid: EntityUid) -> Option<NetEntity> {
        self.world.net_entity(uid)
    }

    /// Transport-independent entity reference, or [`NeutralValue::Null`] if absent.
    pub fn entity(&self, uid: Option<EntityUid>) -> NeutralValue {
        NeutralValue::entity(uid.and_then(|u| self.world.net_entity(u)))
    }
}

/// Serializes selected fields of component `C` to a transport-independent value.
pub type StateSerializer<C> = fn(&StateContext<'_>, &C, FieldSelection) -> NeutralValue;

type ErasedSerializer =
    Box<dyn Fn(&StateContext<'_>, EntityUid, FieldSelection) -> Option<NeutralValue> + Send + Sync>;

/// Component index in [`ReplicatedComponents`].
pub(crate) type EntryIndex = u16;

pub(crate) struct ReplicatedEntry {
    pub(crate) name: Arc<str>,
    pub(crate) policy: NetworkPolicy,
    last_modified: fn(&World, EntityUid) -> Option<Tick>,
    changed_fields: Option<fn(&World, EntityUid, Tick) -> FieldMask>,
    serialize: ErasedSerializer,
}

impl ReplicatedEntry {
    pub(crate) fn last_modified(&self, world: &World, uid: EntityUid) -> Option<Tick> {
        (self.last_modified)(world, uid)
    }

    /// Field selection for a delta since `since`. `None` means the component has no
    /// per-field deltas and is sent whole.
    pub(crate) fn delta_selection(
        &self,
        world: &World,
        uid: EntityUid,
        since: Tick,
    ) -> FieldSelection {
        match self.changed_fields {
            Some(changed) => FieldSelection::Fields(changed(world, uid, since)),
            None => FieldSelection::All,
        }
    }

    pub(crate) fn has_field_deltas(&self) -> bool {
        self.changed_fields.is_some()
    }

    pub(crate) fn serialize(
        &self,
        ctx: &StateContext<'_>,
        uid: EntityUid,
        selection: FieldSelection,
    ) -> Option<NeutralValue> {
        (self.serialize)(ctx, uid, selection)
    }
}

/// Replicated component registration error.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ReplicationError {
    /// The component is missing from the ECS registry and has no name or policy.
    NotInComponentRegistry(&'static str),
    /// A serializer is already registered for the component.
    AlreadyReplicated(String),
    /// More than `u16::MAX` components are registered for replication.
    TooManyComponents,
}

impl fmt::Display for ReplicationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReplicationError::NotInComponentRegistry(t) => {
                write!(f, "type {t} is absent from the ECS component registry")
            }
            ReplicationError::AlreadyReplicated(name) => {
                write!(f, "component `{name}` already has a serializer")
            }
            ReplicationError::TooManyComponents => {
                write!(f, "too many replicated components")
            }
        }
    }
}

impl std::error::Error for ReplicationError {}

/// Registry combining ECS names and policies with replication serializers.
///
/// A component requires a serializer to appear in state. Server-only components
/// are always omitted, even if a serializer is registered.
///
#[derive(Default)]
pub struct ReplicatedComponents {
    entries: Vec<ReplicatedEntry>,
    by_type: HashMap<TypeId, EntryIndex>,
}

impl ReplicatedComponents {
    /// Creates an empty registry.
    pub fn new() -> Self {
        ReplicatedComponents::default()
    }

    /// Sets a serializer for a component without per-field deltas. Any change includes
    /// the whole component ([`FieldSelection::All`]).
    pub fn register<C: Component>(
        &mut self,
        registry: &ComponentRegistry,
        serialize: StateSerializer<C>,
    ) -> Result<(), ReplicationError> {
        self.insert::<C>(registry, None, serialize)
    }

    /// Sets a serializer for a component with per-field deltas (`#[networked]`).
    /// A delta includes only fields changed since the acknowledged tick.
    pub fn register_networked<C: NetworkedComponent>(
        &mut self,
        registry: &ComponentRegistry,
        serialize: StateSerializer<C>,
    ) -> Result<(), ReplicationError> {
        self.insert::<C>(
            registry,
            Some(|world: &World, uid, since| world.fields_changed_since::<C>(uid, since)),
            serialize,
        )
    }

    fn insert<C: Component>(
        &mut self,
        registry: &ComponentRegistry,
        changed_fields: Option<fn(&World, EntityUid, Tick) -> FieldMask>,
        serialize: StateSerializer<C>,
    ) -> Result<(), ReplicationError> {
        let registration: &ComponentRegistration =
            registry
                .by_type::<C>()
                .ok_or(ReplicationError::NotInComponentRegistry(
                    std::any::type_name::<C>(),
                ))?;
        if self.by_type.contains_key(&TypeId::of::<C>()) {
            return Err(ReplicationError::AlreadyReplicated(
                registration.name().to_owned(),
            ));
        }
        let index = EntryIndex::try_from(self.entries.len())
            .map_err(|_| ReplicationError::TooManyComponents)?;
        self.entries.push(ReplicatedEntry {
            name: Arc::from(registration.name()),
            policy: registration.network_policy(),
            last_modified: |world, uid| world.component_last_modified::<C>(uid),
            changed_fields,
            serialize: Box::new(move |ctx, uid, selection| {
                ctx.world()
                    .get::<C>(uid)
                    .map(|component| serialize(ctx, component, selection))
            }),
        });
        self.by_type.insert(TypeId::of::<C>(), index);
        Ok(())
    }

    /// Replicated component names in registration order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|e| e.name.as_ref())
    }

    /// Number of replicated components.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub(crate) fn entry(&self, index: EntryIndex) -> &ReplicatedEntry {
        &self.entries[usize::from(index)]
    }

    /// Entity's replicated component indices in ascending order, regardless of policy.
    pub(crate) fn present_on(&self, world: &World, uid: EntityUid) -> Vec<EntryIndex> {
        let mut out: Vec<EntryIndex> = world
            .component_types(uid)
            .filter_map(|t| self.by_type.get(&t).copied())
            .collect();
        out.sort_unstable();
        out
    }
}
