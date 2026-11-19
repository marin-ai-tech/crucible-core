//! Core modules and their dependency graph.

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use crucible_ecs::World;
use crucible_proto::PrototypeSet;

use crate::points::ExtensionPoints;
use crate::register::RegisterContext;

/// A core module provides a game mechanic independent of any setting.
///
/// The launcher registers a module only when the active extension or another enabled module
/// requires it. Otherwise its systems, components, and prototype kinds are absent.
pub trait CoreModule: Send + Sync + 'static {
    /// The module ID used in the manifest's `[modules] require` list.
    fn id(&self) -> &'static str;

    /// Dependencies, registered before this module.
    fn dependencies(&self) -> &'static [&'static str] {
        &[]
    }

    /// Registers components, systems, prototype kinds, round rules, and commands.
    fn register(&self, ctx: &mut RegisterContext<'_>);

    /// Sets default extension point implementations or marks slots mandatory
    /// ([`ExtensionPoints::require`]).
    fn configure(&self, points: &mut ExtensionPoints) {
        let _ = points;
    }

    /// At readiness, the world contains [`Prototypes`](crate::Prototypes) and extension point resources.
    /// The module can build tables derived from data, such as numeric tile IDs.
    /// Modules run in registration order. A failure aborts startup with
    /// [`LaunchError::ModuleReady`](crate::LaunchError).
    fn ready(&self, world: &mut World) -> Result<(), String> {
        let _ = world;
        Ok(())
    }

    /// Validates constraints across the prototype set, such as requiring exactly one empty tile.
    /// Runs during launch, `validate-content`, and hot reload. A failure prevents the lobby
    /// from opening or the reloaded data from taking effect.
    fn validate(&self, set: &PrototypeSet) -> Vec<String> {
        let _ = set;
        Vec::new()
    }

    /// Rebuilds data-derived tables after a successful prototype hot reload in a development build.
    /// By default, [`ready`](CoreModule::ready) runs again to replace prototype-derived resources.
    /// Override this method when repeating the ready hook would disturb
    /// existing module state.
    fn reloaded(&self, world: &mut World) -> Result<(), String> {
        self.ready(world)
    }

    /// Module data root, loaded after the core and before the extension.
    fn data_dir(&self) -> Option<PathBuf> {
        None
    }

    /// Shuts down in reverse registration order.
    fn shutdown(&self, world: &mut World) {
        let _ = world;
    }
}

/// Module resolution error.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ModuleError {
    /// The server was built without a required module.
    Unknown {
        /// Missing module ID.
        module: String,
        /// Extension or module that requires it.
        required_by: String,
    },
    /// Dependency cycle.
    Cycle {
        /// Modules in the cycle, in traversal order.
        modules: Vec<String>,
    },
    /// Two modules share an ID.
    Duplicate {
        /// Module ID.
        module: String,
    },
}

impl fmt::Display for ModuleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ModuleError::Unknown {
                module,
                required_by,
            } => write!(
                f,
                "the server has no core module `{module}`, required by `{required_by}`"
            ),
            ModuleError::Cycle { modules } => {
                write!(f, "module dependency cycle: {}", modules.join(" → "))
            }
            ModuleError::Duplicate { module } => {
                write!(f, "duplicate core module `{module}`")
            }
        }
    }
}

impl std::error::Error for ModuleError {}

/// Catalog of core modules built into the server.
#[derive(Default, Clone)]
pub struct ModuleCatalog {
    modules: Vec<Arc<dyn CoreModule>>,
}

impl ModuleCatalog {
    /// Creates an empty catalog.
    pub fn new() -> Self {
        ModuleCatalog::default()
    }

    /// Adds a module.
    pub fn add(&mut self, module: impl CoreModule) -> &mut Self {
        self.modules.push(Arc::new(module));
        self
    }

    /// Returns module IDs in the catalog.
    pub fn ids(&self) -> Vec<&'static str> {
        self.modules.iter().map(|m| m.id()).collect()
    }

    /// Resolves the modules in `required` for extension `requester`, including dependencies.
    /// Dependencies precede their dependents. Otherwise, list order is preserved.
    pub fn resolve(
        &self,
        requester: &str,
        required: &[String],
    ) -> Result<Vec<Arc<dyn CoreModule>>, ModuleError> {
        let mut by_id: HashMap<&str, &Arc<dyn CoreModule>> = HashMap::new();
        for m in &self.modules {
            if by_id.insert(m.id(), m).is_some() {
                return Err(ModuleError::Duplicate {
                    module: m.id().to_owned(),
                });
            }
        }
        let mut state: HashMap<&str, Mark> = HashMap::new();
        let mut order = Vec::new();
        let mut stack = Vec::new();
        for id in required {
            visit(id, requester, &by_id, &mut state, &mut stack, &mut order)?;
        }
        Ok(order)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mark {
    Visiting,
    Done,
}

fn visit<'a>(
    id: &str,
    required_by: &str,
    by_id: &HashMap<&'a str, &'a Arc<dyn CoreModule>>,
    state: &mut HashMap<&'a str, Mark>,
    stack: &mut Vec<&'a str>,
    order: &mut Vec<Arc<dyn CoreModule>>,
) -> Result<(), ModuleError> {
    let Some((&key, &module)) = by_id.get_key_value(id) else {
        return Err(ModuleError::Unknown {
            module: id.to_owned(),
            required_by: required_by.to_owned(),
        });
    };
    match state.get(key) {
        Some(Mark::Done) => return Ok(()),
        Some(Mark::Visiting) => {
            let start = stack.iter().position(|s| *s == key).unwrap_or(0);
            let mut modules: Vec<String> = stack[start..].iter().map(|s| (*s).to_owned()).collect();
            modules.push(key.to_owned());
            return Err(ModuleError::Cycle { modules });
        }
        None => {}
    }
    state.insert(key, Mark::Visiting);
    stack.push(key);
    for dep in module.dependencies() {
        visit(dep, key, by_id, state, stack, order)?;
    }
    stack.pop();
    state.insert(key, Mark::Done);
    order.push(Arc::clone(module));
    Ok(())
}
