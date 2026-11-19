//! ECS physics system and core `physics` module.
//!
//! The [`PhysicsWorld`] simulation is independent of the ECS world. This module is
//! available with the default `world` feature. It synchronizes ECS components during
//! the physics phase of the tick, after input and systems and before deletion and
//! replication:
//!
//! 1. **Before the step.** Entities with [`Physics`] and `Transform` get a body in
//!    the [`PhysicsWorld`] for their map. Shapes come from [`Fixtures`], movement from
//!    [`InputMover`] and [`MovementSpeedModifier`], and the [`GroundTile`] under each
//!    body from the map's [`GroundTiles`] table. External changes to transforms,
//!    velocities, and fixtures are synchronized before stepping.
//! 2. **Step.** Advance each map at the current tick.
//! 3. **After the step.** Write position and rotation to `Transform` using
//!    `set_world_position` for change tracking and map events. Update velocity in
//!    [`Physics`] and input state in [`InputMover`]. Mark changes to transforms
//!    and velocities in the same tick. Body sleep does not mark the component.
//!    Raise directed [`StartCollide`] and [`EndCollide`] events for both entities
//!    when fixtures begin or stop touching.
//!
//! [`set_move_input`] updates the held movement intent from player input and marks
//! [`InputMover`] changed when appropriate.
//!
//! The map and solver each define an `f32` vector type. Use [`to_physics`] and
//! [`to_map`] to convert them. Transform angles use `f64`; body angles use `f32`.
//!
//! # Robust and SS14 to Crucible types
//!
//! | Robust / SS14 (C#) | Crucible (Rust) |
//! |---|---|
//! | `PhysicsComponent` (`BodyType`, `LinearVelocity`, `FixedRotation`, `CanCollide`…) | [`Physics`] |
//! | `FixturesComponent`, `PhysShapeCircle`, `PhysShapeAabb`, `PolygonShape` | [`Fixtures`] |
//! | `InputMoverComponent`, `MovementSpeedModifierComponent` | [`InputMover`], [`MovementSpeedModifier`] |
//! | `StartCollideEvent`, `EndCollideEvent` | [`StartCollide`], [`EndCollide`] |
//! | `SharedPhysicsSystem.Update` (tick step), `SharedMoverController` (input) | [`physics_phase`], [`set_move_input`] |
//! | `ContentTileDefinition.Friction/MobFriction/MobAcceleration` | [`tile_fields`], [`GroundTiles`] |

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use crucible_ecs::{
    Component, ComponentRemove, EntityUid, FieldMask, NetworkPolicy, NetworkedComponent, System,
    SystemInit, Tick, World, networked,
};
use crucible_ext::{
    ComponentData, ComponentSpec, CoreModule, FieldError, Prototypes, RegisterContext,
};
use crucible_loop::{SimTime, TickPhase};
use crucible_map::{TileDefinitions, Transform, set_local_rotation, set_world_position, tile_at};
use crucible_proto::{FieldDef, FieldType, Node, PolymorphicDef, Value};
use crucible_replication::{
    ButtonState, FieldSelection, InputCommandReceived, NeutralValue, StateContext, StructWriter,
};

use crate::body::{Body, BodyId, BodyType, Fixture};
use crate::contact::ContactEventKind;
use crate::math::Vec2;
use crate::mover::{GroundTile, InputMover, MoveButtons, MovementSpeedModifier, Mover, SubTick};
use crate::shape::{Polygon, Shape};
use crate::world::{PhysicsConfig, PhysicsWorld};

/// Physics module ID in `[modules] require`.
pub const MODULE_ID: &str = "physics";

/// Neutral movement input action names, corresponding to Robust's EngineKeyFunctions.
pub const MOVE_UP: &str = "MoveUp";
/// Move down.
pub const MOVE_DOWN: &str = "MoveDown";
/// Move left.
pub const MOVE_LEFT: &str = "MoveLeft";
/// Move right.
pub const MOVE_RIGHT: &str = "MoveRight";
/// Walk instead of running.
pub const WALK: &str = "Walk";

/// Prototype base type for polymorphic fixture shapes such as `!type:PhysShapeCircle`.
pub const SHAPE_BASE: &str = "IPhysShape";

// Vector conversions

/// Converts a map vector to the physics vector type. Both use the same `f32` values.
#[inline]
pub fn to_physics(v: crucible_map::Vec2) -> Vec2 {
    Vec2::new(v.x, v.y)
}

/// Converts a physics vector to the map vector type.
#[inline]
pub fn to_map(v: Vec2) -> crucible_map::Vec2 {
    crucible_map::Vec2::new(v.x, v.y)
}

// Components

