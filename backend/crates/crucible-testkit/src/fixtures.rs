//! Fixture modules and extensions for testing the core independently of a setting.
//!
//! Modules: `damage`, `combat` (depends on `damage`), and `energy-networks`.
//! The `alpha` extension requires `combat` and `beta` requires `energy-networks`.
//! Their data lives in `crucible-testkit/fixtures/`. [`FixtureExtension`] assembles an extension
//! from an arbitrary manifest and closures.

use std::path::PathBuf;

use crucible_ecs::{Component, NetworkPolicy, System, World};
use crucible_ext::{
    ComponentSpec, CoreModule, Extension, ExtensionPoints, ManifestSource, RegisterContext,
};
use crucible_proto::FieldType;

/// Test fixture directory.
pub fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

/// Component for the damage module.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Damageable {
    /// Durability.
    pub max: i64,
}
impl Component for Damageable {}

/// Component for the combat module.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MeleeWeapon {
    /// Damage per hit.
    pub damage: i64,
}
impl Component for MeleeWeapon {}

/// Component for the energy network module.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EnergyConsumer {
    /// Power consumption.
    pub draw: f64,
}
impl Component for EnergyConsumer {}

/// System for the damage module.
pub struct DamageSystem;
impl System for DamageSystem {}

/// Combat system that runs after the damage system.
pub struct CombatSystem;
impl System for CombatSystem {
    fn initialize(&mut self, init: &mut crucible_ecs::SystemInit<'_>) {
        init.updates_after::<DamageSystem>();
    }
}

/// System for the energy network module.
pub struct EnergyNetworkSystem;
impl System for EnergyNetworkSystem {}

/// Damage module.
pub struct DamageModule;
impl CoreModule for DamageModule {
    fn id(&self) -> &'static str {
        "damage"
    }
    fn register(&self, ctx: &mut RegisterContext<'_>) {
        ctx.component(
            ComponentSpec::<Damageable>::new("Damageable")
                .field("max", FieldType::Int)
                .network(NetworkPolicy::AllViewers)
                .from_prototype(|d| {
                    Ok(Damageable {
                        max: d.int("max")?.unwrap_or(100),
                    })
                }),
        )
        .system(DamageSystem);
    }
}

/// Combat module that depends on the damage module.
pub struct CombatModule;
impl CoreModule for CombatModule {
    fn id(&self) -> &'static str {
        "combat"
    }
    fn dependencies(&self) -> &'static [&'static str] {
        &["damage"]
    }
    fn register(&self, ctx: &mut RegisterContext<'_>) {
        ctx.component(
            ComponentSpec::<MeleeWeapon>::new("MeleeWeapon")
                .field("damage", FieldType::Int)
                .from_prototype(|d| {
                    Ok(MeleeWeapon {
                        damage: d.int("damage")?.unwrap_or(1),
                    })
                }),
        )
        .system(CombatSystem);
    }
}

/// Energy network module.
pub struct EnergyNetworksModule;
impl CoreModule for EnergyNetworksModule {
    fn id(&self) -> &'static str {
        "energy-networks"
    }
    fn register(&self, ctx: &mut RegisterContext<'_>) {
        ctx.component(
            ComponentSpec::<EnergyConsumer>::new("EnergyConsumer")
                .field("draw", FieldType::Float)
                .from_prototype(|d| {
                    Ok(EnergyConsumer {
                        draw: d.float("draw")?.unwrap_or(0.0),
                    })
                }),
        )
        .system(EnergyNetworkSystem);
    }
}

