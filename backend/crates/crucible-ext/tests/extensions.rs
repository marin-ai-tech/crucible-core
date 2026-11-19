//! Core extension scenarios using the fixture modules and extensions from `crucible-testkit`.
//!

use std::sync::{Arc, Mutex};
use std::time::Duration;

use crucible_ecs::World;
use crucible_ext::{
    CoreModule, ExtensionManifest, ExtensionPointKind, LaunchConfig, LaunchError, Launcher,
    LoadPhase, ManifestSource, PrototypeOverride, RegisterContext, RegisterError, ServerRuntime,
};
use crucible_loop::{
    CampaignState, RoundConfig, RoundEndSource, RoundPhase, RoundPreparation, RoundPreparationHook,
};
use crucible_players::NetUserId;
use crucible_players::Players;
use crucible_proto::IgnoreCategory;
use crucible_testkit::fixtures::{
    CombatModule, DamageModule, EnergyNetworksModule, FixtureExtension, fixtures_dir, manifest,
};
use semver::Version;

fn all_modules(launcher: Launcher) -> Launcher {
    launcher
        .module(DamageModule)
        .module(CombatModule)
        .module(EnergyNetworksModule)
}

#[test]
fn launcher_provides_players_resource() {
    let launcher = Launcher::new().extension(FixtureExtension::new(
        &manifest("players", "^0.1", &[]),
        fixtures_dir(),
    ));
    let runtime = launcher
        .launch(LaunchConfig::for_extension("players"))
        .unwrap();
    assert!(runtime.world().resource::<Players>().is_some());
}

fn launch_err(launcher: &Launcher, config: LaunchConfig) -> LaunchError {
    match launcher.launch(config) {
        Ok(_) => panic!("launch should fail"),
        Err(e) => e,
    }
}

// Manifest

/// Scenario: incompatible core version.
#[test]
fn incompatible_core_version() {
    let launcher = Launcher::new()
        .with_core_version(Version::new(0, 1, 3))
        .extension(FixtureExtension::new(
            &manifest("future", "^0.2", &[]),
            fixtures_dir(),
        ));
    let error = launch_err(&launcher, LaunchConfig::for_extension("future"));
    assert_eq!(
        error.phase(),
        LoadPhase::Resolution,
        "the server does not start"
    );
    let message = error.to_string();
    for part in ["`future`", "^0.2", "0.1.3"] {
        assert!(
            message.contains(part),
            "message does not contain {part}: {message}"
        );
    }
}

#[test]
fn manifest_format() {
    let text = r#"
[extension]
id = "sample"
version = "1.2.3"
core = "^0.1"
id_prefix = "Sa"

[modules]
require = ["damage"]

[content]
roots = ["data", "more"]

[[content.overrides]]
kind = "entity"
id = "BaseItem"

[ignore]
client_only = ["Sprite"]
not_implemented = ["Climbable"]

[client]
shared_namespace = "Content.Shared.Sample"
"#;
    let m = ExtensionManifest::parse(&ManifestSource::new(text, "/ext/sample")).unwrap();
    assert_eq!(m.id, "sample");
    assert_eq!(m.version, Version::new(1, 2, 3));
    assert_eq!(m.id_prefix.as_deref(), Some("Sa"));
    assert_eq!(m.modules, vec!["damage"]);
    assert_eq!(m.content_roots.len(), 2);
    assert!(m.content_roots[0].ends_with("data"));
    assert_eq!(
        m.overrides,
        vec![PrototypeOverride {
            kind: "entity".into(),
            id: "BaseItem".into()
        }]
    );
    assert_eq!(
        m.ignored,
        vec![
            ("Sprite".into(), IgnoreCategory::ClientOnly),
            ("Climbable".into(), IgnoreCategory::NotImplemented)
        ]
    );
    let root = m.data_root();
    assert_eq!(root.name, "ext:sample");
    assert_eq!(root.overrides, vec![("entity".into(), "BaseItem".into())]);

    for (bad, why) in [
        (
            "[extension]\nid = \"x\"\nversion = \"1\"\ncore = \"^0.1\"",
            "version",
        ),
        (
            "[extension]\nid = \"x\"\nversion = \"0.1.0\"\ncore = \"^0.1\"\nextra = 1",
            "extra",
        ),
        (
            "[extension]\nid = \"X Y\"\nversion = \"0.1.0\"\ncore = \"^0.1\"",
            "id",
        ),
        ("[extension]\nid = \"x\"\nversion = \"0.1.0\"", "core"),
        ("[unknown]\n", "unknown"),
    ] {
        let error = ExtensionManifest::parse(&ManifestSource::new(bad, "/ext")).unwrap_err();
        assert!(error.to_string().contains(why), "{why}: {error}");
    }
}

