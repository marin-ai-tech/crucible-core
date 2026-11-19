//! Manages player sessions, entity control, disconnects, and reconnects.
//!
//! [`Players`] stores session state as a world resource. [`PlayerManager`] operations take
//! `&mut World`, so an entity-removal handler can detach a player using the world alone.
//! Event delivery never retains a borrow of the resource.

use std::collections::HashMap;
use std::fmt;
use std::time::Duration;

use crucible_ecs::{EntityTerminating, EntityUid, System, SystemInit, World};

use crate::session::{
    ConnectionDropRequested, ConnectionId, DetachReason, DisconnectReason, NetUserId,
    PlayerAttached, PlayerControlled, PlayerDetached, PlayerDisconnected, PlayerSession,
    SessionStatus, SessionStatusChanged,
};

/// State of all sessions, installed as a world resource by [`PlayersSystem`].
#[derive(Default)]
pub struct Players {
    sessions: HashMap<NetUserId, PlayerSession>,
    by_entity: HashMap<EntityUid, NetUserId>,
}

impl Players {
    /// User session, including disconnected sessions retained for reconnection.
    pub fn session(&self, user: &NetUserId) -> Option<&PlayerSession> {
        self.sessions.get(user)
    }

    /// User attached to an entity.
    pub fn user_of(&self, entity: EntityUid) -> Option<&NetUserId> {
        self.by_entity.get(&entity)
    }

    /// All sessions, in unspecified order.
    pub fn sessions(&self) -> impl Iterator<Item = &PlayerSession> {
        self.sessions.values()
    }
}

/// Errors operating on sessions.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum PlayerError {
    /// No session exists for the user.
    UnknownUser(NetUserId),
    /// The current session status does not allow the operation.
    WrongStatus {
        /// User whose session was used.
        user: NetUserId,
        /// Session status.
        status: SessionStatus,
    },
    /// The entity is absent or being removed.
    EntityNotFound(EntityUid),
    /// The entity is already attached to another session.
    EntityOccupied {
        /// Entity.
        entity: EntityUid,
        /// User who controls or has reserved the entity.
        by: NetUserId,
    },
}

impl fmt::Display for PlayerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlayerError::UnknownUser(u) => write!(f, "no session for user {u}"),
            PlayerError::WrongStatus { user, status } => {
                write!(
                    f,
                    "operation unavailable for session {user} in status {status:?}"
                )
            }
            PlayerError::EntityNotFound(e) => write!(f, "entity {e} is absent or being removed"),
            PlayerError::EntityOccupied { entity, by } => {
                write!(f, "entity {entity} is already attached to {by}")
            }
        }
    }
}

impl std::error::Error for PlayerError {}

/// Outcome of [`PlayerManager::begin_connect`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ConnectOutcome {
    /// Connection replaced by a new login. The adapter received a
    /// [`ConnectionDropRequested`] for it.
    pub replaced: Option<ConnectionId>,
}

/// Installs the [`Players`] resource and detaches players when their entities are removed.
/// Add this system once to the `WorldBuilder`.
pub struct PlayersSystem;

impl System for PlayersSystem {
    fn initialize(&mut self, init: &mut SystemInit<'_>) {
        init.insert_resource(Players::default());
        init.subscribe::<PlayerControlled, EntityTerminating, _>(|world, uid, _| {
            if let Some(user) = players(world).by_entity.get(&uid).cloned() {
                unbind(world, &user, uid, DetachReason::EntityDeleted);
            }
        });
    }
}

/// Operations on sessions. The caller supplies `now`, for example from simulation time,
/// allowing tests to advance without sleeping.
pub struct PlayerManager;