type RegisterFn = Box<dyn Fn(&mut RegisterContext<'_>) + Send + Sync>;
type ConfigureFn = Box<dyn Fn(&mut ExtensionPoints) + Send + Sync>;
type ShutdownFn = Box<dyn Fn(&mut World) + Send + Sync>;

/// Extension built from a manifest and closures.
pub struct FixtureExtension {
    manifest: String,
    base_dir: PathBuf,
    register: Option<RegisterFn>,
    configure: Option<ConfigureFn>,
    shutdown: Option<ShutdownFn>,
}

impl FixtureExtension {
    /// Creates an extension from a TOML `manifest` and a `base_dir`.
    pub fn new(manifest: &str, base_dir: impl Into<PathBuf>) -> Self {
        FixtureExtension {
            manifest: manifest.to_owned(),
            base_dir: base_dir.into(),
            register: None,
            configure: None,
            shutdown: None,
        }
    }

    /// Loads an extension from `fixtures/<name>/extension.toml`.
    pub fn from_fixture(name: &str) -> Self {
        let dir = fixtures_dir().join(name);
        let manifest = std::fs::read_to_string(dir.join("extension.toml"))
            .unwrap_or_else(|e| panic!("fixture manifest {name}: {e}"));
        FixtureExtension::new(&manifest, dir)
    }

    /// Registers a callback for registration.
    pub fn on_register(
        mut self,
        f: impl Fn(&mut RegisterContext<'_>) + Send + Sync + 'static,
    ) -> Self {
        self.register = Some(Box::new(f));
        self
    }

    /// Registers a callback for configuration.
    pub fn on_configure(
        mut self,
        f: impl Fn(&mut ExtensionPoints) + Send + Sync + 'static,
    ) -> Self {
        self.configure = Some(Box::new(f));
        self
    }

    /// Registers a shutdown callback.
    pub fn on_shutdown(mut self, f: impl Fn(&mut World) + Send + Sync + 'static) -> Self {
        self.shutdown = Some(Box::new(f));
        self
    }
}

impl Extension for FixtureExtension {
    fn manifest(&self) -> ManifestSource {
        ManifestSource::new(self.manifest.clone(), self.base_dir.clone())
    }
    fn register(&self, ctx: &mut RegisterContext<'_>) {
        if let Some(f) = &self.register {
            f(ctx);
        }
    }
    fn configure(&self, points: &mut ExtensionPoints) {
        if let Some(f) = &self.configure {
            f(points);
        }
    }
    fn shutdown(&self, world: &mut World) {
        if let Some(f) = &self.shutdown {
            f(world);
        }
    }
}

/// Light source fixture for entity spawning.
#[derive(Clone, Debug, PartialEq)]
pub struct Light {
    /// Radius.
    pub radius: f64,
    /// Color.
    pub color: String,
}
impl Component for Light {}

/// Health fixture for entity spawning.
#[derive(Clone, Debug, PartialEq)]
pub struct Health {
    /// Health points.
    pub max: i64,
}
impl Component for Health {}

/// Label fixture for entity spawning.
#[derive(Clone, Debug, PartialEq)]
pub struct Label {
    /// Text.
    pub text: String,
}
impl Component for Label {}

/// Component lifecycle log with (component, event) pairs in delivery order.
#[derive(Default, Debug)]
pub struct LifecycleLog(pub Vec<(&'static str, &'static str)>);

/// Records lifecycle events for `Light`, `Health`, and `Label`.
pub struct LifecycleRecorder;

fn record<C: Component>(init: &mut crucible_ecs::SystemInit<'_>, name: &'static str) {
    use crucible_ecs::{ComponentAdd, ComponentInit, ComponentStartup};
    init.subscribe::<C, ComponentAdd, _>(move |w, _, _| {
        w.resource_mut::<LifecycleLog>()
            .unwrap()
            .0
            .push((name, "add"))
    });
    init.subscribe::<C, ComponentInit, _>(move |w, _, _| {
        w.resource_mut::<LifecycleLog>()
            .unwrap()
            .0
            .push((name, "init"))
    });
    init.subscribe::<C, ComponentStartup, _>(move |w, _, _| {
        w.resource_mut::<LifecycleLog>()
            .unwrap()
            .0
            .push((name, "startup"))
    });
}

impl System for LifecycleRecorder {
    fn initialize(&mut self, init: &mut crucible_ecs::SystemInit<'_>) {
        init.insert_resource(LifecycleLog::default());
        record::<Light>(init, "Light");
        record::<Health>(init, "Health");
        record::<Label>(init, "Label");
    }
}

/// Registers entity spawning fixture components and the lifecycle logging system.
pub fn register_spawning(ctx: &mut RegisterContext<'_>) {
    ctx.component(
        ComponentSpec::<Light>::new("Light")
            .field("radius", FieldType::Float)
            .field("color", FieldType::String)
            .from_prototype(|d| {
                Ok(Light {
                    radius: d.float("radius")?.unwrap_or(1.0),
                    color: d.string("color")?.unwrap_or_default(),
                })
            }),
    )
    .component(
        ComponentSpec::<Health>::new("Health")
            .field("max", FieldType::Int)
            .from_prototype(|d| {
                Ok(Health {
                    max: d.int("max")?.unwrap_or(1),
                })
            }),
    )
    .component(
        ComponentSpec::<Label>::new("Label")
            .field("text", FieldType::String)
            .from_prototype(|d| {
                Ok(Label {
                    text: d.string("text")?.unwrap_or_default(),
                })
            }),
    )
    .system(LifecycleRecorder);
}

/// The `spawning` extension with a `data` root that defaults to `fixtures/spawning`.
pub fn spawning_extension(data: Option<&std::path::Path>) -> FixtureExtension {
    let dir = data
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| fixtures_dir().join("spawning"));
    let manifest = format!(
        "[extension]\nid = \"spawning\"\nversion = \"0.1.0\"\ncore = \"^0.1\"\n\n[content]\nroots = [{:?}]\n",
        dir.to_string_lossy().replace('\\', "/")
    );
    FixtureExtension::new(&manifest, fixtures_dir()).on_register(register_spawning)
}

/// Builds a minimal manifest from an identifier, a core version range, and module names.
pub fn manifest(id: &str, core: &str, modules: &[&str]) -> String {
    let modules = modules
        .iter()
        .map(|m| format!("\"{m}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "[extension]\nid = \"{id}\"\nversion = \"0.1.0\"\ncore = \"{core}\"\n\n[modules]\nrequire = [{modules}]\n"
    )
}