// Module resolution

/// Scenario: a module is disabled.
#[test]
fn module_not_connected() {
    let launcher = all_modules(Launcher::new()).extension(FixtureExtension::from_fixture("alpha"));
    let server = launcher
        .launch(LaunchConfig::for_extension("alpha"))
        .unwrap();
    assert!(!server.modules().contains(&"energy-networks"));
    let systems = server.game().systems().names();
    assert!(
        !systems.iter().any(|s| s.contains("EnergyNetworkSystem")),
        "the energy network module has no systems: {systems:?}"
    );
    assert!(
        server
            .world()
            .registry()
            .by_name("EnergyConsumer")
            .is_none()
    );

    // A prototype using the module's component fails validation.
    let mut config = LaunchConfig::for_extension("alpha");
    config.content_roots = Some(vec![fixtures_dir().join("alpha-energy")]);
    let LaunchError::Validation(report) = launch_err(&launcher, config.clone()) else {
        panic!("expected a validation error");
    };
    let text = report.to_string();
    assert!(
        text.contains("EnergyConsumer") && text.contains("AlGenerator"),
        "{text}"
    );

    // An explicitly ignored component is allowed.
    let ignoring = all_modules(Launcher::new()).extension(FixtureExtension::new(
        "[extension]\nid = \"alpha\"\nversion = \"0.1.0\"\ncore = \"^0.1\"\nid_prefix = \"Al\"\n\n\
         [modules]\nrequire = [\"combat\"]\n\n[ignore]\nnot_implemented = [\"EnergyConsumer\"]\n",
        fixtures_dir(),
    ));
    let server = ignoring
        .launch(config)
        .expect("ignored components do not block launch");
    assert!(
        server.load_report().warnings().count() >= 1,
        "one aggregate warning"
    );
}

/// Scenario: a module's dependencies are enabled automatically.
#[test]
fn module_dependency_connected_automatically() {
    // The build adds the damage module last, and the manifest does not list it.
    let launcher = Launcher::new()
        .module(CombatModule)
        .module(EnergyNetworksModule)
        .module(DamageModule)
        .extension(FixtureExtension::new(
            &manifest("fighter", "^0.1", &["combat"]),
            fixtures_dir(),
        ));
    let server = launcher
        .launch(LaunchConfig::for_extension("fighter"))
        .unwrap();
    assert_eq!(
        server.modules(),
        vec!["damage", "combat"],
        "damage precedes combat"
    );
    let registry = server.world().registry();
    assert_eq!(
        registry.by_name("Damageable").unwrap().source(),
        "module:damage"
    );
    assert_eq!(
        registry.by_name("MeleeWeapon").unwrap().source(),
        "module:combat"
    );
    let names = server.game().systems().names();
    let damage = names
        .iter()
        .position(|n| n.ends_with("DamageSystem"))
        .unwrap();
    let combat = names
        .iter()
        .position(|n| n.ends_with("CombatSystem"))
        .unwrap();
    assert!(damage < combat);
}

#[test]
fn unknown_module_stops_launch() {
    let launcher = Launcher::new().extension(FixtureExtension::new(
        &manifest("lonely", "^0.1", &["nonexistent"]),
        fixtures_dir(),
    ));
    let error = launch_err(&launcher, LaunchConfig::for_extension("lonely"));
    assert_eq!(error.phase(), LoadPhase::Resolution);
    assert!(error.to_string().contains("nonexistent"));
}

// Registration

struct PingEvent;
struct OtherPing;