impl PlayerManager {
    /// Starts a connection in the connecting state.
    ///
    /// An existing active session is disconnected with [`DisconnectReason::Replaced`] and the
    /// adapter receives [`ConnectionDropRequested`]. The new login proceeds as a reconnection,
    /// so only one session remains active.
    ///
    /// # Panics
    /// Panics if the world does not contain [`PlayersSystem`].
    pub fn begin_connect(
        world: &mut World,
        user: &NetUserId,
        name: &str,
        connection: ConnectionId,
        now: Duration,
    ) -> ConnectOutcome {
        let active = players(world)
            .sessions
            .get(user)
            .filter(|s| s.status != SessionStatus::Disconnected)
            .map(|s| s.connection);
        if active.is_some() {
            disconnect_now(world, user, DisconnectReason::Replaced, now);
        }
        let old = {
            let state = players(world);
            match state.sessions.get_mut(user) {
                Some(s) => {
                    let old = s.status;
                    s.name = name.to_owned();
                    s.presentation_badge = None;
                    s.status = SessionStatus::Connecting;
                    s.connection = connection;
                    s.advance_control_generation();
                    s.connected_at = now;
                    s.disconnected_at = None;
                    Some(old)
                }
                None => {
                    state.sessions.insert(
                        user.clone(),
                        PlayerSession {
                            user: user.clone(),
                            name: name.to_owned(),
                            presentation_badge: None,
                            status: SessionStatus::Connecting,
                            connection,
                            control_generation: 1,
                            entity: None,
                            connected_at: now,
                            disconnected_at: None,
                        },
                    );
                    None
                }
            }
        };
        announce(world, user, old, SessionStatus::Connecting);
        ConnectOutcome { replaced: active }
    }

    /// Sets a trusted display label on an existing session.
    pub fn set_presentation_badge(
        world: &mut World,
        user: &NetUserId,
        badge: Option<&'static str>,
    ) {
        if let Some(session) = players(world).sessions.get_mut(user) {
            session.presentation_badge = badge;
        }
    }

    /// Completes the transition from connecting to connected. If a returning user still has
    /// an entity, this restores control, removes [`PlayerDisconnected`], raises [`PlayerAttached`]
    /// on the entity, and switches the session to in-game.
    pub fn finish_connect(world: &mut World, user: &NetUserId) -> Result<(), PlayerError> {
        require_status(world, user, &[SessionStatus::Connecting])?;
        set_status(world, user, SessionStatus::Connected);
        let retained = players(world).sessions.get(user).and_then(|s| s.entity);
        if let Some(entity) = retained {
            world.remove_component::<PlayerDisconnected>(entity);
            world.raise(entity, &mut PlayerAttached { user: user.clone() });
            set_status(world, user, SessionStatus::InGame);
        }
        Ok(())
    }

    /// Moves the player from connected to in-game.
    pub fn enter_game(world: &mut World, user: &NetUserId) -> Result<(), PlayerError> {
        require_status(world, user, &[SessionStatus::Connected])?;
        set_status(world, user, SessionStatus::InGame);
        Ok(())
    }

    /// Returns an in-game player to connected status in the lobby, for example after round cleanup.
    /// Detaches any remaining entity with [`DetachReason::Requested`].
    pub fn leave_game(world: &mut World, user: &NetUserId) -> Result<(), PlayerError> {
        require_status(world, user, &[SessionStatus::InGame])?;
        let entity = players(world).sessions.get(user).and_then(|s| s.entity);
        if let Some(entity) = entity {
            unbind(world, user, entity, DetachReason::Requested);
        }
        set_status(world, user, SessionStatus::Connected);
        Ok(())
    }

    /// Disconnects `connection`. Returns `false` for an already disconnected session or a stale
    /// connection that has been replaced. The attached entity stays in the world with
    /// [`PlayerDisconnected`] and receives [`PlayerDetached`].
    pub fn disconnect(
        world: &mut World,
        user: &NetUserId,
        connection: ConnectionId,
        reason: DisconnectReason,
        now: Duration,
    ) -> bool {
        let current = players(world)
            .sessions
            .get(user)
            .is_some_and(|s| s.connection == connection && s.status != SessionStatus::Disconnected);
        if current {
            disconnect_now(world, user, reason, now);
        }
        current
    }

