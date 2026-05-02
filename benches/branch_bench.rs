//! Criterion benchmarks for core claw-branch operations.
//!
//! Performance targets (reference machine: M2 Pro, 32 GB):
//! - Fork 1k entities:              < 50ms
//! - Diff 10k entities (10% mod):   < 200ms
//! - Merge 100 non-conflicting:     < 100ms
//! - Snapshot verify 10MB:          < 20ms

use criterion::{criterion_group, criterion_main, Criterion};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use tempfile::TempDir;
use uuid::Uuid;

use claw_branch::{BranchConfig, BranchEngine, MergeStrategy};

// ── Helpers ───────────────────────────────────────────────────────────────────

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
}

async fn seed_db(path: &std::path::Path, count: usize) {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(path)
                .create_if_missing(true)
                .journal_mode(SqliteJournalMode::Wal),
        )
        .await
        .expect("pool");

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS memory_records (
            id TEXT PRIMARY KEY, content TEXT, metadata TEXT,
            created_at TEXT, updated_at TEXT
        )",
    )
    .execute(&pool)
    .await
    .expect("create table");

    sqlx::query("CREATE TABLE IF NOT EXISTS sessions (id TEXT PRIMARY KEY, name TEXT, metadata TEXT, created_at TEXT, updated_at TEXT)")
        .execute(&pool).await.expect("sessions");
    sqlx::query("CREATE TABLE IF NOT EXISTS tool_outputs (id TEXT PRIMARY KEY, tool_name TEXT, output TEXT, metadata TEXT, created_at TEXT, updated_at TEXT)")
        .execute(&pool).await.expect("tool_outputs");

    for i in 0..count {
        let id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO memory_records (id, content, metadata, created_at, updated_at)
             VALUES (?, ?, ?, datetime('now'), datetime('now'))",
        )
        .bind(&id)
        .bind(format!("content_{i}"))
        .bind(format!("{{\"i\":{i}}}"))
        .execute(&pool)
        .await
        .expect("insert");
    }
    pool.close().await;
}

async fn setup_engine(entity_count: usize) -> (BranchEngine, TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let trunk_db = dir.path().join("source.db");
    seed_db(&trunk_db, entity_count).await;
    let config = BranchConfig::builder()
        .workspace_id(Uuid::new_v4())
        .branches_dir(dir.path().join("branches"))
        .build()
        .expect("config");
    let engine = BranchEngine::new(config, &trunk_db).await.expect("engine");
    (engine, dir)
}

// ── Fork ──────────────────────────────────────────────────────────────────────

fn bench_fork_empty_db(c: &mut Criterion) {
    let rt = rt();
    let (engine, _dir) = rt.block_on(setup_engine(0));
    c.bench_function("fork/empty_db", |b| {
        b.iter(|| {
            rt.block_on(async {
                let trunk = engine.trunk().await.expect("trunk");
                engine.fork(trunk.id, &format!("b-{}", Uuid::new_v4()), None).await.expect("fork");
            })
        })
    });
}

fn bench_fork_1k_entities(c: &mut Criterion) {
    let rt = rt();
    let (engine, _dir) = rt.block_on(setup_engine(1_000));
    c.bench_function("fork/1k_entities", |b| {
        b.iter(|| {
            rt.block_on(async {
                let trunk = engine.trunk().await.expect("trunk");
                engine.fork(trunk.id, &format!("b-{}", Uuid::new_v4()), None).await.expect("fork");
            })
        })
    });
}

fn bench_fork_10k_entities(c: &mut Criterion) {
    let rt = rt();
    let (engine, _dir) = rt.block_on(setup_engine(10_000));
    c.bench_function("fork/10k_entities", |b| {
        b.iter(|| {
            rt.block_on(async {
                let trunk = engine.trunk().await.expect("trunk");
                engine.fork(trunk.id, &format!("b-{}", Uuid::new_v4()), None).await.expect("fork");
            })
        })
    });
}

// ── Diff ──────────────────────────────────────────────────────────────────────

