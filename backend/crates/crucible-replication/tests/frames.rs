//! State frames, deltas, and acknowledgments.
//!

mod common;

use common::{Bench, Flag, Label, Speed, Stats, component_names};
use crucible_ecs::Tick;
use crucible_map::{Vec2, set_world_position};

/// Scenario: only changed state is sent.
#[test]
fn only_changes() {
    let mut b = Bench::new();
    let body = b.spawn(0.0, 0.0);
    let thing = b.spawn(2.0, 0.0);
    // Five replicated components: Transform and four test components.
    b.world.add_component(thing, Stats::default()).unwrap();
    b.world.add_component(thing, Flag::default()).unwrap();
    b.world
        .add_component(thing, Label { text: "x".into() })
        .unwrap();
    b.world.add_component(thing, Speed::default()).unwrap();
    let a = b.player("a", Some(body));
    let net = b.net(thing);

    let ups = b.finish_tick();
    let first = ups[&a].frame.entity(net).unwrap();
    assert_eq!(first.components.len(), 5);
    b.ack(&a);

    b.next_tick();
    Flag::set_on(&mut b.world, thing, true);
    let ups = b.finish_tick();
    let frame = &ups[&a].frame;
    assert_eq!(frame.from_tick, Tick(1));
    assert_eq!(
        frame.entities.len(),
        1,
        "only this entity should be sent: {frame:?}"
    );
    let state = &frame.entities[0];
    assert_eq!(state.entity, net);
    assert!(!state.full);
    assert_eq!(component_names(state), vec!["Flag"]);
    assert_eq!(state.components[0].fields, Some(Flag::FIELD_ON));
    assert!(state.component_set.is_none());
    b.ack(&a);

    // The per-field delta contains the one field that changed.
    b.next_tick();
    Stats::set_score(&mut b.world, thing, 9);
    let ups = b.finish_tick();
    let state = &ups[&a].frame.entities[0];
    assert_eq!(component_names(state), vec!["Stats"]);
    assert_eq!(state.components[0].fields, Some(Stats::FIELD_SCORE));
    assert_eq!(state.components[0].value.field_names(), vec!["score"]);
}

/// Scenario: a client requests full state.
#[test]
fn full_state_request() {
    let mut b = Bench::new();
    let body = b.spawn(0.0, 0.0);
    let thing = b.spawn(2.0, 0.0);
    b.world.add_component(thing, Stats::default()).unwrap();
    let a = b.player("a", Some(body));
    b.finish_tick();
    b.ack(&a);
    b.next_tick();
    Stats::set_level(&mut b.world, thing, 1);
    let ups = b.finish_tick();
    assert!(!ups[&a].frame.is_full());
    let stale = b.tick();

    assert!(b.replication.request_full_state(&a));
    let ups = b.step();
    let frame = &ups[&a].frame;
    assert!(frame.is_full());
    assert!(
        frame.reliable,
        "full state requested by the client is delivered reliably"
    );
    for uid in [b.map, body, thing] {
        let state = frame
            .entity(b.net(uid))
            .expect("full state includes this visible entity");
        assert!(state.full);
        assert!(state.component_set.is_some());
    }
    assert_eq!(
        component_names(frame.entity(b.net(thing)).unwrap()),
        vec!["Transform", "Stats"]
    );

    // Reject acknowledgments for frames built before the request.
    assert!(!b.replication.acknowledge(&a, stale));
    let ups = b.step();
    assert!(
        ups[&a].frame.is_full(),
        "repeat full state until it is acknowledged"
    );
    b.ack(&a);
    let ups = b.step();
    assert!(!ups[&a].frame.is_full());
    assert!(ups[&a].frame.is_empty());
}

/// Scenario: acknowledgments fall behind.
#[test]
fn acknowledgement_lag() {
    let mut b = Bench::new();
    let body = b.spawn(0.0, 0.0);
    let a = b.player("a", Some(body));
    let threshold = b.replication.config().force_ack_threshold;
    assert_eq!(threshold, 60);
    b.finish_tick();
    b.ack(&a); // Acknowledge tick 1.

    // Normal delivery while the delay remains within the threshold.
    for _ in 0..threshold {
        let ups = b.step();
        assert!(!ups[&a].frame.reliable, "tick {}", b.tick());
    }
    // At tick 62, the delay of 61 exceeds the threshold of 60.
    let ups = b.step();
    assert_eq!(ups[&a].frame.to_tick, Tick(threshold + 2));
    assert!(ups[&a].frame.reliable);

    // Normal delivery resumes after the acknowledgment.
    b.ack(&a);
    let ups = b.step();
    assert!(!ups[&a].frame.reliable);
}

