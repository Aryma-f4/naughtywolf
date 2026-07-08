use naughtywolf::config::Config;
use naughtywolf::db;
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

    // Router setup (wired in Task 3+)
    // axum::serve(listener, app).await?;

    tracing::info!("NaughtyWolf stopped");

    Ok(())
}
