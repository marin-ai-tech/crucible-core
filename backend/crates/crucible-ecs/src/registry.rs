//! Component registry for SS14 YAML names, field schemas, network policies, and flags.
//! The registry corresponds to Robust's `ComponentFactory`.

use std::any::TypeId;
use std::collections::HashMap;
use std::fmt;

use crate::change::NetworkedComponent;
use crate::component::{Component, ErasedStorage, new_storage};

/// Who receives replicas of a component.
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash, Debug)]
pub enum NetworkPolicy {
    /// Server only. Player states never include this data.
    #[default]
    ServerOnly,
    /// Only the player controlling the entity.
    OwnerOnly,
    /// Everyone who can see the entity.
    AllViewers,
}

/// Field description in a component schema.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FieldSchema {
    /// YAML field name.
    pub name: String,
    /// Value type name for diagnostics and schema export.
    pub type_name: String,
}

/// All registry metadata for a component known by name.
#[derive(Clone, Debug)]
pub struct ComponentRegistration {
    name: String,
    source: String,
    type_id: TypeId,
    type_name: &'static str,
    fields: Vec<FieldSchema>,
    network: NetworkPolicy,
    persisted: bool,
    debug: bool,
    pub(crate) make_storage: fn() -> Box<dyn ErasedStorage>,
}

impl ComponentRegistration {
    /// Registers type `C` under `name`, as it appears in SS14 YAML (for example, `"Door"`).
    /// `source` identifies the registering core module or extension, such as `"core:doors"` or `"demo-source"`.
    ///
    /// Defaults to an empty schema, server-only policy, and no persistence or debugging flags.
    pub fn new<C: Component>(name: impl Into<String>, source: impl Into<String>) -> Self {
        ComponentRegistration {
            name: name.into(),
            source: source.into(),
            type_id: TypeId::of::<C>(),
            type_name: std::any::type_name::<C>(),
            fields: Vec::new(),
            network: NetworkPolicy::ServerOnly,
            persisted: false,
            debug: false,
            make_storage: new_storage::<C>,
        }
    }

    /// Adds a field to the schema.
    pub fn field(mut self, name: impl Into<String>, type_name: impl Into<String>) -> Self {
        self.fields.push(FieldSchema {
            name: name.into(),
            type_name: type_name.into(),
        });
        self
    }

    /// Adds names of fields replicated by [`networked`](crate::networked) to the schema,
    /// leaving their types unspecified.
    pub fn networked_fields<C: NetworkedComponent>(mut self) -> Self {
        for name in C::FIELD_NAMES {
            self.fields.push(FieldSchema {
                name: (*name).to_owned(),
                type_name: String::new(),
            });
        }
        self
    }

    /// Network policy.
    pub fn network(mut self, policy: NetworkPolicy) -> Self {
        self.network = policy;
        self
    }

    /// Persistence flag. A profile or campaign saves the component.
    pub fn persisted(mut self, persisted: bool) -> Self {
        self.persisted = persisted;
        self
    }

    /// Debugging flag. The component is visible in debug tools.
    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }

    /// YAML component name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Registration source.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// `TypeId` of the component.
    pub fn type_id(&self) -> TypeId {
        self.type_id
    }

    /// Rust component type name.
    pub fn type_name(&self) -> &'static str {
        self.type_name
    }

    /// Field schema.
    pub fn fields(&self) -> &[FieldSchema] {
        &self.fields
    }

    /// Network policy.
    pub fn network_policy(&self) -> NetworkPolicy {
        self.network
    }

    /// Persistence flag.
    pub fn is_persisted(&self) -> bool {
        self.persisted
    }

    /// Debugging flag.
    pub fn is_debug(&self) -> bool {
        self.debug
    }
}

/// Error registering a component.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum RegistryError {
    /// The name is already in use, possibly by another type.
    DuplicateName {
        /// Component name.
        name: String,
        /// Source of the first registration.
        first_source: String,
        /// Source of the duplicate registration.
        second_source: String,
    },
    /// The type is already registered under a different name.
    DuplicateType {
        /// Rust type name.
        type_name: &'static str,
        /// Name in the first registration.
        first_name: String,
        /// Source of the first registration.
        first_source: String,
        /// Source of the duplicate registration.
        second_source: String,
    },
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RegistryError::DuplicateName {
                name,
                first_source,
                second_source,
            } => write!(
                f,
                "component '{name}' is registered twice by '{first_source}' and '{second_source}'"
            ),
            RegistryError::DuplicateType {
                type_name,
                first_name,
                first_source,
                second_source,
            } => write!(
                f,
                "type {type_name} is already registered as '{first_name}' by '{first_source}' \
                  and was registered again by '{second_source}'"
            ),
        }
    }
}

impl std::error::Error for RegistryError {}

/// Registry of world components, populated through [`WorldBuilder::register_component`](crate::WorldBuilder::register_component).
#[derive(Default)]
pub struct ComponentRegistry {
    entries: Vec<ComponentRegistration>,
    by_name: HashMap<String, usize>,
    by_type: HashMap<TypeId, usize>,
}

impl ComponentRegistry {
    pub(crate) fn register(
        &mut self,
        registration: ComponentRegistration,
    ) -> Result<(), RegistryError> {
        if let Some(&i) = self.by_name.get(&registration.name) {
            return Err(RegistryError::DuplicateName {
                name: registration.name,
                first_source: self.entries[i].source.clone(),
                second_source: registration.source,
            });
        }
        if let Some(&i) = self.by_type.get(&registration.type_id) {
            return Err(RegistryError::DuplicateType {
                type_name: registration.type_name,
                first_name: self.entries[i].name.clone(),
                first_source: self.entries[i].source.clone(),
                second_source: registration.source,
            });
        }
        let index = self.entries.len();
        self.by_name.insert(registration.name.clone(), index);
        self.by_type.insert(registration.type_id, index);
        self.entries.push(registration);
        Ok(())
    }

    /// Looks up a registration by component name.
    pub fn by_name(&self, name: &str) -> Option<&ComponentRegistration> {
        self.by_name.get(name).map(|&i| &self.entries[i])
    }

    /// Looks up a registration by `TypeId`.
    pub fn by_type_id(&self, type_id: TypeId) -> Option<&ComponentRegistration> {
        self.by_type.get(&type_id).map(|&i| &self.entries[i])
    }

    /// Looks up the registration for type `C`.
    pub fn by_type<C: Component>(&self) -> Option<&ComponentRegistration> {
        self.by_type_id(TypeId::of::<C>())
    }

    /// All entries in registration order.
    pub fn iter(&self) -> impl Iterator<Item = &ComponentRegistration> {
        self.entries.iter()
    }

    /// Number of registrations.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
