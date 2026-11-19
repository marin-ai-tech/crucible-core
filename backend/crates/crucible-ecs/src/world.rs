//! The world owns entities, components, resources, and the event bus.

use std::any::{Any, TypeId};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use crate::builder::WorldBuilder;
use crate::change::{FieldMask, NetworkedComponent, Tick};
use crate::component::{
    Component, ComponentAdd, ComponentError, ComponentInit, ComponentLifeStage, ComponentMeta,
    ComponentRemove, ComponentShutdown, ComponentStartup, EntityTerminating, ErasedStorage,
    SparseSet, new_storage,
};
use crate::entity::{Entities, EntityLifeStage, EntityMeta, EntityUid, NetEntity};
use crate::event::EventBus;
use crate::query::{ComponentSet, Query};
use crate::registry::{ComponentRegistration, ComponentRegistry, RegistryError};

/// Default event dispatch nesting limit.
pub const DEFAULT_MAX_EVENT_DEPTH: u32 = 64;

type QueuedEvent = Box<dyn FnOnce(&mut World) + Send + Sync>;

/// ECS world, equivalent to Robust's `EntityManager` with its event bus.
///
/// All game logic receives `&mut World`. Structural edits such as spawning an entity or adding
/// or removing a component take effect immediately and are visible to all subsequent code,
/// including nested event handlers. Entity removal is usually deferred to the end of the tick
/// ([`queue_delete`](World::queue_delete) → [`end_tick`](World::end_tick)).
pub struct World {
    entities: Entities,
    storages: Vec<Box<dyn ErasedStorage>>,
    storage_by_type: HashMap<TypeId, usize>,
    pub(crate) bus: EventBus,
    registry: ComponentRegistry,
    resources: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
    current_tick: Tick,
    event_depth: u32,
    max_event_depth: u32,
    event_depth_overflows: u64,
    deletion_queue: VecDeque<EntityUid>,
    event_queue: VecDeque<QueuedEvent>,
}

impl Default for World {
    fn default() -> Self {
        World::new()
    }
}

impl World {
    /// An empty world without subscriptions or systems. Use [`World::builder`] to add those.
    pub fn new() -> Self {
        World {
            entities: Entities::default(),
            storages: Vec::new(),
            storage_by_type: HashMap::new(),
            bus: EventBus::default(),
            registry: ComponentRegistry::default(),
            resources: HashMap::new(),
            current_tick: Tick::ZERO,
            event_depth: 0,
            max_event_depth: DEFAULT_MAX_EVENT_DEPTH,
            event_depth_overflows: 0,
            deletion_queue: VecDeque::new(),
            event_queue: VecDeque::new(),
        }
    }

    /// Builder for a world with a component registry and systems.
    pub fn builder() -> WorldBuilder {
        WorldBuilder::new()
    }

    // ── Storage ─────────────────────────────────────────────────────────────────────────────

    pub(crate) fn storage_index<C: Component>(&mut self) -> usize {
        self.storage_index_with(TypeId::of::<C>(), new_storage::<C>)
    }

    fn storage_index_with(
        &mut self,
        type_id: TypeId,
        make: fn() -> Box<dyn ErasedStorage>,
    ) -> usize {
        if let Some(&i) = self.storage_by_type.get(&type_id) {
            return i;
        }
        let i = self.storages.len();
        self.storages.push(make());
        self.storage_by_type.insert(type_id, i);
        i
    }

    fn existing_storage<C: Component>(&self) -> Option<usize> {
        self.storage_by_type.get(&TypeId::of::<C>()).copied()
    }

    fn typed<C: Component>(&self, index: usize) -> &SparseSet<C> {
        self.storages[index]
            .as_any()
            .downcast_ref()
            .expect("storage type matches the component type")
    }

    fn typed_mut<C: Component>(&mut self, index: usize) -> &mut SparseSet<C> {
        self.storages[index]
            .as_any_mut()
            .downcast_mut()
            .expect("storage type matches the component type")
    }

    pub(crate) fn storage_contains(&self, index: usize, uid: EntityUid) -> bool {
        self.storages[index].contains(uid)
    }

    fn stage_at(&self, index: usize, uid: EntityUid) -> Option<ComponentLifeStage> {
        self.storages[index].meta(uid).map(|m| m.stage)
    }

    fn set_stage(&mut self, index: usize, uid: EntityUid, stage: ComponentLifeStage) {
        if let Some(meta) = self.storages[index].meta_mut(uid) {
            meta.stage = stage;
        }
    }