/// A lost frame causes full entity state to repeat until entering view is acknowledged.
#[test]
fn unacknowledged_entry_is_repeated_in_full() {
    let mut b = Bench::new();
    let body = b.spawn(0.0, 0.0);
    let a = b.player("a", Some(body));
    b.finish_tick();
    b.ack(&a);

    b.next_tick();
    let thing = b.spawn(3.0, 0.0);
    let net = b.net(thing);
    let ups = b.finish_tick();
    assert!(ups[&a].frame.entity(net).unwrap().full);
    // No acknowledgment for the lost frame, so the next frame sends the full entity again.
    let ups = b.step();
    let frame = &ups[&a].frame;
    assert_eq!(frame.from_tick, Tick(1));
    assert!(frame.entity(net).unwrap().full);
    b.ack(&a);
    let ups = b.step();
    assert!(ups[&a].frame.entity(net).is_none());
}

/// Repeat removal until acknowledged, and send it only to players that knew about the entity.
#[test]
fn deletions_repeat_until_acknowledged() {
    let mut b = Bench::new();
    let body = b.spawn(0.0, 0.0);
    let other = b.spawn(500.0, 0.0);
    let thing = b.spawn(2.0, 0.0);
    let a = b.player("a", Some(body));
    let far = b.player("far", Some(other));
    let net = b.net(thing);
    b.finish_tick();
    b.ack(&a);
    b.ack(&far);

    b.next_tick();
    b.world.queue_delete(thing);
    let ups = b.finish_tick();
    assert_eq!(ups[&a].frame.deletions, vec![net]);
    assert!(ups[&a].frame.left_view.is_empty());
    assert!(ups[&far].frame.deletions.is_empty());
    let ups = b.step();
    assert_eq!(ups[&a].frame.deletions, vec![net]);
    b.ack(&a);
    let ups = b.step();
    assert!(ups[&a].frame.deletions.is_empty());
}

/// After component removal, a delta includes the complete remaining component list.
#[test]
fn component_removal_sends_component_set() {
    let mut b = Bench::new();
    let body = b.spawn(0.0, 0.0);
    let thing = b.spawn(2.0, 0.0);
    b.world.add_component(thing, Stats::default()).unwrap();
    b.world.add_component(thing, Flag::default()).unwrap();
    let a = b.player("a", Some(body));
    let net = b.net(thing);
    b.finish_tick();
    b.ack(&a);

    b.next_tick();
    b.world.remove_component::<Flag>(thing);
    let ups = b.finish_tick();
    let state = ups[&a].frame.entity(net).unwrap();
    let set: Vec<&str> = state
        .component_set
        .as_ref()
        .unwrap()
        .iter()
        .map(|n| n.as_ref())
        .collect();
    assert_eq!(set, vec!["Transform", "Stats"]);
    assert!(state.components.is_empty());
}

/// Every viewer sees movement through the transport-independent transform value.
#[test]
fn movement_reaches_every_viewer() {
    let mut b = Bench::new();
    let a_body = b.spawn(0.0, 0.0);
    let b_body = b.spawn(1.0, 0.0);
    let a = b.player("a", Some(a_body));
    let bb = b.player("b", Some(b_body));
    b.finish_tick();
    b.ack(&a);
    b.ack(&bb);

    b.next_tick();
    set_world_position(&mut b.world, a_body, Vec2::new(0.5, 0.25)).unwrap();
    let ups = b.finish_tick();
    for user in [&a, &bb] {
        let state = ups[user].frame.entity(b.net(a_body)).unwrap();
        let t = state.component("Transform").unwrap();
        assert_eq!(
            t.value.field("local_position"),
            Some(&crucible_replication::NeutralValue::Vec2([0.5, 0.25]))
        );
    }
}
