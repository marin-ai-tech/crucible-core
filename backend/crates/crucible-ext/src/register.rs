//! Module and extension registration for components, systems, prototype kinds,
//! network names, round rules, and console commands.

use std::any::{TypeId, type_name};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use crucible_ecs::{
    Component, ComponentRegistration, NetworkPolicy, NetworkedComponent, RegistryError, System,
    World, WorldBuilder,
};
use crucible_loop::{GameRule, PhaseHandler, TickPhase};
use crucible_players::PlayersSystem;
use crucible_proto::{
    ComponentDef, FieldDef, FieldType, IgnoreCategory, KindDef, PolymorphicDef,
    RegistryError as ProtoRegistryError,
};

use crate::factory::{ComponentData, ComponentFactories, FieldError, ReadComponent};

const RESOURCE_POLYMORPHIC_PREFIX: &str = "@resource:";

fn resource_polymorphic_base<Scope: 'static>() -> String {
    format!("{RESOURCE_POLYMORPHIC_PREFIX}{}", type_name::<Scope>())
}

/// Component registration metadata with its YAML name, field schema, network policy,
/// and decoder for resolved prototype fields.
///
/// A component without [`from_prototype`](ComponentSpec::from_prototype) is code-only.
/// Prototype validation reports it as an unknown component.
pub struct ComponentSpec<C: Component> {
    name: String,
    fields: Vec<FieldDef>,
    network: NetworkPolicy,
    persisted: bool,
    read: Option<ReadComponent<C>>,
    networked_fields: Option<fn(ComponentRegistration) -> ComponentRegistration>,
}

impl<C: Component> ComponentSpec<C> {
    /// Component `C` registered as `name`.
    pub fn new(name: &str) -> Self {
        ComponentSpec {
            name: name.to_owned(),
            fields: Vec::new(),
            network: NetworkPolicy::ServerOnly,
            persisted: false,
            read: None,
            networked_fields: None,
        }
    }

    /// Field schema for prototype validation.
    pub fn field(mut self, name: &str, ty: FieldType) -> Self {
        self.fields.push(FieldDef::new(name, ty));
        self
    }

    /// Network policy. A replicated component uses its component name as
    /// its stable network name.
    pub fn network(mut self, policy: NetworkPolicy) -> Self {
        self.network = policy;
        self
    }

    /// Persistence flag.
    pub fn persisted(mut self, persisted: bool) -> Self {
        self.persisted = persisted;
        self
    }

    /// Decodes a component from resolved fields and makes it available to prototypes.
    pub fn from_prototype(
        mut self,
        read: impl Fn(&ComponentData<'_>) -> Result<C, FieldError> + Send + Sync + 'static,
    ) -> Self {
        self.read = Some(Arc::new(read));
        self
    }
}

impl<C: NetworkedComponent> ComponentSpec<C> {
    /// The ECS registry uses the `#[networked]` field schema for per-field deltas.
    /// Prototype fields are tracked separately.
    pub fn networked_fields(mut self) -> Self {
        fn apply<C: NetworkedComponent>(r: ComponentRegistration) -> ComponentRegistration {
            r.networked_fields::<C>()
        }
        self.networked_fields = Some(apply::<C>);
        self
    }
}

impl<C: Component + Default> ComponentSpec<C> {
    /// Uses the default value for a component with no prototype fields.
    pub fn from_default(self) -> Self {
        self.from_prototype(|_| Ok(C::default()))
    }
}

/// Registration error.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum RegisterError {
    /// ECS component registry conflict by name or type, with both sources.
    Component(RegistryError),
    /// Prototype registry conflict in a kind, polymorphic type, or ignore category.
    Prototype {
        /// Registration source.
        source: String,
        /// Description.
        message: String,
    },
    /// Two sources registered a network event under the same name.
    NetEventConflict {
        /// Event name.
        name: String,
        /// First source.
        first_source: String,
        /// Second source.
        second_source: String,
    },
    /// The network event type was already registered under another name.
    NetEventTypeConflict {
        /// Event type.
        type_name: &'static str,
        /// First name.
        first_name: String,
        /// First source.
        first_source: String,
        /// Second source.
        second_source: String,
    },
    /// Two sources registered a console command under the same name.
    CommandConflict {
        /// Command name.
        name: String,
        /// First source.
        first_source: String,
        /// Second source.
        second_source: String,
    },
    /// Two round rules share an ID.
    RuleConflict {
        /// Rule ID.
        id: String,
        /// First source.
        first_source: String,
        /// Second source.
        second_source: String,
    },
}

