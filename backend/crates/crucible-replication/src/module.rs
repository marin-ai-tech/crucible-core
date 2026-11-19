//! Core module for input at the start of the tick and frame assembly after entity deletion.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard};

use crucible_ecs::{EntityUid, NetEntity, System, SystemInit, World};
use crucible_ext::{CoreModule, RegisterContext};
use crucible_loop::{CurrentRoundPhase, RoundPhase, RoundPhaseChanged, TickPhase};
use crucible_players::{NetUserId, PlayerControlled, PlayerDetached, Players, SessionStatus};

use crate::{
    ButtonState, ClientEvent, InputCommand, InputPayload, NetEvents, PlayerUpdate,
    ReplicatedComponents, Replication, ReplicationConfig, register_map_states,
};

/// ID used for the replication module in an integration manifest.
pub const MODULE_ID: &str = "replication";

/// An accepted input event sent to the controlled entity.
#[derive(Clone, Debug, PartialEq)]
pub struct InputCommandReceived {
    /// Input function name.
    pub function: Arc<str>,
    /// Press or release.
    pub state: ButtonState,
    /// Network target of pointer input, when present.
    pub target: Option<NetEntity>,
    /// Fractional position within the input tick.
    pub sub_tick: u16,
    /// Command sequence number.
    pub sequence: u32,
    /// The command arrived after its client tick.
    pub late: bool,
}

/// An accepted client request sent to the controlled entity.
pub struct ClientEventReceived {
    pub event: ClientEvent,
}

/// Shared replication model in a world resource. Adapters submit input and acknowledgments here.
#[derive(Clone)]
pub struct ReplicationState(Arc<Mutex<Replication>>);

impl ReplicationState {
    /// Exclusive access to the model for input, acknowledgments, and configuration.
    pub fn lock(&self) -> MutexGuard<'_, Replication> {
        self.0.lock().expect("replication model lock was poisoned")
    }
}

/// Frames and events prepared in the replication phase and waiting for adapters.
#[derive(Default)]
pub struct OutgoingUpdates(Vec<PlayerUpdate>);

impl OutgoingUpdates {
    /// Takes all pending updates.
    pub fn drain(&mut self) -> Vec<PlayerUpdate> {
        std::mem::take(&mut self.0)
    }

    /// Takes a viewer's update and leaves the other updates for their adapters.
    pub fn take_latest_for(&mut self, user: &crucible_players::NetUserId) -> Option<PlayerUpdate> {
        self.0.extract_if(.., |update| &update.user == user).last()
    }
}

/// Protocol-neutral replication module (`replication`).
#[derive(Clone, Copy, Default)]
pub struct ReplicationModule;

impl CoreModule for ReplicationModule {
    fn id(&self) -> &'static str {
        MODULE_ID
    }

    fn dependencies(&self) -> &'static [&'static str] {
        &[crucible_map::MODULE_ID]
    }

    fn register(&self, ctx: &mut RegisterContext<'_>) {
        ctx.world_builder(crate::register)
            .resource(NetEvents::default())
            .resource(OutgoingUpdates::default())
            .resource(ScopedHeldInputs::default())
            .system(ScopedInputLifecycle)
            .tick_phase(TickPhase::Input, "replication_input", input_phase)
            .tick_phase(TickPhase::Replication, "replication", replication_phase);
    }

    fn ready(&self, world: &mut World) -> Result<(), String> {
        let mut components = ReplicatedComponents::new();
        register_map_states(&mut components, world.registry()).map_err(|e| e.to_string())?;
        world.insert_resource(ReplicationState(Arc::new(Mutex::new(Replication::new(
            ReplicationConfig::default(),
            components,
        )))));
        Ok(())
    }

    // A reload of prototypes must not discard viewers, acknowledgements or queued input.
    fn reloaded(&self, _world: &mut World) -> Result<(), String> {
        Ok(())
    }
}

