use naughtywolf::{
    config::Config,
    db,
    web::{self, routes::AppState},
};
use tower_http::services::ServeDir;
use tower_sessions::{cookie::time::Duration, session_store::ExpiredDeletion, SessionManagerLayer};
use tower_sessions_sqlx_store::PostgresStore;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,naughtywolf=debug"));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .init();

    let config = Config::from_env()?;
    tracing::info!("NaughtyWolf starting on {}", config.bind);

    let pool = db::create_pool(&config.database_url).await?;
    db::run_migrations(&pool).await?;
    tracing::info!("Database connected and migrations applied");

    // Session store in PostgreSQL
    let session_store = PostgresStore::new(pool.clone());
    session_store.migrate().await?;

    // Background task to clean expired sessions
    let deletion_store = session_store.clone();
    let deletion_task = tokio::task::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(60));
        interval.tick().await; // skip first immediate tick
        loop {
            interval.tick().await;
            deletion_store.delete_expired().await.ok();
        }
    });

    let session_layer = SessionManagerLayer::new(session_store)
        .with_secure(false)         // set true when behind HTTPS reverse proxy
        .with_expiry(tower_sessions::Expiry::OnInactivity(
            Duration::hours(8) // 8 hours
        ));

    let state = AppState { pool: pool.clone() };

    let app = web::routes::routes()
        .with_state(state)
        .layer(session_layer)
        .nest_service("/static", ServeDir::new("static"));

    let listener = tokio::net::TcpListener::bind(&config.bind).await?;
    tracing::info!("Listening on {}", config.bind);

    axum::serve(listener, app).await?;

    deletion_task.abort();
    tracing::info!("NaughtyWolf stopped");
    Ok(())
}
