use sqlx::postgres::PgPoolOptions;
use std::env;

#[tokio::test]
#[ignore = "Requires DATABASE_URL and running PostgreSQL"]
async fn test_migration_applies_cleanly() {
    let database_url = env::var("DATABASE_URL")
        .expect("DATABASE_URL must be set for integration tests");
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await
        .expect("Failed to connect to postgres");
    naughtywolf::db::run_migrations(&pool)
        .await
        .expect("Migration failed");
    // Verify tables exist
    let row: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = 'public'"
    )
    .fetch_one(&pool)
    .await
    .expect("Query failed");
    assert!(row.0 >= 5, "Expected at least 5 tables, got {}", row.0);
}
