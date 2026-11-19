//! Test server and virtual clients using the transport-independent replication interface.

use std::collections::BTreeMap;
use std::error::Error;
use std::sync::Arc;
use std::time::Duration;

use crucible_ecs::{EntityUid, NetEntity, Tick};
use crucible_ext::{LaunchConfig, Launcher, ServerRuntime};
use crucible_loop::ManualClock;
use crucible_map::{Vec2, place};
use crucible_players::{ConnectionId, NetUserId, PlayerManager, PlayerSession, Players};
use crucible_replication::{
    ButtonState, InputCommand, InputPayload, InputReceipt, NeutralValue, OutgoingEvent,
    OutgoingUpdates, PlayerInput, PlayerStateFrame, PlayerUpdate, Replication, ReplicationState,
};

/// A virtual client's copy of a visible entity.
#[derive(Clone, Debug, Default)]
pub struct VirtualEntity {
    /// Prototype from the full state.
    pub prototype: Option<Arc<str>>,
    /// Latest component values in a transport-independent form.
    pub components: BTreeMap<String, NeutralValue>,
}

impl VirtualEntity {
    /// Returns the component value by name.
    pub fn component(&self, name: &str) -> Option<&NeutralValue> {
        self.components.get(name)
    }

    /// Returns the local position from the replicated `Transform`.
    pub fn local_position(&self) -> Option<[f32; 2]> {
        match self.component("Transform")?.field("local_position")? {
            NeutralValue::Vec2(position) => Some(*position),
            _ => None,
        }
    }
}

/// Receives frames, events, and acknowledgments without a transport.
pub struct VirtualClient {
    /// Player session identifier.
    pub user: NetUserId,
    /// Controlled entity on the server. The harness refreshes it from the session after each tick
    /// so the new character is used in the next round. Between rounds, an unattached session
    /// retains its most recently controlled entity here.
    pub entity: EntityUid,
    /// Network ID of the controlled entity, updated with `entity`.
    pub net_entity: NetEntity,
    /// Visible entities after applying frames.
    pub entities: BTreeMap<NetEntity, VirtualEntity>,
    /// Received frames in tick order.
    pub frames: Vec<PlayerStateFrame>,
    /// Received reliable events in delivery order.
    pub events: Vec<OutgoingEvent>,
    ack_delay: Option<u32>,
    pending_acks: Vec<(u32, Tick)>,
    next_input: u32,
}

impl VirtualClient {
    fn new(user: NetUserId, entity: EntityUid, net_entity: NetEntity) -> Self {
        VirtualClient {
            user,
            entity,
            net_entity,
            entities: BTreeMap::new(),
            frames: Vec::new(),
            events: Vec::new(),
            ack_delay: Some(0),
            pending_acks: Vec::new(),
            next_input: 1,
        }
    }

    fn receive(&mut self, update: PlayerUpdate) {
        let frame = update.frame;
        for net in frame.deletions.iter().chain(&frame.left_view) {
            self.entities.remove(net);
        }
        for state in &frame.entities {
            let entity = self.entities.entry(state.entity).or_default();
            if state.full {
                entity.components.clear();
                entity.prototype = state.prototype.clone();
            }
            if let Some(names) = &state.component_set {
                entity
                    .components
                    .retain(|name, _| names.iter().any(|n| n.as_ref() == name));
            }
            for component in &state.components {
                let name = component.name.to_string();
                let value = component.value.as_ref().clone();
                match (entity.components.get_mut(&name), value) {
                    (Some(NeutralValue::Struct(old)), NeutralValue::Struct(fields))
                        if component.fields.is_some() =>
                    {
                        for (field, value) in fields {
                            if let Some((_, previous)) = old.iter_mut().find(|(n, _)| *n == field) {
                                *previous = value;
                            } else {
                                old.push((field, value));
                            }
                        }
                    }
                    (_, value) => {
                        entity.components.insert(name, value);
                    }
                }
            }
        }
        if let Some(delay) = self.ack_delay {
            self.pending_acks
                .push((frame.to_tick.0.saturating_add(delay), frame.to_tick));
        }
        self.events.extend(update.events);
        self.frames.push(frame);
    }
}

/// A harness player in the lobby or a virtual client controlling an entity.
enum Seat {
    /// The player entered the lobby and has no character. Frames without a controlled entity
    /// are neither retained nor acknowledged. The replication model sends unacknowledged entities
    /// with their full state, so the client can populate its copy when the character spawns.
    Lobby(NetUserId),
    /// A player with a character.
    Playing(VirtualClient),
}

impl Seat {
    fn user(&self) -> &NetUserId {
        match self {
            Seat::Lobby(user) => user,
            Seat::Playing(client) => &client.user,
        }
    }
}