/// Entity physics body, corresponding to `PhysicsComponent`. Replicates velocity
/// changes to all viewers.
#[networked]
#[derive(Clone, Debug, PartialEq)]
pub struct Physics {
    /// Body type.
    pub body_type: BodyType,
    /// Linear velocity in meters per second.
    pub linear_velocity: Vec2,
    /// Angular velocity in radians per second.
    pub angular_velocity: f32,
    /// Whether rotation is locked.
    pub fixed_rotation: bool,
    /// Whether the body can collide.
    pub can_collide: bool,
    /// Whether the body can sleep.
    pub sleeping_allowed: bool,
    /// Linear damping in inverse seconds.
    pub linear_damping: f32,
    /// Angular damping in inverse seconds.
    pub angular_damping: f32,
    /// Whether the body is awake. Sleeping does not mark this component changed.
    #[networked(skip)]
    pub awake: bool,
}

impl Component for Physics {}

impl Default for Physics {
    /// Static body, the default `bodyType` in SS14.
    fn default() -> Self {
        Physics {
            body_type: BodyType::Static,
            linear_velocity: Vec2::ZERO,
            angular_velocity: 0.0,
            fixed_rotation: false,
            can_collide: true,
            sleeping_allowed: true,
            linear_damping: 0.0,
            angular_damping: 0.0,
            awake: false,
        }
    }
}

impl Physics {
    /// Body of the given type with default properties.
    pub fn new(body_type: BodyType) -> Self {
        Physics {
            body_type,
            awake: body_type != BodyType::Static,
            ..Physics::default()
        }
    }
}

/// Body type name used by neutral replication.
pub fn body_type_name(body: BodyType) -> &'static str {
    match body {
        BodyType::Static => "Static",
        BodyType::Kinematic => "Kinematic",
        BodyType::KinematicController => "KinematicController",
        BodyType::Dynamic => "Dynamic",
    }
}

/// Selected physics body fields used by neutral replication.
pub fn physics_state(_: &StateContext<'_>, p: &Physics, selection: FieldSelection) -> NeutralValue {
    let mut w = StructWriter::new(Physics::FIELD_NAMES, selection);
    w.field("body_type", || {
        NeutralValue::str(body_type_name(p.body_type))
    });
    w.field("linear_velocity", || {
        NeutralValue::Vec2([p.linear_velocity.x, p.linear_velocity.y])
    });
    w.field("angular_velocity", || NeutralValue::F32(p.angular_velocity));
    w.field("fixed_rotation", || NeutralValue::Bool(p.fixed_rotation));
    w.field("can_collide", || NeutralValue::Bool(p.can_collide));
    w.field("sleeping_allowed", || {
        NeutralValue::Bool(p.sleeping_allowed)
    });
    w.field("linear_damping", || NeutralValue::F32(p.linear_damping));
    w.field("angular_damping", || NeutralValue::F32(p.angular_damping));
    w.finish()
}

/// Body fixtures keyed by ID, in data order, corresponding to `FixturesComponent`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Fixtures {
    /// Fixture ID and definition.
    pub fixtures: Vec<(String, Fixture)>,
}

impl Component for Fixtures {}

impl Fixtures {
    /// Fixture with ID `id`.
    pub fn single(id: &str, fixture: Fixture) -> Self {
        Fixtures {
            fixtures: vec![(id.to_owned(), fixture)],
        }
    }

    /// Fixture ID for a body-local fixture number.
    pub fn id(&self, index: u32) -> Option<&str> {
        self.fixtures.get(index as usize).map(|(id, _)| id.as_str())
    }
}

impl Component for InputMover {}

/// Bit mask for the held buttons, the only replicated field of [`InputMover`].
/// The client receives `HeldMoveButtons` and sub-tick state stays on the server.
pub const INPUT_MOVER_HELD_MOVE_BUTTONS: FieldMask = FieldMask::bit(0);

impl NetworkedComponent for InputMover {
    const FIELD_NAMES: &'static [&'static str] = &["heldMoveButtons"];
}

impl Component for MovementSpeedModifier {}

// Collision events

/// Two fixtures started touching, corresponding to `StartCollideEvent`. Both entities
/// receive a directed event as the local side of the collision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartCollide {
    /// Entity on which this event is raised.
    pub our_entity: EntityUid,
    /// The other entity.
    pub other_entity: EntityUid,
    /// Fixture number on this entity.
    pub our_fixture: u32,
    /// Fixture number on the other entity.
    pub other_fixture: u32,
}

/// Two fixtures stopped touching, corresponding to `EndCollideEvent`; fields match
/// [`StartCollide`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EndCollide {
    /// Entity on which this event is raised.
    pub our_entity: EntityUid,
    /// The other entity.
    pub other_entity: EntityUid,
    /// Fixture number on this entity.
    pub our_fixture: u32,
    /// Fixture number on the other entity.
    pub other_fixture: u32,
}

// Ground tiles

/// Movement fields read from SS14-style tile prototypes: `friction`, `mobFriction`,
/// and `mobAcceleration`. The map module adds them to the `tile` kind:
/// `MapModule::new().with_tile_fields(crucible_physics::ecs::tile_fields())`.
pub fn tile_fields() -> Vec<FieldDef> {
    vec![
        FieldDef::new("friction", FieldType::Float),
        FieldDef::new("mobFriction", FieldType::Float),
        FieldDef::new("mobAcceleration", FieldType::Float),
    ]
}

