//! Shared helpers load reference trajectories in `tests/data/trajectories` and build a world
//! from their numeric data in the `crucible-e7-trajectory/1` format.

#![allow(dead_code)]

use std::path::PathBuf;

use crucible_physics::{
    Body, BodyId, BodyType, Fixture, GroundTile, MassData, MoveButtons, MovementSpeedModifier,
    Mover, PhysicsConfig, PhysicsWorld, Polygon, Shape, Vec2,
};
use serde_json::Value;

/// Names of the reference scenarios.
pub const SCENARIOS: [&str; 5] = ["walk", "run", "wall", "push", "doorway"];

/// Tolerance of 0.001 m in position and 0.001 m/s in velocity.
pub const TOLERANCE_POSITION: f32 = 0.001;
pub const TOLERANCE_VELOCITY: f32 = 0.001;

/// Body state at a tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct State {
    pub p: Vec2,
    pub a: f32,
    pub v: Vec2,
    pub awake: bool,
}

/// Reference trajectory.
pub struct Trajectory {
    pub name: String,
    json: Value,
    /// Moving bodies in `bodies[]` order.
    pub body_names: Vec<String>,
    /// Input changes as (tick, buttons) pairs.
    pub inputs: Vec<(u32, MoveButtons)>,
}

fn f(v: &Value) -> f32 {
    v.as_f64().unwrap_or_else(|| panic!("number expected: {v}")) as f32
}

fn v2(v: &Value) -> Vec2 {
    Vec2::new(f(&v[0]), f(&v[1]))
}

/// Recorded inputs use `U/D/L/R` for directions and `W` for walking.
pub fn parse_buttons(s: &str) -> MoveButtons {
    let mut b = MoveButtons::NONE;
    for c in s.chars() {
        b = b | match c {
            'U' => MoveButtons::UP,
            'D' => MoveButtons::DOWN,
            'L' => MoveButtons::LEFT,
            'R' => MoveButtons::RIGHT,
            'W' => MoveButtons::WALK,
            other => panic!("button {other}"),
        };
    }
    b
}

pub fn data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/trajectories")
}

