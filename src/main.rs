use axum::{
    Form, Router,
    extract::State,
    http::StatusCode,
    response::{Html, IntoResponse, Redirect},
    routing::post,
};
use clap::Parser;
use naughtywolf::{
    auth::{authenticate, middleware::AuthSession},
    cli::{Cli, Commands},
    config::Config,
    db::{self, repositories::Repository},
    portal,
};
use serde::Deserialize;
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

    let session_store = SqliteStore::new(pool.clone());
    session_store.migrate().await?;
    let session_layer = SessionManagerLayer::new(session_store)
        .with_secure(config.cookie_secure)
        .with_expiry(Expiry::OnInactivity(Duration::hours(8)))
        .with_signed(Key::derive_from(&config.session_secret_bytes()));
    let app = Router::<Repository>::new()
        .route("/login", post(login_handler))
        .with_state(Repository { pool })
        .merge(public_router())
        .layer(session_layer);

    let listener = tokio::net::TcpListener::bind(config.bind).await?;
    tracing::info!(bind = %config.bind, "local standalone server listening");
    axum::serve(listener, app).await?;
    Ok(())
}

pub fn public_router() -> Router {
    portal::public_router()
}

#[derive(Deserialize)]
struct LoginForm {
    username: String,
    password: String,
    csrf_token: String,
}

async fn login_handler(
    State(repository): State<Repository>,
    session: tower_sessions::Session,
    Form(form): Form<LoginForm>,
) -> impl IntoResponse {
    let expected_csrf_token: Option<String> = match session.get(portal::CSRF_TOKEN_KEY).await {
        Ok(token) => token,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    if expected_csrf_token.as_deref() != Some(form.csrf_token.as_str()) {
        return (
            StatusCode::BAD_REQUEST,
            Html(portal::templates::login_page(
                Some("Invalid username or password"),
                "",
            )),
        )
            .into_response();
    }

    match authenticate(&repository, &form.username, &form.password).await {
        Ok(Some(user)) => match (AuthSession { session }).login(&user).await {
            Ok(()) => Redirect::to("/").into_response(),
            Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        },
        Ok(None) => (
            StatusCode::UNAUTHORIZED,
            Html(portal::templates::login_page(
                Some("Invalid username or password"),
                &form.csrf_token,
            )),
        )
            .into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Html(portal::templates::login_page(
                Some("Invalid username or password"),
                &form.csrf_token,
            )),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::public_router;
    use axum::{body::to_bytes, http::Request};
    use tower::ServiceExt;

    #[tokio::test]
    async fn root_route_serves_the_local_landing_page() {
        let response = public_router()
            .oneshot(
                Request::builder()
                    .uri("/")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(std::str::from_utf8(&body).unwrap().contains("NaughtyWolf"));
    }

    #[tokio::test]
    async fn login_page_is_available() {
        let response = public_router()
            .oneshot(
                Request::builder()
                    .uri("/login")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), axum::http::StatusCode::OK);
    }
}