/// Module registering a `Ping` network event.
struct PingModule;
impl CoreModule for PingModule {
    fn id(&self) -> &'static str {
        "ping"
    }
    fn register(&self, ctx: &mut RegisterContext<'_>) {
        ctx.network_event::<PingEvent>("Ping");
    }
}

/// Scenario: conflicting network event names.
#[test]
fn network_event_name_conflict() {
    let launcher = Launcher::new().module(PingModule).extension(
        FixtureExtension::new(&manifest("echo", "^0.1", &["ping"]), fixtures_dir()).on_register(
            |ctx| {
                ctx.network_event::<OtherPing>("Ping");
            },
        ),
    );
    let error = launch_err(&launcher, LaunchConfig::for_extension("echo"));
    assert_eq!(error.phase(), LoadPhase::Registration, "launch stops");
    let LaunchError::Registration(errors) = &error else {
        panic!("expected a registration error: {error}");
    };
    assert_eq!(
        errors,
        &vec![RegisterError::NetEventConflict {
            name: "Ping".into(),
            first_source: "module:ping".into(),
            second_source: "ext:echo".into(),
        }]
    );
    let message = error.to_string();
    assert!(
        message.contains("module:ping") && message.contains("ext:echo"),
        "{message}"
    );

    // Without a conflict, the stable name is discoverable by event type.
    let ok = Launcher::new()
        .module(PingModule)
        .extension(FixtureExtension::new(
            &manifest("echo", "^0.1", &["ping"]),
            fixtures_dir(),
        ));
    let server = ok.launch(LaunchConfig::for_extension("echo")).unwrap();
    assert_eq!(server.net_names().event_name::<PingEvent>(), Some("Ping"));
    assert_eq!(
        server.net_names().event("Ping").unwrap().source,
        "module:ping"
    );
}

#[test]
fn console_command_registered() {
    let launcher = Launcher::new().extension(
        FixtureExtension::new(&manifest("cmd", "^0.1", &[]), fixtures_dir()).on_register(|ctx| {
            ctx.console_command("count", "entity count", |world: &mut World, _| {
                Ok(world.entity_count().to_string())
            });
        }),
    );
    let mut server = launcher.launch(LaunchConfig::for_extension("cmd")).unwrap();
    server.world_mut().spawn();
    assert_eq!(server.execute_command("count", &[]), Ok("1".to_owned()));
    assert!(server.execute_command("missing", &[]).is_err());
}

// Extension points

fn fast_rounds() -> RoundConfig {
    RoundConfig {
        lobby_duration: Duration::from_secs(1),
        round_duration: Some(Duration::from_secs(2)),
        post_round_duration: Duration::from_secs(1),
    }
}

fn run_until(server: &mut ServerRuntime, state: RoundPhase, limit: u32) {
    for _ in 0..limit {
        if server.rounds().phase() == state {
            return;
        }
        server.tick();
    }
    panic!("round flow did not reach `{state}` within {limit} ticks");
}

/// Scenario: default implementation.
#[test]
fn default_implementation() {
    let launcher = Launcher::new().extension(FixtureExtension::new(
        &manifest("plain", "^0.1", &[]),
        fixtures_dir(),
    ));
    let mut config = LaunchConfig::for_extension("plain");
    config.rounds = fast_rounds();
    let mut server = launcher.launch(config).unwrap();
    server.rounds_mut().add_player(NetUserId::new("1"));
    server.rounds_mut().set_ready(NetUserId::new("1"), true);
    server.rounds_mut().request_start();
    run_until(&mut server, RoundPhase::PostRound, 200);

    let summary = server.rounds().last_summary().unwrap();
    assert_eq!(summary.end_reason.source, RoundEndSource::Policy);
    assert!(
        summary.end_reason.text.contains("timer"),
        "{}",
        summary.end_reason.text
    );
    assert_eq!(
        summary.duration_ticks, 60,
        "the round ends after the configured 2 s timeout"
    );
}

