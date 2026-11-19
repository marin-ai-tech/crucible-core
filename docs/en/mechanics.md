# Core mechanics and capabilities

Crucible Core provides the building blocks for a multiplayer simulation in 12 Rust crates. This catalogue covers the public 0.1.0 snapshot. Each section explains what the corresponding API does and points to code or tests included in the snapshot.

## Entity world

`crucible-ecs` stores entities, components, and shared world resources. `crucible-ecs-macros` adds field-level change tracking.

| Capability | What it provides |
| --- | --- |
| Entity identity | An `EntityUid` combines a slot and generation so an old identifier cannot resolve to an entity created later in the same slot. Separate `NetEntity` IDs stay unique within the lifetime of a world. |
| Components and queries | Components are typed data registered under stable names. Systems can add or remove them immediately and query entities by component combination. Query iteration tolerates structural changes and checks that each returned entity still matches. |
| Lifecycle | Entities can be created uninitialized and started after their components are in place. Components emit events on addition, initialization, startup, shutdown, and removal. Entity deletion can run immediately or at the end of a tick. |
| Event delivery | Systems receive directed events for an entity and broadcast events for the world. Handlers can specify an order, cancel an action through a cancellable event, or queue events for later delivery. A configurable recursion limit bounds nested events. |
| Change tracking | The world records when components change. The `#[networked]` macro generates field masks and setters for up to 64 tracked fields on a named-field struct. Use `#[networked(skip)]` to exclude a field from tracking. |

[Entity tests](../../backend/crates/crucible-ecs/tests/entities.rs) · [Event tests](../../backend/crates/crucible-ecs/tests/events.rs) · [Change-tracking tests](../../backend/crates/crucible-ecs/tests/changes.rs) · [Macro implementation](../../backend/crates/crucible-ecs-macros/src/lib.rs)

## Prototype data and localization

`crucible-proto` describes and validates reusable data records. `crucible-loc` resolves strings referenced by their keys. The runtime in `crucible-ext` turns validated entity records into live entities.

| Capability | What it provides |
| --- | --- |
| Typed YAML data | The loader reads source positions, treats scalar types according to field schemas, and validates polymorphic values marked with `!type:` tags. It reports duplicate keys and unsupported tags by position. |
| Defaults and inheritance | Prototypes have a kind and identifier. A child can inherit from multiple parents and supply its own component fields. Load roots support explicit overrides and optional identifier prefixes. Abstract prototypes can serve as parents. |
| Validation | A single report collects diagnostics across the loaded files. Explicitly ignored component types can be omitted from the resolved data. Unknown types, invalid fields, and inheritance cycles produce errors. |
| Entity creation and reload | The runtime instantiates entities from resolved prototypes using registered component factories. Builds with debug assertions support data reload. A rejected prototype load keeps the previous validated set. |
| Fluent messages | Load `.ftl` resources by language, look up message keys and attributes, and substitute named arguments. Missing messages fall back to the default language. Prototype references to keys absent from that language generate warnings. |

[YAML parsing tests](../../backend/crates/crucible-proto/tests/parsing.rs) · [Prototype loading tests](../../backend/crates/crucible-proto/tests/loading.rs) · [Spawn and reload tests](../../backend/crates/crucible-ext/tests/prototypes.rs) · [Localization tests](../../backend/crates/crucible-loc/tests/localization.rs)

## Maps and spatial queries

`crucible-map` models tile worlds and the positions of their contents.

| Capability | What it provides |
| --- | --- |
| Maps, grids, and tiles | Maps and their grids are entities. Each grid holds square tile chunks, 16 by 16 tiles by default. Tile definitions come from prototypes, and tile changes raise an event. An `OverVoid` event lets the host decide what happens when an entity has no tile beneath it. |
| Transform hierarchy | An entity can have a map, grid, or other entity as its parent. The hierarchy computes world positions from local positions and rotations. Entities can be anchored to grid tiles. |
| Spatial queries | Systems can query entities on a tile, inside a box, or within a radius. |
| Map import | The loader supports SS14 YAML map and chunk formats 6 and 7. A file can override or remove inherited components. A failed load removes the entities created during that operation and reports the source location and entity when available. |

[Map and transform tests](../../backend/crates/crucible-map/tests/world_map.rs) · [Load a tile map](examples/load-a-map.md)

## 2D physics and movement

`crucible-physics` provides a physics world for each map and a module that exchanges state with the ECS world.

| Capability | What it provides |
| --- | --- |
| Bodies and fixtures | Static, kinematic, controller-driven, and dynamic bodies can use circle or convex polygon fixtures. Fixtures carry collision layers and masks, density, friction, and restitution. Sensor fixtures generate contacts without physical response. |
| Player movement | Button input includes a fraction of the current tick. The controller uses acceleration and ground friction to move, can slide along walls, and follows the solver's rules for contacts with dynamic bodies. |
| Contact simulation | The solver advances with configurable fixed substeps and handles contact impulses, friction, damping, and sleeping. The default physics configuration uses two substeps per tick at 30 Hz. ECS systems can observe contact start and end events. |
| Snapshots and replay | Save and restore the physics state for replay. The included checks cover bitwise repeatability and five recorded trajectories with position and velocity tolerances of 0.001 m and 0.001 m/s. |

[Movement and collision tests](../../backend/crates/crucible-physics/tests/scenarios.rs) · [Contact event tests](../../backend/crates/crucible-physics/tests/ecs_world.rs) · [Replay tests](../../backend/crates/crucible-physics/tests/determinism.rs) · [Reference trajectories](../../backend/crates/crucible-physics/tests/data/trajectories/README.md)