fn bench_diff_identical(c: &mut Criterion) {
    let rt = rt();
    let (engine, _dir) = rt.block_on(async {
        let e = setup_engine(1_000).await;
        let trunk = e.0.trunk().await.expect("trunk");
        e.0.fork(trunk.id, "diff-target", None).await.expect("fork");
        e
    });
    c.bench_function("diff/identical_1k", |b| {
        b.iter(|| {
            rt.block_on(async {
                let branches = engine.list(None).await.expect("list");
                engine.diff(branches[0].id, branches[1].id).await.expect("diff");
            })
        })
    });
}

fn bench_diff_10pct_modified(c: &mut Criterion) {
    let rt = rt();
    let (engine, _dir) = rt.block_on(async {
        let e = setup_engine(1_000).await;
        let trunk = e.0.trunk().await.expect("trunk");
        let branch = e.0.fork(trunk.id, "diff-modified", None).await.expect("fork");
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(&branch.db_path)
                    .create_if_missing(false)
                    .journal_mode(SqliteJournalMode::Wal),
            )
            .await
            .expect("pool");
        sqlx::query("UPDATE memory_records SET content = 'mod' WHERE rowid % 10 = 0")
            .execute(&pool).await.expect("update");
        pool.close().await;
        e
    });
    c.bench_function("diff/10pct_modified_1k", |b| {
        b.iter(|| {
            rt.block_on(async {
                let branches = engine.list(None).await.expect("list");
                engine.diff(branches[0].id, branches[1].id).await.expect("diff");
            })
        })
    });
}

fn bench_diff_100pct_diverged(c: &mut Criterion) {
    let rt = rt();
    let (engine, _dir) = rt.block_on(async {
        let e = setup_engine(500).await;
        let trunk = e.0.trunk().await.expect("trunk");
        let branch = e.0.fork(trunk.id, "diff-all-mod", None).await.expect("fork");
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(&branch.db_path)
                    .create_if_missing(false)
                    .journal_mode(SqliteJournalMode::Wal),
            )
            .await
            .expect("pool");
        sqlx::query("UPDATE memory_records SET content = 'all_mod'")
            .execute(&pool).await.expect("update");
        pool.close().await;
        e
    });
    c.bench_function("diff/100pct_diverged_500", |b| {
        b.iter(|| {
            rt.block_on(async {
                let branches = engine.list(None).await.expect("list");
                engine.diff(branches[0].id, branches[1].id).await.expect("diff");
            })
        })
    });
}

// ── Merge & Commit ────────────────────────────────────────────────────────────

fn bench_merge_no_conflicts(c: &mut Criterion) {
    let rt = rt();
    let (engine, _dir) = rt.block_on(async {
        let e = setup_engine(100).await;
        let trunk = e.0.trunk().await.expect("trunk");
        let branch = e.0.fork(trunk.id, "merge-clean", None).await.expect("fork");
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(&branch.db_path)
                    .create_if_missing(false)
                    .journal_mode(SqliteJournalMode::Wal),
            )
            .await
            .expect("pool");
        for i in 0..20 {
            sqlx::query(
                "INSERT INTO memory_records (id, content, metadata, created_at, updated_at)
                 VALUES (?, ?, '{}', datetime('now'), datetime('now'))",
            )
            .bind(Uuid::new_v4().to_string())
            .bind(format!("new_{i}"))
            .execute(&pool).await.expect("insert");
        }
        pool.close().await;
        e
    });
    c.bench_function("merge/no_conflicts_100", |b| {
        b.iter(|| {
            rt.block_on(async {
                let branches = engine.list(None).await.expect("list");
                engine.merge(branches[1].id, branches[0].id, MergeStrategy::Theirs).await.expect("merge");
            })
        })
    });
}

