//! Builds the world by registering components and ordering subscriptions and systems.

use std::any::TypeId;
use std::collections::HashSet;
use std::fmt;

use crate::event::PendingSubscription;
use crate::order::{self, OrderNode, SystemKey};
use crate::registry::{ComponentRegistration, RegistryError};
use crate::system::{System, SystemEntry, SystemInit, Systems};
use crate::world::World;

/// Error constructing a world.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum BuildError {
    /// A cycle in before or after constraints for subscriptions to the same event.
    EventOrderCycle {
        /// Event type name.
        event: &'static str,
        /// Systems in the cycle.
        systems: Vec<&'static str>,
    },
    /// A cycle in `updates_before` or `updates_after` constraints.
    SystemOrderCycle {
        /// Systems in the cycle.
        systems: Vec<&'static str>,
    },
    /// The same system type was added twice.
    DuplicateSystem {
        /// System name.
        system: &'static str,
    },
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BuildError::EventOrderCycle { event, systems } => write!(
                f,
                "cyclic handler ordering for event {event}: {}",
                systems.join(" → ")
            ),
            BuildError::SystemOrderCycle { systems } => {
                write!(f, "cyclic system ordering: {}", systems.join(" → "))
            }
            BuildError::DuplicateSystem { system } => {
                write!(f, "system {system} was added twice")
            }
        }
    }
}

impl std::error::Error for BuildError {}

/// Builder for a world and its systems, equivalent to Robust's `EntityManager` initialization phase.
///
/// ```
/// use crucible_ecs::{System, SystemInit, World};
///
/// #[derive(Default)]
/// struct Ping(u32);
///
/// struct PingSystem;
/// impl System for PingSystem {
///     fn initialize(&mut self, init: &mut SystemInit<'_>) {
///         init.subscribe_broadcast::<Ping, _>(|_world, ping| ping.0 += 1);
///     }
/// }
///
/// let mut builder = World::builder();
/// builder.add_system(PingSystem);
/// let (mut world, _systems) = builder.build().unwrap();
/// let mut ping = Ping::default();
/// world.raise_broadcast(&mut ping);
/// assert_eq!(ping.0, 1);
/// ```
pub struct WorldBuilder {
    world: World,
    systems: Vec<(OrderNode, SystemEntry)>,
    system_types: HashSet<TypeId>,
    subscriptions: Vec<PendingSubscription>,
    duplicate_system: Option<&'static str>,
}

impl Default for WorldBuilder {
    fn default() -> Self {
        WorldBuilder::new()
    }
}

impl WorldBuilder {
    /// An empty builder.
    pub fn new() -> Self {
        WorldBuilder {
            world: World::new(),
            systems: Vec::new(),
            system_types: HashSet::new(),
            subscriptions: Vec::new(),
            duplicate_system: None,
        }
    }

    /// Registers a component. A duplicate name or type reports both registration sources.
    pub fn register_component(
        &mut self,
        registration: ComponentRegistration,
    ) -> Result<&mut Self, RegistryError> {
        self.world.register_component(registration)?;
        Ok(self)
    }

    /// Adds a system and immediately calls [`System::initialize`].
    pub fn add_system<S: System>(&mut self, mut system: S) -> &mut Self {
        let owner = SystemKey::of::<S>();
        if !self.system_types.insert(owner.type_id) {
            self.duplicate_system.get_or_insert(owner.name);
            return self;
        }
        let mut update_order = OrderNode {
            owner,
            seq: self.systems.len(),
            before: Vec::new(),
            after: Vec::new(),
        };
        let mut init = SystemInit {
            owner,
            world: &mut self.world,
            subscriptions: &mut self.subscriptions,
            update_order: &mut update_order,
        };
        system.initialize(&mut init);
        self.systems.push((
            update_order,
            SystemEntry {
                name: owner.name,
                system: Box::new(system),
            },
        ));
        self
    }

    /// A world resource.
    pub fn insert_resource<R: Send + Sync + 'static>(&mut self, resource: R) -> &mut Self {
        self.world.insert_resource(resource);
        self
    }

    /// Accesses an already registered resource before building the world.
    pub fn resource_mut<R: Send + Sync + 'static>(&mut self) -> Option<&mut R> {
        self.world.resource_mut::<R>()
    }

    /// Maximum nesting depth for event dispatch (default 64).
    pub fn max_event_depth(&mut self, depth: u32) -> &mut Self {
        self.world.set_max_event_depth(depth);
        self
    }

    /// Orders subscriptions and systems, then returns the completed world and systems.
    pub fn build(self) -> Result<(World, Systems), BuildError> {
        let WorldBuilder {
            mut world,
            systems,
            subscriptions,
            duplicate_system,
            ..
        } = self;
        if let Some(system) = duplicate_system {
            return Err(BuildError::DuplicateSystem { system });
        }

        // Group subscriptions by (kind, event) in order of first appearance for deterministic ordering.
        let mut groups: Vec<(bool, TypeId, Vec<PendingSubscription>)> = Vec::new();
        for sub in subscriptions {
            match groups
                .iter_mut()
                .find(|(b, e, _)| *b == sub.broadcast && *e == sub.event)
            {
                Some((_, _, list)) => list.push(sub),
                None => groups.push((sub.broadcast, sub.event, vec![sub])),
            }
        }
        for (broadcast, event, subs) in groups {
            let nodes: Vec<OrderNode> = subs.iter().map(|s| s.node.clone()).collect();
            let order = order::sort(&nodes).map_err(|systems| BuildError::EventOrderCycle {
                event: subs[0].event_name,
                systems,
            })?;
            let assemble = subs[0].assemble;
            let mut slots: Vec<Option<PendingSubscription>> = subs.into_iter().map(Some).collect();
            let handlers = order
                .into_iter()
                .map(|i| slots[i].take().expect("each index occurs once").handler)
                .collect();
            world.bus.insert(broadcast, event, assemble(handlers));
        }

        let nodes: Vec<OrderNode> = systems.iter().map(|(n, _)| n.clone()).collect();
        let order =
            order::sort(&nodes).map_err(|systems| BuildError::SystemOrderCycle { systems })?;
        let mut slots: Vec<Option<SystemEntry>> =
            systems.into_iter().map(|(_, e)| Some(e)).collect();
        let entries = order
            .into_iter()
            .map(|i| slots[i].take().expect("each index occurs once"))
            .collect();
        Ok((world, Systems { entries }))
    }
}