## Ticks, timers, and rounds

`crucible-loop` advances the world under a clock supplied by the host or test harness.

| Capability | What it provides |
| --- | --- |
| Fixed tick and catch-up | The default rate is 30 ticks per second. A loop iteration runs at most five catch-up ticks by default and counts additional dropped ticks. Systems can declare their order or run every specified number of ticks. |
| Simulation time and random streams | Timers advance with simulation time. Named random streams derive from the configured seed so tests and replays can reproduce a sequence. |
| Lobby and round flow | The phase machine advances through lobby, preparation, play, and results. It supports readiness and a countdown that can pause or be delayed. Map selection, rule selection, spawning, and late admission use configurable policies. |
| Results and restart | The round flow builds results, calls result hooks, and clears round entities when returning to the lobby. A host can supply a `CampaignPersistence` adapter if it needs to carry confirmed results between rounds. |

[Tick order](architecture.md#simulation-ticks) · [Timing tests](../../backend/crates/crucible-loop/tests/timing.rs) · [Lobby tests](../../backend/crates/crucible-loop/tests/lobby.rs) · [Round flow tests](../../backend/crates/crucible-loop/tests/round_phases.rs)

## Modules and extension points

`crucible-ext` assembles an application from Rust modules and one active extension.

| Capability | What it provides |
| --- | --- |
| Activation and dependencies | The application compiles modules into its host and selects an extension at startup. Its manifest names required modules, a compatible core version, and any data roots. The launcher resolves dependencies before registering behavior. |
| Registration | Modules and extensions can register components, systems, resources, prototype types, phase hooks, network event names, round rules, and console commands. |
| Policies | Extension points let a host configure map selection, spawning, void behavior, round ending, rule selection, event scheduling, and round hooks. |
| Startup validation | Discovery, resolution, registration, data loading, and validation happen before the world is ready. The host can validate data separately through `Launcher::validate`. |

[Extend the engine](extensions.md) · [Launcher and module tests](../../backend/crates/crucible-ext/tests/extensions.rs)

## Effects and conditions

`crucible-effects` makes ordered conditions and effects configurable in prototype data.

| Capability | What it provides |
| --- | --- |
| Typed registration | Applications register condition and effect types with schemas and decoders. The prototype loader validates the `!type:` entries and their fields before the runtime prepares an effect block. |
| Execution | A block checks conditions in declaration order, stops at the first false condition, and applies effects in order only when all conditions pass. Each effect sees the changes made by earlier effects. |
| Target lifetime | The block checks the target before evaluating conditions and before each effect. If the target becomes unavailable, effects already applied retain their changes. |

[Effect registration and decoding](../../backend/crates/crucible-effects/src/registry.rs) · [Effect execution](../../backend/crates/crucible-effects/src/execute.rs)

## Player sessions and profiles

`crucible-players` manages player identity, connections, and control within the simulation.

| Capability | What it provides |
| --- | --- |
| Session state | The host supplies a stable user identifier and connection identity. The session transitions through connecting, connected, in game, and disconnected, with events for status changes. |
| Control and reconnect | A session controls at most one entity, and an entity belongs to at most one controlling session. Attach and detach operations raise events. A reconnect can restore control of the existing entity, and a replacement login can take over an old connection. |
| Persistent profiles | A profile records characters, role preferences, and settings. The provided SQLite store persists this data across restarts. |

[Session tests](../../backend/crates/crucible-players/tests/sessions.rs) · [Profile tests](../../backend/crates/crucible-players/tests/profiles.rs)

## Replication and player input

`crucible-replication` gives a host the state and events to deliver to each player. The host's protocol adapter encodes these values and handles transport.

| Capability | What it provides |
| --- | --- |
| Per-player visibility | The core selects nearby entities on the same map and accounts for always-visible entities, controlled entities, equipment, ancestors, and closed-container rules. Component policies control which data is server-only, owner-only, or visible to all viewers of an entity. |
| Entry budgets and frame deltas | Configurable budgets spread visibility entries across ticks. The core sends full state on first contact or resynchronization, then builds deltas from the last acknowledged tick. Frames include component removals, entity deletions, and visibility exits. Updates can be marked for reliable delivery when acknowledgments lag. |
| Tick-based input | The input buffer orders commands and events by tick and sequence, delays future input, and handles late input on the current tick. The processed sequence is included in player state. Connection and control checks prevent previously bound commands from following a rebind. |
| Ordered events | The host can queue events for one player, viewers of an entity, or all players. Events preserve order for each recipient and can carry a simulation tick. |

[Visibility tests](../../backend/crates/crucible-replication/tests/visibility.rs) · [Frame tests](../../backend/crates/crucible-replication/tests/frames.rs) · [Input phase tests](../../backend/crates/crucible-replication/tests/module.rs) · [Event tests](../../backend/crates/crucible-replication/tests/events.rs)

## Test support

`crucible-testkit` starts an in-process host and drives it with a manual clock. Virtual clients apply frames, send input, and simulate delayed or missing acknowledgments. The examples exercise lobby joins, movement between virtual clients, and full-state recovery.

[Virtual-client tests](../../backend/crates/crucible-testkit/tests/virtual_client.rs) · [Lobby tests](../../backend/crates/crucible-testkit/tests/lobby.rs) · [Build a headless host](running.md)
