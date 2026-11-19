//! Buffering player input by tick.

mod common;

use std::sync::Arc;

use common::Bench;
use crucible_ecs::Tick;
use crucible_players::NetUserId;
use crucible_replication::{
    ButtonState, ClientEvent, InputBuffer, InputCommand, InputPayload, InputReceipt, NeutralValue,
    PlayerInput,
};

fn command(client_tick: u32, sequence: u32) -> PlayerInput {
    PlayerInput {
        client_tick: Tick(client_tick),
        sub_tick: 0,
        sequence,
        payload: InputPayload::Command(InputCommand {
            function: Arc::from("MoveUp"),
            state: ButtonState::Down,
            coordinates: None,
            target: None,
        }),
    }
}

fn client_event(client_tick: u32, sequence: u32) -> PlayerInput {
    PlayerInput {
        client_tick: Tick(client_tick),
        sub_tick: 0,
        sequence,
        payload: InputPayload::Event(ClientEvent {
            name: Arc::from("UseItem"),
            value: NeutralValue::Null,
        }),
    }
}

/// Scenario: input from a future tick.
#[test]
fn input_from_future_tick() {
    let user = NetUserId::new("a");
    let mut buffer = InputBuffer::new();
    assert_eq!(buffer.push(&user, command(12, 1)), InputReceipt::Queued);
    assert!(buffer.take_due(Tick(10)).is_empty());
    assert!(buffer.take_due(Tick(11)).is_empty());
    assert_eq!(buffer.last_processed(&user), 0);
    let due = buffer.take_due(Tick(12));
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].input.sequence, 1);
    assert!(!due[0].late);
    assert_eq!(buffer.last_processed(&user), 1);
}

/// Scenario: late input.
#[test]
fn late_input() {
    let mut b = Bench::new();
    let body = b.spawn(0.0, 0.0);
    let a = b.player("a", Some(body));
    while b.tick() < Tick(101) {
        b.next_tick();
    }
    b.finish_tick();

    // Input for tick 100 arrives while the server is processing tick 102.
    b.next_tick();
    assert_eq!(b.tick(), Tick(102));
    b.replication.receive_input(&a, command(100, 4));
    let due = b.replication.take_due_inputs(b.tick());
    assert_eq!(due.len(), 1, "late input is applied in the current tick");
    assert!(due[0].late);
    let ups = b.finish_tick();
    assert_eq!(ups[&a].frame.to_tick, Tick(102));
    assert_eq!(ups[&a].frame.last_processed_input, 4);
}

/// Scenario: sequence number of processed input.
#[test]
fn processed_input_number() {
    let mut b = Bench::new();
    let body = b.spawn(0.0, 0.0);
    let a = b.player("a", Some(body));
    let ups = b.finish_tick();
    assert_eq!(ups[&a].frame.last_processed_input, 0);

    b.next_tick();
    for seq in 55..=57 {
        b.replication.receive_input(&a, command(2, seq));
    }
    let applied: Vec<u32> = b
        .replication
        .take_due_inputs(b.tick())
        .iter()
        .map(|d| d.input.sequence)
        .collect();
    assert_eq!(applied, vec![55, 56, 57]);
    let ups = b.finish_tick();
    assert_eq!(ups[&a].frame.last_processed_input, 57);
}

/// Scenario: sequence number of processed but rejected input.
#[test]
fn rejected_input_is_acknowledged_without_application() {
    let mut b = Bench::new();
    let body = b.spawn(0.0, 0.0);
    let user = b.player("a", Some(body));
    b.replication.reject_input(&user, 17);
    assert!(b.replication.take_due_inputs(b.tick()).is_empty());
    let updates = b.finish_tick();
    assert_eq!(updates[&user].frame.last_processed_input, 17);
}

/// Commands and events share sequence numbers and are applied in tick and sequence order.
#[test]
fn order_by_tick_then_sequence_shared_with_client_events() {
    let user = NetUserId::new("a");
    let mut buffer = InputBuffer::new();
    buffer.push(&user, command(5, 3));
    buffer.push(&user, client_event(4, 2));
    buffer.push(&user, command(5, 1)); // Same tick with a lower sequence number.
    buffer.push(&user, client_event(6, 4));
    let due = buffer.take_due(Tick(5));
    let order: Vec<u32> = due.iter().map(|d| d.input.sequence).collect();
    assert_eq!(order, vec![2, 1, 3]);
    assert!(matches!(due[0].input.payload, InputPayload::Event(_)));
    assert_eq!(buffer.last_processed(&user), 3);
    assert_eq!(buffer.pending(&user), 1);
    let due = buffer.take_due(Tick(6));
    assert_eq!(due[0].input.sequence, 4);
    assert_eq!(
        buffer.last_processed(&user),
        4,
        "client events are acknowledged too"
    );
}

#[test]
fn reliable_events_keep_transport_order_alongside_predicted_inputs() {
    let mut b = Bench::new();
    let actor = b.spawn(0.0, 0.0);
    let user = b.player("a", Some(actor));
    let event = |name| ClientEvent {
        name: Arc::from(name),
        value: NeutralValue::Null,
    };
    b.replication.receive_input(&user, command(12, 66));
    b.replication.receive_input(&user, command(12, 64));
    b.replication
        .receive_ordered_event(&user, Tick(12), 0, event("queue"));
    b.replication
        .receive_ordered_event(&user, Tick(12), 65, event("close"));
    assert_eq!(
        b.replication
            .receive_ordered_event(&user, Tick(12), 65, event("close")),
        InputReceipt::Duplicate
    );
    assert!(b.replication.take_due_inputs(Tick(11)).is_empty());
    let due = b.replication.take_due_inputs(Tick(12));
    let sequences: Vec<_> = due.iter().map(|accepted| accepted.input.sequence).collect();
    assert_eq!(sequences, [64, 66, 0, 65]);
    assert_eq!(b.replication.inputs().last_processed(&user), 66);
}

/// Resending accepted input does not apply it a second time.
#[test]
fn repeated_input_is_not_applied_twice() {
    let user = NetUserId::new("a");
    let mut buffer = InputBuffer::new();
    assert_eq!(buffer.push(&user, command(3, 1)), InputReceipt::Queued);
    assert_eq!(buffer.push(&user, command(3, 1)), InputReceipt::Duplicate);
    assert_eq!(buffer.take_due(Tick(3)).len(), 1);
    assert_eq!(buffer.push(&user, command(3, 1)), InputReceipt::Duplicate);
    assert!(buffer.take_due(Tick(4)).is_empty());
}