#[test]
fn missing_required_point_stops_launch() {
    struct MapsModule;
    impl CoreModule for MapsModule {
        fn id(&self) -> &'static str {
            "maps"
        }
        fn register(&self, _: &mut RegisterContext<'_>) {}
        fn configure(&self, points: &mut crucible_ext::ExtensionPoints) {
            points.require(ExtensionPointKind::MapSelection, "module:maps");
        }
    }
    let launcher = Launcher::new()
        .module(MapsModule)
        .extension(FixtureExtension::new(
            &manifest("nomaps", "^0.1", &["maps"]),
            fixtures_dir(),
        ));
    let error = launch_err(&launcher, LaunchConfig::for_extension("nomaps"));
    assert_eq!(error.phase(), LoadPhase::Ready);
    let message = error.to_string();
    assert!(
        message.contains("map selection") && message.contains("module:maps"),
        "{message}"
    );
}

/// Campaign preparation hook used to check that there is no preparation without a campaign.
struct PrepProbe(Arc<Mutex<Vec<RoundPreparation>>>);
impl RoundPreparationHook for PrepProbe {
    fn prepare_round(&mut self, _: &CampaignState, preparation: &mut RoundPreparation) {
        self.0.lock().unwrap().push(preparation.clone());
    }
}

/// Scenario: extension without a campaign.
#[test]
fn extension_without_campaign() {
    let launcher = Launcher::new().extension(FixtureExtension::new(
        &manifest("solo", "^0.1", &[]),
        fixtures_dir(),
    ));
    let mut config = LaunchConfig::for_extension("solo");
    config.rounds = fast_rounds();
    let mut server = launcher.launch(config).unwrap();
    let stored = server.rounds().campaign_state().clone();
    assert_eq!(stored, CampaignState::default());

    let mut summaries = Vec::new();
    for _ in 0..2 {
        server.rounds_mut().request_start();
        run_until(&mut server, RoundPhase::PostRound, 200);
        summaries.push(server.rounds().last_summary().unwrap().clone());
        run_until(&mut server, RoundPhase::Lobby, 200);
        assert_eq!(
            server.rounds().campaign_state(),
            &stored,
            "round results do not change stored campaign state"
        );
    }
    assert_eq!(summaries[0].round, 1);
    assert_eq!(summaries[1].round, 2);
    assert_eq!(
        summaries[0].duration_ticks, summaries[1].duration_ticks,
        "rounds run independently"
    );

    // With a campaign hook installed, round preparation sees its data.
    let seen = Arc::new(Mutex::new(Vec::new()));
    let probe = seen.clone();
    let with_hook = Launcher::new().extension(
        FixtureExtension::new(&manifest("camp", "^0.1", &[]), fixtures_dir()).on_configure(
            move |points| {
                points.add_round_preparation_hook(PrepProbe(probe.clone()));
            },
        ),
    );
    let mut server = with_hook
        .launch(LaunchConfig::for_extension("camp"))
        .unwrap();
    server.rounds_mut().request_start();
    server.tick();
    assert_eq!(seen.lock().unwrap().len(), 1);
}

/// The spawn policy creates a character for the connected lobby player, attaches it, and enters
/// the game. A participant without a session gets a character without attachment.
#[test]
fn spawn_from_prototype_attaches_and_enters_game() {
    use crucible_players::{ConnectionId, PlayerControlled, PlayerManager, SessionStatus};
    use crucible_testkit::fixtures::spawning_extension;

    let launcher = Launcher::new().extension(spawning_extension(None).on_configure(|points| {
        points.set_spawn_policy(crucible_ext::SpawnFromPrototype {
            prototype: "Lantern".to_owned(),
        });
    }));
    let mut server = launcher
        .launch(LaunchConfig::for_extension("spawning"))
        .unwrap();
    let (player, guest) = (NetUserId::new("player"), NetUserId::new("guest"));
    let world = server.world_mut();
    PlayerManager::begin_connect(world, &player, "P", ConnectionId(1), Duration::ZERO);
    PlayerManager::finish_connect(world, &player).unwrap();
    for user in [&player, &guest] {
        server.rounds_mut().add_player(user.clone());
        server.rounds_mut().set_ready(user.clone(), true);
    }
    server.rounds_mut().request_start();
    server.tick();
    assert_eq!(server.rounds().phase(), RoundPhase::InRound);

    let body = |user: &NetUserId| {
        server
            .rounds()
            .participants()
            .iter()
            .find(|p| &p.player == user)
            .and_then(|p| p.entity)
            .expect("character was spawned")
    };
    let (player_body, guest_body) = (body(&player), body(&guest));
    let world = server.world();
    let session = world
        .resource::<Players>()
        .unwrap()
        .session(&player)
        .unwrap();
    assert_eq!(session.status(), SessionStatus::InGame);
    assert_eq!(session.entity(), Some(player_body));
    assert_eq!(
        world.get::<PlayerControlled>(player_body).map(|c| &c.user),
        Some(&player)
    );
    assert_eq!(
        world.entity_meta(player_body).unwrap().prototype(),
        Some("Lantern")
    );
    assert!(
        world
            .resource::<Players>()
            .unwrap()
            .session(&guest)
            .is_none()
    );
    assert!(!world.has::<PlayerControlled>(guest_body));
}

