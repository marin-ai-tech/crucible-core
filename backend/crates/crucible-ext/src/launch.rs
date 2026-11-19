//! Launch phases, active extension selection, and the running server.
//!

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crucible_ecs::{BuildError, EntityUid, Tick, World};
use crucible_loop::{GameLoop, LoopConfig, RoundConfig, RoundFlow, Steps};
use crucible_proto::{
    DataRoot, Diagnostic, DiagnosticKind, PrototypeLoader, PrototypeSet, Registry, Report,
};
use semver::Version;

use crate::factory::{ComponentFactories, Prototypes, SpawnError, spawn_entity};
use crate::manifest::{ExtensionManifest, IncompatibleCore, ManifestError, ManifestSource};
use crate::module::{CoreModule, ModuleCatalog, ModuleError};
use crate::points::{CampaignDefinition, ExtensionPointKind, ExtensionPoints, VoidTile};
use crate::register::{ConsoleCommands, NetNames, RegisterContext, RegisterError, Registration};

/// Core version used to validate manifest requirements.
pub const CORE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Map subdirectory within a data root. Maps use a separate loader from prototypes,
/// like `Resources/Maps` in SS14.
pub const MAPS_DIR: &str = "maps";

/// Extension interface for a manifest, registration, and extension points.
pub trait Extension: Send + Sync + 'static {
    /// The `extension.toml` manifest source.
    fn manifest(&self) -> ManifestSource;

    /// Registers components, ordered systems and subscriptions, prototype kinds,
    /// round rules, network events, and console commands.
    fn register(&self, ctx: &mut RegisterContext<'_>);

    /// Configures extension point implementations.
    fn configure(&self, points: &mut ExtensionPoints) {
        let _ = points;
    }

    /// Shuts down before modules.
    fn shutdown(&self, world: &mut World) {
        let _ = world;
    }
}

/// A launch phase.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum LoadPhase {
    /// Discovers the active extension and its manifest.
    Discovery,
    /// Resolves versions and modules.
    Resolution,
    /// Registers modules, then the extension.
    Registration,
    /// Loads data from the core, modules, then the extension.
    Data,
    /// Validates loaded data.
    Validation,
    /// Builds the world and opens the lobby.
    Ready,
}

impl fmt::Display for LoadPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            LoadPhase::Discovery => "discovery",
            LoadPhase::Resolution => "version and module resolution",
            LoadPhase::Registration => "registration",
            LoadPhase::Data => "data loading",
            LoadPhase::Validation => "validation",
            LoadPhase::Ready => "readiness",
        })
    }
}

/// A launch failure with a phase and cause. The lobby remains closed.
#[derive(Debug)]
pub enum LaunchError {
    /// The server build does not include the active extension.
    UnknownExtension {
        /// Requested extension.
        requested: String,
        /// Extensions included in the build.
        available: Vec<String>,
    },
    /// Invalid manifest.
    Manifest(ManifestError),
    /// Incompatible core version.
    IncompatibleCore(IncompatibleCore),
    /// Module resolution failed.
    Modules(ModuleError),
    /// Registration conflicts.
    Registration(Vec<RegisterError>),
    /// System or subscription order could not be resolved.
    Build(BuildError),
    /// A data root could not be read.
    Data {
        /// Data root.
        root: PathBuf,
        /// Cause.
        message: String,
    },
    /// Data validation failed, with a complete report.
    Validation(Report),
    /// Required extension points are unfilled.
    MissingRequiredPoints(Vec<(ExtensionPointKind, Vec<String>)>),
    /// A core module failed its [`CoreModule::ready`] hook.
    ModuleReady {
        /// Module ID.
        module: String,
        /// Cause.
        message: String,
    },
}

impl LaunchError {
    /// Phase in which launch stopped.
    pub fn phase(&self) -> LoadPhase {
        match self {
            LaunchError::UnknownExtension { .. } | LaunchError::Manifest(_) => LoadPhase::Discovery,
            LaunchError::IncompatibleCore(_) | LaunchError::Modules(_) => LoadPhase::Resolution,
            LaunchError::Registration(_) | LaunchError::Build(_) => LoadPhase::Registration,
            LaunchError::Data { .. } => LoadPhase::Data,
            LaunchError::Validation(_) => LoadPhase::Validation,
            LaunchError::MissingRequiredPoints(_) | LaunchError::ModuleReady { .. } => {
                LoadPhase::Ready
            }
        }
    }
}

