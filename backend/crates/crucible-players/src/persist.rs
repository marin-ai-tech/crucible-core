//! Profile persistence with the [`ProfileStore`] abstraction, SQLite,
//! and schema migrations. A PostgreSQL backend could use the same trait.

use std::fmt;
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};

use crate::profile::PlayerProfile;
use crate::session::NetUserId;

/// Schema migration versions equal their index plus one. Preserve applied migrations
/// and append new migrations to the end of the list.
const MIGRATIONS: &[&str] = &["CREATE TABLE profiles (
        user_id          TEXT PRIMARY KEY NOT NULL,
        characters       TEXT NOT NULL,
        role_preferences TEXT NOT NULL,
        settings         TEXT NOT NULL
    )"];

/// Storage errors.
#[derive(Debug)]
pub enum PersistError {
    /// SQLite error.
    Sqlite(rusqlite::Error),
    /// Error parsing or writing a JSON field.
    Json(serde_json::Error),
    /// A newer version of the server created the database.
    SchemaTooNew {
        /// Version in the database.
        found: u32,
        /// Latest supported version.
        supported: u32,
    },
}

impl fmt::Display for PersistError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PersistError::Sqlite(e) => write!(f, "SQLite error: {e}"),
            PersistError::Json(e) => write!(f, "profile JSON error: {e}"),
            PersistError::SchemaTooNew { found, supported } => write!(
                f,
                "database schema version {found} is newer than the supported version ({supported})"
            ),
        }
    }
}

impl std::error::Error for PersistError {}

impl From<rusqlite::Error> for PersistError {
    fn from(e: rusqlite::Error) -> Self {
        PersistError::Sqlite(e)
    }
}

impl From<serde_json::Error> for PersistError {
    fn from(e: serde_json::Error) -> Self {
        PersistError::Json(e)
    }
}

/// Persistent profile storage with [`SqliteProfileStore`] as the SQLite implementation.
pub trait ProfileStore: Send {
    /// User profile, or `None` if it has not been created.
    fn load(&self, user: &NetUserId) -> Result<Option<PlayerProfile>, PersistError>;

    /// Creates or replaces a profile.
    fn save(&self, user: &NetUserId, profile: &PlayerProfile) -> Result<(), PersistError>;

    /// Loads a user's profile, creating and saving a default for a new player.
    /// The second value is `true` when this call created the profile.
    fn load_or_create(&self, user: &NetUserId) -> Result<(PlayerProfile, bool), PersistError> {
        match self.load(user)? {
            Some(profile) => Ok((profile, false)),
            None => {
                let profile = PlayerProfile::default();
                self.save(user, &profile)?;
                Ok((profile, true))
            }
        }
    }
}

/// Applies pending migrations and returns the current schema version. Repeated calls without
/// new migrations do nothing. The `schema_version` table tracks versions, and each migration has its own transaction.
pub fn migrate(conn: &mut Connection) -> Result<u32, PersistError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version (version INTEGER PRIMARY KEY NOT NULL)",
    )?;
    let current: u32 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_version",
        [],
        |row| row.get(0),
    )?;
    let supported = MIGRATIONS.len() as u32;
    if current > supported {
        return Err(PersistError::SchemaTooNew {
            found: current,
            supported,
        });
    }
    for (index, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.execute(
            "INSERT INTO schema_version (version) VALUES (?1)",
            params![index as u32 + 1],
        )?;
        tx.commit()?;
    }
    Ok(supported)
}

/// SQLite profile storage. JSON fields are stored as text.
pub struct SqliteProfileStore {
    conn: Connection,
}

impl SqliteProfileStore {
    /// Opens or creates the database file and applies migrations.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, PersistError> {
        Self::from_connection(Connection::open(path)?)
    }

    /// In-memory database for tests. Its contents are discarded on restart.
    pub fn open_in_memory() -> Result<Self, PersistError> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(mut conn: Connection) -> Result<Self, PersistError> {
        migrate(&mut conn)?;
        Ok(SqliteProfileStore { conn })
    }

    /// Database schema version.
    pub fn schema_version(&self) -> Result<u32, PersistError> {
        Ok(self.conn.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |row| row.get(0),
        )?)
    }
}

impl ProfileStore for SqliteProfileStore {
    fn load(&self, user: &NetUserId) -> Result<Option<PlayerProfile>, PersistError> {
        let row: Option<(String, String, String)> = self
            .conn
            .query_row(
                "SELECT characters, role_preferences, settings FROM profiles WHERE user_id = ?1",
                params![user.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        row.map(|(characters, role_preferences, settings)| {
            Ok(PlayerProfile {
                characters: serde_json::from_str(&characters)?,
                role_preferences: serde_json::from_str(&role_preferences)?,
                settings: serde_json::from_str(&settings)?,
            })
        })
        .transpose()
    }

    fn save(&self, user: &NetUserId, profile: &PlayerProfile) -> Result<(), PersistError> {
        self.conn.execute(
            "INSERT INTO profiles (user_id, characters, role_preferences, settings)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(user_id) DO UPDATE SET
                characters = excluded.characters,
                role_preferences = excluded.role_preferences,
                settings = excluded.settings",
            params![
                user.as_str(),
                serde_json::to_string(&profile.characters)?,
                serde_json::to_string(&profile.role_preferences)?,
                serde_json::to_string(&profile.settings)?,
            ],
        )?;
        Ok(())
    }
}
