//! Systems with event subscriptions, before and after constraints, and sequential execution.
//! Implements handler ordering and sequential gameplay logic.

use std::any::{Any, TypeId};
use std::time::{Duration, Instant};

use crate::component::Component;
use crate::entity::EntityUid;
use crate::event::{
    BroadcastHandler, DirectedHandler, PendingSubscription, assemble_broadcast, assemble_directed,
};
use crate::order::{OrderNode, SystemKey};
use crate::world::World;

/// Ordered unit of logic, equivalent to Robust's `EntitySystem`.
///
/// During [`initialize`](System::initialize), a system subscribes to events and declares where it
/// runs in the update order. [`update`](System::update) runs once per tick. Event handlers receive
/// the world, and their state lives in components or world resources
/// ([`World::insert_resource`]).
///
/// The fully qualified system type name resolves ties in ordering.
pub trait System: Send + 'static {
    /// Registers subscriptions and ordering constraints, equivalent to `EntitySystem.Initialize`.
    fn initialize(&mut self, init: &mut SystemInit<'_>) {
        let _ = init;
    }

    /// Updates the system for this tick, equivalent to `EntitySystem.Update(frameTime)`.
    /// `frame_time` is the step duration in seconds.
    fn update(&mut self, world: &mut World, frame_time: f32) {
        let _ = (world, frame_time);
    }
}

/// Context for [`System::initialize`], containing this system's subscriptions and update order.
pub struct SystemInit<'a> {
    pub(crate) owner: SystemKey,
    pub(crate) world: &'a mut World,
    pub(crate) subscriptions: &'a mut Vec<PendingSubscription>,
    pub(crate) update_order: &'a mut OrderNode,
}

impl SystemInit<'_> {
    /// Subscribes to directed event `E` for entities with component `C`, equivalent to
    /// `SubscribeLocalEvent<C, E>`. The handler receives the world, entity, and event.
    pub fn subscribe<C, E, F>(&mut self, handler: F) -> SubscriptionOrder<'_>
    where
        C: Component,
        E: 'static,
        F: Fn(&mut World, EntityUid, &mut E) + Send + Sync + 'static,
    {
        let storage = self.world.storage_index::<C>();
        let handler = DirectedHandler::<E> {
            storage,
            f: Box::new(handler),
        };
        self.push(
            false,
            Box::new(handler),
            assemble_directed::<E>,
            TypeId::of::<E>(),
            std::any::type_name::<E>(),
        )
    }

    /// Subscribes to broadcast event `E`, equivalent to `SubscribeLocalEvent<E>` without a component.
    pub fn subscribe_broadcast<E, F>(&mut self, handler: F) -> SubscriptionOrder<'_>
    where
        E: 'static,
        F: Fn(&mut World, &mut E) + Send + Sync + 'static,
    {
        let handler = BroadcastHandler::<E> {
            f: Box::new(handler),
        };
        self.push(
            true,
            Box::new(handler),
            assemble_broadcast::<E>,
            TypeId::of::<E>(),
            std::any::type_name::<E>(),
        )
    }

    fn push(
        &mut self,
        broadcast: bool,
        handler: Box<dyn Any + Send>,
        assemble: crate::event::Assembler,
        event: TypeId,
        event_name: &'static str,
    ) -> SubscriptionOrder<'_> {
        let seq = self.subscriptions.len();
        self.subscriptions.push(PendingSubscription {
            event,
            event_name,
            broadcast,
            node: OrderNode {
                owner: self.owner,
                seq,
                before: Vec::new(),
                after: Vec::new(),
            },
            handler,
            assemble,
        });
        SubscriptionOrder {
            node: &mut self
                .subscriptions
                .last_mut()
                .expect("subscription was just added")
                .node,
        }
    }

    /// Updates this system before `S`, equivalent to `UpdatesBefore`.
    pub fn updates_before<S: System>(&mut self) -> &mut Self {
        self.update_order.before.push(SystemKey::of::<S>());
        self
    }

    /// Updates this system after `S`, equivalent to `UpdatesAfter`.
    pub fn updates_after<S: System>(&mut self) -> &mut Self {
        self.update_order.after.push(SystemKey::of::<S>());
        self
    }

    /// Inserts a world resource for the system's handlers.
    pub fn insert_resource<R: Send + Sync + 'static>(&mut self, resource: R) {
        self.world.insert_resource(resource);
    }
}

/// Orders one subscription relative to other systems' subscriptions to the same event.
/// Directed and broadcast subscriptions are ordered separately.
///
/// Constraints have no effect if the other system is absent or has no subscriptions to this event.
/// A constraint cycle causes [`BuildError::EventOrderCycle`](crate::BuildError::EventOrderCycle)
/// during world construction. Unconstrained handlers sort by system name and then subscription order.
pub struct SubscriptionOrder<'a> {
    node: &'a mut OrderNode,
}

impl SubscriptionOrder<'_> {
    /// Calls this handler before system `S`'s handlers for this event.
    pub fn before<S: System>(self) -> Self {
        self.node.before.push(SystemKey::of::<S>());
        self
    }

    /// Calls this handler after system `S`'s handlers for this event.
    pub fn after<S: System>(self) -> Self {
        self.node.after.push(SystemKey::of::<S>());
        self
    }
}

pub(crate) struct SystemEntry {
    pub(crate) name: &'static str,
    pub(crate) system: Box<dyn System>,
}

/// World systems in execution order, equivalent to Robust's `EntitySystemManager`.
///
/// [`WorldBuilder::build`](crate::WorldBuilder::build) computes this order once using
/// `updates_before` and `updates_after`, with system names breaking ties. The same set of systems
/// runs in the same order on each tick and across runs regardless of registration
/// order.
#[derive(Default)]
pub struct Systems {
    pub(crate) entries: Vec<SystemEntry>,
}

impl Systems {
    /// Calls `update` for each system in order on one thread.
    pub fn update(&mut self, world: &mut World, frame_time: f32) {
        self.update_observed(world, frame_time, None);
    }

    /// Measures each system update when an observer is supplied.
    pub fn update_observed(
        &mut self,
        world: &mut World,
        frame_time: f32,
        mut observe: Option<&mut dyn FnMut(&'static str, Duration)>,
    ) {
        for entry in &mut self.entries {
            let start = observe.as_ref().map(|_| Instant::now());
            entry.system.update(world, frame_time);
            if let (Some(start), Some(observe)) = (start, &mut observe) {
                observe(entry.name, start.elapsed());
            }
        }
    }

    /// System names in execution order.
    pub fn names(&self) -> Vec<&'static str> {
        self.entries.iter().map(|e| e.name).collect()
    }

    /// Number of systems.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the world has no systems.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
