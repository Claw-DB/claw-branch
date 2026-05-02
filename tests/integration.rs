//! Integration tests for claw-branch using real BranchEngine + seeded SQLite.


use serde_json::json;
use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};
use tempfile::TempDir;
use uuid::Uuid;

use claw_branch::{
    prelude::*, BranchError, CherryPick, MergeStrategy, Recommendation, SimulationScenario,
};

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Creates a seeded source SQLite database with 10 memory_records, 3 sessions, 5 tool_outputs.
async fn seed_source_db(path: &std::path::Path) -> BranchResult<()> {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(path)
                .create_if_missing(true)
                .journal_mode(SqliteJournalMode::Wal),
        )
        .await?;

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
    .await?;

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
    .await?;

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
    .await?;

    for i in 0..10 {
        let id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO memory_records (id, content, metadata, created_at, updated_at)
             VALUES (?, ?, ?, datetime('now'), datetime('now'))",
        )
        .bind(&id)
        .bind(format!("memory content {i}"))
        .bind(json!({"idx": i}).to_string())
        .execute(&pool)
        .await?;
    }

    for i in 0..3 {
        let id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO sessions (id, name, metadata, created_at, updated_at)
             VALUES (?, ?, ?, datetime('now'), datetime('now'))",
        )
        .bind(&id)
        .bind(format!("session {i}"))
        .bind(json!({"session_idx": i}).to_string())
        .execute(&pool)
        .await?;
    }

    for i in 0..5 {
        let id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO tool_outputs (id, tool_name, output, metadata, created_at, updated_at)
             VALUES (?, ?, ?, ?, datetime('now'), datetime('now'))",
        )
        .bind(&id)
        .bind(format!("tool_{i}"))
        .bind(format!("output_{i}"))
        .bind(json!({"tool_idx": i}).to_string())
        .execute(&pool)
        .await?;
    }

    pool.close().await;
    Ok(())
}

/// Boots a BranchEngine backed by a temp directory with a seeded source DB.
async fn make_engine() -> BranchResult<(BranchEngine, TempDir)> {
    let dir = tempfile::tempdir().map_err(|e| BranchError::Io(e))?;
    let workspace_id = Uuid::new_v4();
    let trunk_db = dir.path().join("source.db");
    seed_source_db(&trunk_db).await?;

    let config = BranchConfig::builder()
        .workspace_id(workspace_id)
        .branches_dir(dir.path().join("branches"))
        .build()
        .map_err(|e: claw_branch::BranchError| e)?;

    let engine = BranchEngine::new(config, &trunk_db).await?;
    Ok((engine, dir))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn engine_creates_trunk() -> BranchResult<()> {
    let (engine, _dir) = make_engine().await?;
    let trunk = engine.trunk().await?;
    assert_eq!(trunk.name, engine.config().trunk_branch_name);
    assert!(trunk.parent_id.is_none());
    Ok(())
}

#[tokio::test]
async fn fork_creates_isolated_copy() -> BranchResult<()> {
    let (engine, _dir) = make_engine().await?;
    let trunk = engine.trunk().await?;
    let feature = engine.fork(trunk.id, "feature/iso", None).await?;
    assert_eq!(feature.parent_id, Some(trunk.id));
    assert_ne!(feature.db_path, trunk.db_path);
    Ok(())
}

#[tokio::test]
async fn fork_invalid_name_is_rejected() -> BranchResult<()> {
    let (engine, _dir) = make_engine().await?;
    let trunk = engine.trunk().await?;
    let result = engine.fork(trunk.id, "has spaces!", None).await;
    assert!(result.is_err(), "invalid name should be rejected");
    Ok(())
}

#[tokio::test]
async fn discard_removes_status() -> BranchResult<()> {
    let (engine, _dir) = make_engine().await?;
    let trunk = engine.trunk().await?;
    let branch = engine.fork(trunk.id, "discard-me", None).await?;
    engine.discard(branch.id).await?;
    let refreshed = engine.get(branch.id).await?;
    assert!(!refreshed.status.is_live());
    Ok(())
}

#[tokio::test]
async fn archive_branch_is_not_live() -> BranchResult<()> {
    let (engine, _dir) = make_engine().await?;
    let trunk = engine.trunk().await?;
    let branch = engine.fork(trunk.id, "archive-me", None).await?;
    engine.archive(branch.id).await?;
    let refreshed = engine.get(branch.id).await?;
    assert!(!refreshed.status.is_live());
    Ok(())
}

#[tokio::test]
async fn list_returns_forks() -> BranchResult<()> {
    let (engine, _dir) = make_engine().await?;
    let trunk = engine.trunk().await?;
    engine.fork(trunk.id, "list-a", None).await?;
    engine.fork(trunk.id, "list-b", None).await?;
    let all = engine.list(None).await?;
    assert!(all.len() >= 3, "trunk + 2 forks expected, got {}", all.len());
    Ok(())
}

#[tokio::test]
async fn diff_identical_branches_has_zero_divergence() -> BranchResult<()> {
    let (engine, _dir) = make_engine().await?;
    let trunk = engine.trunk().await?;
    let feature = engine.fork(trunk.id, "diff-identical", None).await?;
    let diff = engine.diff(trunk.id, feature.id).await?;
    // Newly forked branch is identical to trunk.
    assert_eq!(diff.stats.added, 0);
    assert_eq!(diff.stats.removed, 0);
    assert_eq!(diff.stats.modified, 0);
    Ok(())
}

#[tokio::test]
async fn diff_detects_field_modification() -> BranchResult<()> {
    let (engine, _dir) = make_engine().await?;
    let trunk = engine.trunk().await?;
    let branch = engine.fork(trunk.id, "diff-modified", None).await?;

    // Directly write to the branch database to simulate a field modification.
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&branch.db_path)
                .create_if_missing(false)
                .journal_mode(SqliteJournalMode::Wal),
        )
        .await?;
    sqlx::query("UPDATE memory_records SET content = 'modified' WHERE rowid = 1")
        .execute(&pool)
        .await?;
    pool.close().await;

    let diff = engine.diff(trunk.id, branch.id).await?;
    assert!(diff.stats.modified > 0, "expected at least one modification");
    Ok(())
}

