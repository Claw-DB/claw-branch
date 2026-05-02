use std::time::Instant;

use claw_branch::{BranchConfig, BranchEngine};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use uuid::Uuid;

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
            id TEXT PRIMARY KEY,
            content TEXT,
            metadata TEXT,
            created_at TEXT,
            updated_at TEXT
        )",
    )
    .execute(&pool)
    .await
    .expect("create table");

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS sessions (
            id TEXT PRIMARY KEY,
            name TEXT,
            metadata TEXT,
            created_at TEXT,
            updated_at TEXT
        )",
    )
    .execute(&pool)
    .await
    .expect("create sessions");

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS tool_outputs (
            id TEXT PRIMARY KEY,
            tool_name TEXT,
            output TEXT,
            metadata TEXT,
            created_at TEXT,
            updated_at TEXT
        )",
    )
    .execute(&pool)
    .await
    .expect("create tool_outputs");

    for i in 0..count {
        sqlx::query(
            "INSERT INTO memory_records (id, content, metadata, created_at, updated_at)
             VALUES (?, ?, '{}', datetime('now'), datetime('now'))",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(format!("seed_{i}"))
        .execute(&pool)
        .await
        .expect("insert");
    }

    pool.close().await;
}

#[tokio::test]
async fn bench_fork_1k_entities_target_under_50ms() {
    let dir = tempfile::tempdir().expect("tempdir");
    let source_db = dir.path().join("source.db");
    seed_db(&source_db, 1_000).await;

    let config = BranchConfig::builder()
        .workspace_id(Uuid::new_v4())
        .branches_dir(dir.path().join("branches"))
        .build()
        .expect("config");

    let engine = BranchEngine::new(config, &source_db).await.expect("engine");
    let start = Instant::now();
    let _branch = engine.fork_trunk("bench-fork-target").await.expect("fork");
    let elapsed = start.elapsed();

    assert!(
        elapsed.as_millis() < 50,
        "fork_trunk with 1k entities exceeded target: {}ms",
        elapsed.as_millis()
    );
}