/// An in-process server with a controllable clock and virtual clients.
///
/// The caller supplies the launcher and configuration, and the test supplies the fixture extension.
/// Game loop phases collect input and build frames after physics.
///
/// Connect a player in one of two ways:
/// - [`connect`](Self::connect) starts with a character and skips round flow.
/// - [`join_lobby`](Self::join_lobby) starts in the round lobby. The round's spawn policy
///   creates the character. The harness exposes the player as a virtual
///   client ([`client`](Self::client)) from the first tick with an attached entity.
pub struct TestServer {
    runtime: ServerRuntime,
    clock: ManualClock,
    clients: Vec<Seat>,
}

impl TestServer {
    /// Launches an extension with the replication module enabled.
    pub fn new(launcher: Launcher, config: LaunchConfig) -> Result<Self, Box<dyn Error>> {
        let mut runtime = launcher.launch(config)?;
        let clock = ManualClock::new();
        runtime = runtime.with_clock(Box::new(clock.clone()));
        assert!(
            runtime.world().resource::<ReplicationState>().is_some(),
            "replication module is not enabled"
        );
        Ok(Self {
            runtime,
            clock,
            clients: Vec::new(),
        })
    }

    /// Returns the built server for map loading and world inspection.
    pub fn runtime(&self) -> &ServerRuntime {
        &self.runtime
    }

    /// Returns the built server for map loading and between-tick world changes.
    pub fn runtime_mut(&mut self) -> &mut ServerRuntime {
        &mut self.runtime
    }

    /// Returns the controllable clock. A clone shares the game loop clock.
    pub fn clock(&self) -> ManualClock {
        self.clock.clone()
    }

    /// Returns the current server tick.
    pub fn tick(&self) -> Tick {
        self.runtime.game().current_tick()
    }

    /// Configures replication before stepping ticks, such as registering extension serializers.
    pub fn configure_replication(&mut self, configure: impl FnOnce(&mut Replication)) {
        configure(
            &mut self
                .runtime
                .world()
                .resource::<ReplicationState>()
                .unwrap()
                .lock(),
        );
    }

    /// Manually acknowledges a received frame, including after lost acknowledgments.
    pub fn acknowledge(&mut self, index: usize, tick: Tick) -> bool {
        self.runtime
            .world()
            .resource::<ReplicationState>()
            .unwrap()
            .lock()
            .acknowledge(self.clients[index].user(), tick)
    }

    /// Connects a player and creates a prototype character on a map or grid.
    pub fn connect(
        &mut self,
        name: &str,
        prototype: &str,
        parent: EntityUid,
        position: Vec2,
    ) -> Result<usize, Box<dyn Error>> {
        let index = self.clients.len();
        let user = NetUserId::new(format!("virtual-{}", index + 1));
        let entity = self.runtime.spawn(prototype)?;
        place(
            self.runtime.world_mut(),
            entity,
            Some(parent),
            position,
            0.0,
        )?;
        let net_entity = self
            .runtime
            .world()
            .net_entity(entity)
            .expect("spawned entity");
        let now = self
            .runtime
            .world()
            .resource::<crucible_loop::SimTime>()
            .expect("game loop clock")
            .elapsed();
        let world = self.runtime.world_mut();
        PlayerManager::begin_connect(world, &user, name, ConnectionId(index as u64 + 1), now);
        PlayerManager::finish_connect(world, &user)?;
        PlayerManager::enter_game(world, &user)?;
        PlayerManager::attach(world, &user, entity)?;
        self.clients
            .push(Seat::Playing(VirtualClient::new(user, entity, net_entity)));
        Ok(index)
    }

    /// Adds a player to the round lobby without a character. The connected player starts unready
    /// (see [`set_ready`](Self::set_ready)). The spawn policy creates the character
    /// at the start of the round.
    pub fn join_lobby(&mut self, name: &str) -> Result<usize, Box<dyn Error>> {
        let index = self.clients.len();
        let user = NetUserId::new(format!("virtual-{}", index + 1));
        let now = self
            .runtime
            .world()
            .resource::<crucible_loop::SimTime>()
            .expect("game loop clock")
            .elapsed();
        let world = self.runtime.world_mut();
        PlayerManager::begin_connect(world, &user, name, ConnectionId(index as u64 + 1), now);
        PlayerManager::finish_connect(world, &user)?;
        self.runtime.rounds_mut().add_player(user.clone());
        self.clients.push(Seat::Lobby(user));
        Ok(index)
    }

    /// Sets a player's readiness in the round lobby.
    pub fn set_ready(&mut self, index: usize, ready: bool) {
        let user = self.clients[index].user().clone();
        self.runtime.rounds_mut().set_ready(user, ready);
    }

    /// Returns the user ID for a connection index, including in the lobby.
    pub fn user(&self, index: usize) -> &NetUserId {
        self.clients[index].user()
    }

    /// Returns the player session for a connection index.
    pub fn session(&self, index: usize) -> Option<&PlayerSession> {
        self.runtime
            .world()
            .resource::<Players>()
            .and_then(|players| players.session(self.clients[index].user()))
    }