fn bench_merge_conflicts(c: &mut Criterion) {
    let rt = rt();
    let (engine, _dir) = rt.block_on(async {
        let e = setup_engine(100).await;
        let trunk = e.0.trunk().await.expect("trunk");
        let branch = e.0.fork(trunk.id, "merge-conflict", None).await.expect("fork");
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(&branch.db_path)
                    .create_if_missing(false)
                    .journal_mode(SqliteJournalMode::Wal),
            )
            .await
            .expect("pool");
        sqlx::query("UPDATE memory_records SET content = 'conflict_val'")
            .execute(&pool).await.expect("update");
        pool.close().await;
        e
    });
    c.bench_function("merge/conflicts_100", |b| {
        b.iter(|| {
            rt.block_on(async {
                let branches = engine.list(None).await.expect("list");
                engine.merge(branches[1].id, branches[0].id, MergeStrategy::Ours).await.expect("merge");
            })
        })
    });
}

fn bench_selective_commit_100(c: &mut Criterion) {
    let rt = rt();
    let (engine, _dir) = rt.block_on(setup_engine(100));
    c.bench_function("commit/all_100_entities", |b| {
        b.iter(|| {
            rt.block_on(async {
                let trunk = engine.trunk().await.expect("trunk");
                let branch = engine.fork(trunk.id, &format!("c-{}", Uuid::new_v4()), None)
                    .await.expect("fork");
                engine.commit_to_trunk(branch.id).await.expect("commit");
            })
        })
    });
}

// ── Infrastructure ────────────────────────────────────────────────────────────

fn bench_snapshot_verify(c: &mut Criterion) {
    let rt = rt();
    let (engine, _dir) = rt.block_on(setup_engine(500));
    c.bench_function("snapshot/verify_via_gc", |b| {
        b.iter(|| {
            rt.block_on(async {
                engine.gc().await.expect("gc");
            })
        })
    });
}

fn bench_dag_lca_depth_10(c: &mut Criterion) {
    let rt = rt();
    let (engine, _dir) = rt.block_on(async {
        let e = setup_engine(5).await;
        let mut parent_id = e.0.trunk().await.expect("trunk").id;
        for d in 0..10 {
            let b = e.0.fork(parent_id, &format!("depth-{d}"), None).await.expect("fork");
            parent_id = b.id;
        }
        e
    });
    c.bench_function("dag/lineage_depth_10", |b| {
        b.iter(|| {
            rt.block_on(async {
                let branches = engine.list(None).await.expect("list");
                let leaf = branches.last().expect("leaf").id;
                engine.lineage(leaf).await.expect("lineage");
            })
        })
    });
}

fn bench_metrics_refresh(c: &mut Criterion) {
    let rt = rt();
    let (engine, _dir) = rt.block_on(setup_engine(200));
    c.bench_function("metrics/refresh_trunk", |b| {
        b.iter(|| {
            rt.block_on(async {
                let trunk = engine.trunk().await.expect("trunk");
                engine.metrics(trunk.id).await.expect("metrics");
            })
        })
    });
}

fn bench_gc_scan_100_branches(c: &mut Criterion) {
    let rt = rt();
    let (engine, _dir) = rt.block_on(async {
        let e = setup_engine(5).await;
        let trunk_id = e.0.trunk().await.expect("trunk").id;
        for i in 0..20 {
            let b = e.0.fork(trunk_id, &format!("gc-{i}"), None).await.expect("fork");
            e.0.discard(b.id).await.expect("discard");
        }
        e
    });
    c.bench_function("gc/scan_20_discarded", |b| {
        b.iter(|| {
            rt.block_on(async {
                engine.gc().await.expect("gc");
            })
        })
    });
}

// ── Groups ────────────────────────────────────────────────────────────────────

criterion_group!(fork_benches, bench_fork_empty_db, bench_fork_1k_entities, bench_fork_10k_entities);
criterion_group!(diff_benches, bench_diff_identical, bench_diff_10pct_modified, bench_diff_100pct_diverged);
criterion_group!(merge_benches, bench_merge_no_conflicts, bench_merge_conflicts, bench_selective_commit_100);
criterion_group!(infra_benches, bench_snapshot_verify, bench_dag_lca_depth_10, bench_metrics_refresh, bench_gc_scan_100_branches);
criterion_main!(fork_benches, diff_benches, merge_benches, infra_benches);