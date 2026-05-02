# claw-branch Architecture

This document describes the high-level design of claw-branch, a Rust library for isolated SQLite branch workflows.

## Goals

- Give agents safe, isolated branch environments for experimentation.
- Support deterministic diff/merge/commit flows between branches.
- Preserve lineage with strict DAG invariants.
- Keep branch operations fast through file-based snapshots.

## System layout

```text
                         +-------------------+
                         |   BranchEngine    |
                         |  (public facade)  |
                         +----+---+---+---+--+
                              |   |   |   |
               +--------------+   |   |   +----------------+
               |                  |   |                    |
       +-------v------+   +-------v---v------+   +--------v--------+
       | BranchStore  |   | DAG + Traversal  |   | Snapshot System |
       | registry DB  |   | lineage + checks |   | copy/verify/gc  |
       +-------+------+   +-------+----------+   +--------+--------+
               |                  |                       |
               |                  |                       |
        +------v-------+   +------v-------+      +--------v--------+
        | Lifecycle    |   | Diff / Merge |      | Sandbox / Eval  |
        | fork/archive |   | 3-way apply  |      | run + recommend |
        +--------------+   +--------------+      +-----------------+

Each branch is a separate SQLite file under branches_dir.
Registry metadata is stored in registry_db_path.
```

## Core modules

- engine: Unified API entry point for branch operations.
- branch/store: Registry persistence, lookups, and commit counters.
- branch/lifecycle: Branch state transitions and snapshot lifecycle.
- snapshot/copier, snapshot/verifier, snapshot/gc: Copy, integrity verification, and cleanup.
- dag/graph and dag/traversal: Branch lineage graph and ancestry/path operations.
- diff/extractor and diff/scorer: Entity and field-level change detection.
- merge/three_way and merge/resolver: Conflict detection and merge strategies.
- commit/selective: Cherry-pick and full commit to trunk.
- sandbox/environment, sandbox/runner, sandbox/evaluator: Isolated simulation loop.
- metrics/tracker and metrics/reporter: Branch-level metrics and workspace reports.

## Data model and storage

Two storage layers are used:

- Registry database: tracks branch metadata (id, status, parent, timestamps, db_path).
- Branch databases: one SQLite file per branch with copied ClawDB data tables.

Important tables used by branch logic:

- memory_records
- sessions
- tool_outputs

## Branch lifecycle

1. Create/open workspace with BranchEngine.
2. Ensure trunk exists (root of DAG).
3. Fork parent branch to create isolated child snapshot.
4. Run changes directly in child DB (or via simulation sandbox).
5. Diff child vs target.
6. Merge or selective commit.
7. Archive/discard branches as needed.
8. GC prunes orphan/discarded snapshots after threshold.

## Merge flow

Three-way merge uses:

- base branch: source parent when available, otherwise target.
- ours/theirs/union/field-level/manual strategy.
- per-entity and per-field conflict capture.
- atomic write transaction in target branch.

Outputs include applied/skipped/conflicts/success and duration.

## Sandbox flow

Simulation creates a temporary branch and executes an agent closure with a SQLite pool.

- runner: executes with optional timeout.
- evaluator: computes diff + metrics and returns recommendation.
- recommendation:
  - Commit when changes are low risk.
  - Discard when no meaningful changes were produced.
  - NeedsReview when divergence exceeds threshold or conflicts exist.

## DAG invariants

- Trunk is the only root branch.
- Parent links always point to existing branches.
- No cycle is allowed when adding edges.
- Traversal operations (topological sort, LCA, subtree, leaves) must operate on an acyclic graph.

## Snapshot layout

```text
<branches_dir>/
  branch_registry.db
  <branch_uuid_1>.db
  <branch_uuid_2>.db
  ...
```

Snapshot integrity uses BLAKE3 checksums during copy/restore and optional verification during GC passes.

## Metrics and divergence

Branch metrics track:

- entity counts by table.
- operation counters (insert/update/delete).
- file size and commit counts.
- divergence score against parent branch.

Divergence score is normalized to [0, 1] and can be decay-weighted by age.

## Concurrency model

- Async APIs are backed by tokio.
- Graph state uses parking_lot RwLock.
- Registry and branch DB access use SQLx pools.
- SQLite WAL mode is used in tests/benchmarks to improve parallel read/write behavior.

## Failure model

BranchError variants cover:

- configuration/validation failures.
- branch existence and state errors.
- merge and sandbox errors.
- SQL and I/O failures.

All public APIs return BranchResult and avoid panic-prone unwrap/expect in library code.

## Extensibility points

- Add table support by extending EntityType and diff/commit mapping.
- Add merge policies via MergeStrategy and resolver logic.
- Add custom evaluation signals in sandbox evaluator.
- Add custom report sinks on top of WorkspaceReport and BranchMetrics.