    /// Returns the client for a connection index.
    ///
    /// # Panics
    /// Panics if the player is still in the lobby without a character (see [`try_client`](Self::try_client)).
    pub fn client(&self, index: usize) -> &VirtualClient {
        self.try_client(index)
            .unwrap_or_else(|| panic!("player {index} is in the lobby without a character"))
    }

    /// Returns the client for a connection index, or `None` if the player has never had a character.
    pub fn try_client(&self, index: usize) -> Option<&VirtualClient> {
        match &self.clients[index] {
            Seat::Lobby(_) => None,
            Seat::Playing(client) => Some(client),
        }
    }

    fn client_mut(&mut self, index: usize) -> &mut VirtualClient {
        match &mut self.clients[index] {
            Seat::Lobby(_) => panic!("player {index} is in the lobby without a character"),
            Seat::Playing(client) => client,
        }
    }

    /// Acknowledges frames after `delay` ticks. `None` simulates lost acknowledgments.
    pub fn set_ack_delay(&mut self, index: usize, delay: Option<u32>) {
        let client = self.client_mut(index);
        client.ack_delay = delay;
        if delay.is_none() {
            client.pending_acks.clear();
        }
    }

    /// Submits transport-independent input to the replication buffer.
    pub fn send_input(&mut self, index: usize, input: PlayerInput) -> InputReceipt {
        let user = self.clients[index].user();
        self.runtime
            .world()
            .resource::<ReplicationState>()
            .unwrap()
            .lock()
            .receive_input(user, input)
    }

    /// Presses or releases a movement button on the next tick.
    pub fn move_button(&mut self, index: usize, function: &str, down: bool) -> InputReceipt {
        let client_tick = Tick(self.tick().0 + 1);
        let client = self.client_mut(index);
        let input = PlayerInput {
            client_tick,
            sub_tick: 0,
            sequence: client.next_input,
            payload: InputPayload::Command(InputCommand {
                function: Arc::from(function),
                state: if down {
                    ButtonState::Down
                } else {
                    ButtonState::Up
                },
                coordinates: None,
                target: None,
            }),
        };
        client.next_input += 1;
        self.send_input(index, input)
    }

    /// Requests a full frame and clears the client copy before recovery.
    pub fn request_full_state(&mut self, index: usize) -> bool {
        if let Seat::Playing(client) = &mut self.clients[index] {
            client.entities.clear();
            client.pending_acks.clear();
        }
        self.runtime
            .world()
            .resource::<ReplicationState>()
            .unwrap()
            .lock()
            .request_full_state(self.clients[index].user())
    }

    /// Runs exactly `count` ticks without waiting for the system clock.
    pub fn step(&mut self, count: u32) {
        for _ in 0..count {
            self.clock.advance(
                self.runtime
                    .world()
                    .resource::<crucible_loop::SimTime>()
                    .expect("game loop clock")
                    .tick_duration(),
            );
            self.runtime.tick();
            let tick = self.tick();
            self.sync_controlled_entities();
            let updates = self
                .runtime
                .world_mut()
                .resource_mut::<OutgoingUpdates>()
                .unwrap()
                .drain();
            for update in updates {
                if let Some(Seat::Playing(client)) =
                    self.clients.iter_mut().find(|s| *s.user() == update.user)
                {
                    client.receive(update);
                }
            }
            let model = self
                .runtime
                .world()
                .resource::<ReplicationState>()
                .unwrap()
                .clone();
            let mut model = model.lock();
            for client in self.clients.iter_mut().filter_map(|s| match s {
                Seat::Lobby(_) => None,
                Seat::Playing(client) => Some(client),
            }) {
                let mut latest = None;
                client.pending_acks.retain(|(due, ack)| {
                    if *due <= tick.0 {
                        latest = Some(*ack);
                        false
                    } else {
                        true
                    }
                });
                if let Some(ack) = latest {
                    model.acknowledge(&client.user, ack);
                }
            }
        }
    }

    /// Reconciles controlled entities with player sessions after a tick. A lobby player
    /// with an attached character becomes a virtual client. When an existing client gets a new
    /// character, the harness updates its `entity` and `net_entity`.
    fn sync_controlled_entities(&mut self) {
        let world = self.runtime.world();
        let Some(players) = world.resource::<Players>() else {
            return;
        };
        for seat in &mut self.clients {
            let Some(entity) = players.session(seat.user()).and_then(PlayerSession::entity) else {
                continue;
            };
            let Some(net_entity) = world.net_entity(entity) else {
                continue;
            };
            match seat {
                Seat::Lobby(user) => {
                    *seat = Seat::Playing(VirtualClient::new(user.clone(), entity, net_entity));
                }
                Seat::Playing(client) => {
                    client.entity = entity;
                    client.net_entity = net_entity;
                }
            }
        }
    }

    /// Advances the controllable clock without running ticks.
    pub fn advance_time(&self, duration: Duration) {
        self.clock.advance(duration);
    }
}
