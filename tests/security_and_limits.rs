use std::os::unix::fs as unix_fs;

use claw_branch::{
    branch::naming::NamingValidator,
    snapshot::{manifest::SnapshotManifest, verifier::verify_snapshot},
    BranchConfig, BranchEngine, BranchError,
};
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

#[tokio::test]
async fn branch_name_validation_required_cases() {
    assert!(NamingValidator::validate("feature/a.b-c_1").is_ok());
    assert!(matches!(
        NamingValidator::validate("feature/../escape"),
        Err(BranchError::InvalidBranchName)
    ));
    assert!(matches!(
        NamingValidator::validate("/etc/passwd"),
        Err(BranchError::InvalidBranchName)
    ));
    assert!(matches!(
        NamingValidator::validate(&"a".repeat(129)),
        Err(BranchError::InvalidBranchName)
    ));
}

#[tokio::test]
async fn branches_dir_symlink_outside_root_is_rejected() {
    let root = tempfile::tempdir().expect("root tempdir");
    let external = tempfile::tempdir().expect("external tempdir");
    let link_path = root.path().join("branches-link");
    unix_fs::symlink(external.path(), &link_path).expect("create symlink");

    let source_db = root.path().join("source.db");
    seed_source_db(&source_db).await;

    let config = BranchConfig::builder()
        .workspace_id(Uuid::new_v4())
        .branches_dir(link_path)
        .build()
        .expect("config");

    let result = BranchEngine::new(config, &source_db).await;
    assert!(matches!(result, Err(BranchError::InvalidConfig(_))));
}

#[tokio::test]
async fn fork_respects_max_branches_per_workspace() {
    let dir = tempfile::tempdir().expect("tempdir");
    let source_db = dir.path().join("source.db");
    seed_source_db(&source_db).await;

    let config = BranchConfig::builder()
        .workspace_id(Uuid::new_v4())
        .branches_dir(dir.path().join("branches"))
        .max_branches_per_workspace(1)
        .build()
        .expect("config");

    let engine = BranchEngine::new(config, &source_db).await.expect("engine");
    let result = engine.fork_trunk("too-many").await;
    assert!(matches!(result, Err(BranchError::BranchLimitExceeded)));
}

#[tokio::test]
async fn snapshot_sidecar_detects_tamper() {
    let dir = TempDir::new().expect("tempdir");
    let source_db = dir.path().join("source.db");
    seed_source_db(&source_db).await;

    let config = BranchConfig::builder()
        .workspace_id(Uuid::new_v4())
        .branches_dir(dir.path().join("branches"))
        .build()
        .expect("config");

    let engine = BranchEngine::new(config, &source_db).await.expect("engine");
    let branch = engine.trunk().await.expect("trunk");
    std::fs::write(&branch.db_path, b"tampered").expect("tamper write");

    let manifest = SnapshotManifest::load(
        branch
            .db_path
            .parent()
            .expect("snapshot path parent should exist"),
    )
    .expect("load manifest");

    let result = verify_snapshot(&manifest).await;
    assert!(matches!(result, Err(BranchError::SnapshotCorrupt { .. })));
}

#[tokio::test]
async fn missing_sidecar_is_rejected() {
    let dir = TempDir::new().expect("tempdir");
    let source_db = dir.path().join("source.db");
    seed_source_db(&source_db).await;

    let config = BranchConfig::builder()
        .workspace_id(Uuid::new_v4())
        .branches_dir(dir.path().join("branches"))
        .build()
        .expect("config");

    let engine = BranchEngine::new(config, &source_db).await.expect("engine");
    let branch = engine.trunk().await.expect("trunk");

    let sidecar = branch.db_path.with_extension("hash");
    std::fs::remove_file(&sidecar).expect("remove sidecar");

    let manifest = SnapshotManifest::load(
        branch
            .db_path
            .parent()
            .expect("snapshot path parent should exist"),
    )
    .expect("load manifest");

    let result = verify_snapshot(&manifest).await;
    assert!(matches!(
        result,
        Err(BranchError::SnapshotHashMissing { .. })
    ));
}