/// Movement properties by tile ID, built from prototypes at server startup and stored
/// as a world resource. Unknown tiles use [`GroundTile::default`].
#[derive(Clone, Debug, Default)]
pub struct GroundTiles {
    by_id: Vec<GroundTile>,
}

impl GroundTiles {
    /// Builds the table from tile prototypes and numeric IDs provided by `tiles`.
    pub fn from_prototypes(set: &crucible_proto::PrototypeSet, tiles: &TileDefinitions) -> Self {
        let mut by_id = vec![GroundTile::default(); tiles.len()];
        for (id, proto_id) in tiles.iter() {
            let Some(proto) = set.get(crucible_map::TILE_KIND, proto_id) else {
                continue;
            };
            let float = |f: &str| {
                proto
                    .field(f)
                    .and_then(Node::as_str)
                    .and_then(|s| s.parse::<f32>().ok())
            };
            let defaults = GroundTile::default();
            by_id[id as usize] = GroundTile {
                friction: float("friction").unwrap_or(defaults.friction),
                mob_friction: float("mobFriction"),
                mob_acceleration: float("mobAcceleration"),
            };
        }
        GroundTiles { by_id }
    }

    /// Movement properties for tile ID `type_id`.
    pub fn get(&self, type_id: i32) -> GroundTile {
        usize::try_from(type_id)
            .ok()
            .and_then(|i| self.by_id.get(i))
            .copied()
            .unwrap_or_default()
    }
}

// Player input

/// Presses or releases movement `button` at `sub_tick`, corresponding to
/// `SharedMoverController.SetMoveInput`. Marks [`InputMover`] changed if held buttons
/// changed. Returns `false` when the entity has no [`InputMover`].
///
/// Uses the **current simulation tick** (`world.current_tick()`), like SS14's use of
/// `Timing.CurTick`. Late input is applied at the current tick, as supplied by
/// `crucible-replication`'s input buffer. Call in the input or systems phase so
/// the physics phase runs with the same tick number.
/// A debug assertion rejects decreasing input ticks because they could place
/// movement in a tick already processed.
pub fn set_move_input(
    world: &mut World,
    uid: EntityUid,
    sub_tick: SubTick,
    button: MoveButtons,
    enabled: bool,
) -> bool {
    let tick = world.current_tick().0;
    let diagonal = world
        .resource::<PhysicsState>()
        .map_or(PhysicsConfig::robust_defaults().diagonal_movement, |s| {
            s.config.diagonal_movement
        });
    let Some(input) = world.get_mut::<InputMover>(uid) else {
        return false;
    };
    if !input.can_move {
        return true;
    }
    debug_assert!(
        tick >= input.last_input().0,
        "movement input for tick {tick} received after tick {}; input ticks must not decrease",
        input.last_input().0
    );
    let before = input.held_move_buttons();
    input.set_move_input(tick, sub_tick, button, enabled, diagonal);
    if input.held_move_buttons() != before {
        world.dirty_fields::<InputMover>(uid, INPUT_MOVER_HELD_MOVE_BUTTONS);
    }
    true
}

// World state

/// Entity's physics body and the last values written to its ECS components. The
/// stored values detect modifications made outside the physics step.
#[derive(Clone, Debug)]
struct Tracked {
    map: EntityUid,
    body: BodyId,
    position: crucible_map::Vec2,
    rotation: f64,
    physics: Physics,
    fixtures_tick: Tick,
}

/// Physics world for a map and the entities for its bodies.
#[derive(Clone, Debug)]
struct MapPhysics {
    world: PhysicsWorld,
    /// Entity by physics body slot index.
    entities: Vec<Option<EntityUid>>,
}

/// World resource holding map physics worlds and entity bodies.
#[derive(Clone, Debug)]
pub struct PhysicsState {
    config: PhysicsConfig,
    maps: BTreeMap<EntityUid, MapPhysics>,
    tracked: HashMap<EntityUid, Tracked>,
    /// Tick of the last completed step, or zero before the first step.
    last_step: u32,
    /// Contact events produced outside the step, such as removing a body or disabling
    /// collisions, that still need to be raised.
    pending: Vec<ContactNote>,
}

/// Contact event with the event kind and fixture number for each of the two entities.
type ContactNote = (ContactEventKind, EntityUid, u32, EntityUid, u32);

impl MapPhysics {
    /// Moves pending map events into `out` while body slots still map to entities.
    fn drain_notes(&mut self, out: &mut Vec<ContactNote>) {
        for e in self.world.drain_contact_events() {
            let entity = |id: BodyId| self.entities.get(id.index() as usize).copied().flatten();
            if let (Some(a), Some(b)) = (entity(e.body_a), entity(e.body_b)) {
                out.push((e.kind, a, e.fixture_a, b, e.fixture_b));
            }
        }
    }
}