    // ── Registry ────────────────────────────────────────────────────────────────────────────

    pub(crate) fn register_component(
        &mut self,
        registration: ComponentRegistration,
    ) -> Result<(), RegistryError> {
        let (type_id, make) = (registration.type_id(), registration.make_storage);
        self.registry.register(registration)?;
        self.storage_index_with(type_id, make);
        Ok(())
    }

    /// Component registry.
    pub fn registry(&self) -> &ComponentRegistry {
        &self.registry
    }

    // ── Entities ────────────────────────────────────────────────────────────────────────────

    /// Spawns an empty, initialized entity, equivalent to `Spawn(null)`. Components added to it
    /// are immediately added, initialized, and started.
    pub fn spawn(&mut self) -> EntityUid {
        let uid = self.spawn_uninitialized();
        self.initialize_entity(uid);
        uid
    }

    /// Spawns an uninitialized entity, equivalent to `CreateEntityUninitialized`. Components
    /// receive only `ComponentAdd` until [`initialize_entity`](World::initialize_entity) initializes
    /// and starts them. This is how entities are created from prototypes.
    pub fn spawn_uninitialized(&mut self) -> EntityUid {
        self.entities.alloc(self.current_tick)
    }

    /// Initializes and starts the entity, equivalent to `InitializeAndStartEntity`. Sends
    /// `ComponentInit` to every component in insertion order, then sends `ComponentStartup`.
    /// Returns `false` if the entity is missing or already initialized.
    pub fn initialize_entity(&mut self, uid: EntityUid) -> bool {
        match self.entities.meta_mut(uid) {
            Some(meta) if meta.life_stage == EntityLifeStage::PreInit => {
                meta.life_stage = EntityLifeStage::Initializing;
            }
            _ => return false,
        }
        let components = self.component_indices(uid);
        for &index in &components {
            self.lifecycle_step::<ComponentInit>(
                uid,
                index,
                ComponentLifeStage::Added,
                ComponentLifeStage::Initializing,
                ComponentLifeStage::Initialized,
            );
        }
        for &index in &components {
            self.lifecycle_step::<ComponentStartup>(
                uid,
                index,
                ComponentLifeStage::Initialized,
                ComponentLifeStage::Starting,
                ComponentLifeStage::Running,
            );
        }
        if let Some(meta) = self.entities.meta_mut(uid)
            && meta.life_stage == EntityLifeStage::Initializing
        {
            meta.life_stage = EntityLifeStage::Initialized;
        }
        true
    }

    /// Whether the entity exists. Stale identifiers are treated as missing.
    pub fn exists(&self, uid: EntityUid) -> bool {
        self.entities.meta(uid).is_some()
    }

    /// Entity metadata.
    pub fn entity_meta(&self, uid: EntityUid) -> Option<&EntityMeta> {
        self.entities.meta(uid)
    }

    /// Entity life stage.
    pub fn life_stage(&self, uid: EntityUid) -> Option<EntityLifeStage> {
        self.entities.meta(uid).map(EntityMeta::life_stage)
    }

    /// Records the entity's prototype identifier for the prototype loader.
    pub fn set_entity_prototype(&mut self, uid: EntityUid, prototype: Option<Arc<str>>) -> bool {
        match self.entities.meta_mut(uid) {
            Some(meta) => {
                meta.prototype = prototype;
                true
            }
            None => false,
        }
    }

    /// Number of live entities.
    pub fn entity_count(&self) -> usize {
        self.entities.len()
    }

    /// Snapshot of live entities in internal index order.
    pub fn entities(&self) -> Vec<EntityUid> {
        self.entities.iter().collect()
    }

    /// Network entity identifier, equivalent to `GetNetEntity`.
    pub fn net_entity(&self, uid: EntityUid) -> Option<NetEntity> {
        self.entities.meta(uid).map(EntityMeta::net_entity)
    }

    /// Looks up an entity by network identifier, equivalent to `TryGetEntity`. Returns `None`
    /// if the identifier is absent. Leaves world state untouched.
    pub fn entity_by_net(&self, net: NetEntity) -> Option<EntityUid> {
        self.entities.by_net(net)
    }

    /// Queues removal for the end of the tick, equivalent to `QueueDel`. Until
    /// [`end_tick`](World::end_tick), the entity still exists and is flagged for removal.
    /// Repeated requests are harmless. Returns `false` if the entity is absent.
    pub fn queue_delete(&mut self, uid: EntityUid) -> bool {
        let Some(meta) = self.entities.meta_mut(uid) else {
            return false;
        };
        if !meta.queued_for_deletion && meta.life_stage != EntityLifeStage::Terminating {
            meta.queued_for_deletion = true;
            self.deletion_queue.push_back(uid);
        }
        true
    }