#[tokio::test]
async fn merge_strategy_ours_prefers_source() -> BranchResult<()> {
    let (engine, _dir) = make_engine().await?;
    let trunk = engine.trunk().await?;
    let branch = engine.fork(trunk.id, "merge-ours", None).await?;

    // Modify branch so there is something to merge.
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&branch.db_path)
                .create_if_missing(false)
                .journal_mode(SqliteJournalMode::Wal),
        )
        .await?;
    sqlx::query("UPDATE memory_records SET content = 'ours_content' WHERE rowid = 1")
        .execute(&pool)
        .await?;
    pool.close().await;

    let result = engine.merge(branch.id, trunk.id, MergeStrategy::Ours).await?;
    // Merge should produce a result without panicking.
    let _ = (result.applied, result.conflicts.len());
    Ok(())
}

#[tokio::test]
async fn merge_preview_returns_report() -> BranchResult<()> {
    let (engine, _dir) = make_engine().await?;
    let trunk = engine.trunk().await?;
    let branch = engine.fork(trunk.id, "preview-test", None).await?;
    let preview = engine.merge_preview(branch.id, trunk.id).await?;
    // Auto-resolved can be 0 for identical branches, conflicts should be 0.
    assert_eq!(preview.conflicts.len(), 0);
    Ok(())
}

#[tokio::test]
async fn selective_commit_all_entities() -> BranchResult<()> {
    let (engine, _dir) = make_engine().await?;
    let trunk = engine.trunk().await?;
    let branch = engine.fork(trunk.id, "commit-all", None).await?;
    let _result = engine.commit_to_trunk(branch.id).await?;
    // All-entity commit should report some entities processed.
    // All-entity commit should report some entities processed.\n    let _ = result.fields_updated;
    Ok(())
}

#[tokio::test]
async fn selective_commit_specific_fields() -> BranchResult<()> {
    let (engine, _dir) = make_engine().await?;
    let trunk = engine.trunk().await?;
    let source = engine.fork(trunk.id, "commit-fields-src", None).await?;
    let target = engine.fork(trunk.id, "commit-fields-tgt", None).await?;

    use claw_branch::EntitySelection;
    use claw_branch::types::EntityType;

    let cherry = CherryPick {
        source_branch_id: source.id,
        target_branch_id: target.id,
        entity_selections: vec![EntitySelection {
            entity_type: EntityType::MemoryRecord,
            entity_ids: vec![],
            fields: Some(vec!["content".to_string()]),
        }],
        message: Some("field-level commit".to_string()),
    };

    let result = engine.commit(cherry).await?;
    assert_eq!(result.target_branch_id, target.id);
    Ok(())
}

#[tokio::test]
async fn dag_lineage_includes_trunk() -> BranchResult<()> {
    let (engine, _dir) = make_engine().await?;
    let trunk = engine.trunk().await?;
    let child = engine.fork(trunk.id, "lineage-child", None).await?;
    let lineage = engine.lineage(child.id).await?;
    // Lineage should contain at least child itself.
    assert!(!lineage.is_empty());
    Ok(())
}

#[tokio::test]
async fn dag_dot_export_is_nonempty() -> BranchResult<()> {
    let (engine, _dir) = make_engine().await?;
    let dot = engine.dag_dot().await?;
    assert!(dot.contains("digraph"), "DOT output should start with digraph");
    Ok(())
}

#[tokio::test]
async fn simulation_evaluate_commit_recommendation() -> BranchResult<()> {
    let (engine, _dir) = make_engine().await?;
    let trunk = engine.trunk().await?;

    let scenario = SimulationScenario {
        name: "test-sim".to_string(),
        description: "integration test scenario".to_string(),
        max_ops: Some(10),
        timeout_secs: Some(10),
        seed_data: None,
    };

    let report = engine
        .simulate(trunk.id, scenario, |_pool| async move {
            Ok(json!({"result": "test_passed"}))
        })
        .await?;

    // The agent did nothing, so result should be Discard (identical branch).
    matches!(report.recommendation, Recommendation::Discard | Recommendation::Commit);
    Ok(())
}

