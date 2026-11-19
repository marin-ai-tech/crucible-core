//! Entity creation and prototype hot reload scenarios.

use crucible_ext::{LaunchConfig, Launcher, ServerRuntime, SpawnError, SpawnExt};
use crucible_proto::InstantiateError;
use crucible_testkit::fixtures::{Health, Label, LifecycleLog, Light, spawning_extension};

fn launch(data: Option<&std::path::Path>) -> ServerRuntime {
    Launcher::new()
        .extension(spawning_extension(data))
        .launch(LaunchConfig::for_extension("spawning"))
        .unwrap_or_else(|e| panic!("{e}"))
}

/// Scenario: entity with inherited components.
#[test]
fn entity_with_inherited_components() {
    let mut server = launch(None);
    server
        .world_mut()
        .resource_mut::<LifecycleLog>()
        .unwrap()
        .0
        .clear();
    let uid = server.spawn("Lantern").unwrap();
    let world = server.world();

    // Both parents' components and the child's own component have their resolved values.
    assert_eq!(
        world.get::<Light>(uid),
        Some(&Light {
            radius: 8.0,
            color: "white".into()
        }),
        "the child's field overrides the parent; other fields are inherited"
    );
    assert_eq!(world.get::<Health>(uid), Some(&Health { max: 100 }));
    assert_eq!(
        world.get::<Label>(uid),
        Some(&Label {
            text: "lantern".into()
        })
    );
    assert_eq!(
        world.entity_meta(uid).unwrap().prototype(),
        Some("Lantern"),
        "the prototype ID was retained"
    );

    // Lifecycle events include additions first, followed by initialization and startup.
    let log = &world.resource::<LifecycleLog>().unwrap().0;
    let events: Vec<&str> = log.iter().map(|(_, e)| *e).collect();
    assert_eq!(
        events,
        vec![
            "add", "add", "add", "init", "init", "init", "startup", "startup", "startup"
        ]
    );
    assert_eq!(log[0].0, "Light", "parent components are added first");

    // The world method creates the same entity.
    let again = server.world_mut().spawn_prototype("Lantern").unwrap();
    assert_ne!(again, uid);
}

/// Scenario: an abstract prototype.
#[test]
fn abstract_prototype() {
    let mut server = launch(None);
    let before = server.world().entity_count();
    let error = server.spawn("BaseLit").unwrap_err();
    assert_eq!(
        error,
        SpawnError::Prototype(InstantiateError::Abstract {
            kind: "entity".into(),
            id: "BaseLit".into()
        })
    );
    assert!(error.to_string().contains("abstract"));
    assert_eq!(
        server.world().entity_count(),
        before,
        "no entity was created"
    );

    assert!(matches!(
        server.spawn("Missing"),
        Err(SpawnError::Prototype(InstantiateError::NotFound { .. }))
    ));
    assert_eq!(server.world().entity_count(), before);
}

/// Temporary copy of test fixture data.
#[cfg(debug_assertions)]
struct TempData(std::path::PathBuf);

#[cfg(debug_assertions)]
impl TempData {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "crucible-ext-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let source = crucible_testkit::fixtures::fixtures_dir().join("spawning/entities.yml");
        std::fs::copy(source, dir.join("entities.yml")).unwrap();
        TempData(dir)
    }

    fn write(&self, text: &str) {
        std::fs::write(self.0.join("entities.yml"), text).unwrap();
    }

    fn read(&self) -> String {
        std::fs::read_to_string(self.0.join("entities.yml")).unwrap()
    }
}

#[cfg(debug_assertions)]
impl Drop for TempData {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Scenario: a reload reports an error.
#[cfg(debug_assertions)]
#[test]
fn reload_with_error() {
    let data = TempData::new("reload");
    let mut server = launch(Some(&data.0));
    let original = data.read();
    let before = server.prototypes();
    let lantern = server.spawn("Lantern").unwrap();
    assert!(server.poll_hot_reload().is_none(), "files did not change");

    // Save a file with an unknown field and a missing parent reference.
    data.write(
        &original
            .replace("radius: 8", "radius: 8\n    brightness: 3")
            .replace(
                "parent: [ BaseLit, BaseSturdy ]",
                "parent: [ BaseLit, Missing ]",
            ),
    );
    let report = match server.poll_hot_reload() {
        Some(Err(report)) => report,
        other => panic!("expected an error report, got {other:?}"),
    };
    let text = report.to_string();
    assert!(
        text.contains("brightness") && text.contains("Missing"),
        "{text}"
    );
    assert!(
        std::sync::Arc::ptr_eq(&before, &server.prototypes()),
        "the previous version is still active"
    );
    let uid = server.spawn("Lantern").unwrap();
    assert_eq!(server.world().get::<Light>(uid).unwrap().radius, 8.0);
    assert!(server.world().exists(lantern));
    server.tick();

    // Once the fixed file is accepted, new entities use the updated data.
    data.write(&original.replace("radius: 8", "radius: 12"));
    assert!(matches!(server.poll_hot_reload(), Some(Ok(_))));
    let uid = server.spawn("Lantern").unwrap();
    assert_eq!(server.world().get::<Light>(uid).unwrap().radius, 12.0);
    assert_eq!(
        server.world().get::<Light>(lantern).unwrap().radius,
        8.0,
        "existing entities are unchanged"
    );
}

/// A release build does not support hot reload.
#[test]
fn hot_reload_only_in_dev_build() {
    assert_eq!(crucible_ext::HOT_RELOAD_AVAILABLE, cfg!(debug_assertions));
}

/// Without `debug_assertions`, `ServerRuntime` lacks `reload_prototypes` and `poll_hot_reload`.
/// Calls resolve to this trait's stub. In a development build, the inherent method takes precedence.
#[cfg(not(debug_assertions))]
mod release {
    use super::*;

    pub enum Probe {
        Absent,
    }

    pub trait AbsentInRelease {
        fn reload_prototypes(&mut self) -> Probe {
            Probe::Absent
        }
        fn poll_hot_reload(&mut self) -> Probe {
            Probe::Absent
        }
    }
    impl AbsentInRelease for ServerRuntime {}

    #[test]
    fn hot_reload_absent_in_release() {
        let mut server = launch(None);
        assert!(matches!(server.reload_prototypes(), Probe::Absent));
        assert!(matches!(server.poll_hot_reload(), Probe::Absent));
        assert!(!crucible_ext::HOT_RELOAD_AVAILABLE);
    }
}
