//! Shared replication test harness with a map, components with several network policies,
//! and a replication model advanced without a simulation loop.

#![allow(dead_code)]

use std::collections::BTreeMap;

use crucible_ecs::{
    Component, ComponentRegistration, EntityUid, NetEntity, NetworkPolicy, NetworkedComponent,
    Tick, World, networked,
};
use crucible_map::{Vec2, create_map, spawn_at};
use crucible_players::NetUserId;
use crucible_replication::{
    FieldSelection, NeutralValue, PlayerUpdate, ReplicatedComponents, Replication,
    ReplicationConfig, StateContext, StructWriter, register_map_states,
};

/// A component with two fields, per-field deltas, and an all-viewers policy.
#[networked]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stats {
    pub level: i64,
    pub score: i64,
}
impl Component for Stats {}

/// A one-field component visible to all viewers.
#[networked]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Flag {
    pub on: bool,
}
impl Component for Flag {}

/// A component visible to all viewers that has no per-field deltas.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Label {
    pub text: String,
}
impl Component for Label {}

/// An all-viewers component.
#[networked]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Speed {
    pub value: f32,
}
impl Component for Speed {}

/// An owner-only component.
#[networked]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Secret {
    pub code: i64,
}
impl Component for Secret {}

/// A server-only component whose serializer must never contribute to a frame.
#[networked]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Hidden {
    pub note: i64,
}
impl Component for Hidden {}

fn stats_state(_: &StateContext<'_>, c: &Stats, sel: FieldSelection) -> NeutralValue {
    let mut w = StructWriter::new(Stats::FIELD_NAMES, sel);
    w.field("level", || NeutralValue::Int(c.level));
    w.field("score", || NeutralValue::Int(c.score));
    w.finish()
}

fn flag_state(_: &StateContext<'_>, c: &Flag, sel: FieldSelection) -> NeutralValue {
    let mut w = StructWriter::new(Flag::FIELD_NAMES, sel);
    w.field("on", || NeutralValue::Bool(c.on));
    w.finish()
}

fn label_state(_: &StateContext<'_>, c: &Label, sel: FieldSelection) -> NeutralValue {
    let mut w = StructWriter::new(&[], sel);
    w.field("text", || NeutralValue::str(c.text.as_str()));
    w.finish()
}

fn speed_state(_: &StateContext<'_>, c: &Speed, sel: FieldSelection) -> NeutralValue {
    let mut w = StructWriter::new(Speed::FIELD_NAMES, sel);
    w.field("value", || NeutralValue::F32(c.value));
    w.finish()
}

fn secret_state(_: &StateContext<'_>, c: &Secret, sel: FieldSelection) -> NeutralValue {
    let mut w = StructWriter::new(Secret::FIELD_NAMES, sel);
    w.field("code", || NeutralValue::Int(c.code));
    w.finish()
}

fn hidden_state(_: &StateContext<'_>, c: &Hidden, sel: FieldSelection) -> NeutralValue {
    let mut w = StructWriter::new(Hidden::FIELD_NAMES, sel);
    w.field("note", || NeutralValue::Int(c.note));
    w.finish()
}

/// Test harness with a world, map, replication model, and tick.
pub struct Bench {
    pub world: World,
    pub replication: Replication,
    pub map: EntityUid,
    tick: u32,
}

impl Bench {
    /// Builds a harness with default settings.
    pub fn new() -> Bench {
        Bench::with_config(ReplicationConfig::default())
    }

