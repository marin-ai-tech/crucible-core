//! Safe iteration over a set of components using a snapshot and world lookups.

use std::any::TypeId;
use std::marker::PhantomData;

use crate::component::Component;
use crate::entity::EntityUid;
use crate::world::World;

/// Component types to iterate over, either one component `C` or a tuple of up to eight.
pub trait ComponentSet: 'static {
    /// `TypeId` of each component in the set.
    fn type_ids() -> Vec<TypeId>;
}

impl<C: Component> ComponentSet for C {
    fn type_ids() -> Vec<TypeId> {
        vec![TypeId::of::<C>()]
    }
}

macro_rules! impl_component_set {
    ($($name:ident),+) => {
        impl<$($name: Component),+> ComponentSet for ($($name,)+) {
            fn type_ids() -> Vec<TypeId> {
                vec![$(TypeId::of::<$name>()),+]
            }
        }
    };
}

impl_component_set!(A);
impl_component_set!(A, B);
impl_component_set!(A, B, C);
impl_component_set!(A, B, C, D);
impl_component_set!(A, B, C, D, E);
impl_component_set!(A, B, C, D, E, F);
impl_component_set!(A, B, C, D, E, F, G);
impl_component_set!(A, B, C, D, E, F, G, H);

/// Iteration over entities with components `Q`, equivalent to Robust's `EntityQueryEnumerator`.
///
/// [`World::query`] captures the entities that have all requested components at creation.
/// [`Query::next`] yields the next entity in the snapshot if it still exists and has
/// all requested components. The iterator does not keep the world borrowed, so entities and
/// components can be created or removed during iteration:
///
/// - Each entity in the snapshot is yielded at most once.
/// - Entities that gain the required components during iteration are not yielded.
/// - An entity that loses a required component before its turn is skipped.
///
/// ```
/// use crucible_ecs::{Component, World};
///
/// struct Health(i32);
/// impl Component for Health {}
///
/// let mut world = World::new();
/// for _ in 0..3 {
///     let e = world.spawn();
///     world.add_component(e, Health(10)).unwrap();
/// }
/// let mut query = world.query::<Health>();
/// while let Some(uid) = query.next(&world) {
///     world.get_mut::<Health>(uid).unwrap().0 -= 1;
///     let child = world.spawn(); // The world can be modified during iteration.
///     world.add_component(child, Health(1)).unwrap();
/// }
/// assert_eq!(world.query::<Health>().len(), 6);
/// ```
pub struct Query<Q: ComponentSet> {
    snapshot: Vec<EntityUid>,
    storages: Vec<usize>,
    position: usize,
    _marker: PhantomData<fn() -> Q>,
}

impl<Q: ComponentSet> Query<Q> {
    pub(crate) fn new(snapshot: Vec<EntityUid>, storages: Vec<usize>) -> Self {
        Query {
            snapshot,
            storages,
            position: 0,
            _marker: PhantomData,
        }
    }

    /// Returns the next entity in the snapshot that still matches, or `None` at the end.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self, world: &World) -> Option<EntityUid> {
        while let Some(&uid) = self.snapshot.get(self.position) {
            self.position += 1;
            if world.exists(uid)
                && self
                    .storages
                    .iter()
                    .all(|&s| world.storage_contains(s, uid))
            {
                return Some(uid);
            }
        }
        None
    }

    /// Size of the snapshot taken at creation.
    pub fn len(&self) -> usize {
        self.snapshot.len()
    }

    /// Whether the snapshot is empty.
    pub fn is_empty(&self) -> bool {
        self.snapshot.is_empty()
    }

    /// The complete snapshot, without rechecking entities.
    pub fn into_snapshot(self) -> Vec<EntityUid> {
        self.snapshot
    }
}