impl Trajectory {
    pub fn load(name: &str) -> Self {
        let path = data_dir().join(format!("{name}.json"));
        let text =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let json: Value =
            serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert_eq!(json["format"], "crucible-e7-trajectory/1");
        let body_names = json["bodies"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["name"].as_str().unwrap().to_string())
            .collect();
        let inputs = json["inputs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| {
                (
                    i["tick"].as_u64().unwrap() as u32,
                    parse_buttons(i["buttons"].as_str().unwrap()),
                )
            })
            .collect();
        Self {
            name: name.to_string(),
            json,
            body_names,
            inputs,
        }
    }

    /// Number of tick transitions.
    pub fn tick_count(&self) -> usize {
        self.json["ticks"].as_array().unwrap().len() - 1
    }

    /// Buttons held during the transition from `tick` to `tick + 1`.
    pub fn buttons_at(&self, tick: u32) -> MoveButtons {
        let mut cur = MoveButtons::NONE;
        for &(t, b) in &self.inputs {
            if t <= tick {
                cur = b;
            }
        }
        cur
    }

    /// Reference state of `body` after `tick` ticks.
    pub fn reference(&self, tick: usize, body: &str) -> State {
        let o = &self.json["ticks"][tick][body];
        State {
            p: v2(&o["p"]),
            a: f(&o["a"]),
            v: v2(&o["v"]),
            awake: o["awake"].as_bool().unwrap(),
        }
    }

    /// Step parameters from the fixture's configuration variables.
    pub fn config(&self) -> PhysicsConfig {
        let c = &self.json["cvars"];
        let t = &self.json["timing"];
        let config = PhysicsConfig {
            tick_rate: t["tickrate"].as_u64().unwrap() as u32,
            substeps: t["substeps"].as_u64().unwrap() as u32,
            velocity_iterations: c["physics.veliter"].as_u64().unwrap() as u32,
            position_iterations: c["physics.positer"].as_u64().unwrap() as u32,
            warm_starting: c["physics.warmstart"].as_bool().unwrap(),
            baumgarte: f(&c["physics.baumgarte"]),
            max_linear_correction: f(&c["physics.maxlinearcorrection"]),
            velocity_threshold: f(&c["physics.velocitythreshold"]),
            max_linear_velocity: f(&c["physics.maxlinvelocity"]),
            max_angular_velocity: f(&c["physics.maxangvelocity"]),
            sleep_allowed: c["physics.sleepallowed"].as_bool().unwrap(),
            time_to_sleep: f(&c["physics.timetosleep"]),
            linear_sleep_tolerance_sqr: f(&c["physics.linsleeptol"]),
            angular_sleep_tolerance_sqr: f(&c["physics.angsleeptol"]),
            tile_friction: f(&c["physics.tile_friction"]),
            min_friction: f(&c["physics.min_friction"]),
            diagonal_movement: c["game.diagonalmovement"].as_bool().unwrap(),
        };
        assert_eq!(
            config.tick_dt().to_bits(),
            f(&t["tick_dt"]).to_bits(),
            "tick_dt"
        );
        config
    }

    fn ground(&self) -> GroundTile {
        let fl = &self.json["world"]["floor"];
        GroundTile {
            friction: f(&fl["friction"]),
            mob_friction: fl["mob_friction"].as_f64().map(|x| x as f32),
            mob_acceleration: fl["mob_acceleration"].as_f64().map(|x| x as f32),
        }
    }

    fn body_from_desc(&self, d: &Value, position: Vec2) -> Body {
        let body_type = match d["body_type"].as_str().unwrap() {
            "Static" => BodyType::Static,
            "Kinematic" => BodyType::Kinematic,
            "KinematicController" => BodyType::KinematicController,
            "Dynamic" => BodyType::Dynamic,
            other => panic!("body type {other}"),
        };
        let mut body = Body::new(body_type, position)
            .with_fixed_rotation(d["fixed_rotation"].as_bool().unwrap_or(false))
            .with_mass(MassData {
                mass: f(&d["mass"]),
                inv_mass: f(&d["inv_mass"]),
                inv_inertia: f(&d["inv_i"]),
                local_center: v2(&d["local_center"]),
            });
        body.linear_damping = f(&d["linear_damping"]);
        body.angular_damping = f(&d["angular_damping"]);
        body.sleeping_allowed = d["sleeping_allowed"].as_bool().unwrap();
        body.ground = self.ground();
        // Fixtures are sorted by ID, matching the ordered map used for the reference.
        for (_, fx) in d["fixtures"].as_object().unwrap() {
            let shape = match fx["shape"].as_str().unwrap() {
                "circle" => Shape::circle(f(&fx["radius"]), v2(&fx["center"])),
                "polygon" => {
                    let verts: Vec<Vec2> =
                        fx["vertices"].as_array().unwrap().iter().map(v2).collect();
                    let normals: Vec<Vec2> =
                        fx["normals"].as_array().unwrap().iter().map(v2).collect();
                    Shape::Polygon(
                        Polygon::with_normals(&verts, &normals, f(&fx["radius"])).expect("polygon"),
                    )
                }
                other => panic!("shape {other}"),
            };
            let mut fixture = Fixture::new(shape)
                .with_density(f(&fx["density"]))
                .with_friction(f(&fx["friction"]))
                .with_collision(
                    fx["layer"].as_i64().unwrap() as u32,
                    fx["mask"].as_i64().unwrap() as u32,
                );
            fixture.restitution = f(&fx["restitution"]);
            fixture.hard = fx["hard"].as_bool().unwrap();
            body = body.with_fixture(fixture);
        }
        if let Some(m) = d.get("movement").filter(|m| !m.is_null()) {
            assert!(
                !m["weightless"].as_bool().unwrap(),
                "weightless movement is out of scope"
            );
            body = body.with_mover(Mover {
                speed: MovementSpeedModifier {
                    base_walk_speed: f(&m["walk_speed"]),
                    base_sprint_speed: f(&m["sprint_speed"]),
                    walk_speed_modifier: 1.0,
                    sprint_speed_modifier: 1.0,
                    acceleration: f(&m["acceleration"]),
                    friction: f(&m["friction"]),
                    friction_no_input: f(&m["friction_no_input"]),
                    minimum_friction_speed: f(&m["minimum_friction_speed"]),
                },
                ..Mover::default()
            });
        }
        body
    }

    /// Builds the world with moving bodies in `bodies[]` order followed by walls.
    pub fn build(&self) -> Scene {
        let mut world = PhysicsWorld::new(self.config());
        let mut ids = Vec::new();
        for d in self.json["bodies"].as_array().unwrap() {
            let mut body = self.body_from_desc(d, v2(&d["initial_position"]));
            if let Some(v) = d.get("initial_velocity").filter(|v| !v.is_null()) {
                body.set_linear_velocity(v2(v));
            }
            ids.push(world.add_body(body));
        }
        let walls = &self.json["world"]["walls"];
        if !walls["body"].is_null() {
            for p in walls["positions"].as_array().unwrap() {
                world.add_body(self.body_from_desc(&walls["body"], v2(p)));
            }
        }
        Scene {
            world,
            ids,
            movers_buttons: MoveButtons::NONE,
        }
    }
}

