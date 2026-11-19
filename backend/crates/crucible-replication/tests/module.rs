//! Integration of replication with the launcher, input, and entity deletion queue.

use std::sync::Arc;
use std::time::Duration;

use crucible_ecs::{EntityUid, System, SystemInit, Tick};
use crucible_ext::{Extension, LaunchConfig, Launcher, ManifestSource, RegisterContext};
use crucible_loop::{CurrentRoundPhase, RoundPhase, TickPhase};
use crucible_map::{MapModule, Vec2, create_map, spawn_at};
use crucible_physics::ecs::{PhysicsModule, tile_fields};
use crucible_physics::{InputMover, MoveButtons};
use crucible_players::{ConnectionId, DisconnectReason, NetUserId, PlayerManager, Players};
use crucible_replication::{
    ButtonState, InputCommand, InputCommandReceived, InputControl, InputPayload, InputReceipt,
    OutgoingUpdates, PlayerInput, ReplicationModule, ReplicationState,
};

const MANIFEST: &str = r#"
[extension]
id = "module-test"
version = "0.1.0"
core = "^0.1"
id_prefix = "Test"

[modules]
require = ["map", "physics", "replication"]

[content]
roots = ["../crucible-testkit/tests/data"]
"#;

#[derive(Default)]
struct Observed(Vec<(EntityUid, Tick, u16, u32, bool)>);

#[derive(Default)]
struct DeleteOnTick(Option<EntityUid>);

#[derive(Default)]
struct SeenBeforeDeletion(bool);

struct Observer;

impl System for Observer {
    fn initialize(&mut self, init: &mut SystemInit<'_>) {
        init.subscribe::<InputMover, InputCommandReceived, _>(|world, uid, event| {
            let tick = world.current_tick();
            world.resource_mut::<Observed>().unwrap().0.push((
                uid,
                tick,
                event.sub_tick,
                event.sequence,
                event.late,
            ));
        });
    }
}

struct TestExtension;

impl Extension for TestExtension {
    fn manifest(&self) -> ManifestSource {
        ManifestSource::new(MANIFEST, env!("CARGO_MANIFEST_DIR"))
    }

    fn register(&self, ctx: &mut RegisterContext<'_>) {
        ctx.resource(Observed::default())
            .resource(DeleteOnTick::default())
            .resource(SeenBeforeDeletion::default())
            .system(Observer)
            .tick_phase(TickPhase::Physics, "queue_test_deletion", |world, _| {
                let target = world.resource::<DeleteOnTick>().unwrap().0;
                if let Some(target) = target {
                    world.queue_delete(target);
                }
            })
            .tick_phase(
                TickPhase::PostPhysics,
                "observe_queued_deletion",
                |world, _| {
                    if let Some(target) = world.resource::<DeleteOnTick>().unwrap().0 {
                        let still_exists = world.exists(target);
                        world.resource_mut::<SeenBeforeDeletion>().unwrap().0 = still_exists;
                    }
                },
            );
    }
}

