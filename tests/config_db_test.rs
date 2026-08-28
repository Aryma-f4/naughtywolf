use sqlx::Row;
use std::time::Duration;

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
