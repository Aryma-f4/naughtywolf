use naughtywolf::config::Config;
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

    // DB pool setup (wired in Task 2)
    // Router setup (wired in Task 3+)
    // axum::serve(listener, app).await?;

    tracing::info!("NaughtyWolf stopped");

    Ok(())
}