    /// Whether removal is queued, equivalent to `IsQueuedForDeletion`.
    pub fn is_queued_for_deletion(&self, uid: EntityUid) -> bool {
        self.entities
            .meta(uid)
            .is_some_and(EntityMeta::is_queued_for_deletion)
    }

    /// Whether the entity is absent, queued for removal, or being removed. This is
    /// equivalent to `TerminatingOrDeleted`.
    pub fn terminating_or_deleted(&self, uid: EntityUid) -> bool {
        self.entities
            .meta(uid)
            .is_none_or(|m| m.queued_for_deletion || m.life_stage == EntityLifeStage::Terminating)
    }

    /// Removes the entity immediately, equivalent to `Del`. Raises `EntityTerminating`, stops
    /// all running components, then removes components in insertion order. During event delivery,
    /// the entity exists in the `Terminating` stage. Returns `false` if the entity is absent
    /// or is already being removed.
    pub fn delete(&mut self, uid: EntityUid) -> bool {
        match self.entities.meta_mut(uid) {
            Some(meta) if meta.life_stage != EntityLifeStage::Terminating => {
                meta.life_stage = EntityLifeStage::Terminating;
                meta.queued_for_deletion = false;
            }
            _ => return false,
        }
        self.raise(uid, &mut EntityTerminating);
        let components = self.component_indices(uid);
        for &index in &components {
            self.lifecycle_step::<ComponentShutdown>(
                uid,
                index,
                ComponentLifeStage::Running,
                ComponentLifeStage::Stopping,
                ComponentLifeStage::Stopped,
            );
        }
        for &index in &components {
            if self
                .stage_at(index, uid)
                .is_some_and(|s| s < ComponentLifeStage::Removing)
            {
                self.set_stage(index, uid, ComponentLifeStage::Removing);
                self.raise_component_event(uid, index, &mut ComponentRemove);
            }
        }
        if let Some(meta) = self.entities.meta(uid) {
            for &index in &meta.components {
                self.storages[index].remove(uid);
            }
        }
        self.entities.free(uid);
        true
    }

    fn component_indices(&self, uid: EntityUid) -> Vec<usize> {
        self.entities
            .meta(uid)
            .map(|m| m.components.clone())
            .unwrap_or_default()
    }

    fn touch_entity(&mut self, uid: EntityUid) {
        let tick = self.current_tick;
        if let Some(meta) = self.entities.meta_mut(uid) {
            meta.last_modified = tick;
        }
    }

    // ── Components ──────────────────────────────────────────────────────────────────────────

    /// Adds a component immediately, equivalent to `AddComp`. On an initialized entity, it
    /// receives `ComponentAdd`, `ComponentInit`, and `ComponentStartup`. On an uninitialized one,
    /// it receives only `ComponentAdd`. The addition marks both the component and the entity as
    /// changed in the current tick.
    pub fn add_component<C: Component>(
        &mut self,
        uid: EntityUid,
        component: C,
    ) -> Result<(), ComponentError> {
        let entity_stage = match self.entities.meta(uid) {
            None => return Err(ComponentError::EntityNotFound(uid)),
            Some(meta) => meta.life_stage,
        };
        if entity_stage == EntityLifeStage::Terminating {
            return Err(ComponentError::EntityTerminating(uid));
        }
        let index = self.storage_index::<C>();
        if self.storages[index].contains(uid) {
            return Err(ComponentError::AlreadyPresent {
                entity: uid,
                component: std::any::type_name::<C>(),
            });
        }
        let tick = self.current_tick;
        self.typed_mut::<C>(index).insert(
            uid,
            component,
            ComponentMeta::new(ComponentLifeStage::Adding, tick),
        );
        if let Some(meta) = self.entities.meta_mut(uid) {
            meta.components.push(index);
            meta.last_modified = tick;
        }

        self.raise_component_event(uid, index, &mut ComponentAdd);
        if self.stage_at(index, uid) != Some(ComponentLifeStage::Adding) {
            return Ok(());
        }
        self.set_stage(index, uid, ComponentLifeStage::Added);
        if entity_stage != EntityLifeStage::PreInit
            && self.lifecycle_step::<ComponentInit>(
                uid,
                index,
                ComponentLifeStage::Added,
                ComponentLifeStage::Initializing,
                ComponentLifeStage::Initialized,
            )
        {
            self.lifecycle_step::<ComponentStartup>(
                uid,
                index,
                ComponentLifeStage::Initialized,
                ComponentLifeStage::Starting,
                ComponentLifeStage::Running,
            );
        }
        Ok(())
    }

