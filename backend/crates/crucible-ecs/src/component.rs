//! Components, their lifecycle and events, and sparse storage.

use std::any::{Any, TypeId};
use std::fmt;

use crate::change::Tick;
use crate::entity::EntityUid;

/// Entity data. Implement this trait for any type with `impl Component for Door {}`.
///
/// `Send + Sync` let the world move to the simulation thread and be read concurrently
/// within a single system, as required by sequential gameplay logic.
pub trait Component: Send + Sync + 'static {}

/// Component lifecycle stage, equivalent to Robust's `ComponentLifeStage`.
///
/// Stages are ordered. `stage >= ComponentLifeStage::Stopping` indicates pending removal.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum ComponentLifeStage {
    /// Delivery of [`ComponentAdd`] is in progress.
    Adding,
    /// Added and awaiting initialization. The entity is in `PreInit`.
    Added,
    /// Delivery of [`ComponentInit`] is in progress.
    Initializing,
    /// Initialized and awaiting startup.
    Initialized,
    /// Delivery of [`ComponentStartup`] is in progress.
    Starting,
    /// Running.
    Running,
    /// Delivery of [`ComponentShutdown`] is in progress.
    Stopping,
    /// Stopped and awaiting removal.
    Stopped,
    /// Delivery of [`ComponentRemove`] is in progress. Storage is cleared afterward.
    Removing,
}

/// Lifecycle event sent when a component is added to an entity. Only subscribers to this
/// component and `ComponentAdd` receive it.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct ComponentAdd;

/// Lifecycle event for component initialization.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct ComponentInit;

/// Lifecycle event for component startup.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct ComponentStartup;

/// Lifecycle event for component shutdown. The component remains accessible.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct ComponentShutdown;

/// Lifecycle event for component removal. The component remains accessible until delivery ends.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct ComponentRemove;

/// Directed event sent when an entity starts terminating, equivalent to `EntityTerminatingEvent`.
/// Subscribers to all components on the entity receive it before components are shut down.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct EntityTerminating;

/// An error changing which components an entity has.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ComponentError {
    /// The entity identifier is missing or stale.
    EntityNotFound(EntityUid),
    /// The entity already has a component of this type.
    AlreadyPresent {
        /// Entity.
        entity: EntityUid,
        /// Component type name.
        component: &'static str,
    },
    /// An entity being removed cannot take new components.
    EntityTerminating(EntityUid),
}

impl fmt::Display for ComponentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ComponentError::EntityNotFound(e) => write!(f, "entity {e} does not exist"),
            ComponentError::AlreadyPresent { entity, component } => {
                write!(f, "entity {entity} already has component {component}")
            }
            ComponentError::EntityTerminating(e) => {
                write!(f, "cannot add a component to entity {e} during removal")
            }
        }
    }
}

impl std::error::Error for ComponentError {}

/// Lifecycle and change-tracking metadata for a component instance.
#[derive(Clone, Debug)]
pub(crate) struct ComponentMeta {
    pub(crate) stage: ComponentLifeStage,
    /// Tick of the last component change.
    pub(crate) last_modified: Tick,
    /// Tick when the component was last added or marked dirty as a whole. All fields count as changed.
    pub(crate) last_full_change: Tick,
    /// Field change ticks indexed by the mask's bit positions. This collection grows on demand.
    pub(crate) field_ticks: Vec<Tick>,
}

impl ComponentMeta {
    pub(crate) fn new(stage: ComponentLifeStage, tick: Tick) -> Self {
        ComponentMeta {
            stage,
            last_modified: tick,
            last_full_change: tick,
            field_ticks: Vec::new(),
        }
    }
}

const EMPTY: u32 = u32::MAX;

/// Sparse set with dense arrays and a sparse index keyed by `EntityUid::index`.
pub(crate) struct SparseSet<C> {
    sparse: Vec<u32>,
    entities: Vec<EntityUid>,
    data: Vec<C>,
    meta: Vec<ComponentMeta>,
}

impl<C> SparseSet<C> {
    pub(crate) fn new() -> Self {
        SparseSet {
            sparse: Vec::new(),
            entities: Vec::new(),
            data: Vec::new(),
            meta: Vec::new(),
        }
    }

    /// Position in the dense arrays. Checks the generation so stale identifiers do not match.
    fn position(&self, uid: EntityUid) -> Option<usize> {
        let pos = *self.sparse.get(uid.index() as usize)?;
        if pos == EMPTY {
            return None;
        }
        let pos = pos as usize;
        (self.entities[pos] == uid).then_some(pos)
    }

    pub(crate) fn insert(&mut self, uid: EntityUid, value: C, meta: ComponentMeta) {
        debug_assert!(self.position(uid).is_none());
        let index = uid.index() as usize;
        if self.sparse.len() <= index {
            self.sparse.resize(index + 1, EMPTY);
        }
        self.sparse[index] = u32::try_from(self.entities.len()).expect("too many components");
        self.entities.push(uid);
        self.data.push(value);
        self.meta.push(meta);
    }

    pub(crate) fn get(&self, uid: EntityUid) -> Option<&C> {
        self.position(uid).map(|p| &self.data[p])
    }

    pub(crate) fn get_mut(&mut self, uid: EntityUid) -> Option<&mut C> {
        self.position(uid).map(|p| &mut self.data[p])
    }
}

/// Type-erased component storage.
pub(crate) trait ErasedStorage: Send + Sync {
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
    /// `TypeId` of the stored component.
    fn component_type(&self) -> TypeId;
    fn contains(&self, uid: EntityUid) -> bool;
    fn meta(&self, uid: EntityUid) -> Option<&ComponentMeta>;
    fn meta_mut(&mut self, uid: EntityUid) -> Option<&mut ComponentMeta>;
    /// Removes a component without events. Returns `true` if it was present.
    fn remove(&mut self, uid: EntityUid) -> bool;
    fn len(&self) -> usize;
    /// Entities in dense storage order, which remains stable between removals.
    fn entities(&self) -> &[EntityUid];
}

impl<C: Component> ErasedStorage for SparseSet<C> {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn component_type(&self) -> TypeId {
        TypeId::of::<C>()
    }

    fn contains(&self, uid: EntityUid) -> bool {
        self.position(uid).is_some()
    }

    fn meta(&self, uid: EntityUid) -> Option<&ComponentMeta> {
        self.position(uid).map(|p| &self.meta[p])
    }

    fn meta_mut(&mut self, uid: EntityUid) -> Option<&mut ComponentMeta> {
        self.position(uid).map(|p| &mut self.meta[p])
    }

    fn remove(&mut self, uid: EntityUid) -> bool {
        let Some(pos) = self.position(uid) else {
            return false;
        };
        self.sparse[uid.index() as usize] = EMPTY;
        self.entities.swap_remove(pos);
        self.data.swap_remove(pos);
        self.meta.swap_remove(pos);
        if let Some(moved) = self.entities.get(pos) {
            self.sparse[moved.index() as usize] = pos as u32;
        }
        true
    }

    fn len(&self) -> usize {
        self.entities.len()
    }

    fn entities(&self) -> &[EntityUid] {
        &self.entities
    }
}

/// Creates empty storage for `C`. Subscriptions and registrations retain this constructor.
pub(crate) fn new_storage<C: Component>() -> Box<dyn ErasedStorage> {
    Box::new(SparseSet::<C>::new())
}
