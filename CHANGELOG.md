# Changelog

## v0.1.1 - 2026-05-02

- Bumped dependencies and dev-dependencies to latest compatible versions.
- Refreshed lockfile for current compatible transitive dependencies.
- Expanded crate documentation in README with configuration, module map, and operational guidance.
- Updated dependency policy configuration to match current `cargo-deny` schema.
- Added `cargo-audit` ignore policy for currently-unfixed upstream advisories.

## v0.1.0 - 2026-05-02

- Initial release of the ClawDB branch engine crate.
- SQLite-backed branch lifecycle (fork, merge, discard, simulation).
- DAG lineage tracking and branch metrics.
- Snapshot integrity verification with BLAKE3.