impl PhysicsState {
    /// Creates empty physics state for `config`.
    pub fn new(config: PhysicsConfig) -> Self {
        PhysicsState {
            config,
            maps: BTreeMap::new(),
            tracked: HashMap::new(),
            last_step: 0,
            pending: Vec::new(),
        }
    }

    /// Tick of the last physics step, or zero before the first one.
    pub fn last_step(&self) -> u32 {
        self.last_step
    }

    /// Physics step configuration.
    pub fn config(&self) -> &PhysicsConfig {
        &self.config
    }

    /// Body of the entity in its map's physics world.
    pub fn body(&self, uid: EntityUid) -> Option<&Body> {
        let t = self.tracked.get(&uid)?;
        self.maps.get(&t.map)?.world.try_body(t.body)
    }

    /// Physics world for map `map_uid`.
    pub fn map_world(&self, map_uid: EntityUid) -> Option<&PhysicsWorld> {
        self.maps.get(&map_uid).map(|m| &m.world)
    }

    /// Total body count across maps.
    pub fn body_count(&self) -> usize {
        self.tracked.len()
    }

    fn remove(&mut self, uid: EntityUid) {
        let Some(t) = self.tracked.remove(&uid) else {
            return;
        };
        if let Some(m) = self.maps.get_mut(&t.map) {
            m.world.remove_body(t.body);
            m.drain_notes(&mut self.pending);
            if let Some(slot) = m.entities.get_mut(t.body.index() as usize) {
                *slot = None;
            }
        }
    }
}

fn state(world: &mut World) -> &mut PhysicsState {
    if world.resource::<PhysicsState>().is_none() {
        let mut config = PhysicsConfig::robust_defaults();
        if let Some(time) = world.resource::<SimTime>() {
            config.tick_rate = time.tick_rate();
        }
        world.insert_resource(PhysicsState::new(config));
    }
    world
        .resource_mut::<PhysicsState>()
        .expect("the resource was just inserted")
}

// System and tick phase

/// Removes the body when [`Physics`] is removed, including during entity deletion.
/// [`physics_phase`] handles synchronization and stepping.
pub struct PhysicsSystem;

impl System for PhysicsSystem {
    fn initialize(&mut self, init: &mut SystemInit<'_>) {
        init.subscribe::<InputMover, InputCommandReceived, _>(|world, uid, event| {
            let button = match event.function.as_ref() {
                MOVE_UP => MoveButtons::UP,
                MOVE_DOWN => MoveButtons::DOWN,
                MOVE_LEFT => MoveButtons::LEFT,
                MOVE_RIGHT => MoveButtons::RIGHT,
                WALK => MoveButtons::WALK,
                _ => return,
            };
            set_move_input(
                world,
                uid,
                event.sub_tick,
                button,
                event.state == ButtonState::Down,
            );
        });
        init.subscribe::<Physics, ComponentRemove, _>(|world, uid, _| {
            let Some(s) = world.resource_mut::<PhysicsState>() else {
                return;
            };
            s.remove(uid);
            let notes = std::mem::take(&mut s.pending);
            // The body still belongs to the entity. Send both contact-end events before removing it.
            raise_contacts(world, notes, Some(uid));
        });
    }
}

/// Synchronizes components into each map's physics world, steps it, and writes results and events back.
pub fn physics_phase(world: &mut World, _frame_time: f32) {
    sync_in(world);
    let tick = world.current_tick().0;
    // End events generated outside the step, for example by toggling collisions or replacing a body.
    let mut events = std::mem::take(&mut state(world).pending);
    {
        let s = state(world);
        // Physics uses the current simulation tick. Repeating a tick would send
        // sub-tick input to a tick that has already been processed.
        debug_assert!(
            tick > s.last_step,
            "physics step for tick {tick} follows tick {}; tick numbers must increase",
            s.last_step
        );
        s.last_step = tick;
        for (map_uid, m) in s.maps.iter_mut() {
            if let Err(error) = m.world.step(tick) {
                tracing::error!(target: "crucible_physics::ecs", map = %map_uid, %error, "map physics step failed");
                continue;
            }
            m.drain_notes(&mut events);
        }
    }
    sync_out(world);
    raise_contacts(world, events, None);
}

/// Raises contact events for both sides. Entities being deleted are skipped except
/// for `removing`, whose body is being removed now and must receive its end event
/// before the component disappears.
fn raise_contacts(world: &mut World, events: Vec<ContactNote>, removing: Option<EntityUid>) {
    for (kind, a, fa, b, fb) in events {
        for (our, our_f, other, other_f) in [(a, fa, b, fb), (b, fb, a, fa)] {
            if Some(our) != removing && world.terminating_or_deleted(our) {
                continue;
            }
            match kind {
                ContactEventKind::Started => world.raise(
                    our,
                    &mut StartCollide {
                        our_entity: our,
                        other_entity: other,
                        our_fixture: our_f,
                        other_fixture: other_f,
                    },
                ),
                ContactEventKind::Ended => world.raise(
                    our,
                    &mut EndCollide {
                        our_entity: our,
                        other_entity: other,
                        our_fixture: our_f,
                        other_fixture: other_f,
                    },
                ),
            }
        }
    }
}

