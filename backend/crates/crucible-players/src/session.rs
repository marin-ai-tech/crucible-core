//! Player session identifiers, statuses, and events.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use crucible_ecs::{Component, EntityUid};

/// Stable user identifier, equivalent to Robust's `NetUserId`.
///
/// The protocol adapter assigns it at login and keeps it stable across connections. It
/// identifies a user rather than the controlled entity, which has an [`EntityUid`].
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct NetUserId(Arc<str>);

impl NetUserId {
    /// Identifier from an adapter-provided string, such as a textual GUID.
    pub fn new(id: impl Into<Arc<str>>) -> Self {
        NetUserId(id.into())
    }

    /// String representation stored in the profile.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for NetUserId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Transport connection identifier assigned by an adapter. A session tracks its current connection.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct ConnectionId(pub u64);

/// Session status, corresponding to Robust's `SessionStatus` states used by the core.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum SessionStatus {
    /// Handshake in progress. Admission is pending.
    Connecting,
    /// Player admitted to the lobby.
    Connected,
    /// Player in a round.
    InGame,
    /// Connection closed or lost. Session data is retained for reconnection.
    Disconnected,
}

/// Reason a connection ended.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DisconnectReason {
    /// The connection was lost and the transport is already closed.
    ConnectionLost,
    /// The server or player closed the connection intentionally.
    Requested,
    /// The same user logged in through a new connection.
    Replaced,
}

/// Read-only view of a player session.
#[derive(Clone, Debug)]
pub struct PlayerSession {
    pub(crate) user: NetUserId,
    pub(crate) name: String,
    pub(crate) presentation_badge: Option<&'static str>,
    pub(crate) status: SessionStatus,
    pub(crate) connection: ConnectionId,
    pub(crate) control_generation: u64,
    pub(crate) entity: Option<EntityUid>,
    pub(crate) connected_at: Duration,
    pub(crate) disconnected_at: Option<Duration>,
}

impl PlayerSession {
    /// User identifier.
    pub fn user(&self) -> &NetUserId {
        &self.user
    }

    /// Display name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Name shown in the connection list and public chat. Character name and profile are separate.
    pub fn presentation_name(&self) -> std::borrow::Cow<'_, str> {
        match self.presentation_badge {
            Some(badge) => format!("{} [{badge}]", self.name).into(),
            None => self.name.as_str().into(),
        }
    }

    /// Display label assigned by the server.
    pub fn presentation_badge(&self) -> Option<&'static str> {
        self.presentation_badge
    }

    /// Current status.
    pub fn status(&self) -> SessionStatus {
        self.status
    }

    /// Current or most recent connection.
    pub fn connection(&self) -> ConnectionId {
        self.connection
    }

    /// Control generation, advanced on connection and changes of control.
    pub fn control_generation(&self) -> u64 {
        self.control_generation
    }

    pub(crate) fn advance_control_generation(&mut self) {
        self.control_generation = self
            .control_generation
            .checked_add(1)
            .expect("session control generation exhausted");
    }

    /// Entity the player is attached to. A disconnected player keeps the attachment so control
    /// can resume on reconnection.
    pub fn entity(&self) -> Option<EntityUid> {
        self.entity
    }

    /// Time of the last connection, according to the caller's clock.
    pub fn connected_at(&self) -> Duration {
        self.connected_at
    }

    /// Time of the last disconnect, or `None` while connected.
    pub fn disconnected_at(&self) -> Option<Duration> {
        self.disconnected_at
    }

    /// Whether the player currently controls an attached entity and is connected.
    pub fn is_controlling(&self) -> bool {
        self.entity.is_some() && self.status != SessionStatus::Disconnected
    }
}

/// Broadcast event raised when a session status changes.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SessionStatusChanged {
    /// User whose session changed.
    pub user: NetUserId,
    /// Previous status, or `None` for a newly created session.
    pub old: Option<SessionStatus>,
    /// New status.
    pub new: SessionStatus,
}

/// Broadcast request to the adapter to close a connection after a replacement login or
/// forced disconnect. [`DisconnectReason::ConnectionLost`] already closed the transport.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ConnectionDropRequested {
    /// User whose connection is closing.
    pub user: NetUserId,
    /// Connection to close.
    pub connection: ConnectionId,
    /// Reason the adapter passes to the client.
    pub reason: DisconnectReason,
}

/// Reason a player stopped controlling an entity.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DetachReason {
    /// Attached to a different entity.
    Rebound,
    /// Explicit detach.
    Requested,
    /// Player disconnected. The entity retains [`PlayerDisconnected`].
    Disconnected,
    /// Entity being removed.
    EntityDeleted,
}

/// Directed event when a player takes control of the target entity, like `PlayerAttachedEvent`.
/// Also raised when a disconnected player returns.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PlayerAttached {
    /// User who took control.
    pub user: NetUserId,
}

/// Directed event when a player stops controlling the target entity, like `PlayerDetachedEvent`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PlayerDetached {
    /// User who had control.
    pub user: NetUserId,
    /// Reason for detaching.
    pub reason: DetachReason,
}

/// Component for player-controlled entities. Subscribe to [`PlayerAttached`] and
/// [`PlayerDetached`] on it. [`PlayerManager`](crate::PlayerManager) adds and removes it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PlayerControlled {
    /// Player attached to the entity.
    pub user: NetUserId,
}
impl Component for PlayerControlled {}

/// Marks an uncontrolled entity after a player disconnects. It is removed when the player returns.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct PlayerDisconnected;
impl Component for PlayerDisconnected {}