impl fmt::Display for RegisterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RegisterError::Component(e) => write!(f, "{e}"),
            RegisterError::Prototype { source, message } => write!(f, "«{source}»: {message}"),
            RegisterError::NetEventConflict {
                name,
                first_source,
                second_source,
            } => write!(
                f,
                "network event `{name}` was registered twice by `{first_source}` and `{second_source}`"
            ),
            RegisterError::NetEventTypeConflict {
                type_name,
                first_name,
                first_source,
                second_source,
            } => write!(
                f,
                "type {type_name} was already registered as network event `{first_name}` by \
                 `{first_source}` and registered again by `{second_source}`"
            ),
            RegisterError::CommandConflict {
                name,
                first_source,
                second_source,
            } => write!(
                f,
                "console command `{name}` was registered twice by `{first_source}` and `{second_source}`"
            ),
            RegisterError::RuleConflict {
                id,
                first_source,
                second_source,
            } => write!(
                f,
                "round rule `{id}` was registered twice by `{first_source}` and `{second_source}`"
            ),
        }
    }
}

impl std::error::Error for RegisterError {}

/// Network event registered under a stable name.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct NetEventEntry {
    /// Registration source.
    pub source: String,
    /// Rust type name.
    pub type_name: &'static str,
    type_id: TypeId,
}

/// Stable network names for events. Components use the ECS component registry.
#[derive(Clone, Default, Debug)]
pub struct NetNames {
    events: BTreeMap<String, NetEventEntry>,
}

impl NetNames {
    /// Looks up a network event by name.
    pub fn event(&self, name: &str) -> Option<&NetEventEntry> {
        self.events.get(name)
    }

    /// Returns the network name for event type `E`.
    pub fn event_name<E: 'static>(&self) -> Option<&str> {
        let id = TypeId::of::<E>();
        self.events
            .iter()
            .find(|(_, e)| e.type_id == id)
            .map(|(n, _)| n.as_str())
    }

    /// Network event names in alphabetical order.
    pub fn event_names(&self) -> impl Iterator<Item = &str> {
        self.events.keys().map(String::as_str)
    }

    fn register<E: 'static>(&mut self, name: &str, source: &str) -> Result<(), RegisterError> {
        if let Some(first) = self.events.get(name) {
            return Err(RegisterError::NetEventConflict {
                name: name.to_owned(),
                first_source: first.source.clone(),
                second_source: source.to_owned(),
            });
        }
        let type_id = TypeId::of::<E>();
        if let Some((first_name, first)) = self.events.iter().find(|(_, e)| e.type_id == type_id) {
            return Err(RegisterError::NetEventTypeConflict {
                type_name: std::any::type_name::<E>(),
                first_name: first_name.clone(),
                first_source: first.source.clone(),
                second_source: source.to_owned(),
            });
        }
        self.events.insert(
            name.to_owned(),
            NetEventEntry {
                source: source.to_owned(),
                type_name: std::any::type_name::<E>(),
                type_id,
            },
        );
        Ok(())
    }
}

/// Console command handler that returns a result from the world and arguments.
pub type CommandHandler = fn(&mut World, &[&str]) -> Result<String, String>;

/// Console command.
#[derive(Clone)]
pub struct ConsoleCommand {
    /// Registration source.
    pub source: String,
    /// Help text.
    pub help: String,
    /// Handler.
    pub handler: CommandHandler,
}

/// Console commands keyed by name.
#[derive(Clone, Default)]
pub struct ConsoleCommands {
    commands: BTreeMap<String, ConsoleCommand>,
}

impl ConsoleCommands {
    /// Looks up a command by name.
    pub fn get(&self, name: &str) -> Option<&ConsoleCommand> {
        self.commands.get(name)
    }

    /// Command names in alphabetical order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.commands.keys().map(String::as_str)
    }
}

/// The core entity prototype kind supports inheritance, components, and the
/// `name`, `description`, and `suffix` string fields used by SS14's `EntityPrototype`.
pub fn entity_kind() -> KindDef {
    KindDef::with_components(
        crate::factory::ENTITY_KIND,
        vec![
            FieldDef::new("name", FieldType::String),
            FieldDef::new("description", FieldType::String),
            FieldDef::new("suffix", FieldType::String),
        ],
    )
}