fn build_body(world: &World, uid: EntityUid, physics: &Physics, t: &Transform) -> Body {
    let mut body = Body::new(physics.body_type, to_physics(t.world_position()))
        .with_angle(t.world_rotation() as f32)
        .with_fixed_rotation(physics.fixed_rotation);
    body.linear_damping = physics.linear_damping;
    body.angular_damping = physics.angular_damping;
    body.sleeping_allowed = physics.sleeping_allowed;
    if let Some(f) = world.get::<Fixtures>(uid) {
        for (_, fixture) in &f.fixtures {
            body = body.with_fixture(*fixture);
        }
    }
    body
}

/// Synchronizes components into physics bodies.
fn sync_in(world: &mut World) {
    let mut seen = HashSet::new();
    let uids: Vec<EntityUid> = world.query::<(Physics, Transform)>().into_snapshot();
    for uid in uids {
        if world.terminating_or_deleted(uid) {
            continue;
        }
        let (Some(physics), Some(t)) = (world.get::<Physics>(uid), world.get::<Transform>(uid))
        else {
            continue;
        };
        let physics = physics.clone();
        let Some(map_uid) = t.map_uid().filter(|&m| m != uid) else {
            continue;
        };
        let (position, rotation, map_id) = (t.world_position(), t.world_rotation(), t.map_id());
        seen.insert(uid);

        let fixtures_tick = world
            .component_last_modified::<Fixtures>(uid)
            .unwrap_or(Tick::ZERO);
        let existing = world
            .resource::<PhysicsState>()
            .and_then(|s| s.tracked.get(&uid).cloned());
        let rebuild = match &existing {
            None => true,
            Some(tr) => {
                tr.map != map_uid
                    || tr.physics.body_type != physics.body_type
                    || tr.fixtures_tick != fixtures_tick
            }
        };
        let ground = ground_tile(world, map_id, position);
        let mover = world.get::<InputMover>(uid).map(|input| Mover {
            input: *input,
            speed: world
                .get::<MovementSpeedModifier>(uid)
                .copied()
                .unwrap_or_default(),
        });

        if rebuild {
            let mut body = build_body(
                world,
                uid,
                &physics,
                world.get::<Transform>(uid).expect("entity has a transform"),
            );
            body.mover = mover;
            body.ground = ground;
            let velocity = physics.linear_velocity;
            let angular = physics.angular_velocity;
            let can_collide = physics.can_collide;
            let config = state(world).config;
            let s = state(world);
            s.remove(uid);
            let m = s.maps.entry(map_uid).or_insert_with(|| MapPhysics {
                world: PhysicsWorld::new(config),
                entities: Vec::new(),
            });
            let id = m.world.add_body(body);
            let b = m.world.body_mut(id);
            b.set_linear_velocity(velocity);
            b.set_angular_velocity(angular);
            if !can_collide {
                m.world.set_can_collide(id, false);
            }
            let slot = id.index() as usize;
            if m.entities.len() <= slot {
                m.entities.resize(slot + 1, None);
            }
            m.entities[slot] = Some(uid);
            s.tracked.insert(
                uid,
                Tracked {
                    map: map_uid,
                    body: id,
                    position,
                    rotation,
                    physics,
                    fixtures_tick,
                },
            );
            continue;
        }

        let tr = existing.expect("body is already tracked when no rebuild is needed");
        let s = state(world);
        let m = s.maps.get_mut(&map_uid).expect("body map exists");
        if tr.physics.can_collide != physics.can_collide {
            m.world.set_can_collide(tr.body, physics.can_collide);
            m.drain_notes(&mut s.pending);
        }
        let b = m.world.body_mut(tr.body);
        // A transform update outside the step teleports the body.
        if position != tr.position {
            b.set_position(to_physics(position));
        }
        if rotation != tr.rotation {
            b.set_angle(rotation as f32);
        }
        // Body component changes made outside the step.
        if physics.linear_velocity != tr.physics.linear_velocity {
            b.set_linear_velocity(physics.linear_velocity);
        }
        if physics.angular_velocity != tr.physics.angular_velocity {
            b.set_angular_velocity(physics.angular_velocity);
        }
        b.fixed_rotation = physics.fixed_rotation;
        b.sleeping_allowed = physics.sleeping_allowed;
        b.linear_damping = physics.linear_damping;
        b.angular_damping = physics.angular_damping;
        b.mover = mover;
        b.ground = ground;
        if let Some(t) = s.tracked.get_mut(&uid) {
            t.position = position;
            t.rotation = rotation;
            t.physics = physics;
        }
    }
    let gone: Vec<EntityUid> = world
        .resource::<PhysicsState>()
        .map(|s| {
            s.tracked
                .keys()
                .copied()
                .filter(|u| !seen.contains(u))
                .collect()
        })
        .unwrap_or_default();
    if !gone.is_empty() {
        let s = state(world);
        for uid in gone {
            s.remove(uid);
        }
    }
}

