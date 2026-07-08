use naughtywolf::db;
use naughtywolf::sliver::events::SliverEvent;
use naughtywolf::web::routes::AppState;
use sqlx::PgPool;
use std::sync::Arc;
use tokio::sync::{Mutex, broadcast};

async fn setup_test_pool() -> PgPool {
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:root@localhost:5432/naughtywolf".to_string());
    let pool = PgPool::connect(&database_url).await.unwrap();
    db::run_migrations(&pool).await.unwrap();
    pool
}

#[tokio::test]
#[ignore = "requires DATABASE_URL"]
async fn test_migration_applies() {
    let pool = setup_test_pool().await;

    // Verify 5 expected tables exist after migration
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT table_name FROM information_schema.tables WHERE table_schema = 'public'",
    )
    .fetch_all(&pool)
    .await
    .unwrap();

    let expected_tables = [
        "users",
        "audit_events",
        "sliver_profiles",
        "user_sliver_profiles",
        "ui_preferences",
    ];

    for expected in &expected_tables {
        assert!(
            tables.contains(&expected.to_string()),
            "Missing table: {}",
            expected
        );
    }

    assert_eq!(
        tables.len(),
        5,
        "Expected exactly 5 tables, got {}",
        tables.len()
    );
}

#[tokio::test]
#[ignore = "requires DATABASE_URL"]
async fn test_server_boots() {
    let pool = setup_test_pool().await;
    let (event_tx, _) = broadcast::channel::<SliverEvent>(256);

    use naughtywolf::sliver::connection::SliverConnection;
    let sliver = Arc::new(Mutex::new(None::<SliverConnection>));

    let state = AppState {
        pool,
        event_tx,
        sliver,
    };

    use tower_sessions::{MemoryStore, SessionManagerLayer};

    let session_store = MemoryStore::default();
    let session_layer = SessionManagerLayer::new(session_store).with_secure(false);

    // Build the application router the same way main.rs does,
    // including the session middleware layer required by page handlers.
    let app = naughtywolf::web::routes::routes()
        .merge(naughtywolf::web::api::api_routes())
        .merge(naughtywolf::web::sse::event_stream_routes())
        .with_state(state)
        .layer(session_layer);

    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/login")
                .method("GET")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}