    /// Gets the component, adding `C::default()` if absent, equivalent to `EnsureComp`.
    /// Returns `None` if the entity is absent or being removed.
    pub fn ensure_component<C: Component + Default>(&mut self, uid: EntityUid) -> Option<&mut C> {
        if !self.has::<C>(uid) {
            self.add_component(uid, C::default()).ok()?;
        }
        self.get_mut::<C>(uid)
    }

    /// Removes the component immediately, equivalent to `RemComp`. Sends `ComponentShutdown` if
    /// the component is running, then `ComponentRemove`. The component remains accessible during
    /// these events. Returns `false` if the component is absent or already being removed.
    pub fn remove_component<C: Component>(&mut self, uid: EntityUid) -> bool {
        match self.existing_storage::<C>() {
            Some(index) => self.remove_component_at(uid, index),
            None => false,
        }
    }

    fn remove_component_at(&mut self, uid: EntityUid, index: usize) -> bool {
        let Some(stage) = self.stage_at(index, uid) else {
            return false;
        };
        if stage >= ComponentLifeStage::Stopping {
            return false;
        }
        if stage == ComponentLifeStage::Running {
            self.lifecycle_step::<ComponentShutdown>(
                uid,
                index,
                ComponentLifeStage::Running,
                ComponentLifeStage::Stopping,
                ComponentLifeStage::Stopped,
            );
        }
        if self
            .stage_at(index, uid)
            .is_some_and(|s| s < ComponentLifeStage::Removing)
        {
            self.set_stage(index, uid, ComponentLifeStage::Removing);
            self.raise_component_event(uid, index, &mut ComponentRemove);
        }
        if self.storages[index].remove(uid) {
            let tick = self.current_tick;
            if let Some(meta) = self.entities.meta_mut(uid) {
                meta.components.retain(|&i| i != index);
                meta.last_modified = tick;
            }
        }
        true
    }

