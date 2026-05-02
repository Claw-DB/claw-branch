# claw-branch

The fork/simulate/merge engine for ClawDB.

`claw-branch` is a Rust library for isolated, SQLite-backed branch workflows. It allows agents and applications to fork from a canonical trunk, experiment safely in isolated branch databases, and merge or commit changes back with explicit diff and conflict semantics.

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

## Quick-start

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
        Recommendation::Commit => {
            engine.commit_to_trunk(feature.id).await?;
        }
        Recommendation::Discard => {
            engine.discard(feature.id).await?;
        }
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
| MergeStrategy::Ours | Prefer source values on conflict |
| MergeStrategy::Theirs | Prefer target values on conflict |
| MergeStrategy::Union | Union JSON-like structures where possible |
| MergeStrategy::FieldLevel(map) | Per-field strategy overrides |
| MergeStrategy::Manual | Preserve conflicts for explicit review |

## Simulation flow

1. Fork a temporary simulation branch from a parent.
2. Run agent logic against the simulation SQLite pool.
3. Compute diff and metrics.
4. Produce Recommendation::Commit, Recommendation::Discard, or Recommendation::NeedsReview.
5. Apply workflow policy (promote/discard/review).

## Performance targets

Measured goals on a modern laptop class machine:

| Operation | Target |
|---|---|
| Fork 1k entities | < 50ms |
| Diff 10k entities (10% modified) | < 200ms |
| Merge 100 non-conflicting entities | < 100ms |
| Snapshot verify (10MB) | < 20ms |

## Safety guarantees

- No unwrap/expect in library source, enforced with clippy::unwrap_used deny.
- No missing public docs, enforced with missing_docs deny.
- SQLite isolation by branch file, avoiding shared mutable table state.
- DAG cycle prevention before branch edge insertion.
- Atomic merge transactions.
- Snapshot integrity checks using BLAKE3.

## Development commands

```bash
cargo check --tests
cargo check --benches
cargo test
cargo bench
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
```
