//! Player profiles with characters, role preferences, and settings.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Role preference.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Serialize, Deserialize)]
pub enum RolePriority {
    /// Never assign this role.
    Never,
    /// Low priority.
    Low,
    /// Medium priority.
    Medium,
    /// High priority.
    High,
}

/// Profile character. The extension supplies opaque `data`.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct CharacterProfile {
    /// Character name.
    pub name: String,
    /// Character description in JSON.
    pub data: Value,
}

/// Player profile. Fields are stored as JSON. The default profile is empty.
#[derive(Clone, PartialEq, Debug, Default, Serialize, Deserialize)]
pub struct PlayerProfile {
    /// Characters.
    #[serde(default)]
    pub characters: Vec<CharacterProfile>,
    /// Role preferences by role identifier.
    #[serde(default)]
    pub role_preferences: BTreeMap<String, RolePriority>,
    /// Player settings as an arbitrary JSON object.
    #[serde(default)]
    pub settings: Map<String, Value>,
}
