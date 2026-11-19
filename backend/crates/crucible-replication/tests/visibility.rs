//! Component policies, entity visibility, and the new entity budget.
//!

mod common;

use std::collections::HashSet;

use common::{Bench, Hidden, Secret, Stats, component_names};
use crucible_ecs::Tick;
use crucible_map::{Vec2, set_world_position};
use crucible_replication::{AlwaysVisible, HidesContents, PlayerStateFrame, ReplicationConfig};

fn mentions(frame: &PlayerStateFrame, component: &str) -> bool {
    frame.entities.iter().any(|e| {
        e.components.iter().any(|c| c.name.as_ref() == component)
            || e.component_set
                .as_ref()
                .is_some_and(|set| set.iter().any(|n| n.as_ref() == component))
    })
}

/// Scenario: hidden server state.
#[test]
fn hidden_server_state() {
    let mut b = Bench::new();
    let a_body = b.spawn(0.0, 0.0);
    let b_body = b.spawn(1.0, 0.0);
    b.world.add_component(a_body, Hidden { note: 7 }).unwrap();
    b.world.add_component(a_body, Stats::default()).unwrap();
    let a = b.player("a", Some(a_body));
    let bb = b.player("b", Some(b_body));

    let mut frames: Vec<PlayerStateFrame> =
        b.finish_tick().into_values().map(|u| u.frame).collect();
    b.ack(&a);
    b.ack(&bb);
    for n in 0..5 {
        b.next_tick();
        Hidden::set_note(&mut b.world, a_body, n);
        Stats::set_level(&mut b.world, a_body, n);
        frames.extend(b.finish_tick().into_values().map(|u| u.frame));
        b.ack(&a);
        b.ack(&bb);
    }
    // It must also stay hidden in a requested full state.
    b.replication.request_full_state(&a);
    b.replication.request_full_state(&bb);
    frames.extend(b.step().into_values().map(|u| u.frame));

    assert!(frames.len() >= 14);
    assert!(
        frames.iter().any(|f| mentions(f, "Stats")),
        "the other component is replicated as a control"
    );
    for f in &frames {
        assert!(
            !mentions(f, "Hidden"),
            "a server-only component was sent in a frame: {f:?}"
        );
    }
}

/// Scenario: owner-only data.
#[test]
fn owner_data() {
    let mut b = Bench::new();
    let a_body = b.spawn(0.0, 0.0);
    let b_body = b.spawn(1.0, 0.0);
    b.world.add_component(a_body, Secret { code: 1 }).unwrap();
    let a = b.player("a", Some(a_body));
    let bb = b.player("b", Some(b_body));
    let net = b.net(a_body);

    let ups = b.finish_tick();
    let for_a = ups[&a].frame.entity(net).expect("A sees their own entity");
    let for_b = ups[&bb].frame.entity(net).expect("B sees A's entity");
    assert!(component_names(for_a).contains(&"Secret"));
    assert!(!component_names(for_b).contains(&"Secret"));
    assert!(
        !for_b
            .component_set
            .as_ref()
            .unwrap()
            .iter()
            .any(|n| n.as_ref() == "Secret")
    );
    b.ack(&a);
    b.ack(&bb);

    // Only the owner field changed, so A receives it and B gets no update for this entity.
    b.next_tick();
    Secret::set_code(&mut b.world, a_body, 2);
    let ups = b.finish_tick();
    let for_a = ups[&a].frame.entity(net).unwrap();
    assert_eq!(component_names(for_a), vec!["Secret"]);
    assert!(ups[&bb].frame.entity(net).is_none());
}

/// Scenario: leaving the interest radius.
#[test]
fn leaving_interest_radius() {
    let mut b = Bench::new();
    let body = b.spawn(0.0, 0.0);
    let thing = b.spawn(5.0, 0.0);
    b.world.add_component(thing, Stats::default()).unwrap();
    let a = b.player("a", Some(body));
    let net = b.net(thing);

    let ups = b.finish_tick();
    assert!(ups[&a].frame.entity(net).unwrap().full);
    b.ack(&a);

    // Move outside the default interest radius of 25 m.
    b.next_tick();
    set_world_position(&mut b.world, thing, Vec2::new(40.0, 0.0)).unwrap();
    let ups = b.finish_tick();
    assert_eq!(ups[&a].frame.left_view, vec![net]);
    assert!(ups[&a].frame.entity(net).is_none());
    assert!(!b.replication.is_visible(&a, thing));

    // Repeat the notification until the player acknowledges exit and stop sending changes.
    b.next_tick();
    Stats::set_level(&mut b.world, thing, 5);
    let ups = b.finish_tick();
    assert_eq!(ups[&a].frame.left_view, vec![net]);
    assert!(ups[&a].frame.entity(net).is_none());
    b.ack(&a);
    for _ in 0..3 {
        b.next_tick();
        Stats::set_score(&mut b.world, thing, 1);
        let ups = b.finish_tick();
        assert!(ups[&a].frame.left_view.is_empty());
        assert!(ups[&a].frame.entity(net).is_none());
        b.ack(&a);
    }

    // Reentry sends the full state again.
    b.next_tick();
    set_world_position(&mut b.world, thing, Vec2::new(3.0, 0.0)).unwrap();
    let ups = b.finish_tick();
    let state = ups[&a].frame.entity(net).unwrap();
    assert!(state.full);
    assert!(component_names(state).contains(&"Stats"));
}