/// Resolves the movement properties of the tile beneath a point on the map.
fn ground_tile(
    world: &World,
    map_id: crucible_map::MapId,
    position: crucible_map::Vec2,
) -> GroundTile {
    match (
        tile_at(world, map_id, position),
        world.resource::<GroundTiles>(),
    ) {
        (Some((_, _, tile)), Some(table)) => table.get(tile.type_id),
        _ => GroundTile::default(),
    }
}

/// Writes physics bodies back to ECS components and marks changes.
fn sync_out(world: &mut World) {
    let Some(s) = world.resource::<PhysicsState>() else {
        return;
    };
    let mut updates = Vec::with_capacity(s.tracked.len());
    for (&uid, t) in &s.tracked {
        let Some(b) = s.maps.get(&t.map).and_then(|m| m.world.try_body(t.body)) else {
            continue;
        };
        updates.push((
            uid,
            b.position(),
            b.angle(),
            b.linear_velocity(),
            b.angular_velocity(),
            b.awake(),
            b.mover.map(|m| m.input),
        ));
    }
    // Writes are order-independent. Map events such as OverVoid are raised
    // in entity order.
    updates.sort_by_key(|u| u.0);
    for (uid, position, angle, velocity, angular, awake, input) in updates {
        if world.terminating_or_deleted(uid) {
            continue;
        }
        let Some(t) = world.get::<Transform>(uid) else {
            continue;
        };
        let (old_position, old_rotation) = (t.world_position(), t.world_rotation());
        let parent_rotation = old_rotation - t.local_rotation();
        if to_physics(old_position) != position {
            let _ = set_world_position(world, uid, to_map(position));
        }
        if old_rotation as f32 != angle {
            let _ = set_local_rotation(world, uid, f64::from(angle) - parent_rotation);
        }
        let mut mask = FieldMask::EMPTY;
        if let Some(p) = world.get_mut::<Physics>(uid) {
            if p.linear_velocity != velocity {
                p.linear_velocity = velocity;
                mask |= Physics::FIELD_LINEAR_VELOCITY;
            }
            if p.angular_velocity != angular {
                p.angular_velocity = angular;
                mask |= Physics::FIELD_ANGULAR_VELOCITY;
            }
            p.awake = awake;
        }
        if !mask.is_empty() {
            world.dirty_fields::<Physics>(uid, mask);
        }
        if let Some(input) = input
            && let Some(c) = world.get_mut::<InputMover>(uid)
        {
            // Only input can change and mark the buttons via set_move_input. Physics
            // stores accumulated sub-tick input and desired velocity as server state.
            *c = input;
        }
        // Track what we wrote to detect updates from outside the next step.
        let (position, rotation) = world
            .get::<Transform>(uid)
            .map(|t| (t.world_position(), t.world_rotation()))
            .unwrap_or((old_position, old_rotation));
        let physics = world.get::<Physics>(uid).cloned();
        if let Some(tr) = world
            .resource_mut::<PhysicsState>()
            .and_then(|s| s.tracked.get_mut(&uid))
        {
            tr.position = position;
            tr.rotation = rotation;
            if let Some(p) = physics {
                tr.physics = p;
            }
        }
    }
}

// Prototype deserialization

fn body_type(d: &ComponentData<'_>) -> Result<BodyType, FieldError> {
    let Some(text) = d.string("bodyType")? else {
        return Ok(BodyType::Static);
    };
    match text.as_str() {
        "Static" => Ok(BodyType::Static),
        "Dynamic" => Ok(BodyType::Dynamic),
        "Kinematic" => Ok(BodyType::Kinematic),
        "KinematicController" => Ok(BodyType::KinematicController),
        other => Err(field_error(
            d,
            "bodyType",
            d.get("bodyType"),
            format!(
                "unknown body type '{other}' (expected Static, Dynamic, Kinematic, or KinematicController)"
            ),
        )),
    }
}

fn field_error(
    d: &ComponentData<'_>,
    field: &str,
    node: Option<&Node>,
    message: String,
) -> FieldError {
    FieldError {
        component: d.component().to_owned(),
        field: field.to_owned(),
        pos: node.map(|n| n.pos),
        message,
    }
}

fn read_physics(d: &ComponentData<'_>) -> Result<Physics, FieldError> {
    let body_type = body_type(d)?;
    let float = |f: &str, default: f32| -> Result<f32, FieldError> {
        Ok(d.float(f)?.map_or(default, |v| v as f32))
    };
    Ok(Physics {
        fixed_rotation: d.bool("fixedRotation")?.unwrap_or(false),
        can_collide: d.bool("canCollide")?.unwrap_or(true),
        sleeping_allowed: d.bool("sleepingAllowed")?.unwrap_or(true),
        linear_damping: float("linearDamping", 0.0)?,
        angular_damping: float("angularDamping", 0.0)?,
        ..Physics::new(body_type)
    })
}

