#![cfg(feature = "guarded")]

use claw_branch::{BranchConfig, BranchEngine, BranchError, GuardedBranchEngine};
use claw_guard::{Guard, GuardConfig};
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

fn write_policy_file(path: &std::path::Path) {
    let policy = r#"
[[policies]]
name = "allow-branch-write"
priority = 100
enabled = true

[[policies.rules]]
type = "allow_if"
condition = { scope_contains = "branch:write" }
"#;
    std::fs::write(path, policy).expect("write policy");
}

async fn setup_guarded_engine() -> (GuardedBranchEngine, Guard, TempDir, Uuid) {
    let dir = tempfile::tempdir().expect("tempdir");
    let workspace_id = Uuid::new_v4();

    let source_db = dir.path().join("source.db");
    seed_source_db(&source_db).await;

    let branch_config = BranchConfig::builder()
        .workspace_id(workspace_id)
        .branches_dir(dir.path().join("branches"))
        .build()
        .expect("branch config");
    let engine = BranchEngine::new(branch_config, &source_db)
        .await
        .expect("branch engine");

    let policy_dir = dir.path().join("guard-policies");
    std::fs::create_dir_all(&policy_dir).expect("create policy dir");
    write_policy_file(&policy_dir.join("allow-write.toml"));

    let previous_secret = std::env::var("CLAW_GUARD_JWT_SECRET").ok();
    let previous_db = std::env::var("CLAW_GUARD_DB_PATH").ok();
    let previous_policy = std::env::var("CLAW_GUARD_POLICY_DIR").ok();

    std::env::set_var("CLAW_GUARD_JWT_SECRET", "integration-test-secret");
    std::env::set_var(
        "CLAW_GUARD_DB_PATH",
        dir.path().join("guard.db").to_string_lossy().to_string(),
    );
    std::env::set_var(
        "CLAW_GUARD_POLICY_DIR",
        policy_dir.to_string_lossy().to_string(),
    );

    let guard = Guard::new(GuardConfig::from_env().expect("guard config from env"))
        .await
        .expect("guard init");

    if let Some(value) = previous_secret {
        std::env::set_var("CLAW_GUARD_JWT_SECRET", value);
    } else {
        std::env::remove_var("CLAW_GUARD_JWT_SECRET");
    }
    if let Some(value) = previous_db {
        std::env::set_var("CLAW_GUARD_DB_PATH", value);
    } else {
        std::env::remove_var("CLAW_GUARD_DB_PATH");
    }
    if let Some(value) = previous_policy {
        std::env::set_var("CLAW_GUARD_POLICY_DIR", value);
    } else {
        std::env::remove_var("CLAW_GUARD_POLICY_DIR");
    }

    let guarded = GuardedBranchEngine::new(engine, guard.clone());
    (guarded, guard, dir, workspace_id)
}

#[tokio::test]
async fn guarded_engine_allows_writer_session_to_fork() {
    let (guarded, guard, _dir, workspace_id) = setup_guarded_engine().await;
    let session = guard
        .sessions()
        .create_session(
            Uuid::new_v4(),
            workspace_id,
            "writer",
            vec!["branch:write".to_string()],
            3600,
        )
        .await
        .expect("create writer session");

    let result = guarded.fork_trunk(&session, "guarded-writer-ok").await;
    assert!(result.is_ok(), "writer-scoped session should allow forking");
}

#[tokio::test]
async fn guarded_engine_denies_read_only_session_for_fork() {
    let (guarded, guard, _dir, workspace_id) = setup_guarded_engine().await;
    let session = guard
        .sessions()
        .create_session(
            Uuid::new_v4(),
            workspace_id,
            "reader",
            vec!["branch:read".to_string()],
            3600,
        )
        .await
        .expect("create reader session");

    let result = guarded.fork_trunk(&session, "guarded-reader-nope").await;
    assert!(matches!(result, Err(BranchError::PermissionDenied(_))));
}