impl fmt::Display for LaunchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "launch failed during {}: ", self.phase())?;
        match self {
            LaunchError::UnknownExtension {
                requested,
                available,
            } => write!(
                f,
                "extension `{requested}` is absent from the server build (available: {})",
                available.join(", ")
            ),
            LaunchError::Manifest(e) => write!(f, "{e}"),
            LaunchError::IncompatibleCore(e) => write!(f, "{e}"),
            LaunchError::Modules(e) => write!(f, "{e}"),
            LaunchError::Registration(errors) => {
                f.write_str("registration errors:")?;
                for e in errors {
                    write!(f, "\n  {e}")?;
                }
                Ok(())
            }
            LaunchError::Build(e) => write!(f, "{e}"),
            LaunchError::Data { root, message } => {
                write!(f, "data root {}: {message}", root.display())
            }
            LaunchError::Validation(report) => write!(f, "data errors:\n{report}"),
            LaunchError::MissingRequiredPoints(points) => {
                f.write_str("required extension points are unfilled:")?;
                for (point, by) in points {
                    write!(f, "\n  {point} (required by {})", by.join(", "))?;
                }
                Ok(())
            }
            LaunchError::ModuleReady { module, message } => {
                write!(f, "core module `{module}`: {message}")
            }
        }
    }
}

impl std::error::Error for LaunchError {}

/// Launch configuration.
#[derive(Clone, Debug, Default)]
pub struct LaunchConfig {
    /// ID of the active extension.
    pub extension: String,
    /// Game loop configuration.
    pub loop_config: LoopConfig,
    /// Round configuration.
    pub rounds: RoundConfig,
    /// Overrides manifest data roots to validate a working copy of the data.
    pub content_roots: Option<Vec<PathBuf>>,
}

impl LaunchConfig {
    /// Uses the given active `extension` and defaults for all other settings.
    pub fn for_extension(extension: &str) -> Self {
        LaunchConfig {
            extension: extension.to_owned(),
            ..LaunchConfig::default()
        }
    }
}

/// Server build with the core version, modules, and available extensions.
pub struct Launcher {
    core_version: Version,
    core_data: Option<PathBuf>,
    modules: ModuleCatalog,
    extensions: Vec<Arc<dyn Extension>>,
}

impl Default for Launcher {
    fn default() -> Self {
        Launcher::new()
    }
}

/// Data root description for the loader, plus its directories on disk.
#[derive(Clone)]
struct DataSource {
    root: DataRoot,
    dirs: Vec<PathBuf>,
}

struct Prepared {
    manifest: ExtensionManifest,
    extension: Arc<dyn Extension>,
    modules: Vec<Arc<dyn CoreModule>>,
    registration: Registration,
    sources: Vec<DataSource>,
    phases: Vec<LoadPhase>,
}

impl Launcher {
    /// Creates an empty build using [`CORE_VERSION`].
    pub fn new() -> Self {
        Launcher {
            core_version: Version::parse(CORE_VERSION).expect("crate version must be valid SemVer"),
            core_data: None,
            modules: ModuleCatalog::new(),
            extensions: Vec::new(),
        }
    }

    /// Overrides the core version for compatibility tests.
    pub fn with_core_version(mut self, version: Version) -> Self {
        self.core_version = version;
        self
    }

    /// Sets a core data root to load first.
    pub fn with_core_data(mut self, dir: impl Into<PathBuf>) -> Self {
        self.core_data = Some(dir.into());
        self
    }

    /// Adds a core module to the build.
    pub fn module(mut self, module: impl CoreModule) -> Self {
        self.modules.add(module);
        self
    }

    /// Adds an extension to the build.
    pub fn extension(mut self, extension: impl Extension) -> Self {
        self.extensions.push(Arc::new(extension));
        self
    }