/// Scenario world and IDs of the moving bodies.
pub struct Scene {
    pub world: PhysicsWorld,
    pub ids: Vec<BodyId>,
    movers_buttons: MoveButtons,
}

/// World tick for reference transition `k` to `k + 1`. World ticks start at 1.
pub fn sim_tick(k: usize) -> u32 {
    k as u32 + 1
}

impl Scene {
    pub fn state(&self, i: usize) -> State {
        let b = self.world.body(self.ids[i]);
        State {
            p: b.position(),
            a: b.angle(),
            v: b.linear_velocity(),
            awake: b.awake(),
        }
    }

    /// Restores the world snapshot. `held` is the set of buttons held at that snapshot.
    pub fn restore(&mut self, snapshot: &crucible_physics::PhysicsSnapshot, held: MoveButtons) {
        assert!(self.world.load_state(snapshot));
        self.movers_buttons = held;
    }

    /// Applies buttons as input commands at subtick zero, matching the reference recorder.
    pub fn apply_buttons(&mut self, tick: u32, buttons: MoveButtons) {
        let flags = [
            MoveButtons::UP,
            MoveButtons::DOWN,
            MoveButtons::LEFT,
            MoveButtons::RIGHT,
            MoveButtons::WALK,
        ];
        for flag in flags {
            let was = self.movers_buttons.contains(flag);
            let now = buttons.contains(flag);
            if was != now {
                for &id in &self.ids {
                    self.world.set_move_input(id, tick, 0, flag, now);
                }
            }
        }
        self.movers_buttons = buttons;
    }

    /// Replays the reference transition from `k` to `k + 1`.
    pub fn step_reference_tick(&mut self, traj: &Trajectory, k: usize) {
        let tick = sim_tick(k);
        self.apply_buttons(tick, traj.buttons_at(k as u32));
        self.world.step(tick).expect("step");
    }
}

/// Difference between angles modulo 2π.
pub fn angle_diff(a: f32, b: f32) -> f32 {
    let tau = std::f64::consts::TAU;
    let mut d = (f64::from(a) - f64::from(b)) % tau;
    if d > std::f64::consts::PI {
        d -= tau;
    }
    if d < -std::f64::consts::PI {
        d += tau;
    }
    d.abs() as f32
}

/// Computes a 64-bit FNV-1a hash of state bits for exact cross-run and cross-platform comparisons.
#[derive(Clone, Copy, Debug)]
pub struct StateHash(pub u64);

impl Default for StateHash {
    fn default() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
}

impl StateHash {
    pub fn write_u32(&mut self, x: u32) {
        for byte in x.to_le_bytes() {
            self.0 ^= u64::from(byte);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    pub fn write_state(&mut self, s: &State) {
        for x in [s.p.x, s.p.y, s.a, s.v.x, s.v.y] {
            self.write_u32(x.to_bits());
        }
        self.write_u32(u32::from(s.awake));
    }
}

/// States of all moving bodies at each tick, including the initial state.
pub fn run_states(traj: &Trajectory) -> Vec<Vec<State>> {
    let mut scene = traj.build();
    let mut out = Vec::with_capacity(traj.tick_count() + 1);
    out.push((0..scene.ids.len()).map(|i| scene.state(i)).collect());
    for k in 0..traj.tick_count() {
        scene.step_reference_tick(traj, k);
        out.push((0..scene.ids.len()).map(|i| scene.state(i)).collect());
    }
    out
}

/// Hash of all five scenarios.
pub fn hash_all_scenarios() -> u64 {
    let mut h = StateHash::default();
    for name in SCENARIOS {
        let traj = Trajectory::load(name);
        for tick in run_states(&traj) {
            for s in &tick {
                h.write_state(s);
            }
        }
    }
    h.0
}
