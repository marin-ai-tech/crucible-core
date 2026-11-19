//! Read-only prototypes after inheritance and merging.
//!
//! Each resolved prototype exposes component names and node trees with final field values.
//! Entity creation can build on these values without making
//! `crucible-proto` depend on the ECS.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use crate::node::{Node, Pos};

/// A component of a resolved prototype.
#[derive(Clone, Debug, PartialEq)]
pub struct Component {
    /// Name from the component record's `type` field.
    pub name: Box<str>,
    /// Position of the `type` value in the last file to define the component.
    pub pos: Pos,
    /// Mapping of resolved field values, excluding the `type` field.
    pub data: Node,
}

/// A prototype with its own and inherited components and fields merged.
#[derive(Clone, Debug, PartialEq)]
pub struct Prototype {
    pub kind: Arc<str>,
    pub id: Arc<str>,
    pub is_abstract: bool,
    pub file: Arc<str>,
    pub pos: Pos,
    /// Parent IDs in declaration order.
    pub parents: Vec<String>,
    /// Other record fields, such as `name` and `description`, with resolved values.
    pub fields: Vec<(Box<str>, Node)>,
    /// Inherited components followed by the child's new components.
    /// Ignored components are excluded.
    pub components: Vec<Component>,
}

impl Prototype {
    pub fn component(&self, name: &str) -> Option<&Component> {
        self.components.iter().find(|c| &*c.name == name)
    }

    pub fn field(&self, name: &str) -> Option<&Node> {
        self.fields
            .iter()
            .find(|(k, _)| &**k == name)
            .map(|(_, v)| v)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstantiateError {
    NotFound { kind: String, id: String },
    Abstract { kind: String, id: String },
}

impl fmt::Display for InstantiateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InstantiateError::NotFound { kind, id } => {
                write!(f, "prototype {kind} `{id}` was not found")
            }
            InstantiateError::Abstract { kind, id } => {
                write!(
                    f,
                    "prototype {kind} `{id}` is abstract and cannot be instantiated"
                )
            }
        }
    }
}

impl std::error::Error for InstantiateError {}

/// Immutable store of resolved prototypes. Cheap to clone because the prototypes are in an `Arc`.
#[derive(Clone, Debug, Default)]
pub struct PrototypeSet {
    pub(crate) by_kind: BTreeMap<Arc<str>, BTreeMap<Arc<str>, Arc<Prototype>>>,
}

impl PrototypeSet {
    pub fn get(&self, kind: &str, id: &str) -> Option<&Arc<Prototype>> {
        self.by_kind.get(kind)?.get(id)
    }

    /// Iterates over prototypes of the given kind in ID order.
    pub fn iter_kind<'a>(&'a self, kind: &str) -> impl Iterator<Item = &'a Arc<Prototype>> + 'a {
        self.by_kind.get(kind).into_iter().flat_map(|m| m.values())
    }

    pub fn len(&self) -> usize {
        self.by_kind.values().map(BTreeMap::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns an existing, concrete prototype suitable for entity creation.
    pub fn instantiable(&self, kind: &str, id: &str) -> Result<&Arc<Prototype>, InstantiateError> {
        match self.get(kind, id) {
            None => Err(InstantiateError::NotFound {
                kind: kind.to_owned(),
                id: id.to_owned(),
            }),
            Some(p) if p.is_abstract => Err(InstantiateError::Abstract {
                kind: kind.to_owned(),
                id: id.to_owned(),
            }),
            Some(p) => Ok(p),
        }
    }
}
