//! Core module and extension registration, manifests, and extension points.
//! The launcher selects an active extension and resolves and validates its data.
//! The runtime creates entities from resolved prototypes.
//!
//! # Launch sequence
//!
//! 1. Assemble a server from core modules ([`CoreModule`]) and extensions ([`Extension`]) using
//!    [`Launcher`].
//! 2. [`Launcher::launch`] follows the [`LoadPhase`] sequence. It discovers the extension named
//!    by [`LaunchConfig::extension`], resolves the core version and module dependencies,
//!    registers modules followed by the extension through [`RegisterContext`], then loads data
//!    from the core, modules, and extension, in that order. Validation precedes readiness.
//!    Any phase failure returns a [`LaunchError`] with the phase and cause before the lobby opens.
//! 3. The running [`ServerRuntime`] holds the game loop (`crucible-loop`) and round flow.
//!    [`ExtensionPoints`] supply implementations for the round flow.
//!
//! [`Launcher::validate`] runs phases 1–5 without creating a world, for `validate-content`.
//!
//! # Corresponding Robust and SS14 concepts
//!
//! | Robust / SS14 | Crucible |
//! |---|---|
//! | `IModLoader` and `Content.*` assemblies | [`Launcher`], [`Extension`], [`CoreModule`] |
//! | `[RegisterComponent]`, `ComponentFactory`, and `DataField` | [`RegisterContext::component`] with [`ComponentSpec`] |
//! | `EntityManager.SpawnEntity(protoId, coords)` | [`spawn_entity`], [`SpawnExt::spawn_prototype`], [`ServerRuntime::spawn`] |
//! | `[NetSerializable]` network events | [`RegisterContext::network_event`] uses stable [`NetNames`] |
//! | `IConsoleHost.RegisterCommand` | [`RegisterContext::console_command`] |
//! | `GameRulePrototype`, `GameTicker` presets | [`RegisterContext::game_rule`] + `crucible_loop::GameRule` |
//! | `PrototypeManager.ReloadPrototypes` (dev) | [`ServerRuntime::reload_prototypes`] (dev builds) |

mod factory;
mod launch;
mod manifest;
mod module;
mod points;
mod register;

pub use factory::{
    ComponentData, ComponentDataError, ComponentFactories, ENTITY_KIND, FieldError, Prototypes,
    PrototypesReloaded, ReadComponent, SpawnError, SpawnExt, SpawnFromPrototype, spawn_entity,
};
pub use launch::{
    CORE_VERSION, Extension, HOT_RELOAD_AVAILABLE, LaunchConfig, LaunchError, Launcher, LoadPhase,
    MAPS_DIR, ServerRuntime,
};
pub use manifest::{
    ExtensionManifest, IncompatibleCore, ManifestError, ManifestSource, PrototypeOverride,
};
pub use module::{CoreModule, ModuleCatalog, ModuleError};
pub use points::{
    CampaignDataMigration, CampaignDefinition, ExtensionPointKind, ExtensionPoints, NoVoidEffect,
    VoidTile, VoidTilePolicy,
};
pub use register::{
    CommandHandler, ComponentSpec, ConsoleCommand, ConsoleCommands, NetEventEntry, NetNames,
    RegisterContext, RegisterError, entity_kind,
};
