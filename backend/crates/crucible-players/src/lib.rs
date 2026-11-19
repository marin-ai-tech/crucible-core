//! Player sessions, entity control, reconnection, and profiles.
//!
//! Session handling is transport-independent. Protocol adapters call [`PlayerManager`],
//! subscribe to events, and supply the time as a parameter.
//!
//! | Capability | API |
//! |---|---|
//! | Sessions, statuses, and [`SessionStatusChanged`] | [`PlayerManager::begin_connect`], [`finish_connect`](PlayerManager::finish_connect), [`enter_game`](PlayerManager::enter_game) |
//! | Entity control and [`PlayerAttached`] or [`PlayerDetached`] | [`PlayerManager::attach`], [`detach`](PlayerManager::detach), [`PlayerControlled`] |
//! | Disconnect, reconnect, and replacement login | [`PlayerManager::disconnect`], [`PlayerDisconnected`], [`ConnectionDropRequested`] |
//! | SQLite profiles and migrations | [`persist`], [`ProfileStore`], [`SqliteProfileStore`] |
//!
//! Add [`PlayersSystem`] when building a world. It installs the [`Players`] resource and detaches
//! players from entities being removed.

pub mod manager;
pub mod persist;
pub mod profile;
pub mod session;

pub use manager::{ConnectOutcome, PlayerError, PlayerManager, Players, PlayersSystem};
pub use persist::{PersistError, ProfileStore, SqliteProfileStore};
pub use profile::{CharacterProfile, PlayerProfile, RolePriority};
pub use session::{
    ConnectionDropRequested, ConnectionId, DetachReason, DisconnectReason, NetUserId,
    PlayerAttached, PlayerControlled, PlayerDetached, PlayerDisconnected, PlayerSession,
    SessionStatus, SessionStatusChanged,
};
