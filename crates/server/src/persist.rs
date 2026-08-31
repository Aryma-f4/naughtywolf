//! Shared persistence helpers for the C2 SQLite backend.
//!
//! This module provides schema initialization and row types shared between
//! `session.rs` (session persistence) and `queue.rs` (task persistence).
//! The actual store implementations live in their respective modules so each
//! domain keeps its own query logic.

/// Session row as returned from the SQLite backing store.
#[derive(sqlx::FromRow)]
pub struct SessionRow {
    pub id: String,
    pub hostname: String,
    pub username: String,
    pub os: String,
    pub arch: String,
    pub pid: i64,
    pub addr: String,
    pub session_key: Vec<u8>,
    pub last_seen: String,
}

/// Initialize the C2 tables in a SQLite pool.
pub async fn init_schema(pool: &sqlx::SqlitePool) -> anyhow::Result<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS c2_sessions (
            id TEXT PRIMARY KEY,
            hostname TEXT NOT NULL,
            username TEXT NOT NULL,
            os TEXT NOT NULL,
            arch TEXT NOT NULL,
            pid INTEGER NOT NULL,
            addr TEXT NOT NULL,
            session_key BLOB NOT NULL,
            last_seen TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        )",
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS c2_tasks (
            id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL REFERENCES c2_sessions(id) ON DELETE CASCADE,
            command TEXT NOT NULL,
            args_json TEXT NOT NULL,
            timeout_ms INTEGER NOT NULL,
            status TEXT NOT NULL CHECK (status IN ('pending', 'delivered', 'completed')) DEFAULT 'pending',
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        )"
    ).execute(pool).await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS c2_task_results (
            task_id TEXT PRIMARY KEY REFERENCES c2_tasks(id) ON DELETE CASCADE,
            ok INTEGER NOT NULL,
            stdout BLOB NOT NULL,
            stderr BLOB NOT NULL,
            exit_code INTEGER NOT NULL,
            completed_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        )",
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS c2_operators (
            id TEXT PRIMARY KEY,
            username TEXT NOT NULL UNIQUE,
            password_hash TEXT NOT NULL,
            role TEXT NOT NULL CHECK (role IN ('admin', 'operator', 'viewer')),
            disabled INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        )",
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS c2_audit (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            operator_id TEXT,
            operator_name TEXT NOT NULL,
            action TEXT NOT NULL,
            target_session TEXT,
            details TEXT NOT NULL,
            succeeded INTEGER NOT NULL,
            timestamp TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        )",
    )
    .execute(pool)
    .await?;

    Ok(())
}

/// Open a SQLite pool (file-backed or `:memory:`).
pub async fn open_pool(db_path: &str) -> anyhow::Result<sqlx::SqlitePool> {
    use std::str::FromStr;

    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

    let url = if db_path == ":memory:" {
        "sqlite::memory:".to_string()
    } else {
        format!("sqlite://{}", db_path)
    };
    let options = SqliteConnectOptions::from_str(&url)?
        .create_if_missing(db_path != ":memory:")
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(if db_path == ":memory:" { 1 } else { 5 })
        .connect_with(options)
        .await?;
    init_schema(&pool).await?;
    Ok(pool)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn schema_initializes_clean() {
        let pool = open_pool(":memory:").await.unwrap();
        // Tables should exist; inserting + querying should work.
        sqlx::query(
            "INSERT INTO c2_operators (id, username, password_hash, role) \
                     VALUES (?1, ?2, ?3, ?4)",
        )
        .bind("test-id")
        .bind("admin")
        .bind("hash")
        .bind("admin")
        .execute(&pool)
        .await
        .unwrap();

        let row: (String,) = sqlx::query_as("SELECT username FROM c2_operators WHERE id = ?1")
            .bind("test-id")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(row.0, "admin");
    }
}