    /// Runs phases 1–5 without a world, as used by `validate-content`.
    /// [`LaunchError::Validation`] contains the same data report as a server launch.
    pub fn validate(&self, config: &LaunchConfig) -> Result<Report, LaunchError> {
        let prepared = self.prepare(config)?;
        let (_, report) = load_data(
            &prepared.registration.proto,
            &prepared.sources,
            &prepared.modules,
        )?;
        Ok(report)
    }

    /// Runs all phases and opens the lobby, or returns the failed phase and cause.
    pub fn launch(&self, config: LaunchConfig) -> Result<ServerRuntime, LaunchError> {
        let Prepared {
            manifest,
            extension,
            modules,
            registration,
            sources,
            mut phases,
        } = self.prepare(&config)?;

        // 4–5. Data loading and validation.
        phases.push(LoadPhase::Data);
        let (set, report) = load_data(&registration.proto, &sources, &modules)?;
        phases.push(LoadPhase::Validation);
        for w in report.warnings() {
            tracing::warn!(target: "crucible_ext::load", "{w}");
        }

        // 6. Readiness.
        let Registration {
            builder,
            proto,
            factories,
            net,
            commands,
            rules,
            phases: tick_phases,
            ..
        } = registration;
        let mut points = ExtensionPoints::new();
        for module in &modules {
            module.configure(&mut points);
        }
        extension.configure(&mut points);
        let missing = points.missing_required();
        if !missing.is_empty() {
            return Err(LaunchError::MissingRequiredPoints(missing));
        }
        let has_campaign = points.has_campaign();
        let campaign = points.campaign_definition().cloned();
        let (round_points, void_tile) = points.into_parts();

        let (mut world, systems) = builder.build().map_err(LaunchError::Build)?;
        let factories = Arc::new(factories);
        world.insert_resource(Prototypes {
            set: Arc::new(set),
            factories: Arc::clone(&factories),
            registry: Arc::new(proto.clone()),
        });
        world.insert_resource(VoidTile(void_tile));
        for module in &modules {
            module
                .ready(&mut world)
                .map_err(|message| LaunchError::ModuleReady {
                    module: module.id().to_owned(),
                    message,
                })?;
        }
        let mut game = GameLoop::new(config.loop_config.clone(), world, systems);
        for (phase, name, handler) in tick_phases {
            game.add_phase(phase, name, handler);
        }
        let rounds = RoundFlow::new(
            config.rounds.clone(),
            round_points,
            rules.into_iter().map(|(_, r)| r).collect(),
        );
        phases.push(LoadPhase::Ready);
        tracing::info!(
            target: "crucible_ext::load",
            extension = %manifest.id,
            modules = ?modules.iter().map(|m| m.id()).collect::<Vec<_>>(),
            campaign = has_campaign,
            "server ready, lobby open"
        );
        Ok(ServerRuntime {
            game,
            rounds,
            manifest,
            campaign,
            extension,
            modules,
            commands,
            net,
            report,
            phases,
            hot: HotReload::new(proto, sources),
        })
    }

