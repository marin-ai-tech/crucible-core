# Crucible Core documentation

## Start here

- [User guide](user-guide.md) explains how to build the workspace, run tests, and read the Rust API documentation.
- [Glossary](glossary.md) defines the terms used in the guides.

## Guides by topic

- [Architecture](architecture.md) describes the world, data, simulation loop, and state replication.
- [Simulation loop](simulation-loop.md) explains what each tick phase computes and when the host processes network work.
- [Simulation metrics](metrics.md) shows how to read loop counters and measure tick duration.
- [Administrator client](admin.md) explains process control, logs, and historical telemetry.
- [Core capabilities](mechanics.md) lists the implemented systems and points to their source and tests.
- [Extend the engine](extensions.md) shows how to write, register, and select a small extension and a core module.
- [Run Crucible Core](running.md) explains the client boundary and shows how to run the demo host as a background service.
- [Development and releases](development.md) explains how OpenSpec and SkillForge were used and how the release follows SDLC practices.
- [Contributing](../../CONTRIBUTING.md) covers proposals, fixes, language, and attribution.

## Worked examples

- [Load a tile map](examples/load-a-map.md) traces a real integration test from prototype and map input to ECS state.

## Release reference

- [Versioning](../../VERSIONING.md)
- [Changelog](../../CHANGELOG.md)
- [Third-party notices](../../backend/THIRD_PARTY_NOTICES.md)