/// State accumulated during registration.
pub(crate) struct Registration {
    pub builder: WorldBuilder,
    pub proto: crucible_proto::Registry,
    pub factories: ComponentFactories,
    pub net: NetNames,
    pub commands: ConsoleCommands,
    pub rules: Vec<(String, Box<dyn GameRule>)>,
    pub phases: Vec<(TickPhase, &'static str, PhaseHandler)>,
    pub errors: Vec<RegisterError>,
}

impl Registration {
    pub fn new() -> Self {
        let mut proto = crucible_proto::Registry::new();
        proto.register_kind(entity_kind()).expect("empty registry");
        Registration {
            builder: {
                let mut builder = WorldBuilder::new();
                builder.add_system(PlayersSystem);
                builder
            },
            proto,
            factories: ComponentFactories::new(),
            net: NetNames::default(),
            commands: ConsoleCommands::default(),
            rules: Vec::new(),
            phases: Vec::new(),
            errors: Vec::new(),
        }
    }

    pub fn context(&mut self, source: &str) -> RegisterContext<'_> {
        RegisterContext {
            source: source.to_owned(),
            reg: self,
        }
    }
}

/// Registration context for one source, either a module `module:<id>` or an extension `ext:<id>`.
///
/// At the end of registration, any accumulated errors stop startup and explain why.
pub struct RegisterContext<'a> {
    source: String,
    reg: &'a mut Registration,
}