fn pair(text: &str) -> Option<Vec2> {
    let (x, y) = text.split_once(',')?;
    Some(Vec2::new(x.trim().parse().ok()?, y.trim().parse().ok()?))
}

fn read_shape(d: &ComponentData<'_>, id: &str, node: &Node) -> Result<Shape, FieldError> {
    let err =
        |message: String| field_error(d, &format!("fixtures.{id}.shape"), Some(node), message);
    let tag = node.type_tag.as_deref().unwrap_or("");
    let get = |f: &str| node.get(f).and_then(Node::as_str);
    match tag {
        "PhysShapeCircle" => {
            let radius = get("radius").map_or(Ok(0.5), |r| {
                r.parse::<f32>()
                    .map_err(|_| err(format!("radius: '{r}' is not a number")))
            })?;
            let position = match get("position") {
                None => Vec2::ZERO,
                Some(p) => {
                    pair(p).ok_or_else(|| err(format!("position: '{p}' is not an x,y pair")))?
                }
            };
            Ok(Shape::circle(radius, position))
        }
        "PhysShapeAabb" => {
            let bounds = get("bounds").unwrap_or("-0.5,-0.5,0.5,0.5");
            let v: Vec<f32> = bounds
                .split(',')
                .filter_map(|s| s.trim().parse().ok())
                .collect();
            let [l, b, r, t] = v[..] else {
                return Err(err(format!(
                    "bounds: '{bounds}' is not a left,bottom,right,top tuple"
                )));
            };
            Ok(Shape::Polygon(Polygon::rect(
                Vec2::new(l, b),
                Vec2::new(r, t),
            )))
        }
        "PolygonShape" => {
            let Some(Value::Seq(items)) = node.get("vertices").map(|n| &n.value) else {
                return Err(err("vertices: expected a list of x,y pairs".into()));
            };
            let mut vertices = Vec::with_capacity(items.len());
            for item in items {
                let text = item.as_str().unwrap_or("");
                vertices.push(
                    pair(text).ok_or_else(|| err(format!("vertex '{text}' is not an x,y pair")))?,
                );
            }
            Polygon::new(&vertices)
                .map(Shape::Polygon)
                .map_err(|e| err(format!("polygon: {e:?}")))
        }
        other => Err(err(format!("unknown shape '{other}'"))),
    }
}

/// Combines the numeric collision masks with bitwise OR. Names such as SS14's
/// `MobMask` can be resolved by an extension or adapter.
fn read_bits(d: &ComponentData<'_>, id: &str, node: &Node, field: &str) -> Result<u32, FieldError> {
    let Some(list) = node.get(field) else {
        return Ok(0);
    };
    let Some(items) = list.as_seq() else {
        return Err(field_error(
            d,
            &format!("fixtures.{id}.{field}"),
            Some(list),
            "expected a list".into(),
        ));
    };
    let mut bits = 0u32;
    for item in items {
        let text = item.as_str().unwrap_or("");
        let value = text.trim().parse::<u32>().map_err(|_| {
            field_error(
                d,
                &format!("fixtures.{id}.{field}"),
                Some(item),
                format!("'{text}' is not a numeric mask"),
            )
        })?;
        bits |= value;
    }
    Ok(bits)
}

fn read_fixtures(d: &ComponentData<'_>) -> Result<Fixtures, FieldError> {
    let Some(map) = d.get("fixtures") else {
        return Ok(Fixtures::default());
    };
    let Some(entries) = map.as_map() else {
        return Err(field_error(
            d,
            "fixtures",
            Some(map),
            "expected a mapping".into(),
        ));
    };
    let mut out = Fixtures::default();
    for (key, value) in entries {
        let id = key.as_str().unwrap_or("");
        let Some(shape_node) = value.get("shape") else {
            return Err(field_error(
                d,
                &format!("fixtures.{id}"),
                Some(value),
                "missing `shape`".into(),
            ));
        };
        let shape = read_shape(d, id, shape_node)?;
        let num = |f: &str, default: f32| -> Result<f32, FieldError> {
            match value.get(f).and_then(Node::as_str) {
                None => Ok(default),
                Some(t) => t.parse().map_err(|_| {
                    field_error(
                        d,
                        &format!("fixtures.{id}.{f}"),
                        value.get(f),
                        format!("'{t}' is not a number"),
                    )
                }),
            }
        };
        let mut fixture = Fixture::new(shape)
            .with_density(num("density", 1.0)?)
            .with_friction(num("friction", 0.4)?)
            .with_collision(
                read_bits(d, id, value, "layer")?,
                read_bits(d, id, value, "mask")?,
            );
        fixture.restitution = num("restitution", 0.0)?;
        if let Some(hard) = value.get("hard").and_then(Node::as_str) {
            fixture.hard = crucible_proto::parse_bool(hard).ok_or_else(|| {
                field_error(
                    d,
                    &format!("fixtures.{id}.hard"),
                    value.get("hard"),
                    format!("'{hard}' is not a boolean"),
                )
            })?;
        }
        out.fixtures.push((id.to_owned(), fixture));
    }
    Ok(out)
}