    /// Attaches a player to an entity and releases the prior attachment with [`DetachReason::Rebound`].
    /// Requires a connected or in-game session. Reattaching to the same entity has no effect.
    pub fn attach(
        world: &mut World,
        user: &NetUserId,
        entity: EntityUid,
    ) -> Result<(), PlayerError> {
        let previous = {
            let state = players(world);
            let session = state
                .sessions
                .get(user)
                .ok_or_else(|| PlayerError::UnknownUser(user.clone()))?;
            if !matches!(
                session.status,
                SessionStatus::Connected | SessionStatus::InGame
            ) {
                return Err(PlayerError::WrongStatus {
                    user: user.clone(),
                    status: session.status,
                });
            }
            if session.entity == Some(entity) {
                return Ok(());
            }
            if let Some(by) = state.by_entity.get(&entity) {
                return Err(PlayerError::EntityOccupied {
                    entity,
                    by: by.clone(),
                });
            }
            session.entity
        };
        world
            .add_component(entity, PlayerControlled { user: user.clone() })
            .map_err(|_| PlayerError::EntityNotFound(entity))?;
        if let Some(previous) = previous {
            unbind(world, user, previous, DetachReason::Rebound);
        }
        let state = players(world);
        state.by_entity.insert(entity, user.clone());
        if let Some(s) = state.sessions.get_mut(user) {
            s.entity = Some(entity);
            s.advance_control_generation();
        }
        world.raise(entity, &mut PlayerAttached { user: user.clone() });
        Ok(())
    }

    /// Detaches by request with [`DetachReason::Requested`]. Returns `false` if there was no attachment.
    pub fn detach(world: &mut World, user: &NetUserId) -> bool {
        let entity = players(world).sessions.get(user).and_then(|s| s.entity);
        match entity {
            Some(entity) => {
                unbind(world, user, entity, DetachReason::Requested);
                true
            }
            None => false,
        }
    }
}

fn players(world: &mut World) -> &mut Players {
    world
        .resource_mut::<Players>()
        .expect("PlayersSystem is not installed in the world")
}

fn require_status(
    world: &mut World,
    user: &NetUserId,
    allowed: &[SessionStatus],
) -> Result<(), PlayerError> {
    let session = players(world)
        .sessions
        .get(user)
        .ok_or_else(|| PlayerError::UnknownUser(user.clone()))?;
    if allowed.contains(&session.status) {
        Ok(())
    } else {
        Err(PlayerError::WrongStatus {
            user: user.clone(),
            status: session.status,
        })
    }
}

fn announce(world: &mut World, user: &NetUserId, old: Option<SessionStatus>, new: SessionStatus) {
    world.raise_broadcast(&mut SessionStatusChanged {
        user: user.clone(),
        old,
        new,
    });
}

fn set_status(world: &mut World, user: &NetUserId, new: SessionStatus) {
    let old = players(world).sessions.get_mut(user).map(|s| {
        let old = s.status;
        s.status = new;
        old
    });
    if old.is_some() {
        announce(world, user, old, new);
    }
}

/// Updates state, raises the detach event, then removes the component.
/// During entity deletion, the component is removed with its entity.
fn unbind(world: &mut World, user: &NetUserId, entity: EntityUid, reason: DetachReason) {
    let state = players(world);
    state.by_entity.remove(&entity);
    if let Some(s) = state.sessions.get_mut(user) {
        s.entity = None;
        s.advance_control_generation();
    }
    world.raise(
        entity,
        &mut PlayerDetached {
            user: user.clone(),
            reason,
        },
    );
    if reason != DetachReason::EntityDeleted {
        world.remove_component::<PlayerControlled>(entity);
        world.remove_component::<PlayerDisconnected>(entity);
    }
}

/// Disconnects an active session after the caller has checked the connection.
fn disconnect_now(world: &mut World, user: &NetUserId, reason: DisconnectReason, now: Duration) {
    let Some((connection, entity)) = players(world)
        .sessions
        .get(user)
        .map(|s| (s.connection, s.entity))
    else {
        return;
    };
    if let Some(session) = players(world).sessions.get_mut(user) {
        session.advance_control_generation();
    }
    if reason != DisconnectReason::ConnectionLost {
        world.raise_broadcast(&mut ConnectionDropRequested {
            user: user.clone(),
            connection,
            reason,
        });
    }
    if let Some(entity) = entity {
        world.raise(
            entity,
            &mut PlayerDetached {
                user: user.clone(),
                reason: DetachReason::Disconnected,
            },
        );
        // The handler may have started removing the entity. In that case it needs no disconnected marker.
        let _ = world.add_component(entity, PlayerDisconnected);
    }
    if let Some(s) = players(world).sessions.get_mut(user) {
        s.disconnected_at = Some(now);
    }
    set_status(world, user, SessionStatus::Disconnected);
}