impl RegisterContext<'_> {
    /// Registration source.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Registers the ECS component, its prototype validation schema,
    /// and its entity creation factory.
    pub fn component<C: Component>(&mut self, spec: ComponentSpec<C>) -> &mut Self {
        let mut registration = ComponentRegistration::new::<C>(&spec.name, &self.source)
            .network(spec.network)
            .persisted(spec.persisted);
        match spec.networked_fields {
            Some(apply) => registration = apply(registration),
            None => {
                for f in &spec.fields {
                    registration = registration.field(&f.name, f.ty.to_string());
                }
            }
        }
        if let Err(e) = self.reg.builder.register_component(registration) {
            self.reg.errors.push(RegisterError::Component(e));
            return self;
        }
        if let Some(read) = spec.read {
            if let Err(e) = self.reg.proto.register_component(ComponentDef {
                name: spec.name.clone(),
                fields: spec.fields,
            }) {
                self.prototype_error(e.to_string());
                return self;
            }
            self.reg.factories.insert::<C>(&spec.name, read);
        }
        self
    }

    /// Registers a system, its subscriptions, and ordering constraints.
    pub fn system<S: System>(&mut self, system: S) -> &mut Self {
        self.reg.builder.add_system(system);
        self
    }

    /// Registers directly with the world builder for crates such as `crucible_map::register`.
    /// Component registry errors are collected just as for
    /// [`component`](Self::component).
    pub fn world_builder(
        &mut self,
        register: impl FnOnce(&mut WorldBuilder) -> Result<(), RegistryError>,
    ) -> &mut Self {
        if let Err(e) = register(&mut self.reg.builder) {
            self.reg.errors.push(RegisterError::Component(e));
        }
        self
    }

    /// Registers directly with the prototype registry for crates such as
    /// `crucible_map::register_tile_kind` that provide their own kind registration function.
    pub fn prototype_registry(
        &mut self,
        register: impl FnOnce(&mut crucible_proto::Registry) -> Result<(), ProtoRegistryError>,
    ) -> &mut Self {
        if let Err(e) = register(&mut self.reg.proto) {
            self.prototype_error(e.to_string());
        }
        self
    }

    /// Registers a tick-phase handler outside ECS systems, for tasks such as physics,
    /// input, or replication. Each phase runs handlers in registration order:
    /// modules in dependency order followed by the extension.
    pub fn tick_phase(
        &mut self,
        phase: TickPhase,
        name: &'static str,
        handler: impl FnMut(&mut World, f32) + Send + 'static,
    ) -> &mut Self {
        self.reg.phases.push((phase, name, Box::new(handler)));
        self
    }

    /// Registers a world resource.
    pub fn resource<R: Send + Sync + 'static>(&mut self, resource: R) -> &mut Self {
        self.reg.builder.insert_resource(resource);
        self
    }

    /// Registers a prototype kind.
    pub fn prototype_kind(&mut self, def: KindDef) -> &mut Self {
        if let Err(e) = self.reg.proto.register_kind(def) {
            self.prototype_error(e.to_string());
        }
        self
    }

    /// Registers a polymorphic `!type:` type.
    pub fn polymorphic_type(&mut self, def: PolymorphicDef) -> &mut Self {
        if def.base.starts_with(RESOURCE_POLYMORPHIC_PREFIX) {
            self.prototype_error(format!(
                "type `{}` in scope `{}` must be registered with its handler",
                def.name, def.base,
            ));
        } else if let Err(e) = self.reg.proto.register_polymorphic(def) {
            self.prototype_error(e.to_string());
        }
        self
    }

    /// Creates a field schema for polymorphic types in `Scope` backed by a world resource.
    pub fn polymorphic_resource_field<Scope: 'static>() -> FieldType {
        FieldType::Polymorphic(resource_polymorphic_base::<Scope>())
    }

    /// Registers a polymorphic type and its handler in a world resource.
    /// The handler is registered once its schema in `Scope` succeeds.
    pub fn polymorphic_type_with_resource<Scope: 'static, R: Send + Sync + 'static>(
        &mut self,
        name: &str,
        fields: Vec<FieldDef>,
        register: impl FnOnce(&mut R),
    ) -> &mut Self {
        let Some(resource) = self.reg.builder.resource_mut::<R>() else {
            self.prototype_error(format!(
                "no resource `{}` is registered for polymorphic type `{name}`",
                type_name::<R>(),
            ));
            return self;
        };
        let def = PolymorphicDef {
            base: resource_polymorphic_base::<Scope>(),
            name: name.to_owned(),
            fields,
        };
        match self.reg.proto.register_polymorphic(def) {
            Ok(()) => register(resource),
            Err(error) => self.prototype_error(error.to_string()),
        }
        self
    }

    /// Ignores a component, typically as declared in the manifest.
    pub fn ignore_component(&mut self, name: &str, category: IgnoreCategory) -> &mut Self {
        if let Err(e) = self.reg.proto.ignore_component(name, category) {
            self.prototype_error(e.to_string());
        }
        self
    }

    /// Registers a stable network event name. A duplicate reports both sources.
    pub fn network_event<E: 'static>(&mut self, name: &str) -> &mut Self {
        if let Err(e) = self.reg.net.register::<E>(name, &self.source) {
            self.reg.errors.push(e);
        }
        self
    }

    /// Registers a round rule.
    pub fn game_rule(&mut self, rule: impl GameRule + 'static) -> &mut Self {
        let id = rule.id().to_owned();
        if let Some((first_source, _)) = self.reg.rules.iter().find(|(_, r)| r.id() == id) {
            self.reg.errors.push(RegisterError::RuleConflict {
                id,
                first_source: first_source.clone(),
                second_source: self.source.clone(),
            });
            return self;
        }
        self.reg.rules.push((self.source.clone(), Box::new(rule)));
        self
    }

    /// Registers a console command.
    pub fn console_command(
        &mut self,
        name: &str,
        help: &str,
        handler: CommandHandler,
    ) -> &mut Self {
        if let Some(first) = self.reg.commands.commands.get(name) {
            self.reg.errors.push(RegisterError::CommandConflict {
                name: name.to_owned(),
                first_source: first.source.clone(),
                second_source: self.source.clone(),
            });
            return self;
        }
        self.reg.commands.commands.insert(
            name.to_owned(),
            ConsoleCommand {
                source: self.source.clone(),
                help: help.to_owned(),
                handler,
            },
        );
        self
    }

    fn prototype_error(&mut self, message: String) {
        self.reg.errors.push(RegisterError::Prototype {
            source: self.source.clone(),
            message,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_registration_cannot_define_resource_polymorphic_type() {
        struct Scope;

        let mut registration = Registration::new();
        let scoped_base = resource_polymorphic_base::<Scope>();
        let mut ctx = registration.context("test");
        ctx.polymorphic_type(PolymorphicDef {
            base: scoped_base.clone(),
            name: "Unpaired".to_owned(),
            fields: Vec::new(),
        });
        ctx.polymorphic_type(PolymorphicDef {
            base: "Ordinary".to_owned(),
            name: "Unpaired".to_owned(),
            fields: Vec::new(),
        });

        assert!(
            registration
                .proto
                .polymorphic(&scoped_base, "Unpaired")
                .is_none()
        );
        assert!(
            registration
                .proto
                .polymorphic("Ordinary", "Unpaired")
                .is_some()
        );
        assert!(matches!(
            registration.errors.as_slice(),
            [RegisterError::Prototype { source, message }]
                if source == "test" && message.contains("Unpaired")
        ));
    }
}