fn read_speed(d: &ComponentData<'_>) -> Result<MovementSpeedModifier, FieldError> {
    let base = MovementSpeedModifier::default();
    let float = |f: &str, default: f32| -> Result<f32, FieldError> {
        Ok(d.float(f)?.map_or(default, |v| v as f32))
    };
    Ok(MovementSpeedModifier {
        base_walk_speed: float("baseWalkSpeed", base.base_walk_speed)?,
        base_sprint_speed: float("baseSprintSpeed", base.base_sprint_speed)?,
        acceleration: float("acceleration", base.acceleration)?,
        friction: float("friction", base.friction)?,
        friction_no_input: float("frictionNoInput", base.friction_no_input)?,
        minimum_friction_speed: float("minimumFrictionSpeed", base.minimum_friction_speed)?,
        ..base
    })
}

// Core module

/// Core `physics` module, which depends on `map`. Registers [`Physics`], [`Fixtures`],
/// [`InputMover`], and [`MovementSpeedModifier`] for all viewers, SS14-style fixture
/// prototypes with `!type:` shapes, and [`PhysicsSystem`] with [`physics_phase`].
/// Builds [`GroundTiles`] at server startup.
#[derive(Clone, Copy, Default)]
pub struct PhysicsModule;

impl CoreModule for PhysicsModule {
    fn id(&self) -> &'static str {
        MODULE_ID
    }

    fn dependencies(&self) -> &'static [&'static str] {
        &[crucible_map::MODULE_ID]
    }

    fn register(&self, ctx: &mut RegisterContext<'_>) {
        let float = FieldType::Float;
        let strings = || FieldType::List(Box::new(FieldType::String));
        let fixture = FieldType::Struct(vec![
            FieldDef::new("shape", FieldType::Polymorphic(SHAPE_BASE.to_owned())),
            FieldDef::new("density", float.clone()),
            FieldDef::new("friction", float.clone()),
            FieldDef::new("restitution", float.clone()),
            FieldDef::new("hard", FieldType::Bool),
            FieldDef::new("layer", strings()),
            FieldDef::new("mask", strings()),
        ]);
        for (name, fields) in [
            (
                "PhysShapeCircle",
                vec![
                    FieldDef::new("radius", float.clone()),
                    FieldDef::new("position", FieldType::String),
                ],
            ),
            (
                "PhysShapeAabb",
                vec![FieldDef::new("bounds", FieldType::String)],
            ),
            ("PolygonShape", vec![FieldDef::new("vertices", strings())]),
        ] {
            ctx.polymorphic_type(PolymorphicDef {
                base: SHAPE_BASE.to_owned(),
                name: name.to_owned(),
                fields,
            });
        }
        ctx.component(
            ComponentSpec::<Physics>::new("Physics")
                .field("bodyType", FieldType::String)
                .field("fixedRotation", FieldType::Bool)
                .field("canCollide", FieldType::Bool)
                .field("sleepingAllowed", FieldType::Bool)
                .field("linearDamping", float.clone())
                .field("angularDamping", float.clone())
                .network(NetworkPolicy::AllViewers)
                .networked_fields()
                .from_prototype(read_physics),
        )
        .component(
            ComponentSpec::<Fixtures>::new("Fixtures")
                .field("fixtures", FieldType::Map(Box::new(fixture)))
                .network(NetworkPolicy::AllViewers)
                .from_prototype(read_fixtures),
        )
        .component(
            ComponentSpec::<InputMover>::new("InputMover")
                .network(NetworkPolicy::AllViewers)
                .networked_fields()
                .from_default(),
        )
        .component(
            ComponentSpec::<MovementSpeedModifier>::new("MovementSpeedModifier")
                .field("baseWalkSpeed", float.clone())
                .field("baseSprintSpeed", float.clone())
                .field("acceleration", float.clone())
                .field("friction", float.clone())
                .field("frictionNoInput", float.clone())
                .field("minimumFrictionSpeed", float)
                .network(NetworkPolicy::AllViewers)
                .from_prototype(read_speed),
        )
        .system(PhysicsSystem)
        .tick_phase(TickPhase::Physics, "physics", physics_phase);
    }

    fn ready(&self, world: &mut World) -> Result<(), String> {
        let set = world
            .resource::<Prototypes>()
            .map(|p| Arc::clone(&p.set))
            .ok_or("world has no prototype set")?;
        let tiles = world
            .resource::<TileDefinitions>()
            .ok_or("world has no tile definitions from the map module")?;
        let ground = GroundTiles::from_prototypes(&set, tiles);
        world.insert_resource(ground);
        Ok(())
    }
}
