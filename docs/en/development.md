# Development and releases

## From a requirement to evidence

Crucible used [OpenSpec](https://github.com/Fission-AI/OpenSpec) to record proposed behavior, design decisions, implementation tasks, and acceptance scenarios. Project and agent skills came from SkillForge. Integration tests and fixture-based scenarios capture concrete behavior. The [map-loading example](examples/load-a-map.md) traces one scenario through its inputs and assertions.

## Retrospective SDLC demonstration

Release preparation applies practices from the [SDLC project](https://github.com/marin-ai-tech/sdlc/blob/master/docs/en/05-guide.md#53-lifecycle-of-a-change) to the existing implementation.

1. **Define the release.** Select the workspace version, document changes and compatibility, and identify the exact source tree.
2. **Verify.** Run formatting, lint, test, dependency policy, and source-provenance checks. Associate the evidence with that tree.
3. **Review.** Independently inspect the release contents and the verification evidence. Repeat affected checks when the tree changes.
4. **Approve and release.** The owner approves the reviewed tree. The GitHub tag matches the workspace version and points to the approved public commit on `main`. Only repository administrators can create release tags. Pushing the tag starts the [release workflow](../../.github/workflows/release.yml), which repeats the formatting, lint, test and dependency policy checks and attaches the source archive and its SHA-256 checksum to the GitHub release.
5. **Confirm.** Compare the published tag, tree, and downloadable archive with the approved release. Ship any correction under a new version.

See [Versioning](../../VERSIONING.md) for tag and compatibility rules and [Changelog](../../CHANGELOG.md) for release notes.