    /// If the stage is `from`, transitions to `during`, sends `E` to this component's subscribers,
    /// and sets the stage to `done` if no handler changed it.
    fn lifecycle_step<E: Default + 'static>(
        &mut self,
        uid: EntityUid,
        index: usize,
        from: ComponentLifeStage,
        during: ComponentLifeStage,
        done: ComponentLifeStage,
    ) -> bool {
        if self.stage_at(index, uid) != Some(from) {
            return false;
        }
        self.set_stage(index, uid, during);
        self.raise_component_event(uid, index, &mut E::default());
        if self.stage_at(index, uid) != Some(during) {
            return false;
        }
        self.set_stage(index, uid, done);
        true
    }

    /// Whether the entity has component `C`, equivalent to `HasComp`.
    pub fn has<C: Component>(&self, uid: EntityUid) -> bool {
        self.existing_storage::<C>()
            .is_some_and(|i| self.storages[i].contains(uid))
    }

    /// The entity's component `C`, equivalent to `TryComp`.
    pub fn get<C: Component>(&self, uid: EntityUid) -> Option<&C> {
        let index = self.existing_storage::<C>()?;
        self.typed::<C>(index).get(uid)
    }

    /// Mutably borrows the entity's component `C`. To track the change, call
    /// [`dirty`](World::dirty) or use a [`networked`](crate::networked) component setter.
    pub fn get_mut<C: Component>(&mut self, uid: EntityUid) -> Option<&mut C> {
        let index = self.existing_storage::<C>()?;
        self.typed_mut::<C>(index).get_mut(uid)
    }

    /// Lifecycle stage of the entity's component `C`.
    pub fn component_stage<C: Component>(&self, uid: EntityUid) -> Option<ComponentLifeStage> {
        let index = self.existing_storage::<C>()?;
        self.stage_at(index, uid)
    }

    /// Iterates entities with the component set `Q`. See [`Query`].
    pub fn query<Q: ComponentSet>(&self) -> Query<Q> {
        let mut storages = Vec::new();
        for type_id in Q::type_ids() {
            match self.storage_by_type.get(&type_id) {
                Some(&i) => storages.push(i),
                None => return Query::new(Vec::new(), Vec::new()),
            }
        }
        let Some(&smallest) = storages.iter().min_by_key(|&&i| self.storages[i].len()) else {
            return Query::new(Vec::new(), Vec::new());
        };
        let snapshot = self.storages[smallest]
            .entities()
            .iter()
            .copied()
            .filter(|&uid| storages.iter().all(|&s| self.storages[s].contains(uid)))
            .collect();
        Query::new(snapshot, storages)
    }

    // ── Events ──────────────────────────────────────────────────────────────────────────────

    /// Raises a directed event on an entity, equivalent to `RaiseLocalEvent(uid, ref ev)`.
    ///
    /// Calls handlers synchronously in subscription order when the entity still has the relevant
    /// component at the time of dispatch. A handler receives `&mut World` and can change the world
    /// or raise nested events. Handlers write results back to the event. When the nesting limit
    /// is reached, the event is dropped and an error is logged.
    pub fn raise<E: 'static>(&mut self, uid: EntityUid, event: &mut E) {
        self.raise_filtered(uid, None, event);
    }

    /// Raises an event only for subscribers to one component, as used for lifecycle events.
    fn raise_component_event<E: 'static>(&mut self, uid: EntityUid, index: usize, event: &mut E) {
        self.raise_filtered(uid, Some(index), event);
    }

    fn raise_filtered<E: 'static>(&mut self, uid: EntityUid, only: Option<usize>, event: &mut E) {
        let Some(handlers) = self.bus.directed::<E>() else {
            return;
        };
        if !self.exists(uid) || !self.enter_raise::<E>() {
            return;
        }
        for handler in handlers.iter() {
            if only.is_some_and(|i| i != handler.storage) {
                continue;
            }
            if self.storages[handler.storage].contains(uid) {
                (handler.f)(self, uid, event);
            }
        }
        self.event_depth -= 1;
    }

    /// Raises a broadcast event, calling each subscriber of `E` exactly once in subscription
    /// order.
    pub fn raise_broadcast<E: 'static>(&mut self, event: &mut E) {
        let Some(handlers) = self.bus.broadcast::<E>() else {
            return;
        };
        if !self.enter_raise::<E>() {
            return;
        }
        for handler in handlers.iter() {
            (handler.f)(self, event);
        }
        self.event_depth -= 1;
    }

    fn enter_raise<E: 'static>(&mut self) -> bool {
        if self.event_depth >= self.max_event_depth {
            self.event_depth_overflows += 1;
            tracing::error!(
                event = std::any::type_name::<E>(),
                depth = self.event_depth,
                limit = self.max_event_depth,
                "event dispatch stopped: nesting limit exceeded"
            );
            return false;
        }
        self.event_depth += 1;
        true
    }

    /// Defers a directed event until [`end_tick`](World::end_tick), like a targeted
    /// `QueueLocalEvent`.
    pub fn queue_raise<E: Send + Sync + 'static>(&mut self, uid: EntityUid, mut event: E) {
        self.event_queue
            .push_back(Box::new(move |world: &mut World| {
                world.raise(uid, &mut event)
            }));
    }

    /// Defers a broadcast event until [`end_tick`](World::end_tick), equivalent to
    /// `QueueLocalEvent`).
    pub fn queue_broadcast<E: Send + Sync + 'static>(&mut self, mut event: E) {
        self.event_queue
            .push_back(Box::new(move |world: &mut World| {
                world.raise_broadcast(&mut event)
            }));
    }

    /// Event dispatch nesting limit.
    pub fn max_event_depth(&self) -> u32 {
        self.max_event_depth
    }

    /// Sets the event nesting limit (default 64, minimum 1).
    pub fn set_max_event_depth(&mut self, depth: u32) {
        self.max_event_depth = depth.max(1);
    }

    /// Number of event dispatches stopped by the nesting limit.
    pub fn event_depth_overflows(&self) -> u64 {
        self.event_depth_overflows
    }

    // ── Tick and change tracking ────────────────────────────────────────────────────────────

    /// Current tick.
    pub fn current_tick(&self) -> Tick {
        self.current_tick
    }

    /// Sets the current tick. The simulation loop calls this at the start of each tick.
    pub fn set_current_tick(&mut self, tick: Tick) {
        self.current_tick = tick;
    }

    /// Delivers deferred events and removes queued entities until both queues are empty.
    /// Removal handlers can add further work to either queue.
    pub fn end_tick(&mut self) {
        loop {
            if let Some(event) = self.event_queue.pop_front() {
                event(self);
            } else if let Some(uid) = self.deletion_queue.pop_front() {
                if self.is_queued_for_deletion(uid) {
                    self.delete(uid);
                }
            } else {
                break;
            }
        }
    }

    /// Marks the whole component as changed, equivalent to `Dirty`. Sets the change tick of
    /// the component and entity to the current tick and marks every field for per-field deltas.
    /// Returns `false` if the component is absent.
    pub fn dirty<C: Component>(&mut self, uid: EntityUid) -> bool {
        let tick = self.current_tick;
        let Some(meta) = self
            .existing_storage::<C>()
            .and_then(|i| self.storages[i].meta_mut(uid))
        else {
            return false;
        };
        meta.last_modified = tick;
        meta.last_full_change = tick;
        self.touch_entity(uid);
        true
    }

    /// Marks individual fields of a component with per-field deltas, equivalent to
    /// `DirtyField`, and sets the component and entity change ticks to the current tick.
    pub fn dirty_fields<C: NetworkedComponent>(
        &mut self,
        uid: EntityUid,
        fields: FieldMask,
    ) -> bool {
        let tick = self.current_tick;
        let Some(meta) = self
            .existing_storage::<C>()
            .and_then(|i| self.storages[i].meta_mut(uid))
        else {
            return false;
        };
        meta.last_modified = tick;
        for bit in fields.indices() {
            let bit = usize::from(bit);
            if meta.field_ticks.len() <= bit {
                meta.field_ticks.resize(bit + 1, Tick::ZERO);
            }
            meta.field_ticks[bit] = tick;
        }
        self.touch_entity(uid);
        true
    }

    /// Component `TypeId` values in insertion order. The iterator is empty for missing entities.
    /// This supports type-erased consumers such as replication.
    pub fn component_types(&self, uid: EntityUid) -> impl Iterator<Item = TypeId> + '_ {
        self.entities
            .meta(uid)
            .map(|m| m.components.as_slice())
            .unwrap_or_default()
            .iter()
            .map(|&index| self.storages[index].component_type())
    }

    /// Tick of the last change to the entity's component `C`.
    pub fn component_last_modified<C: Component>(&self, uid: EntityUid) -> Option<Tick> {
        let index = self.existing_storage::<C>()?;
        self.storages[index].meta(uid).map(|m| m.last_modified)
    }

    /// Tick of the last change to any component on the entity.
    pub fn entity_last_modified(&self, uid: EntityUid) -> Option<Tick> {
        self.entities.meta(uid).map(EntityMeta::last_modified)
    }

    /// Fields that changed strictly after tick `since` for a component with per-field deltas.
    /// Includes all fields if the component was added or marked wholly dirty after `since`.
    /// The mask is empty if the component is absent.
    pub fn fields_changed_since<C: NetworkedComponent>(
        &self,
        uid: EntityUid,
        since: Tick,
    ) -> FieldMask {
        let Some(meta) = self
            .existing_storage::<C>()
            .and_then(|i| self.storages[i].meta(uid))
        else {
            return FieldMask::EMPTY;
        };
        if meta.last_full_change > since {
            return C::all_fields();
        }
        let mut mask = FieldMask::EMPTY;
        for (bit, tick) in meta.field_ticks.iter().enumerate() {
            if *tick > since {
                mask |= FieldMask::bit(bit as u8);
            }
        }
        mask
    }

    // ── Resources ───────────────────────────────────────────────────────────────────────────

    /// Inserts a world resource, such as system configuration. The world keeps one instance
    /// per type and returns the previous instance on replacement.
    pub fn insert_resource<R: Send + Sync + 'static>(&mut self, resource: R) -> Option<R> {
        self.resources
            .insert(TypeId::of::<R>(), Box::new(resource))
            .map(|old| *old.downcast::<R>().expect("resource type matches the key"))
    }

    /// Resource of type `R`.
    pub fn resource<R: Send + Sync + 'static>(&self) -> Option<&R> {
        self.resources.get(&TypeId::of::<R>())?.downcast_ref()
    }

    /// Mutable resource of type `R`.
    pub fn resource_mut<R: Send + Sync + 'static>(&mut self) -> Option<&mut R> {
        self.resources.get_mut(&TypeId::of::<R>())?.downcast_mut()
    }

    /// Removes the resource of type `R` from the world.
    pub fn remove_resource<R: Send + Sync + 'static>(&mut self) -> Option<R> {
        self.resources
            .remove(&TypeId::of::<R>())
            .map(|old| *old.downcast::<R>().expect("resource type matches the key"))
    }
}
