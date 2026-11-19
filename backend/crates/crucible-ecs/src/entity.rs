//! Entities, the generational arena, `EntityUid`, and `NetEntity`.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use crate::change::Tick;

/// Server-side entity identifier with an index and generation, equivalent to Robust's `EntityUid`.
///
/// Reused indices get a new generation. The world treats stale identifiers as missing,
/// so a stale identifier cannot point to a new entity.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EntityUid {
    index: u32,
    generation: u32,
}

impl EntityUid {
    /// Arena index, which can be reused after removal.
    pub const fn index(self) -> u32 {
        self.index
    }

    /// Generation of the index, incremented each time an entity at this index is removed.
    pub const fn generation(self) -> u32 {
        self.generation
    }
}

impl fmt::Debug for EntityUid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "EntityUid({}v{})", self.index, self.generation)
    }
}

impl fmt::Display for EntityUid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}v{}", self.index, self.generation)
    }
}

/// Network entity identifier, equivalent to Robust's `NetEntity`.
///
/// A monotonically increasing world counter that starts at 1. Values are unique
/// for the lifetime of the world. Zero is reserved.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct NetEntity(u32);

impl NetEntity {
    /// Creates a network identifier from a raw value, such as one received from a client.
    /// Use [`World::entity_by_net`](crate::World::entity_by_net) to check that the entity exists.
    pub const fn from_raw(raw: u32) -> NetEntity {
        NetEntity(raw)
    }

    /// Raw value.
    pub const fn raw(self) -> u32 {
        self.0
    }
}

impl fmt::Display for NetEntity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "n{}", self.0)
    }
}

/// Entity life stage, equivalent to Robust's `EntityLifeStage` without map stages.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum EntityLifeStage {
    /// Created by [`World::spawn_uninitialized`](crate::World::spawn_uninitialized).
    /// Components receive only `ComponentAdd`.
    PreInit,
    /// [`World::initialize_entity`](crate::World::initialize_entity) is running.
    Initializing,
    /// Initialized. New components are added, initialized, and started immediately.
    Initialized,
    /// Being removed. Components stop and are removed, and adding components is disabled.
    Terminating,
}

/// Entity metadata managed by the world.
#[derive(Debug)]
pub struct EntityMeta {
    net_entity: NetEntity,
    pub(crate) life_stage: EntityLifeStage,
    pub(crate) queued_for_deletion: bool,
    pub(crate) last_modified: Tick,
    pub(crate) prototype: Option<Arc<str>>,
    /// Entity component storage indices in insertion order.
    pub(crate) components: Vec<usize>,
}

impl EntityMeta {
    /// Network identifier.
    pub fn net_entity(&self) -> NetEntity {
        self.net_entity
    }

    /// Life stage.
    pub fn life_stage(&self) -> EntityLifeStage {
        self.life_stage
    }

    /// Whether deletion is queued for the end of the tick.
    pub fn is_queued_for_deletion(&self) -> bool {
        self.queued_for_deletion
    }

    /// Tick of the last component addition, removal, or change mark on the entity.
    pub fn last_modified(&self) -> Tick {
        self.last_modified
    }

    /// Prototype identifier used to create the entity, when specified.
    pub fn prototype(&self) -> Option<&str> {
        self.prototype.as_deref()
    }
}

struct Slot {
    generation: u32,
    meta: Option<EntityMeta>,
}

/// Generational arena and bidirectional mapping between `NetEntity` and `EntityUid`.
#[derive(Default)]
pub(crate) struct Entities {
    slots: Vec<Slot>,
    free: Vec<u32>,
    last_net: u32,
    by_net: HashMap<NetEntity, EntityUid>,
    alive: usize,
}

impl Entities {
    pub(crate) fn alloc(&mut self, tick: Tick) -> EntityUid {
        self.last_net = self
            .last_net
            .checked_add(1)
            .expect("world exhausted its network identifiers (u32)");
        let net_entity = NetEntity(self.last_net);
        let meta = EntityMeta {
            net_entity,
            life_stage: EntityLifeStage::PreInit,
            queued_for_deletion: false,
            last_modified: tick,
            prototype: None,
            components: Vec::new(),
        };
        let uid = match self.free.pop() {
            Some(index) => {
                let slot = &mut self.slots[index as usize];
                slot.meta = Some(meta);
                EntityUid {
                    index,
                    generation: slot.generation,
                }
            }
            None => {
                let index = u32::try_from(self.slots.len()).expect("too many entities");
                self.slots.push(Slot {
                    generation: 0,
                    meta: Some(meta),
                });
                EntityUid {
                    index,
                    generation: 0,
                }
            }
        };
        self.by_net.insert(net_entity, uid);
        self.alive += 1;
        uid
    }

    pub(crate) fn meta(&self, uid: EntityUid) -> Option<&EntityMeta> {
        let slot = self.slots.get(uid.index as usize)?;
        if slot.generation != uid.generation {
            return None;
        }
        slot.meta.as_ref()
    }

    pub(crate) fn meta_mut(&mut self, uid: EntityUid) -> Option<&mut EntityMeta> {
        let slot = self.slots.get_mut(uid.index as usize)?;
        if slot.generation != uid.generation {
            return None;
        }
        slot.meta.as_mut()
    }

    /// Frees the index, increments the generation, and permanently removes the network identifier.
    pub(crate) fn free(&mut self, uid: EntityUid) -> Option<EntityMeta> {
        let slot = self.slots.get_mut(uid.index as usize)?;
        if slot.generation != uid.generation {
            return None;
        }
        let meta = slot.meta.take()?;
        slot.generation = slot.generation.wrapping_add(1);
        self.free.push(uid.index);
        self.by_net.remove(&meta.net_entity);
        self.alive -= 1;
        Some(meta)
    }

    pub(crate) fn by_net(&self, net: NetEntity) -> Option<EntityUid> {
        self.by_net.get(&net).copied()
    }

    pub(crate) fn len(&self) -> usize {
        self.alive
    }

    /// All live entities in index order.
    pub(crate) fn iter(&self) -> impl Iterator<Item = EntityUid> + '_ {
        self.slots.iter().enumerate().filter_map(|(i, slot)| {
            slot.meta.as_ref().map(|_| EntityUid {
                index: i as u32,
                generation: slot.generation,
            })
        })
    }
}