// Launch phases and extension selection

/// Scenario: a validation error stops launch.
#[test]
fn validation_error_stops_launch() {
    let launcher = all_modules(Launcher::new()).extension(FixtureExtension::from_fixture("alpha"));
    let mut config = LaunchConfig::for_extension("alpha");
    config.content_roots = Some(vec![fixtures_dir().join("alpha-energy")]);
    let error = launch_err(&launcher, config.clone());
    assert_eq!(error.phase(), LoadPhase::Validation);
    assert!(
        error.phase() < LoadPhase::Ready,
        "the server never reaches readiness or opens the lobby"
    );
    // The content validation command returns the same report.
    let LaunchError::Validation(launch_report) = error else {
        unreachable!()
    };
    let LaunchError::Validation(validate_report) = launcher.validate(&config).unwrap_err() else {
        panic!("expected a validation error");
    };
    assert_eq!(launch_report, validate_report);
}

/// Scenario: extension selection.
#[test]
fn extension_selection() {
    let launcher = all_modules(Launcher::new())
        .extension(FixtureExtension::from_fixture("alpha"))
        .extension(FixtureExtension::from_fixture("beta"));

    let server = launcher
        .launch(LaunchConfig::for_extension("alpha"))
        .unwrap();
    assert_eq!(server.manifest().id, "alpha");
    let protos = server.prototypes();
    assert!(
        protos.get("entity", "AlSword").is_some(),
        "the active extension's data is loaded"
    );
    assert!(
        protos.get("entity", "BeLamp").is_none(),
        "inactive extension prototypes are unavailable"
    );
    let systems = server.game().systems().names();
    assert!(systems.iter().any(|s| s.ends_with("CombatSystem")));
    assert!(!systems.iter().any(|s| s.ends_with("EnergyNetworkSystem")));
    assert_eq!(
        server.phases(),
        &[
            LoadPhase::Discovery,
            LoadPhase::Resolution,
            LoadPhase::Registration,
            LoadPhase::Data,
            LoadPhase::Validation,
            LoadPhase::Ready
        ]
    );

    let server = launcher
        .launch(LaunchConfig::for_extension("beta"))
        .unwrap();
    let protos = server.prototypes();
    assert!(protos.get("entity", "BeLamp").is_some());
    assert!(protos.get("entity", "AlSword").is_none());
    assert_eq!(server.modules(), vec!["energy-networks"]);

    let error = launch_err(&launcher, LaunchConfig::for_extension("gamma"));
    assert_eq!(error.phase(), LoadPhase::Discovery);
    assert!(error.to_string().contains("alpha, beta"));
}

#[test]
fn shutdown_in_reverse_order() {
    let order = Arc::new(Mutex::new(Vec::new()));
    let log = order.clone();
    let launcher = all_modules(Launcher::new()).extension(
        FixtureExtension::new(&manifest("ordered", "^0.1", &["combat"]), fixtures_dir())
            .on_shutdown(move |_| log.lock().unwrap().push("ext")),
    );
    let server = launcher
        .launch(LaunchConfig::for_extension("ordered"))
        .unwrap();
    assert_eq!(
        server.shutdown(),
        vec!["ext:ordered", "module:combat", "module:damage"],
        "shutdown runs in reverse registration order"
    );
    assert_eq!(*order.lock().unwrap(), vec!["ext"]);
}
