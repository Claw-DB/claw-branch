use std::collections::HashSet;

use claw_branch::{BranchConfig, BranchEngine, BranchStatus};
use proptest::prelude::*;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use tempfile::TempDir;
use uuid::Uuid;

async fn seed_source_db(path: &std::path::Path) {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(path)
                .create_if_missing(true)
                .journal_mode(SqliteJournalMode::Wal),
        )
        .await
        .expect("seed pool");

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS memory_records (id TEXT PRIMARY KEY, content TEXT, metadata TEXT, created_at TEXT, updated_at TEXT)",
    )
    .execute(&pool)
    .await
    .expect("create memory_records");
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS sessions (id TEXT PRIMARY KEY, name TEXT, metadata TEXT, created_at TEXT, updated_at TEXT)",
    )
    .execute(&pool)
    .await
    .expect("create sessions");
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS tool_outputs (id TEXT PRIMARY KEY, tool_name TEXT, output TEXT, metadata TEXT, created_at TEXT, updated_at TEXT)",
    )
    .execute(&pool)
    .await
    .expect("create tool_outputs");

    pool.close().await;
}

fn list_snapshot_db_files(branches_dir: &std::path::Path) -> HashSet<std::path::PathBuf> {
    let mut files = HashSet::new();
    if let Ok(entries) = std::fs::read_dir(branches_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let candidate = path.join("branch.db");
            if candidate.exists() {
                files.insert(candidate);
            }
        }
    }
    files
}

fn run_sequence(ops: Vec<u8>) {
    tokio_test::block_on(async move {
        let dir = TempDir::new().expect("tempdir");
        let source_db = dir.path().join("source.db");
        seed_source_db(&source_db).await;

        let config = BranchConfig::builder()
            .workspace_id(Uuid::new_v4())
            .branches_dir(dir.path().join("branches"))
            .max_branches_per_workspace(10)
            .build()
            .expect("config");
        let engine = BranchEngine::new(config, &source_db).await.expect("engine");

        for (idx, op) in ops.into_iter().enumerate() {
            let all = engine.list(None).await.expect("list branches");
            let trunk = engine.trunk().await.expect("trunk");
            let live: Vec<_> = all
                .iter()
                .filter(|b| matches!(b.status, BranchStatus::Active | BranchStatus::Dormant))
                .cloned()
                .collect();

            match op % 4 {
                0 => {
                    let name = format!("prop-trunk-{idx}");
                    let _ = engine.fork_trunk(&name).await;
                }
                1 => {
                    if !live.is_empty() {
                        let parent = &live[(idx + 1) % live.len()];
                        let name = format!("prop-child-{idx}");
                        let _ = engine.fork(parent.id, &name, None).await;
                    }
                }
                2 => {
                    let discardable: Vec<_> =
                        all.into_iter().filter(|b| b.id != trunk.id).collect();
                    if !discardable.is_empty() {
                        let branch = &discardable[idx % discardable.len()];
                        let _ = engine.discard(branch.id).await;
                    }
                }
                _ => {
                    if live.len() >= 2 {
                        let a = &live[idx % live.len()];
                        let b = &live[(idx + 1) % live.len()];
                        if a.id != b.id {
                            let _ = engine
                                .merge(a.id, b.id, claw_branch::MergeStrategy::Ours)
                                .await;
                        }
                    }
                }
            }

            let current = engine.list(None).await.expect("post-op list");

            for branch in &current {
                assert!(
                    branch.db_path.exists(),
                    "registry branch db must exist on filesystem"
                );
                let lineage = engine.lineage(branch.id).await.expect("lineage");
                let unique: HashSet<_> = lineage.iter().copied().collect();
                assert_eq!(
                    unique.len(),
                    lineage.len(),
                    "lineage for {} contains cycle/repeated node",
                    branch.id
                );
            }

            let listed_paths: HashSet<_> = current.iter().map(|b| b.db_path.clone()).collect();
            let fs_paths = list_snapshot_db_files(&engine.config().branches_dir);
            assert!(
                fs_paths.is_subset(&listed_paths),
                "found orphan snapshot db files not represented in registry"
            );
        }
    });
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn prop_branch_sequences_keep_invariants(ops in proptest::collection::vec(any::<u8>(), 1..64)) {
        run_sequence(ops);
    }
}
