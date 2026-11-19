//! Protocol-neutral replication with visibility, state frames, acknowledgments, tick-based input,
//! and reliable events.
//!
//! The crate decides what to send, when, and to which players. It returns transport-neutral
//! data for adapters to encode and send.
//!
//! # Adapter boundary
//!
//! | Core | Adapter |
//! |---|---|
//! | Component policies ([`NetworkPolicy`](crucible_ecs::NetworkPolicy)), names, and [`NeutralValue`] fields | Numeric IDs, field order, encoding, and compression |
//! | Visibility radius, [`AlwaysVisible`], equipment, [`HidesContents`], and the budget for new entities | Wire messages for entities leaving visibility |
//! | [`PlayerStateFrame`] deltas from the acknowledged tick, complete state (`from_tick == 0`), deletions, and exits | Encoding frames and session lists from `crucible-players` |
//! | [`Replication::acknowledge`], full-state requests, and reliable delivery due to acknowledgment lag | Reliable delivery by size, transport guarantees, and optional immediate acknowledgment after reliable sends |
//! | [`PlayerInput`] buffer, shared input and event sequence, and `last_processed_input` | Wire message decoding and input-key mapping |
//! | Ordered and tick-stamped events ([`NetEvents`], [`EventTarget`]) | Event serialization and delivery channel |
//!
//! # Tick order
//!
//! 1. [`Replication::take_due_inputs`] returns input for the simulation to dispatch to systems.
//! 2. Systems run, physics advances, and queued entities are deleted.
//! 3. [`Replication::update`] produces a state frame and events for each player.
//!
//! # Example
//!
//! ```
//! use crucible_ecs::{Tick, World};
//! use crucible_map::{Vec2, create_map, spawn_at};
//! use crucible_players::NetUserId;
//! use crucible_replication::{Replication, ReplicationConfig, ReplicatedComponents, register_map_states};
//!
//! let mut builder = World::builder();
//! crucible_map::register(&mut builder).unwrap();
//! crucible_replication::register(&mut builder).unwrap();
//! let (mut world, _systems) = builder.build().unwrap();
//!
//! let mut components = ReplicatedComponents::new();
//! register_map_states(&mut components, world.registry()).unwrap();
//! let mut replication = Replication::new(ReplicationConfig::default(), components);
//!
//! world.set_current_tick(Tick(1));
//! let (map, _) = create_map(&mut world);
//! let body = spawn_at(&mut world, map, Vec2::new(1.0, 1.0)).unwrap();
//! let user = NetUserId::new("player-a");
//! replication.add_viewer(user.clone());
//! replication.set_controlled_entity(&user, Some(body));
//!
//! let updates = replication.update(&mut world);
//! let frame = &updates[0].frame;
//! assert!(frame.is_full());
//! let net = world.net_entity(body).unwrap();
//! assert!(frame.entity(net).unwrap().component("Transform").is_some());
//!
//! // The client acknowledged tick 1. Nothing changed during tick 2, so its frame is empty.
//! replication.acknowledge(&user, Tick(1));
//! world.set_current_tick(Tick(2));
//! let frame = &replication.update(&mut world)[0].frame;
//! assert_eq!(frame.from_tick, Tick(1));
//! assert!(frame.is_empty());
//! ```

mod components;
mod events;
mod frame;
mod input;
mod map_states;
mod module;
mod replication;
mod value;
mod visibility;

pub use components::{ReplicatedComponents, ReplicationError, StateContext, StateSerializer};
pub use events::{EventTarget, NetEvents, NetworkEvent, OutgoingEvent, send_network_event};
pub use frame::{ComponentState, EntityState, PlayerStateFrame};
pub use input::{
    ButtonState, ClientEvent, DueInput, InputBuffer, InputCommand, InputControl, InputPayload,
    InputReceipt, NetCoordinates, PlayerInput,
};
pub use map_states::{grid_state, map_state, register_map_states, transform_state};
pub use module::{
    ClientEventReceived, InputCommandReceived, MODULE_ID, OutgoingUpdates, ReplicationModule,
    ReplicationState,
};
pub use replication::{
    DEFAULT_ENTER_BUDGET, DEFAULT_FORCE_ACK_THRESHOLD, DEFAULT_NEW_ENTITY_BUDGET,
    DEFAULT_VIEW_RADIUS, PlayerUpdate, Replication, ReplicationConfig, UpdateAuthority,
};
pub use value::{FieldName, FieldSelection, NeutralValue, StructWriter};
pub use visibility::{AlwaysVisible, HidesContents, SOURCE, register};
