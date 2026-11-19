//! Registries for prototype kinds, components, polymorphic `!type:` values, and ignored components.
//!
//! The core, modules, and extensions populate registries before loading. Each field
//! schema defines the value type used to validate its node tree.

use std::collections::HashMap;
use std::fmt;

/// Type of a field value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FieldType {
    /// The original string, preserving text such as `1.10` and `yes`.
    String,
    Bool,
    Int,
    Float,
    /// A Fluent key. A missing key in the default language generates a warning.
    LocKey,
    /// Reference to a prototype of the specified `kind`. Missing references are errors.
    Prototype(String),
    List(Box<FieldType>),
    /// A mapping with string keys and values of a single type.
    Map(Box<FieldType>),
    /// A nested struct with known fields.
    Struct(Vec<FieldDef>),
    /// A polymorphic `!type:Name` value, where `Name` is registered for `base`.
    Polymorphic(String),
    /// Parses the value without type checking it.
    Any,
}

impl fmt::Display for FieldType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FieldType::String => f.write_str("string"),
            FieldType::Bool => f.write_str("boolean (true/false)"),
            FieldType::Int => f.write_str("integer"),
            FieldType::Float => f.write_str("number"),
            FieldType::LocKey => f.write_str("localization key"),
            FieldType::Prototype(k) => write!(f, "prototype identifier of kind `{k}`"),
            FieldType::List(t) => write!(f, "list ({t})"),
            FieldType::Map(t) => write!(f, "mapping ({t})"),
            FieldType::Struct(_) => f.write_str("struct"),
            FieldType::Polymorphic(b) => write!(f, "`!type:` value for base `{b}`"),
            FieldType::Any => f.write_str("any value"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldDef {
    pub name: String,
    pub ty: FieldType,
}

impl FieldDef {
    pub fn new(name: &str, ty: FieldType) -> Self {
        FieldDef {
            name: name.to_owned(),
            ty,
        }
    }
}

/// A prototype kind, such as `type: entity` or `type: tile`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KindDef {
    pub name: String,
    /// Record fields excluding `type`, `id`, `parent`, `abstract`, and `components`.
    pub fields: Vec<FieldDef>,
    /// The record supports a `components` list.
    pub has_components: bool,
    /// The record supports the `parent` and `abstract` fields.
    pub inheritable: bool,
}

impl KindDef {
    /// A kind without inheritance or components.
    pub fn plain(name: &str, fields: Vec<FieldDef>) -> Self {
        KindDef {
            name: name.to_owned(),
            fields,
            has_components: false,
            inheritable: false,
        }
    }

    /// A kind with inheritance and components, like SS14's `entity`.
    pub fn with_components(name: &str, fields: Vec<FieldDef>) -> Self {
        KindDef {
            name: name.to_owned(),
            fields,
            has_components: true,
            inheritable: true,
        }
    }
}

/// A component with its `type` name and field definitions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComponentDef {
    pub name: String,
    pub fields: Vec<FieldDef>,
}

/// A polymorphic `!type:name` that is valid where `base` is expected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolymorphicDef {
    pub base: String,
    pub name: String,
    pub fields: Vec<FieldDef>,
}

/// Category of an ignored component.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IgnoreCategory {
    /// A visual or other client-only component that is skipped silently.
    ClientOnly,
    /// An unimplemented backend component that contributes to an aggregate warning.
    NotImplemented,
}

#[derive(Debug, PartialEq, Eq)]
pub enum RegistryError {
    DuplicateKind(String),
    DuplicateComponent(String),
    DuplicatePolymorphic {
        base: String,
        name: String,
    },
    /// The name belongs to a registered component or a different ignore category.
    ConflictingIgnore(String),
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RegistryError::DuplicateKind(k) => {
                write!(f, "prototype kind `{k}` is already registered")
            }
            RegistryError::DuplicateComponent(c) => {
                write!(f, "component `{c}` is already registered")
            }
            RegistryError::DuplicatePolymorphic { base, name } => {
                write!(f, "type `{name}` is already registered for `{base}`")
            }
            RegistryError::ConflictingIgnore(c) => {
                write!(
                    f,
                    "component `{c}` is registered or belongs to a different ignore category"
                )
            }
        }
    }
}

impl std::error::Error for RegistryError {}

/// Registries used to validate loaded prototypes.
#[derive(Clone, Debug, Default)]
pub struct Registry {
    kinds: HashMap<String, KindDef>,
    components: HashMap<String, ComponentDef>,
    polymorphic: HashMap<(String, String), PolymorphicDef>,
    ignored: HashMap<String, IgnoreCategory>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_kind(&mut self, def: KindDef) -> Result<(), RegistryError> {
        if self.kinds.contains_key(&def.name) {
            return Err(RegistryError::DuplicateKind(def.name));
        }
        self.kinds.insert(def.name.clone(), def);
        Ok(())
    }

    pub fn register_component(&mut self, def: ComponentDef) -> Result<(), RegistryError> {
        if self.components.contains_key(&def.name) || self.ignored.contains_key(&def.name) {
            return Err(RegistryError::DuplicateComponent(def.name));
        }
        self.components.insert(def.name.clone(), def);
        Ok(())
    }

    pub fn register_polymorphic(&mut self, def: PolymorphicDef) -> Result<(), RegistryError> {
        let key = (def.base.clone(), def.name.clone());
        if self.polymorphic.contains_key(&key) {
            return Err(RegistryError::DuplicatePolymorphic {
                base: def.base,
                name: def.name,
            });
        }
        self.polymorphic.insert(key, def);
        Ok(())
    }

    /// Ignores a component. Multiple extensions can ignore it under the same category.
    ///
    pub fn ignore_component(
        &mut self,
        name: &str,
        category: IgnoreCategory,
    ) -> Result<(), RegistryError> {
        if self.components.contains_key(name) {
            return Err(RegistryError::ConflictingIgnore(name.to_owned()));
        }
        match self.ignored.get(name) {
            Some(c) if *c != category => Err(RegistryError::ConflictingIgnore(name.to_owned())),
            _ => {
                self.ignored.insert(name.to_owned(), category);
                Ok(())
            }
        }
    }

    pub fn kind(&self, name: &str) -> Option<&KindDef> {
        self.kinds.get(name)
    }

    pub fn component(&self, name: &str) -> Option<&ComponentDef> {
        self.components.get(name)
    }

    pub fn polymorphic(&self, base: &str, name: &str) -> Option<&PolymorphicDef> {
        self.polymorphic.get(&(base.to_owned(), name.to_owned()))
    }

    pub fn ignore_category(&self, component: &str) -> Option<IgnoreCategory> {
        self.ignored.get(component).copied()
    }
}
