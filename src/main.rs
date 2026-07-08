use clap::Parser;
use naughtywolf::{
    cli::Commands,
    config::Config,
    db,
    sliver::SliverAdapter,
    sliver::events::SliverEvent,
    web::{self, routes::AppState},
};
use std::sync::Arc;
use tokio::sync::{Mutex, broadcast, watch};
use tower_http::services::ServeDir;
use tower_sessions::{SessionManagerLayer, cookie::time::Duration, session_store::ExpiredDeletion};
use tower_sessions_sqlx_store::PostgresStore;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,naughtywolf=debug"));

    tracing_subscriber::fmt().with_env_filter(filter).init();

    // Use clap::Parser::try_parse to detect CLI mode vs server mode.
    // - No args: MissingSubcommand → fall through to server mode
    // - "serve": Ok(Serve) → fall through to server mode
    // - "user list" / "profile scan" etc: Ok(cmd) → CLI mode (load DB, execute, exit)
    // - "--help" / "--version": clap handles internally and exits
    match naughtywolf::cli::Cli::try_parse() {
        Ok(cli) => match cli.command {
            Commands::Serve => {
                // fall through to server mode below
            }
            cmd => {
                // CLI mode: need DB but not full server
                let config = Config::from_env()?;
                let pool = db::create_pool(&config.database_url).await?;
                db::run_migrations(&pool).await?;

                match cmd {
                    Commands::User(user) => user.execute(&pool).await?,
                    Commands::Profile(profile) => {
                        profile.execute(&pool, &config.sliver_config_dir).await?
                    }
                    Commands::Migrate => {
                        println!("Migrations already applied");
                    }
                    Commands::Serve => unreachable!(),
                }
                return Ok(());
            }
        },
        Err(e) => {
            // MissingSubcommand (no args) → server mode
            // DisplayHelp / DisplayVersion → clap handles and exits
            if e.kind() != clap::error::ErrorKind::MissingSubcommand {
                e.exit();
            }
        }
    }

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
        .with_secure(false) // set true when behind HTTPS reverse proxy
        .with_expiry(tower_sessions::Expiry::OnInactivity(
            Duration::hours(8), // 8 hours
        ));

    let (event_tx, _) = broadcast::channel::<SliverEvent>(256);

    // Sliver adapter — creates the shared connection storage that the event
    // listener and API handlers both access.
    let (_profile_tx, profile_rx) = watch::channel(None);
    let adapter = SliverAdapter {
        connection: Arc::new(Mutex::new(None)),
        active_profile: profile_rx,
    };
    let sliver_conn = adapter.connection.clone();

    let state = AppState {
        pool: pool.clone(),
        event_tx,
        sliver: sliver_conn,
    };

    let app = web::routes::routes()
        .merge(web::api::api_routes())
        .merge(web::sse::event_stream_routes())
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