#[test]
fn launcher_delivers_input_and_deletions_in_their_phases() {
    let launcher = Launcher::new()
        .module(MapModule::new().with_tile_fields(tile_fields()))
        .module(PhysicsModule)
        .module(ReplicationModule)
        .extension(TestExtension);
    let mut runtime = launcher
        .launch(LaunchConfig::for_extension("module-test"))
        .unwrap();
    let (map, _) = create_map(runtime.world_mut());
    let player = spawn_at(runtime.world_mut(), map, Vec2::new(1.0, 1.0)).unwrap();
    runtime
        .world_mut()
        .add_component(player, InputMover::default())
        .unwrap();
    let doomed = spawn_at(runtime.world_mut(), map, Vec2::new(2.0, 1.0)).unwrap();
    let doomed_net = runtime.world().net_entity(doomed).unwrap();
    let user = NetUserId::new("player");
    PlayerManager::begin_connect(
        runtime.world_mut(),
        &user,
        "Player",
        ConnectionId(1),
        Duration::ZERO,
    );
    PlayerManager::finish_connect(runtime.world_mut(), &user).unwrap();
    PlayerManager::enter_game(runtime.world_mut(), &user).unwrap();
    PlayerManager::attach(runtime.world_mut(), &user, player).unwrap();

    let input = PlayerInput {
        client_tick: Tick(1),
        sub_tick: 123,
        sequence: 7,
        payload: InputPayload::Command(InputCommand {
            function: Arc::from("MoveRight"),
            state: ButtonState::Down,
            coordinates: None,
            target: None,
        }),
    };
    assert_eq!(
        runtime
            .world()
            .resource::<ReplicationState>()
            .unwrap()
            .lock()
            .receive_input(&user, input),
        InputReceipt::Queued
    );
    runtime.tick();
    assert_eq!(
        runtime.world().resource::<Observed>().unwrap().0,
        vec![(player, Tick(1), 123, 7, false)]
    );
    assert!(
        runtime
            .world()
            .get::<InputMover>(player)
            .unwrap()
            .held_move_buttons()
            .contains(MoveButtons::RIGHT)
    );
    let first = runtime
        .world_mut()
        .resource_mut::<OutgoingUpdates>()
        .unwrap()
        .drain();
    assert_eq!(first.len(), 1);
    assert!(first[0].frame.entity(doomed_net).is_some());
    runtime
        .world()
        .resource::<ReplicationState>()
        .unwrap()
        .lock()
        .acknowledge(&user, Tick(1));

    runtime
        .world_mut()
        .resource_mut::<DeleteOnTick>()
        .unwrap()
        .0 = Some(doomed);
    runtime.tick();
    let second = runtime
        .world_mut()
        .resource_mut::<OutgoingUpdates>()
        .unwrap()
        .drain();
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].frame.to_tick, Tick(2));
    assert!(second[0].frame.deletions.contains(&doomed_net));
    assert!(runtime.world().resource::<SeenBeforeDeletion>().unwrap().0);
    assert!(!runtime.world().exists(doomed));
}

#[test]
fn queued_network_commands_require_the_original_active_connection() {
    let launcher = Launcher::new()
        .module(MapModule::new().with_tile_fields(tile_fields()))
        .module(PhysicsModule)
        .module(ReplicationModule)
        .extension(TestExtension);
    let mut runtime = launcher
        .launch(LaunchConfig::for_extension("module-test"))
        .unwrap();
    let (map, _) = create_map(runtime.world_mut());
    let actor = spawn_at(runtime.world_mut(), map, Vec2::new(1.0, 1.0)).unwrap();
    runtime
        .world_mut()
        .add_component(actor, InputMover::default())
        .unwrap();
    let user = NetUserId::new("player");
    let old = ConnectionId(1);
    let current = ConnectionId(2);
    PlayerManager::begin_connect(runtime.world_mut(), &user, "Player", old, Duration::ZERO);
    PlayerManager::finish_connect(runtime.world_mut(), &user).unwrap();
    PlayerManager::enter_game(runtime.world_mut(), &user).unwrap();
    PlayerManager::attach(runtime.world_mut(), &user, actor).unwrap();

    let model = runtime
        .world()
        .resource::<ReplicationState>()
        .unwrap()
        .clone();
    let queue = |connection, tick, sequence, function| {
        let mut replication = model.lock();
        replication.set_input_connection(&user, connection);
        assert_eq!(
            replication.receive_input(
                &user,
                PlayerInput {
                    client_tick: Tick(tick),
                    sub_tick: 0,
                    sequence,
                    payload: InputPayload::Command(InputCommand {
                        function: Arc::from(function),
                        state: ButtonState::Down,
                        coordinates: None,
                        target: None,
                    }),
                }
            ),
            InputReceipt::Queued
        );
    };
    queue(old, 1, 7, "Drop");
    PlayerManager::begin_connect(
        runtime.world_mut(),
        &user,
        "Player",
        current,
        Duration::ZERO,
    );
    PlayerManager::finish_connect(runtime.world_mut(), &user).unwrap();
    let retained = {
        runtime
            .world()
            .resource::<Players>()
            .unwrap()
            .session(&user)
            .unwrap()
            .entity()
    };
    assert_eq!(retained, Some(actor));
    runtime.tick();
    assert!(runtime.world().resource::<Observed>().unwrap().0.is_empty());
    assert_eq!(model.lock().inputs().last_processed(&user), 0);

    queue(current, 2, 1, "Use");
    runtime.tick();
    assert_eq!(
        runtime.world().resource::<Observed>().unwrap().0,
        [(actor, Tick(2), 0, 1, false)]
    );
    assert_eq!(model.lock().inputs().last_processed(&user), 1);

    queue(current, 3, 2, "ActivateItemInHand");
    assert!(PlayerManager::disconnect(
        runtime.world_mut(),
        &user,
        current,
        DisconnectReason::ConnectionLost,
        Duration::ZERO,
    ));
    assert_eq!(
        runtime
            .world()
            .resource::<Players>()
            .unwrap()
            .session(&user)
            .unwrap()
            .entity(),
        Some(actor)
    );
    runtime.tick();
    assert_eq!(runtime.world().resource::<Observed>().unwrap().0.len(), 1);
}

