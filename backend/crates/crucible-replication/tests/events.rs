//! Reliable events and recipient selection.

mod common;

use common::Bench;
use crucible_ecs::Tick;
use crucible_replication::{EventTarget, NeutralValue, send_network_event};

/// Scenario: event ordering.
#[test]
fn event_order() {
    let mut b = Bench::new();
    let body = b.spawn(0.0, 0.0);
    let a = b.player("a", Some(body));
    b.finish_tick();

    b.next_tick();
    for (i, name) in ["First", "Second", "Third"].into_iter().enumerate() {
        send_network_event(
            &mut b.world,
            EventTarget::Player(a.clone()),
            name,
            NeutralValue::Int(i as i64),
        );
    }
    let ups = b.finish_tick();
    let events = &ups[&a].events;
    let names: Vec<&str> = events.iter().map(|e| e.event.name.as_ref()).collect();
    assert_eq!(names, vec!["First", "Second", "Third"]);
    let seqs: Vec<u64> = events.iter().map(|e| e.sequence).collect();
    assert_eq!(seqs, vec![1, 2, 3]);
    assert!(
        events.iter().all(|e| e.event.tick == Some(Tick(2))),
        "the event records its tick"
    );

    // Sequence numbers continue through later ticks without gaps.
    b.next_tick();
    send_network_event(
        &mut b.world,
        EventTarget::Player(a.clone()),
        "Fourth",
        NeutralValue::Null,
    );
    let ups = b.finish_tick();
    assert_eq!(ups[&a].events[0].sequence, 4);
}

/// The visible-entity target reaches current viewers and preserves
/// the order of events for each recipient.
#[test]
fn viewers_and_broadcast_targets() {
    let mut b = Bench::new();
    let a_body = b.spawn(0.0, 0.0);
    let far_body = b.spawn(500.0, 0.0);
    let thing = b.spawn(2.0, 0.0);
    let a = b.player("a", Some(a_body));
    let far = b.player("far", Some(far_body));
    b.finish_tick();

    b.next_tick();
    send_network_event(
        &mut b.world,
        EventTarget::AllPlayers,
        "One",
        NeutralValue::Null,
    );
    send_network_event(
        &mut b.world,
        EventTarget::ViewersOf(thing),
        "Two",
        NeutralValue::Null,
    );
    send_network_event(
        &mut b.world,
        EventTarget::Player(far.clone()),
        "Three",
        NeutralValue::Null,
    );
    send_network_event(
        &mut b.world,
        EventTarget::AllPlayers,
        "Four",
        NeutralValue::Null,
    );
    let ups = b.finish_tick();
    let names = |u| -> Vec<String> {
        ups[u]
            .events
            .iter()
            .map(|e| e.event.name.to_string())
            .collect()
    };
    assert_eq!(names(&a), vec!["One", "Two", "Four"]);
    assert_eq!(names(&far), vec!["One", "Three", "Four"]);
}
