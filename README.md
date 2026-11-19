# Crucible Core

<img src="docs/assets/crucible-core-icon.png" alt="Crucible Core icon" width="88" height="88" align="right">

Crucible Core is a Rust library workspace for building multiplayer simulations. It provides an entity-component-system world, typed data prototypes, tile maps, 2D physics, and a fixed-tick simulation loop.

![A layered tile grid with three entity markers and traces between them](docs/assets/crucible-core-cover.jpg)

## On this page

- [Capabilities](#capabilities)
- [Quick start](#quick-start)
- [Documentation](#documentation)
- [Development process](#development-process)
- [License](#license)

## Capabilities

| Area | Libraries |
| --- | --- |
| World and data | `crucible-ecs`, `crucible-ecs-macros`, `crucible-proto`, `crucible-loc` |
| Simulation | `crucible-map`, `crucible-physics`, `crucible-loop`, `crucible-effects` |
| Integration | `crucible-ext`, `crucible-players`, `crucible-replication` |
| Test support | `crucible-testkit` |

## Quick start

Use Rust 1.100.0 and a C compiler and linker on Linux. Rustup selects the pinned toolchain from `backend/rust-toolchain.toml`.

```sh
cd backend
cargo test --workspace --locked
```

## Documentation

- [User guide](docs/en/user-guide.md) covers the toolchain, builds and API documentation.
- [Architecture](docs/en/architecture.md) explains the workspace and how data moves through a simulation tick.
- [Simulation loop](docs/en/simulation-loop.md) traces the work done inside a tick and the data passed to the host.
- [Simulation metrics](docs/en/metrics.md) documents counters, tick timings, and integration with host monitoring.
- [Administrator client](docs/en/admin.md) shows how to control a host, inspect logs, and read tick history and mechanic statistics.
- [Core capabilities](docs/en/mechanics.md) lists the implemented systems with release source and test references.
- [Extend the engine](docs/en/extensions.md) shows how to register and connect a module in a minimal Rust host.
- [Run Crucible Core](docs/en/running.md) explains the UI boundary and Linux background service setup.
- [Worked example](docs/en/examples/load-a-map.md) follows the map-loading scenario from input to assertions.
- [Development and releases](docs/en/development.md) covers OpenSpec, SkillForge and the SDLC release stages.
- [Documentation index](docs/en/README.md) links the full set of guides and reference material.

## Development process

The core was developed with [OpenSpec](https://github.com/Fission-AI/OpenSpec) for specifications and SkillForge for project and agent skills. Release preparation follows the [SDLC project's](https://github.com/marin-ai-tech/sdlc) practices for verifying a fixed source snapshot, independent review and owner approval. This is a retrospective demonstration of the workflow using the existing implementation.

See the [contribution guide](CONTRIBUTING.md), [versioning policy](VERSIONING.md), and [changelog](CHANGELOG.md).

## License

Crucible Core is available under either the [MIT license](LICENSE-MIT) or the [Apache License 2.0](LICENSE-APACHE). Source files adapted from other projects carry their original notices. See [third-party notices](backend/THIRD_PARTY_NOTICES.md).
