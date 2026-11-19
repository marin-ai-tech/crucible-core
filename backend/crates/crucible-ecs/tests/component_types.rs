//! Entity component types for type-erased consumers such as replication.

use std::any::TypeId;

use crucible_ecs::{Component, World};

#[derive(Default)]
struct A;
impl Component for A {}

#[derive(Default)]
struct B;
impl Component for B {}

#[test]
fn component_types_follow_add_and_remove() {
    let mut world = World::new();
    let e = world.spawn();
    assert_eq!(world.component_types(e).count(), 0);

    world.add_component(e, B).unwrap();
    world.add_component(e, A).unwrap();
    assert_eq!(
        world.component_types(e).collect::<Vec<_>>(),
        vec![TypeId::of::<B>(), TypeId::of::<A>()],
        "insertion order"
    );

    world.remove_component::<B>(e);
    assert_eq!(
        world.component_types(e).collect::<Vec<_>>(),
        vec![TypeId::of::<A>()]
    );

    world.delete(e);
    assert_eq!(
        world.component_types(e).count(),
        0,
        "the missing entity has no components"
    );
}