    /// Builds a harness with the given settings.
    pub fn with_config(config: ReplicationConfig) -> Bench {
        let mut builder = World::builder();
        crucible_map::register(&mut builder).unwrap();
        crucible_replication::register(&mut builder).unwrap();
        let reg = |b: &mut crucible_ecs::WorldBuilder, r: ComponentRegistration| {
            b.register_component(r).unwrap();
        };
        reg(
            &mut builder,
            ComponentRegistration::new::<Stats>("Stats", "test")
                .networked_fields::<Stats>()
                .network(NetworkPolicy::AllViewers),
        );
        reg(
            &mut builder,
            ComponentRegistration::new::<Flag>("Flag", "test")
                .networked_fields::<Flag>()
                .network(NetworkPolicy::AllViewers),
        );
        reg(
            &mut builder,
            ComponentRegistration::new::<Label>("Label", "test").network(NetworkPolicy::AllViewers),
        );
        reg(
            &mut builder,
            ComponentRegistration::new::<Speed>("Speed", "test")
                .networked_fields::<Speed>()
                .network(NetworkPolicy::AllViewers),
        );
        reg(
            &mut builder,
            ComponentRegistration::new::<Secret>("Secret", "test")
                .networked_fields::<Secret>()
                .network(NetworkPolicy::OwnerOnly),
        );
        reg(
            &mut builder,
            ComponentRegistration::new::<Hidden>("Hidden", "test")
                .networked_fields::<Hidden>()
                .network(NetworkPolicy::ServerOnly),
        );
        let (mut world, _systems) = builder.build().unwrap();

        let mut components = ReplicatedComponents::new();
        register_map_states(&mut components, world.registry()).unwrap();
        let reg = world.registry();
        components
            .register_networked::<Stats>(reg, stats_state)
            .unwrap();
        components
            .register_networked::<Flag>(reg, flag_state)
            .unwrap();
        components.register::<Label>(reg, label_state).unwrap();
        components
            .register_networked::<Speed>(reg, speed_state)
            .unwrap();
        components
            .register_networked::<Secret>(reg, secret_state)
            .unwrap();
        components
            .register_networked::<Hidden>(reg, hidden_state)
            .unwrap();

        world.set_current_tick(Tick(1));
        let (map, _) = create_map(&mut world);
        Bench {
            world,
            replication: Replication::new(config, components),
            map,
            tick: 1,
        }
    }

    /// Current tick.
    pub fn tick(&self) -> Tick {
        Tick(self.tick)
    }

    /// An entity at `(x, y)` on the map.
    pub fn spawn(&mut self, x: f32, y: f32) -> EntityUid {
        spawn_at(&mut self.world, self.map, Vec2::new(x, y)).unwrap()
    }

    /// An entity parented to another entity by its transform.
    pub fn spawn_in(&mut self, parent: EntityUid) -> EntityUid {
        spawn_at(&mut self.world, parent, Vec2::ZERO).unwrap()
    }

    /// A player with a controlled entity.
    pub fn player(&mut self, name: &str, entity: Option<EntityUid>) -> NetUserId {
        let user = NetUserId::new(name);
        self.replication.add_viewer(user.clone());
        self.replication.set_controlled_entity(&user, entity);
        user
    }

    /// Finishes the current tick, processes queued removals, and builds player frames.
    pub fn finish_tick(&mut self) -> BTreeMap<NetUserId, PlayerUpdate> {
        self.world.end_tick();
        self.replication
            .update(&mut self.world)
            .into_iter()
            .map(|u| (u.user.clone(), u))
            .collect()
    }

    /// Starts the next tick.
    pub fn next_tick(&mut self) -> Tick {
        self.tick += 1;
        self.world.set_current_tick(Tick(self.tick));
        Tick(self.tick)
    }

    /// Builds frames for the next tick without changing the world.
    pub fn step(&mut self) -> BTreeMap<NetUserId, PlayerUpdate> {
        self.next_tick();
        self.finish_tick()
    }

    /// Network entity ID.
    pub fn net(&self, uid: EntityUid) -> NetEntity {
        self.world.net_entity(uid).unwrap()
    }

    /// Acknowledges the current tick for the player.
    pub fn ack(&mut self, user: &NetUserId) {
        assert!(self.replication.acknowledge(user, Tick(self.tick)));
    }
}

/// Component names in an entity state.
pub fn component_names(state: &crucible_replication::EntityState) -> Vec<&str> {
    state.components.iter().map(|c| c.name.as_ref()).collect()
}