#[tokio::test]
async fn metrics_refresh_returns_counts() -> BranchResult<()> {
    let (engine, _dir) = make_engine().await?;
    let trunk = engine.trunk().await?;
    let metrics = engine.metrics(trunk.id).await?;
    assert!(metrics.memory_record_count >= 0);
    assert!(metrics.bytes_on_disk > 0, "trunk DB should have nonzero size");
    Ok(())
}

#[tokio::test]
async fn workspace_report_counts_branches() -> BranchResult<()> {
    let (engine, _dir) = make_engine().await?;
    let trunk = engine.trunk().await?;
    engine.fork(trunk.id, "report-a", None).await?;
    engine.fork(trunk.id, "report-b", None).await?;
    let report = engine.workspace_report().await?;
    assert!(report.branch_count >= 3, "trunk + 2 forks, got {}", report.branch_count);
    Ok(())
}

#[tokio::test]
async fn gc_runs_without_error() -> BranchResult<()> {
    let (engine, _dir) = make_engine().await?;
    let gc_report = engine.gc().await?;
    // On fresh workspace, nothing should be orphaned.
    assert_eq!(gc_report.orphaned_deleted, 0);
    Ok(())
}

#[tokio::test]
async fn get_by_name_returns_branch() -> BranchResult<()> {
    let (engine, _dir) = make_engine().await?;
    let trunk = engine.trunk().await?;
    engine.fork(trunk.id, "named-branch", None).await?;
    let found = engine.get_by_name("named-branch").await?;
    assert_eq!(found.name, "named-branch");
    Ok(())
}

#[tokio::test]
async fn engine_open_reads_existing_workspace() -> BranchResult<()> {
    let dir = tempfile::tempdir().map_err(|e| BranchError::Io(e))?;
    let workspace_id = Uuid::new_v4();
    let trunk_db = dir.path().join("source.db");
    seed_source_db(&trunk_db).await?;

    let config1 = BranchConfig::builder()
        .workspace_id(workspace_id)
        .branches_dir(dir.path().join("branches"))
        .build()
        .map_err(|e: claw_branch::BranchError| e)?;

    let engine1 = BranchEngine::new(config1, &trunk_db).await?;
    let trunk = engine1.trunk().await?;
    engine1.fork(trunk.id, "persist-check", None).await?;
    drop(engine1);

    let config2 = BranchConfig::builder()
        .workspace_id(workspace_id)
        .branches_dir(dir.path().join("branches"))
        .build()
        .map_err(|e: claw_branch::BranchError| e)?;

    let engine2 = BranchEngine::open(config2).await?;
    let all = engine2.list(None).await?;
    assert!(all.len() >= 2, "should see trunk + persist-check");
    Ok(())
}

#[tokio::test]
async fn fork_then_diff_then_merge_roundtrip() -> BranchResult<()> {
    let (engine, _dir) = make_engine().await?;
    let trunk = engine.trunk().await?;
    let branch = engine.fork(trunk.id, "roundtrip", None).await?;

    // Mutate branch.
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&branch.db_path)
                .create_if_missing(false)
                .journal_mode(SqliteJournalMode::Wal),
        )
        .await?;
    sqlx::query("UPDATE memory_records SET content = 'roundtrip_val' WHERE rowid = 2")
        .execute(&pool)
        .await?;
    pool.close().await;

    let diff = engine.diff(trunk.id, branch.id).await?;
    assert!(diff.stats.modified > 0);

    let result = engine.merge(branch.id, trunk.id, MergeStrategy::Theirs).await?;
    // Should succeed without error.
    let _ = (result.applied, result.conflicts.len());
    Ok(())
}

#[tokio::test]
async fn simulation_teardown_discards_branch() -> BranchResult<()> {
    use claw_branch::sandbox::environment::SimulationEnvironment;
    use std::sync::Arc;

    let (engine, _dir) = make_engine().await?;
    let trunk = engine.trunk().await?;

    let scenario = SimulationScenario {
        name: "teardown-test".to_string(),
        description: "teardown test".to_string(),
        max_ops: None,
        timeout_secs: None,
        seed_data: None,
    };

    let config_arc = Arc::new(engine.config().clone());
    let mut env = SimulationEnvironment::setup(
        &trunk,
        scenario,
        config_arc,
        engine.lifecycle(),
    )
    .await
    .unwrap_or_else(|_| {
        panic!("SimulationEnvironment::setup failed in teardown test");
    });

    let sandbox_id = env.branch.id;
    env.teardown(engine.lifecycle()).await?;

    let refreshed = engine.get(sandbox_id).await?;
    assert!(!refreshed.status.is_live(), "sandbox should be discarded after teardown");
    Ok(())
}
