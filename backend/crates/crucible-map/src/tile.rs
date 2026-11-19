//! Grid tiles and their definitions.
//!
//! Independent implementation with the same fields as Robust's `Tile`, so an adapter
//! can translate the values directly.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use crucible_proto::{FieldDef, FieldType, KindDef, PrototypeSet, Registry, RegistryError};

/// Grid tile with a definition ID, flags, variant, rotation, and reflection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Tile {
    /// Numeric definition ID ([`TileDefinitions`]). Zero represents the empty tile.
    pub type_id: i32,
    pub flags: u8,
    pub variant: u8,
    pub rotation_mirroring: u8,
}

impl Tile {
    /// Empty tile (ID 0).
    pub const EMPTY: Tile = Tile {
        type_id: 0,
        flags: 0,
        variant: 0,
        rotation_mirroring: 0,
    };

    /// Tile with definition `type_id` and default flags, variant, and rotation.
    pub const fn new(type_id: i32) -> Tile {
        Tile {
            type_id,
            flags: 0,
            variant: 0,
            rotation_mirroring: 0,
        }
    }

    /// Whether the tile is empty. See [`OverVoid`](crate::OverVoid) for the void event.
    pub const fn is_empty(&self) -> bool {
        self.type_id == 0
    }
}

/// Prototype kind for tile definitions.
pub const TILE_KIND: &str = "tile";

/// Prototype field identifying the empty tile (ID 0).
pub const EMPTY_FIELD: &str = "empty";

/// Registers the `tile` prototype kind with the core fields `name` (localization key),
/// `empty`, `isSubfloor`, and `baseTurf` (the underlying tile). Extensions can add
/// `extra_fields`.
pub fn register_tile_kind(
    registry: &mut Registry,
    extra_fields: Vec<FieldDef>,
) -> Result<(), RegistryError> {
    let mut fields = vec![
        FieldDef::new("name", FieldType::LocKey),
        FieldDef::new(EMPTY_FIELD, FieldType::Bool),
        FieldDef::new("isSubfloor", FieldType::Bool),
        FieldDef::new("baseTurf", FieldType::Prototype(TILE_KIND.to_owned())),
    ];
    fields.extend(extra_fields);
    registry.register_kind(KindDef::plain(TILE_KIND, fields))
}

/// Error building the tile definitions table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TileDefinitionsError {
    /// More than one prototype was marked as empty.
    SeveralEmpty(Vec<String>),
}

impl fmt::Display for TileDefinitionsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TileDefinitionsError::SeveralEmpty(ids) => write!(
                f,
                "multiple tile prototypes marked as empty (`{EMPTY_FIELD}: true`): {}",
                ids.join(", ")
            ),
        }
    }
}

impl std::error::Error for TileDefinitionsError {}

/// World resource mapping tile definitions to numeric IDs.
///
/// Assigns ID 0 to the empty tile, then assigns IDs from 1 in lexicographic
/// prototype ID order, independent of file load order. Numeric IDs are local to
/// the core. Adapters can map them to client IDs by prototype name
/// ([`TileDefinitions::prototype`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TileDefinitions {
    /// Indexed by numeric ID. Entry 0 is `None` when no prototype defines the empty tile.
    ids: Vec<Option<Arc<str>>>,
    by_prototype: HashMap<Arc<str>, i32>,
}

impl Default for TileDefinitions {
    /// Table consisting only of the empty tile, with no prototype assigned.
    fn default() -> Self {
        TileDefinitions::new(None, std::iter::empty::<&str>())
    }
}

impl TileDefinitions {
    /// Builds a table from prototype IDs, assigning 0 to the optional `empty` prototype
    /// and collapsing duplicate IDs.
    pub fn new<I, S>(empty: Option<&str>, prototypes: I) -> TileDefinitions
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut sorted: Vec<&str> = Vec::new();
        let owned: Vec<S> = prototypes.into_iter().collect();
        for s in &owned {
            let s = s.as_ref();
            if Some(s) != empty {
                sorted.push(s);
            }
        }
        sorted.sort_unstable();
        sorted.dedup();

        let mut defs = TileDefinitions {
            ids: Vec::with_capacity(sorted.len() + 1),
            by_prototype: HashMap::with_capacity(sorted.len() + 1),
        };
        let empty: Option<Arc<str>> = empty.map(Arc::from);
        if let Some(e) = &empty {
            defs.by_prototype.insert(e.clone(), 0);
        }
        defs.ids.push(empty);
        for id in sorted {
            let id: Arc<str> = Arc::from(id);
            let numeric = defs.ids.len() as i32;
            defs.by_prototype.insert(id.clone(), numeric);
            defs.ids.push(Some(id));
        }
        defs
    }

    /// Builds a table from prototypes of kind [`TILE_KIND`]. At most one prototype
    /// can define the empty tile with `empty: true`.
    pub fn from_prototypes(set: &PrototypeSet) -> Result<TileDefinitions, TileDefinitionsError> {
        let mut empty = Vec::new();
        let mut ids = Vec::new();
        for proto in set.iter_kind(TILE_KIND) {
            let is_empty = proto
                .field(EMPTY_FIELD)
                .and_then(|n| n.as_str())
                .and_then(crucible_proto::parse_bool)
                .unwrap_or(false);
            if is_empty {
                empty.push(proto.id.to_string());
            }
            ids.push(proto.id.clone());
        }
        if empty.len() > 1 {
            return Err(TileDefinitionsError::SeveralEmpty(empty));
        }
        Ok(TileDefinitions::new(
            empty.first().map(String::as_str),
            ids.iter().map(|s| &**s),
        ))
    }

    /// Numeric ID for a tile prototype.
    pub fn id(&self, prototype: &str) -> Option<i32> {
        self.by_prototype.get(prototype).copied()
    }

    /// Prototype ID for a numeric tile ID. Zero returns the empty tile's prototype
    /// when one was defined.
    pub fn prototype(&self, id: i32) -> Option<&str> {
        let index = usize::try_from(id).ok()?;
        self.ids.get(index)?.as_deref()
    }

    /// Numeric IDs and their prototypes in ascending ID order.
    pub fn iter(&self) -> impl Iterator<Item = (i32, &str)> {
        self.ids
            .iter()
            .enumerate()
            .filter_map(|(i, p)| p.as_deref().map(|p| (i as i32, p)))
    }

    /// Number of definitions, including the empty tile.
    pub fn len(&self) -> usize {
        self.ids.len()
    }

    /// Whether the empty tile is the only definition.
    pub fn is_empty(&self) -> bool {
        self.ids.len() <= 1
    }
}