#[derive(Default)]
struct ScopedHeldInputs(BTreeMap<(NetUserId, EntityUid), BTreeSet<Arc<str>>>);

struct ScopedInputLifecycle;

impl System for ScopedInputLifecycle {
    fn initialize(&mut self, init: &mut SystemInit<'_>) {
        init.subscribe::<PlayerControlled, PlayerDetached, _>(|world, entity, event| {
            let held = world
                .resource_mut::<ScopedHeldInputs>()
                .and_then(|state| state.0.remove(&(event.user.clone(), entity)));
            if let Some(buttons) = held {
                release_buttons(world, entity, buttons);
            }
        });
        init.subscribe_broadcast::<RoundPhaseChanged, _>(|world, event| {
            if event.from == RoundPhase::InRound {
                let held = world
                    .resource_mut::<ScopedHeldInputs>()
                    .map(|state| std::mem::take(&mut state.0))
                    .unwrap_or_default();
                for ((_, entity), buttons) in held {
                    release_buttons(world, entity, buttons);
                }
            }
        });
    }
}

fn release_buttons(world: &mut World, entity: EntityUid, buttons: BTreeSet<Arc<str>>) {
    for function in buttons {
        world.raise(
            entity,
            &mut InputCommandReceived {
                function,
                state: ButtonState::Up,
                target: None,
                sub_tick: u16::MAX,
                sequence: 0,
                late: false,
            },
        );
    }
}

fn track_held(world: &mut World, user: &NetUserId, entity: EntityUid, command: &InputCommand) {
    let Some(state) = world.resource_mut::<ScopedHeldInputs>() else {
        return;
    };
    let key = (user.clone(), entity);
    match command.state {
        ButtonState::Down => {
            state
                .0
                .entry(key)
                .or_default()
                .insert(command.function.clone());
        }
        ButtonState::Up => {
            if let Some(buttons) = state.0.get_mut(&key) {
                buttons.remove(&command.function);
                if buttons.is_empty() {
                    state.0.remove(&key);
                }
            }
        }
    }
}

fn input_phase(world: &mut World, _dt: f32) {
    let Some(model) = world.resource::<ReplicationState>().cloned() else {
        return;
    };
    let due = model.lock().take_due_inputs(world.current_tick());
    for input in due {
        let entity: Option<EntityUid> = world
            .resource::<Players>()
            .and_then(|players| players.session(&input.user))
            .filter(|session| {
                session.status() == SessionStatus::InGame
                    && input.connection.is_none_or(|id| session.connection() == id)
                    && input.control.is_none_or(|control| {
                        session.entity() == Some(control.entity)
                            && session.control_generation() == control.generation
                            && !world.terminating_or_deleted(control.entity)
                            && world.resource::<CurrentRoundPhase>().is_some_and(|phase| {
                                phase.phase == RoundPhase::InRound && phase.round == control.round
                            })
                    })
            })
            .and_then(|session| session.entity());
        let Some(entity) = entity else {
            continue;
        };
        match input.input.payload {
            InputPayload::Command(command) => {
                if input.control.is_some_and(|control| control.track_held) {
                    track_held(world, &input.user, entity, &command);
                }
                world.raise(
                    entity,
                    &mut InputCommandReceived {
                        function: command.function,
                        state: command.state,
                        target: command.target,
                        sub_tick: input.input.sub_tick,
                        sequence: input.input.sequence,
                        late: input.late,
                    },
                );
            }
            InputPayload::Event(event) => world.raise(entity, &mut ClientEventReceived { event }),
        }
    }
}

fn replication_phase(world: &mut World, _dt: f32) {
    let Some(model) = world.resource::<ReplicationState>().cloned() else {
        return;
    };
    let mut model = model.lock();
    if let Some(players) = world.resource::<Players>() {
        model.sync_with_players(players);
    }
    let updates = model.update(world);
    if let Some(outgoing) = world.resource_mut::<OutgoingUpdates>() {
        outgoing.0.extend(updates);
    }
}