#[test]
fn bound_input_releases_old_buttons_and_rejects_commands_after_same_connection_rebind() {
    let launcher = Launcher::new()
        .module(MapModule::new().with_tile_fields(tile_fields()))
        .module(PhysicsModule)
        .module(ReplicationModule)
        .extension(TestExtension);
    let mut runtime = launcher
        .launch(LaunchConfig::for_extension("module-test"))
        .unwrap();
    runtime.world_mut().insert_resource(CurrentRoundPhase {
        phase: RoundPhase::InRound,
        round: 1,
    });
    let (map, _) = create_map(runtime.world_mut());
    let actor = spawn_at(runtime.world_mut(), map, Vec2::new(1.0, 1.0)).unwrap();
    runtime
        .world_mut()
        .add_component(actor, InputMover::default())
        .unwrap();
    let user = NetUserId::new("bound-user");
    let connection = ConnectionId(1);
    PlayerManager::begin_connect(
        runtime.world_mut(),
        &user,
        "Player",
        connection,
        Duration::ZERO,
    );
    PlayerManager::finish_connect(runtime.world_mut(), &user).unwrap();
    PlayerManager::enter_game(runtime.world_mut(), &user).unwrap();
    PlayerManager::attach(runtime.world_mut(), &user, actor).unwrap();
    let generation = |runtime: &crucible_ext::ServerRuntime| {
        runtime
            .world()
            .resource::<Players>()
            .unwrap()
            .session(&user)
            .unwrap()
            .control_generation()
    };
    let original = generation(&runtime);
    let model = runtime
        .world()
        .resource::<ReplicationState>()
        .unwrap()
        .clone();
    let queue = |tick, sequence, function, generation| {
        let mut model = model.lock();
        model.set_input_connection(&user, connection);
        assert_eq!(
            model.receive_bound_input(
                &user,
                PlayerInput {
                    client_tick: Tick(tick),
                    sub_tick: 0,
                    sequence,
                    payload: InputPayload::Command(InputCommand {
                        function: Arc::from(function),
                        state: ButtonState::Down,
                        coordinates: None,
                        target: None,
                    }),
                },
                InputControl {
                    entity: actor,
                    generation,
                    round: 1,
                    track_held: true
                }
            ),
            InputReceipt::Queued
        );
    };
    queue(1, 1, "MoveRight", original);
    queue(2, 2, "MoveLeft", original);
    runtime.tick();
    let held = |runtime: &crucible_ext::ServerRuntime| {
        runtime
            .world()
            .get::<InputMover>(actor)
            .unwrap()
            .held_move_buttons()
    };
    assert_eq!(held(&runtime), MoveButtons::RIGHT);

    assert!(PlayerManager::detach(runtime.world_mut(), &user));
    assert_eq!(held(&runtime), MoveButtons::NONE);
    PlayerManager::attach(runtime.world_mut(), &user, actor).unwrap();
    assert_ne!(generation(&runtime), original);
    runtime.tick();
    assert_eq!(held(&runtime), MoveButtons::NONE);
    assert!(
        !runtime
            .world()
            .resource::<Observed>()
            .unwrap()
            .0
            .iter()
            .any(|(_, _, _, sequence, _)| *sequence == 2)
    );

    queue(3, 3, "MoveUp", generation(&runtime));
    runtime.tick();
    assert_eq!(held(&runtime), MoveButtons::UP);
    PlayerManager::begin_connect(
        runtime.world_mut(),
        &user,
        "Player",
        ConnectionId(2),
        Duration::ZERO,
    );
    PlayerManager::finish_connect(runtime.world_mut(), &user).unwrap();
    let session = runtime
        .world()
        .resource::<Players>()
        .unwrap()
        .session(&user)
        .unwrap();
    assert_eq!(session.entity(), Some(actor));
    assert_eq!(held(&runtime), MoveButtons::NONE);
}