    /// Runs phases 1–3.
    fn prepare(&self, config: &LaunchConfig) -> Result<Prepared, LaunchError> {
        let mut phases = vec![LoadPhase::Discovery];

        // 1. Discover manifests of all compiled extensions. Inactive extensions
        // provide no registrations, data, or extension point implementations.
        let mut extension = None;
        let mut available = Vec::new();
        for ext in &self.extensions {
            let manifest =
                ExtensionManifest::parse(&ext.manifest()).map_err(LaunchError::Manifest)?;
            available.push(manifest.id.clone());
            if extension.is_none() && manifest.id == config.extension {
                extension = Some((manifest, Arc::clone(ext)));
            }
        }
        let Some((manifest, extension)) = extension else {
            return Err(LaunchError::UnknownExtension {
                requested: config.extension.clone(),
                available,
            });
        };

        // 2. Resolve versions and modules.
        phases.push(LoadPhase::Resolution);
        manifest
            .check_core(&self.core_version)
            .map_err(LaunchError::IncompatibleCore)?;
        let modules = self
            .modules
            .resolve(&format!("ext:{}", manifest.id), &manifest.modules)
            .map_err(LaunchError::Modules)?;

        // 3. Register modules, then the extension.
        phases.push(LoadPhase::Registration);
        let mut registration = Registration::new();
        for module in &modules {
            module.register(&mut registration.context(&format!("module:{}", module.id())));
        }
        {
            let mut ctx = registration.context(&format!("ext:{}", manifest.id));
            for (name, category) in &manifest.ignored {
                ctx.ignore_component(name, *category);
            }
            extension.register(&mut ctx);
        }
        if !registration.errors.is_empty() {
            return Err(LaunchError::Registration(std::mem::take(
                &mut registration.errors,
            )));
        }

        let mut sources = Vec::new();
        if let Some(dir) = &self.core_data {
            sources.push(DataSource {
                root: DataRoot::new("core"),
                dirs: vec![dir.clone()],
            });
        }
        for module in &modules {
            if let Some(dir) = module.data_dir() {
                sources.push(DataSource {
                    root: DataRoot::new(&format!("module:{}", module.id())),
                    dirs: vec![dir],
                });
            }
        }
        sources.push(DataSource {
            root: manifest.data_root(),
            dirs: config
                .content_roots
                .clone()
                .unwrap_or_else(|| manifest.content_roots.clone()),
        });

        Ok(Prepared {
            manifest,
            extension,
            modules,
            registration,
            sources,
            phases,
        })
    }
}

