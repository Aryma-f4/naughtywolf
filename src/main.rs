use axum::{Router, http::StatusCode, routing::get};
use clap::Parser;
use naughtywolf::{
    cli::{Cli, Commands},
    config::Config,
    db,
};
use time::Duration;
use tower_sessions::{Expiry, SessionManagerLayer, cookie::Key};
use tower_sessions_sqlx_store::SqliteStore;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,naughtywolf=debug")),
        )
        .init();

    let cli = Cli::parse();
    let database_url = Config::database_url_from_env();
    let pool = db::create_pool(&database_url).await?;
    db::run_migrations(&pool).await?;

    match cli.command {
        Commands::User(user) => user.execute(&pool).await?,
        Commands::Migrate => tracing::info!("SQLite database migrations applied"),
        Commands::Serve => serve(Config::from_env()?, pool).await?,
    }

    Ok(())
}

async fn serve(config: Config, pool: sqlx::SqlitePool) -> anyhow::Result<()> {
    anyhow::ensure!(
        config.session_secret_bytes().len() >= 32,
        "NAUGHTYWOLF_SESSION_SECRET must contain at least 32 bytes"
    );

    let session_store = SqliteStore::new(pool);
    session_store.migrate().await?;
    let session_layer = SessionManagerLayer::new(session_store)
        .with_secure(config.cookie_secure)
        .with_expiry(Expiry::OnInactivity(Duration::hours(8)))
        .with_signed(Key::derive_from(&config.session_secret_bytes()));
    let app = Router::new()
        .route("/healthz", get(|| async { StatusCode::NO_CONTENT }))
        .layer(session_layer);

    let listener = tokio::net::TcpListener::bind(config.bind).await?;
    tracing::info!(bind = %config.bind, "local standalone server listening");
    axum::serve(listener, app).await?;
    Ok(())
}
