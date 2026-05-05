# claw-branch

[![License: Apache-2.0](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)
[![Docs](https://img.shields.io/docsrs/claw-branch)](https://docs.rs/claw-branch)
[![Crates.io](https://img.shields.io/crates/v/claw-branch)](https://crates.io/crates/claw-branch)

`claw-branch` is the branch orchestration engine for ClawDB-style workflows.

It provides isolated, SQLite-backed branch execution for agent and application workloads where you need to fork a trusted baseline, run experiments safely, inspect exact diffs, and merge with explicit conflict policy.

## Why this crate exists

Traditional branch workflows in application databases are hard to make deterministic, inspectable, and safe for concurrent agents. `claw-branch` solves this with:

- File-level branch isolation (one SQLite DB file per branch).
- Explicit lineage and merge-base tracking via DAG.
- Three-way merge and selective commit primitives.
- Sandboxed simulation with recommendation output.
- Built-in metrics and divergence reporting.
- Snapshot verification and garbage-collection tooling.

## Installation

```toml
[dependencies]
claw-branch = "0.1.2"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

Optional guarded mode (integrates with `claw-guard` policy checks):

```toml
[dependencies]
claw-branch = { version = "0.1.2", features = ["guarded"] }
```

Production recommendation: use `GuardedBranchEngine` for built-in session and policy checks.
`BranchEngine` remains the bare engine for environments that already enforce auth externally.

## Feature flags

| Feature | Default | Description |
|---|---|---|
| guarded | no | Enables `GuardedBranchEngine` integration with `claw-guard` |

## Architecture overview

```text
┌─────────────────────────────────────────────────────────────────┐
│                         BranchEngine                           │
│   ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌─────────────┐    │
│   │ Lifecycle│  │  Commit  │  │  Merge   │  │   Sandbox   │    │
│   │ fork/    │  │ cherry-  │  │ 3-way +  │  │ run/eval    │    │
│   │ archive  │  │ pick/all │  │ resolver │  │ recommendation│   │
│   └────┬─────┘  └────┬─────┘  └────┬─────┘  └──────┬──────┘    │
│        │              │              │                │           │
│   ┌────▼──────────────▼──────────────▼────────────────▼──────┐   │
│   │                  BranchStore (registry DB)                │   │
│   └───────────────────────────────────────────────────────────┘   │
│                                                                   │
│   ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌─────────────┐      │
│   │   DAG    │  │   Diff   │  │ Metrics  │  │  Snapshot   │      │
│   │ lineage  │  │ entity + │  │ tracker/ │  │ copy/verify │      │
│   │ + LCA    │  │ field    │  │ reporter │  │ + gc        │      │
│   └──────────┘  └──────────┘  └──────────┘  └─────────────┘      │
└─────────────────────────────────────────────────────────────────┘

Each branch is a separate SQLite file.
The trunk is the canonical root branch and is never discarded.
```

## Quick start

```rust
use claw_branch::prelude::*;
use std::path::PathBuf;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = BranchConfig::builder()
        .workspace_id(uuid::Uuid::new_v4())
        .branches_dir(PathBuf::from("/tmp/demo/branches"))
        .build()?;

    let engine = BranchEngine::new(config, std::path::Path::new("/data/source.db")).await?;
    let feature = engine.fork_trunk("feature/summariser").await?;

    let trunk = engine.trunk().await?;
    let diff = engine.diff(trunk.id, feature.id).await?;
    println!(
        "changed: +{} ~{} -{}",
        diff.stats.added, diff.stats.modified, diff.stats.removed
    );

    let report = engine
        .simulate(
            feature.id,
            SimulationScenario {
                name: "trial-run".into(),
                description: "evaluate summarisation strategy".into(),
                max_ops: Some(100),
                timeout_secs: Some(30),
                seed_data: None,
            },
            |pool| async move {
                let _ = pool;
                Ok(serde_json::json!({"status": "done", "summaries_generated": 42}))
            },
        )
        .await?;

    match report.recommendation {
        Recommendation::Commit => engine.commit_to_trunk(feature.id).await?,
        Recommendation::Discard => engine.discard(feature.id).await?,
        Recommendation::NeedsReview(notes) => {
            println!("manual review required: {notes:?}");
        }
    }

    Ok(())
}
```

## Core concepts

| Concept | Description |
|---|---|
| Branch | Isolated SQLite snapshot with lifecycle state |
| Trunk | Canonical baseline branch |
| Fork | Snapshot copy of a parent branch |
| Diff | Entity-level and field-level comparison |
| Merge | Three-way merge with strategy-based conflict handling |
| Commit | Selective or full promotion to a target branch |
| Sandbox | Temporary evaluation environment with recommendation output |
| DAG | Branch lineage graph with cycle checks |
| Metrics | Branch counters, size, and divergence signals |
| GC | Snapshot cleanup for discarded/orphan branches |

## Public module map

| Module | Purpose |
|---|---|
| `branch` | Branch model, naming, lifecycle, and persistence |
| `commit` | Full and selective commit flows including cherry-pick |
| `dag` | Lineage graph, traversal, merge-base, and serialization |
| `diff` | Diff extraction, scoring, and formatting |
| `merge` | Three-way merge, strategies, and conflict resolution |
| `metrics` | Divergence tracking and workspace reporting |
| `sandbox` | Simulation environment and evaluation routines |
| `snapshot` | Snapshot copy, integrity sidecar, and cleanup |

## BranchConfig reference

| Field | Default | Description |
|---|---|---|
| workspace_id | required | Unique workspace UUID |
| branches_dir | required in builder | Directory containing branch SQLite files |
| registry_db_path | branches_dir/branch_registry.db | Registry SQLite path |
| trunk_branch_name | trunk | Canonical branch name |
| divergence_threshold | 0.50 | Threshold used by sandbox recommendation |
| max_branches_per_workspace | 100 | Hard cap on active branches |
| gc_orphan_threshold_secs | 86400 | Orphan age cutoff for GC |
| auto_metrics | true | Auto refresh metrics on lifecycle operations |

## Merge strategies

| Strategy | Behavior |
|---|---|
| `MergeStrategy::Ours` | Prefer source values on conflict |
| `MergeStrategy::Theirs` | Prefer target values on conflict |
| `MergeStrategy::Union` | Union JSON-like structures where possible |
| `MergeStrategy::FieldLevel(map)` | Per-field strategy overrides |
| `MergeStrategy::Manual` | Preserve conflicts for explicit review |

## Simulation lifecycle

1. Fork a temporary simulation branch from a parent.
2. Run agent logic against the simulation SQLite pool.
3. Compute diff and metrics.
4. Produce `Recommendation::Commit`, `Recommendation::Discard`, or `Recommendation::NeedsReview`.
5. Apply workflow policy (promote/discard/review).

## Error model

Most operations return `Result<T, claw_branch::error::BranchError>` and are designed to fail explicitly on:

- Invalid branch names and lifecycle transitions.
- Snapshot or sidecar integrity violations.
- DAG cycle or invalid ancestry operations.
- SQLite IO and migration failures.
- Merge conflicts requiring manual strategy.

## Safety and invariants

- No `unwrap`/`expect` in library source (`clippy::unwrap_used` denied).
- Public API docs required (`missing_docs` denied).
- SQLite isolation by branch file to avoid cross-branch mutable overlap.
- DAG cycle prevention before edge insertion.
- Atomic merge and commit transactions.
- Snapshot integrity checks via BLAKE3 hashes.

## Performance targets (design goals)

These targets are validated by benchmark suites in `benches/branch_bench.rs`.

| Operation | Target |
|---|---|
| Fork 1k entities | < 50ms |
| Diff 10k entities (10% modified) | < 200ms |
| Merge 100 non-conflicting entities | < 100ms |
| Snapshot verify (10MB) | < 20ms |

## Compatibility

- Rust edition: 2021
- MSRV: 1.75 (see `Cargo.toml`)
- Runtime: Tokio
- Storage backend: SQLite via SQLx

## Development and CI commands

```bash
cargo build --all-features
cargo test --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
cargo deny check
cargo audit
cargo bench --no-run
cargo bench -- --test
```

## Release process

1. Update `Cargo.toml` version and `CHANGELOG.md`.
2. Ensure CI passes on `main`.
3. Create and push a version tag (`vX.Y.Z`).
4. `publish.yml` verifies the tag/version match, publishes to crates.io, and creates a GitHub release.