/// Scenario: closed container.
#[test]
fn closed_container() {
    let mut b = Bench::new();
    let body = b.spawn(0.0, 0.0);
    let chest = b.spawn(2.0, 0.0);
    b.world.add_component(chest, HidesContents).unwrap();
    let item = b.spawn_in(chest);
    b.world.add_component(item, Stats::default()).unwrap();
    let a = b.player("a", Some(body));
    let (chest_net, item_net) = (b.net(chest), b.net(item));

    let ups = b.finish_tick();
    let frame = &ups[&a].frame;
    assert!(frame.entity(chest_net).is_some(), "the chest is visible");
    assert!(frame.entity(item_net).is_none(), "the item is hidden");
    assert!(!b.replication.is_visible(&a, item));
    b.ack(&a);
    for _ in 0..3 {
        b.next_tick();
        Stats::set_level(&mut b.world, item, 3);
        let ups = b.finish_tick();
        let frame = &ups[&a].frame;
        assert!(frame.entity(item_net).is_none());
        assert!(!frame.left_view.contains(&item_net) && !frame.deletions.contains(&item_net));
        b.ack(&a);
    }

    // Opening the container sends the item's full state as it enters view.
    b.next_tick();
    b.world.remove_component::<HidesContents>(chest);
    let ups = b.finish_tick();
    assert!(ups[&a].frame.entity(item_net).unwrap().full);
}

/// A player can see their own closed bag's contents. Other players cannot.
#[test]
fn own_equipment_is_visible_inside_closed_container() {
    let mut b = Bench::new();
    let a_body = b.spawn(0.0, 0.0);
    let b_body = b.spawn(1.0, 0.0);
    let bag = b.spawn_in(a_body);
    b.world.add_component(bag, HidesContents).unwrap();
    let item = b.spawn_in(bag);
    let a = b.player("a", Some(a_body));
    let bb = b.player("b", Some(b_body));

    b.finish_tick();
    assert!(b.replication.is_visible(&a, item));
    assert!(b.replication.is_visible(&bb, bag));
    assert!(!b.replication.is_visible(&bb, item));
}

/// An always-visible entity is visible at any distance, together with its map ancestor.
#[test]
fn always_visible_entities_ignore_radius() {
    let mut b = Bench::new();
    let body = b.spawn(0.0, 0.0);
    let far = b.spawn(500.0, 0.0);
    let far_marked = b.spawn(-500.0, 0.0);
    b.world.add_component(far_marked, AlwaysVisible).unwrap();
    let a = b.player("a", Some(body));
    let lobby = b.player("lobby", None);

    b.finish_tick();
    assert!(b.replication.is_visible(&a, far_marked));
    assert!(!b.replication.is_visible(&a, far));
    assert!(b.replication.is_visible(&a, b.map));
    assert!(
        b.replication.is_visible(&lobby, far_marked),
        "visible even without a controlled entity"
    );
    assert!(!b.replication.is_visible(&lobby, body));
}

/// On ownership transfer, the new owner receives owner-only components,
/// and the old owner receives a component list without them.
#[test]
fn owner_change_resends_entity() {
    let mut b = Bench::new();
    let body = b.spawn(0.0, 0.0);
    let spare = b.spawn(1.0, 0.0);
    b.world.add_component(body, Secret { code: 3 }).unwrap();
    let a = b.player("a", Some(body));
    let bb = b.player("b", Some(spare));
    let net = b.net(body);
    b.finish_tick();
    b.ack(&a);
    b.ack(&bb);

    b.next_tick();
    b.replication.set_controlled_entity(&a, Some(spare));
    b.replication.set_controlled_entity(&bb, Some(body));
    let ups = b.finish_tick();
    let for_b = ups[&bb].frame.entity(net).unwrap();
    assert!(for_b.full);
    assert!(component_names(for_b).contains(&"Secret"));
    let for_a = ups[&a].frame.entity(net).unwrap();
    let set = for_a.component_set.as_ref().unwrap();
    assert!(!set.iter().any(|n| n.as_ref() == "Secret"));
}

/// Scenario: new entity budget.
#[test]
fn new_entity_budget() {
    const BUDGET: usize = 5;
    let mut b = Bench::with_config(ReplicationConfig {
        new_entity_budget: BUDGET,
        ..ReplicationConfig::default()
    });
    let mut area = Vec::new();
    for i in 0..(3 * BUDGET) {
        area.push(b.spawn(1.0 + i as f32, 1.0));
    }
    let body = b.spawn(0.0, 0.0);
    let a = b.player("a", Some(body));

    let mut seen = HashSet::new();
    let mut frames = 0;
    loop {
        let ups = if frames == 0 {
            b.finish_tick()
        } else {
            b.step()
        };
        let frame = &ups[&a].frame;
        let new: Vec<_> = frame
            .entities
            .iter()
            .filter(|e| seen.insert(e.entity))
            .collect();
        assert!(
            new.len() <= BUDGET,
            "frame {} contains {} new entities",
            frame.to_tick,
            new.len()
        );
        if frames == 0 {
            assert!(
                frame.entity(b.net(body)).is_some(),
                "the first state includes the controlled entity"
            );
        }
        b.ack(&a);
        frames += 1;
        if area.iter().all(|&e| b.replication.is_visible(&a, e)) {
            break;
        }
        assert!(
            frames < 10,
            "every entity in the area should enter view within a few ticks"
        );
    }
    assert!(frames > 1, "the entities arrive across several ticks");
    assert_eq!(b.tick(), Tick(frames));
}
