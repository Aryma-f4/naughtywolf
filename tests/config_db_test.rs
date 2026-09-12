use sha2::Digest;
use sqlx::Row;
use std::time::Duration;

#[derive(Debug, PartialEq, sqlx::FromRow)]
struct CallbackPreservationRow {
    id: String,
    asset_id: Option<String>,
    operation_id: Option<String>,
    host: String,
    user_name: String,
    process: String,
    arch: String,
    os: String,
    protocol: String,
    status: String,
    session_key: String,
    last_seen: String,
    created_at: String,
}

#[tokio::test]
async fn callback_workspace_migration_preserves_completed_task_results() {
    let path = std::env::temp_dir().join(format!("naughtywolf-schema-{}.db", uuid::Uuid::new_v4()));
    let url = format!(
        "sqlite:///{}?mode=rwc",
        path.to_string_lossy().trim_start_matches('/')
    );
    let pool = naughtywolf::db::create_pool(&url).await.unwrap();
    for (migration, version, description) in [
        (include_str!("../migrations/001_core.sql"), 1_i64, "core"),
        (
            include_str!("../migrations/002_payload_features.sql"),
            2,
            "payload_features",
        ),
        (
            include_str!("../migrations/003_c2_sessions.sql"),
            3,
            "c2_sessions",
        ),
    ] {
        sqlx::raw_sql(migration).execute(&pool).await.unwrap();
        let checksum = sha2::Sha384::digest(migration.as_bytes());
        sqlx::query("CREATE TABLE IF NOT EXISTS _sqlx_migrations (version BIGINT PRIMARY KEY, description TEXT NOT NULL, installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP, success BOOLEAN NOT NULL, checksum BLOB NOT NULL, execution_time BIGINT NOT NULL)").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) VALUES (?, ?, 1, ?, 0)").bind(version).bind(description).bind(checksum.as_slice()).execute(&pool).await.unwrap();
    }
    sqlx::query("INSERT INTO callbacks (id, host, user_name, process, arch, os, protocol, status, session_key) VALUES ('c', 'h', 'u', 'p', 'x64', 'linux', 'http', 'active', 'key')").execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO c2_sessions (id, hostname, username, os, arch, pid, addr, session_key, last_seen) VALUES ('s', 'h', 'u', 'linux', 'x64', 1, '', X'01', 'now')").execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO c2_tasks (id, session_id, command, args_json, timeout_ms, status, result_output, result_ok, result_exit_code) VALUES ('t', 's', 'x', '{}', 1, 'completed', 'out', 1, 0)").execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO c2_task_results (task_id, ok, stdout, stderr, exit_code, completed_at) VALUES ('t', 1, X'000102', X'0304', 0, 'done')").execute(&pool).await.unwrap();
    let callbacks_before: Vec<CallbackPreservationRow> = sqlx::query_as("SELECT id, asset_id, operation_id, host, user_name, process, arch, os, protocol, status, session_key, last_seen, created_at FROM callbacks ORDER BY id").fetch_all(&pool).await.unwrap();
    type C2TaskRow = (
        String,
        String,
        String,
        String,
        i64,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<i64>,
        Option<i64>,
        String,
    );
    type C2TaskResultRow = (String, i64, Vec<u8>, Vec<u8>, i64, String);
    let tasks_before: Vec<C2TaskRow> = sqlx::query_as("SELECT id, session_id, command, args_json, timeout_ms, status, processing_at, completed_at, result_output, result_ok, result_exit_code, created_at FROM c2_tasks ORDER BY id").fetch_all(&pool).await.unwrap();
    let results_before: Vec<C2TaskResultRow> = sqlx::query_as("SELECT task_id, ok, stdout, stderr, exit_code, completed_at FROM c2_task_results ORDER BY task_id").fetch_all(&pool).await.unwrap();
    pool.close().await;
    let pool = naughtywolf::db::create_pool(&url).await.unwrap();
    naughtywolf::db::run_migrations(&pool).await.unwrap();
    let callbacks_after: Vec<CallbackPreservationRow> = sqlx::query_as("SELECT id, asset_id, operation_id, host, user_name, process, arch, os, protocol, status, session_key, last_seen, created_at FROM callbacks ORDER BY id").fetch_all(&pool).await.unwrap();
    assert_eq!(callbacks_before, callbacks_after);
    let tasks_after: Vec<C2TaskRow> = sqlx::query_as("SELECT id, session_id, command, args_json, timeout_ms, status, processing_at, completed_at, result_output, result_ok, result_exit_code, created_at FROM c2_tasks ORDER BY id").fetch_all(&pool).await.unwrap();
    assert_eq!(tasks_before, tasks_after);
    let results_after: Vec<C2TaskResultRow> = sqlx::query_as("SELECT task_id, ok, stdout, stderr, exit_code, completed_at FROM c2_task_results ORDER BY task_id").fetch_all(&pool).await.unwrap();
    assert_eq!(results_before, results_after);
    let fk_violations: Vec<(String, i64, String, i64)> = sqlx::query_as("PRAGMA foreign_key_check")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert!(
        fk_violations.is_empty(),
        "foreign key violations: {fk_violations:?}"
    );
    pool.close().await;
    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn callback_workspace_schema_persists_lifecycle_snapshots_and_transfers() {
    let pool = naughtywolf::db::create_pool("sqlite::memory:")
        .await
        .unwrap();
    naughtywolf::db::run_migrations(&pool).await.unwrap();
    let task_columns: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('c2_tasks') ORDER BY cid")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert!(task_columns.contains(&"operator_id".into()));
    assert!(task_columns.contains(&"parent_task_id".into()));
    assert!(task_columns.contains(&"updated_at".into()));
    assert!(task_columns.contains(&"cancellation_requested_at".into()));
    for table in [
        "c2_process_snapshots",
        "c2_file_snapshots",
        "c2_file_transfers",
    ] {
        let found: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?")
                .bind(table)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(found, 1, "missing {table}");
    }

    naughtywolf::db::run_migrations(&pool).await.unwrap();
}