/// Iterates through a data root's `*.yml` and `*.yaml` files recursively in path order,
/// excluding the root [`MAPS_DIR`] directory.
fn prototype_files(dir: &Path) -> Result<Vec<PathBuf>, LaunchError> {
    fn walk(dir: &Path, top: bool, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if path.is_dir() {
                if !(top && path.file_name().is_some_and(|n| n == MAPS_DIR)) {
                    walk(&path, false, out)?;
                }
            } else if path.extension().is_some_and(|e| e == "yml" || e == "yaml") {
                out.push(path);
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(dir, true, &mut out).map_err(|e| LaunchError::Data {
        root: dir.to_path_buf(),
        message: e.to_string(),
    })?;
    out.sort();
    Ok(out)
}

/// Phases 4–5: load and validate data. Failures return [`LaunchError::Validation`].
fn load_data(
    registry: &Registry,
    sources: &[DataSource],
    modules: &[Arc<dyn CoreModule>],
) -> Result<(PrototypeSet, Report), LaunchError> {
    let mut loader = PrototypeLoader::new(registry);
    for source in sources {
        let id = loader.add_root(source.root.clone());
        for dir in &source.dirs {
            for file in prototype_files(dir)? {
                let text = std::fs::read_to_string(&file).map_err(|e| LaunchError::Data {
                    root: dir.clone(),
                    message: format!("{}: {e}", file.display()),
                })?;
                loader.add_source(id, &file.to_string_lossy().replace('\\', "/"), &text);
            }
        }
    }
    let mut result = loader.finish(None);
    // Module checks also run on an incomplete set. An omitted record
    // cannot introduce a false error.
    for module in modules {
        for message in module.validate(&result.set) {
            result.report.diagnostics.push(Diagnostic {
                file: None,
                pos: None,
                proto: None,
                kind: DiagnosticKind::ModuleCheck {
                    module: module.id().to_owned(),
                    message,
                },
            });
        }
    }
    result.into_result().map_err(LaunchError::Validation)
}

/// Hot reload state with the registry, data roots, and file fingerprints.
struct HotReload {
    #[cfg_attr(not(debug_assertions), allow(dead_code))]
    registry: Registry,
    #[cfg_attr(not(debug_assertions), allow(dead_code))]
    sources: Vec<DataSource>,
    #[cfg_attr(not(debug_assertions), allow(dead_code))]
    stamp: BTreeMap<PathBuf, (std::time::SystemTime, u64)>,
}

impl HotReload {
    fn new(registry: Registry, sources: Vec<DataSource>) -> Self {
        let stamp = if cfg!(debug_assertions) {
            stamp(&sources)
        } else {
            BTreeMap::new()
        };
        HotReload {
            registry,
            sources,
            stamp,
        }
    }
}

fn stamp(sources: &[DataSource]) -> BTreeMap<PathBuf, (std::time::SystemTime, u64)> {
    let mut out = BTreeMap::new();
    for source in sources {
        for dir in &source.dirs {
            for file in prototype_files(dir).unwrap_or_default() {
                if let Ok(meta) = std::fs::metadata(&file) {
                    let modified = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
                    out.insert(file, (modified, meta.len()));
                }
            }
        }
    }
    out
}

/// Whether the build supports prototype hot reload (requires `debug_assertions`).
pub const HOT_RELOAD_AVAILABLE: bool = cfg!(debug_assertions);

/// The running in-process server with its game loop, round flow, and active extension.
pub struct ServerRuntime {
    game: GameLoop,
    rounds: RoundFlow,
    manifest: ExtensionManifest,
    campaign: Option<CampaignDefinition>,
    extension: Arc<dyn Extension>,
    modules: Vec<Arc<dyn CoreModule>>,
    commands: ConsoleCommands,
    net: NetNames,
    report: Report,
    phases: Vec<LoadPhase>,
    hot: HotReload,
}

impl ServerRuntime {
    /// Manifest of the active extension.
    pub fn manifest(&self) -> &ExtensionManifest {
        &self.manifest
    }

    /// Campaign data and persistence rules declared by the active extension.
    pub fn campaign_definition(&self) -> Option<&CampaignDefinition> {
        self.campaign.as_ref()
    }

    /// Enabled modules in registration order.
    pub fn modules(&self) -> Vec<&'static str> {
        self.modules.iter().map(|m| m.id()).collect()
    }

    /// Completed launch phases.
    pub fn phases(&self) -> &[LoadPhase] {
        &self.phases
    }

    /// Data loading report containing warnings.
    pub fn load_report(&self) -> &Report {
        &self.report
    }

    /// Network event names.
    pub fn net_names(&self) -> &NetNames {
        &self.net
    }

    /// Console commands.
    pub fn commands(&self) -> &ConsoleCommands {
        &self.commands
    }

    /// Game loop.
    pub fn game(&self) -> &GameLoop {
        &self.game
    }

    /// Mutable game loop access for clock changes and ticks without round flow.
    pub fn game_mut(&mut self) -> &mut GameLoop {
        &mut self.game
    }

    /// Replaces the game loop clock.
    pub fn with_clock(mut self, clock: Box<dyn crucible_loop::Clock>) -> Self {
        self.game = self.game.with_clock(clock);
        self
    }

    /// World.
    pub fn world(&self) -> &World {
        self.game.world()
    }

    /// Mutable world access.
    pub fn world_mut(&mut self) -> &mut World {
        self.game.world_mut()
    }

    /// Round flow.
    pub fn rounds(&self) -> &RoundFlow {
        &self.rounds
    }

    /// Mutable round flow access for player and admin operations.
    pub fn rounds_mut(&mut self) -> &mut RoundFlow {
        &mut self.rounds
    }

    /// Mutable access to the world and round flow together for requested phase transitions.
    pub fn world_and_rounds(&mut self) -> (&mut World, &mut RoundFlow) {
        (self.game.world_mut(), &mut self.rounds)
    }

    /// Current prototype set.
    pub fn prototypes(&self) -> Arc<PrototypeSet> {
        Arc::clone(
            &self
                .world()
                .resource::<Prototypes>()
                .expect("launch inserts the prototypes resource")
                .set,
        )
    }

    /// Runs one tick, updating systems before round flow.
    pub fn tick(&mut self) -> Tick {
        let rounds = &mut self.rounds;
        self.game
            .tick_with(&mut |world, dt| rounds.update(world, dt))
    }

    /// Runs `n` consecutive ticks.
    pub fn run_ticks(&mut self, n: u32) {
        for _ in 0..n {
            self.tick();
        }
    }

    /// Runs one iteration using the loop clock.
    pub fn run_iteration(&mut self) -> Steps {
        let rounds = &mut self.rounds;
        self.game
            .run_iteration_with(&mut |world, dt| rounds.update(world, dt))
    }

    /// Creates an entity from an `entity` prototype.
    pub fn spawn(&mut self, id: &str) -> Result<EntityUid, SpawnError> {
        let Prototypes { set, factories, .. } = self
            .world()
            .resource::<Prototypes>()
            .cloned()
            .ok_or(SpawnError::NoPrototypes)?;
        spawn_entity(self.world_mut(), &set, &factories, id)
    }

    /// Component factories.
    pub fn factories(&self) -> Arc<ComponentFactories> {
        Arc::clone(
            &self
                .world()
                .resource::<Prototypes>()
                .expect("launch inserts the prototypes resource")
                .factories,
        )
    }

    /// Runs console command `name` with arguments.
    pub fn execute_command(&mut self, name: &str, args: &[&str]) -> Result<String, String> {
        let command = self
            .commands
            .get(name)
            .ok_or_else(|| format!("unknown command `{name}`"))?;
        (command.handler)(self.game.world_mut(), args)
    }

    /// Stops the extension, then the modules in reverse registration order.
    /// Returns the source names in shutdown order.
    pub fn shutdown(mut self) -> Vec<String> {
        let mut order = Vec::new();
        let world = self.game.world_mut();
        self.extension.shutdown(world);
        order.push(format!("ext:{}", self.manifest.id));
        for module in self.modules.iter().rev() {
            module.shutdown(world);
            order.push(format!("module:{}", module.id()));
        }
        order
    }

    /// Reloads prototypes from disk in development builds. On success, replaces the set
    /// for newly created entities and emits
    /// [`PrototypesReloaded`](crate::PrototypesReloaded). On failure, logs the report
    /// and continues using the previous set.
    #[cfg(debug_assertions)]
    pub fn reload_prototypes(&mut self) -> Result<Report, Report> {
        let result = load_data(&self.hot.registry, &self.hot.sources, &self.modules);
        self.hot.stamp = stamp(&self.hot.sources);
        match result {
            Ok((set, report)) => {
                let count = set.len();
                let world = self.game.world_mut();
                let old = world
                    .resource::<Prototypes>()
                    .expect("launch inserts the prototypes resource");
                let (factories, registry) = (Arc::clone(&old.factories), Arc::clone(&old.registry));
                world.insert_resource(Prototypes {
                    set: Arc::new(set),
                    factories,
                    registry,
                });
                tracing::info!(target: "crucible_ext::reload", prototypes = count, "prototypes reloaded");
                // Rebuild data-derived tables such as tiles and friction before emitting the event.
                let mut failed = Report::default();
                for module in &self.modules {
                    if let Err(message) = module.reloaded(world) {
                        tracing::error!(target: "crucible_ext::reload", module = module.id(), %message, "module failed to rebuild its data after reload");
                        failed.diagnostics.push(Diagnostic {
                            file: None,
                            pos: None,
                            proto: None,
                            kind: DiagnosticKind::ModuleCheck {
                                module: module.id().to_owned(),
                                message,
                            },
                        });
                    }
                }
                world.raise_broadcast(&mut crate::PrototypesReloaded { prototypes: count });
                if failed.has_errors() {
                    return Err(failed);
                }
                Ok(report)
            }
            Err(LaunchError::Validation(report)) => {
                tracing::error!(
                    target: "crucible_ext::reload",
                    "prototype reload rejected, keeping the previous set:\n{report}"
                );
                Err(report)
            }
            Err(other) => {
                let text = other.to_string();
                tracing::error!(target: "crucible_ext::reload", "prototype reload rejected: {text}");
                Err(Report::default())
            }
        }
    }

    /// Reloads prototypes when their source files have changed since the last load.
    /// Development servers call this periodically.
    #[cfg(debug_assertions)]
    pub fn poll_hot_reload(&mut self) -> Option<Result<Report, Report>> {
        if stamp(&self.hot.sources) == self.hot.stamp {
            return None;
        }
        Some(self.reload_prototypes())
    }
}
