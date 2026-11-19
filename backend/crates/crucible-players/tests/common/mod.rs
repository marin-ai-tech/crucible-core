//! Shared test world with the player system and an event log.
#![allow(dead_code)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use crucible_ecs::{EntityUid, System, SystemInit, World};
use crucible_players::{
    ConnectionDropRequested, ConnectionId, DetachReason, DisconnectReason, NetUserId,
    PlayerAttached, PlayerControlled, PlayerDetached, PlayerManager, PlayersSystem, SessionStatus,
    SessionStatusChanged,
};

/// Record of a dispatched event.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Logged {
    Status(SessionStatus),
    Attached(EntityUid, NetUserId),
    Detached(EntityUid, NetUserId, DetachReason),
    Drop(ConnectionId, DisconnectReason),
}

pub type Log = Arc<Mutex<Vec<Logged>>>;

struct Recorder(Log);

impl System for Recorder {
    fn initialize(&mut self, init: &mut SystemInit<'_>) {
        let log = self.0.clone();
        init.subscribe_broadcast::<SessionStatusChanged, _>(move |_, ev| {
            log.lock().unwrap().push(Logged::Status(ev.new));
        });
        let log = self.0.clone();
        init.subscribe_broadcast::<ConnectionDropRequested, _>(move |_, ev| {
            log.lock()
                .unwrap()
                .push(Logged::Drop(ev.connection, ev.reason.clone()));
        });
        let log = self.0.clone();
        init.subscribe::<PlayerControlled, PlayerAttached, _>(move |_, uid, ev| {
            log.lock()
                .unwrap()
                .push(Logged::Attached(uid, ev.user.clone()));
        });
        let log = self.0.clone();
        init.subscribe::<PlayerControlled, PlayerDetached, _>(move |_, uid, ev| {
            log.lock()
                .unwrap()
                .push(Logged::Detached(uid, ev.user.clone(), ev.reason));
        });
    }
}

pub fn world_with_log() -> (World, Log) {
    let log = Log::default();
    let mut builder = World::builder();
    builder
        .add_system(PlayersSystem)
        .add_system(Recorder(log.clone()));
    let (world, _systems) = builder.build().expect("acyclic ordering");
    (world, log)
}

pub fn take(log: &Log) -> Vec<Logged> {
    std::mem::take(&mut *log.lock().unwrap())
}

pub fn user(id: &str) -> NetUserId {
    NetUserId::new(id)
}

pub fn secs(s: u64) -> Duration {
    Duration::from_secs(s)
}

/// Completes login from connecting to connected.
pub fn join(world: &mut World, user: &NetUserId, conn: u64, now: Duration) {
    PlayerManager::begin_connect(world, user, "Player", ConnectionId(conn), now);
    PlayerManager::finish_connect(world, user).unwrap();
}
