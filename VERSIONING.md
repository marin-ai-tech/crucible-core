# Versioning

Crucible Core uses [Semantic Versioning 2.0.0](https://semver.org/). The workspace libraries share one version. A GitHub release tag is `v` followed by the workspace version in `backend/Cargo.toml`, for example `v0.1.0`.

## Before 1.0

- `0.MINOR.PATCH` is the initial development series. Raise `MINOR` for new APIs or breaking changes and reset `PATCH` to zero.
- Raise `PATCH` for backward-compatible fixes within the same minor series.
- Use `-alpha.N`, `-beta.N`, or `-rc.N` when a public prerelease is useful. Review the `core` ranges used by examples and test manifests for each minor version or prerelease. Verify the exact version with Cargo.

Compatibility covers the documented Rust APIs of the released libraries and the Cargo features that expose them. A serialized format enters this contract when the release documentation explicitly names it. A higher minimum supported Rust version and removal of a supported target require a breaking-change bump.

## From 1.0

Declare the stable API before tagging `v1.0.0`. Raise the major version for incompatible changes, the minor version for backward-compatible features and deprecations, and the patch version for backward-compatible fixes. The [Cargo compatibility guide](https://doc.rust-lang.org/cargo/reference/semver.html) informs Rust API classification.

## Release records

Each release has a changelog entry, a lockfile, and a tag on the reviewed source commit. Document each breaking change with migration guidance. Corrections use a new version and tag. The first distribution channel is GitHub source releases.