#[tokio::test]
async fn migration_creates_the_core_schema() {
    let pool = naughtywolf::db::create_pool("sqlite::memory:")
        .await
        .unwrap();
    naughtywolf::db::run_migrations(&pool).await.unwrap();

    let table_names: Vec<String> =
        sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .fetch_all(&pool)
            .await
            .unwrap();

    for expected in [
        "users",
        "operations",
        "operation_members",
        "assets",
        "asset_tags",
        "checks",
        "check_runs",
        "evidence",
        "audit_events",
    ] {
        assert!(
            table_names.contains(&expected.to_string()),
            "missing core table {expected}"
        );
    }
}

#[tokio::test]
async fn pool_enforces_asset_operation_foreign_keys() {
    let pool = naughtywolf::db::create_pool("sqlite::memory:")
        .await
        .unwrap();
    naughtywolf::db::run_migrations(&pool).await.unwrap();

    let error = sqlx::query(
        "INSERT INTO assets (id, operation_id, name, kind, owner, address) \
         VALUES ('asset-1', 'missing-operation', 'web-01', 'host', 'lab', '127.0.0.1')",
    )
    .execute(&pool)
    .await
    .unwrap_err();

    assert!(error.to_string().contains("FOREIGN KEY constraint failed"));

    let foreign_keys_enabled: i64 = sqlx::query("PRAGMA foreign_keys")
        .fetch_one(&pool)
        .await
        .unwrap()
        .get(0);
    assert_eq!(foreign_keys_enabled, 1);
}

#[tokio::test]
async fn in_memory_pool_does_not_open_an_isolated_second_database() {
    let pool = naughtywolf::db::create_pool("sqlite::memory:")
        .await
        .unwrap();
    let _first_connection = pool.acquire().await.unwrap();

    let second_acquisition = tokio::time::timeout(Duration::from_millis(100), pool.acquire()).await;

    assert!(
        second_acquisition.is_err(),
        "a second connection would point at a separate in-memory SQLite database"
    );
}

#[test]
fn config_uses_local_sqlite_by_default() {
    let config = naughtywolf::config::Config::for_test();

    assert_eq!(config.bind.to_string(), "127.0.0.1:8080");
    assert!(config.database_url.starts_with("sqlite:"));
    assert!(!config.evidence_dir.as_os_str().is_empty());
    assert!(!config.cookie_secure);
}
