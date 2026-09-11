use std::sync::Arc;

use axum::{
    Extension, Form, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Redirect},
    routing::post,
};
use clap::Parser;
use naughtywolf::{
    auth::{authenticate, middleware::AuthSession},
    c2,
    cli::{Cli, Commands},
    config::Config,
    db::{self, repositories::Repository},
    evidence::EvidenceStore,
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
    let repository = Repository { pool };
    let c2_psk = Arc::new(config.c2_psk.clone());
    let evidence_store = EvidenceStore::from_config(repository.clone(), &config);
    let transfer_store = naughtywolf::callback_workspace::transfers::TransferStore::from_config(
        repository.clone(),
        &config,
    )?;
    let app = Router::<Repository>::new()
        .route("/login", post(login_handler))
        .merge(portal::authenticated_router())
        .merge(c2::router(c2_psk.clone()))
        .with_state(repository.clone())
        .merge(public_router())
        .layer(Extension(c2_psk.clone()))
        .layer(axum::Extension(evidence_store))
        .layer(axum::Extension(transfer_store))
        .layer(session_layer);

    if let Some(bind) = config.tcp_bind {
        let tcp_listener = tokio::net::TcpListener::bind(bind).await?;
        let tcp_repository = repository.clone();
        let tcp_psk = c2_psk.clone();
        let tcp_protocol = config.tcp_protocol.clone();
        tokio::spawn(async move {
            if let Err(error) = naughtywolf::c2_tcp::serve_tcp_listener(
                tcp_repository,
                tcp_psk,
                tcp_listener,
                tcp_protocol,
            )
            .await
            {
                tracing::error!(%error, "portal raw-tcp listener stopped");
            }
        });
    }
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
        return login_failure_response(&session, StatusCode::BAD_REQUEST).await;
    }

    match authenticate(&repository, &form.username, &form.password).await {
        Ok(Some(user)) => match (AuthSession { session }).login(&user).await {
            Ok(()) => Redirect::to("/dashboard").into_response(),
            Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        },
        Ok(None) => login_failure_response(&session, StatusCode::UNAUTHORIZED).await,
        Err(_) => login_failure_response(&session, StatusCode::INTERNAL_SERVER_ERROR).await,
    }
}

async fn login_failure_response(
    session: &tower_sessions::Session,
    status: StatusCode,
) -> axum::response::Response {
    match portal::login_page_with_new_csrf(session, Some("Invalid username or password")).await {
        Ok(page) => (status, page).into_response(),
        Err(error) => error.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::{login_handler, public_router};
    use axum::{
        Router,
        body::{Body, to_bytes},
        http::{Request, StatusCode},
        routing::post,
    };
    use naughtywolf::{db, db::repositories::Repository};
    use tower::ServiceExt;
    use tower_sessions::{MemoryStore, SessionManagerLayer};

    async fn csrf_test_app() -> Router {
        let pool = db::create_pool("sqlite::memory:").await.unwrap();
        db::run_migrations(&pool).await.unwrap();

        Router::<Repository>::new()
            .route("/login", post(login_handler))
            .with_state(Repository { pool })
            .merge(public_router())
            .layer(SessionManagerLayer::new(MemoryStore::default()).with_secure(false))
    }

    fn csrf_token(body: &str) -> String {
        body.split("name=\"csrf_token\" value=\"")
            .nth(1)
            .and_then(|remainder| remainder.split('\"').next())
            .unwrap()
            .to_owned()
    }

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

    #[tokio::test]
    async fn csrf_mismatch_issues_a_replacement_token_that_can_be_submitted() {
        let app = csrf_test_app().await;
        let login_page = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/login")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let (parts, body) = login_page.into_parts();
        let cookie = parts
            .headers
            .get("set-cookie")
            .unwrap()
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        let initial_token = csrf_token(
            &String::from_utf8(to_bytes(body, usize::MAX).await.unwrap().to_vec()).unwrap(),
        );

        let rejected = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/login")
                    .header("cookie", &cookie)
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from(format!(
                        "username=missing-user&password=wrong&csrf_token={initial_token}-mismatch"
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
        let replacement_token = csrf_token(
            &String::from_utf8(
                to_bytes(rejected.into_body(), usize::MAX)
                    .await
                    .unwrap()
                    .to_vec(),
            )
            .unwrap(),
        );
        assert!(!replacement_token.is_empty());
        assert_ne!(replacement_token, initial_token);

        let retry = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/login")
                    .header("cookie", cookie)
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from(format!(
                        "username=missing-user&password=wrong&csrf_token={replacement_token}"
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(retry.status(), StatusCode::UNAUTHORIZED);
    }
}
