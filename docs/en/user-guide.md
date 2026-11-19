# User guide

## Requirements

The source release is verified on Linux with Rust 1.100.0. Install a C compiler and linker, and use Rustup to select the toolchain in `backend/rust-toolchain.toml`. Run Cargo commands from the workspace directory.

```sh
cd backend
cargo build --workspace --locked
```

## Checks

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked --no-fail-fast
cargo deny check
```

Install [cargo-deny](https://github.com/EmbarkStudios/cargo-deny) to run the last command. It checks the dependency policies in `backend/deny.toml`. The test suite includes self-contained fixtures and recorded physics trajectories.

## API documentation

```sh
cargo doc --workspace --no-deps --locked
```

Open `target/doc/crucible_ecs/index.html` under the workspace directory to start with entity and component APIs. The [architecture guide](architecture.md) lists the other crates and their responsibilities. The [map-loading example](examples/load-a-map.md) shows how the libraries work together.

Use the [extension walkthrough](extensions.md) to attach a module and run a headless host. The [running guide](running.md) explains where the UI lives and how to keep the host running as a service.
