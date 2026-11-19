//! Event bus with directed subscriptions, broadcasts, and cancellation.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::Arc;

use crate::entity::EntityUid;
use crate::order::OrderNode;
use crate::world::World;

/// An event that can be cancelled, equivalent to Robust's `CancellableEntityEventArgs`.
///
/// Delivery continues after cancellation. Later handlers see the cancelled flag, and the caller
/// can read it after `raise` returns.
///
/// ```
/// use crucible_ecs::Cancellable;
///
/// #[derive(Default)]
/// struct OpenAttempt {
///     cancelled: bool,
/// }
///
/// impl Cancellable for OpenAttempt {
///     fn cancel(&mut self) {
///         self.cancelled = true;
///     }
///     fn is_cancelled(&self) -> bool {
///         self.cancelled
///     }
/// }
/// ```
pub trait Cancellable {
    /// Cancel the event.
    fn cancel(&mut self);
    /// Whether the event is cancelled.
    fn is_cancelled(&self) -> bool;
}

pub(crate) type DirectedFn<E> = Box<dyn Fn(&mut World, EntityUid, &mut E) + Send + Sync>;
pub(crate) type BroadcastFn<E> = Box<dyn Fn(&mut World, &mut E) + Send + Sync>;

/// A handler for a component and event pair.
pub(crate) struct DirectedHandler<E> {
    /// Index of the subscribed component's storage.
    pub(crate) storage: usize,
    pub(crate) f: DirectedFn<E>,
}

pub(crate) struct BroadcastHandler<E> {
    pub(crate) f: BroadcastFn<E>,
}

/// Ordered handler lists indexed by event type.
#[derive(Default)]
pub(crate) struct EventBus {
    directed: HashMap<TypeId, Arc<dyn Any + Send + Sync>>,
    broadcast: HashMap<TypeId, Arc<dyn Any + Send + Sync>>,
}

impl EventBus {
    pub(crate) fn directed<E: 'static>(&self) -> Option<Arc<Vec<DirectedHandler<E>>>> {
        let list = self.directed.get(&TypeId::of::<E>())?.clone();
        Some(
            list.downcast()
                .expect("the handler list type matches the event type"),
        )
    }

    pub(crate) fn broadcast<E: 'static>(&self) -> Option<Arc<Vec<BroadcastHandler<E>>>> {
        let list = self.broadcast.get(&TypeId::of::<E>())?.clone();
        Some(
            list.downcast()
                .expect("the handler list type matches the event type"),
        )
    }

    pub(crate) fn insert(
        &mut self,
        broadcast: bool,
        event: TypeId,
        list: Arc<dyn Any + Send + Sync>,
    ) {
        if broadcast {
            self.broadcast.insert(event, list);
        } else {
            self.directed.insert(event, list);
        }
    }
}

/// Builds an ordered handler list from type-erased subscriptions.
pub(crate) type Assembler = fn(Vec<Box<dyn Any + Send>>) -> Arc<dyn Any + Send + Sync>;

/// A subscription registered in `System::initialize` and ordered when the world is built.
pub(crate) struct PendingSubscription {
    pub(crate) event: TypeId,
    pub(crate) event_name: &'static str,
    pub(crate) broadcast: bool,
    pub(crate) node: OrderNode,
    pub(crate) handler: Box<dyn Any + Send>,
    pub(crate) assemble: Assembler,
}

pub(crate) fn assemble_directed<E: 'static>(
    handlers: Vec<Box<dyn Any + Send>>,
) -> Arc<dyn Any + Send + Sync> {
    let list: Vec<DirectedHandler<E>> = handlers
        .into_iter()
        .map(|h| *h.downcast::<DirectedHandler<E>>().expect("handler type"))
        .collect();
    Arc::new(list)
}

pub(crate) fn assemble_broadcast<E: 'static>(
    handlers: Vec<Box<dyn Any + Send>>,
) -> Arc<dyn Any + Send + Sync> {
    let list: Vec<BroadcastHandler<E>> = handlers
        .into_iter()
        .map(|h| *h.downcast::<BroadcastHandler<E>>().expect("handler type"))
        .collect();
    Arc::new(list)
}
