use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::str::FromStr;

pub mod models;
pub mod repositories;

/// Create a SQLite connection pool with foreign-key constraints enabled.
pub async fn create_pool(database_url: &str) -> Result<SqlitePool, sqlx::Error> {
    let options = SqliteConnectOptions::from_str(database_url)?.foreign_keys(true);
    // SQLite allocates a distinct database for every `:memory:` connection.
    let max_connections = if database_url == "sqlite::memory:" {
        1
    } else {
        5
    };
    let pool = SqlitePoolOptions::new()
        .max_connections(max_connections)
        .connect_with(options)
        .await?;
    Ok(pool)
}

/// Run database migrations from the migrations directory
pub async fn run_migrations(pool: &SqlitePool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!("./migrations").run(pool).await
}
