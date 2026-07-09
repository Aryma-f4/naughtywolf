use axum::{
    Form, Router,
    extract::State,
    response::{Html, IntoResponse, Redirect},
    routing::get,
};
use serde::Deserialize;
use sqlx::PgPool;
use tower_sessions::Session;

use crate::auth::{
    middleware::{AuthSession, AuthenticatedUserGuard},
    password,
    rbac::Role,
};
use crate::db;
use crate::sliver::connection::SliverConnection;
use crate::sliver::events::SliverEvent;
use std::sync::Arc;
use tokio::sync::{Mutex, broadcast};

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub event_tx: broadcast::Sender<SliverEvent>,
    pub sliver: Arc<Mutex<Option<SliverConnection>>>,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/login", get(login_page).post(login_handler))
        .route("/logout", get(logout_handler))
        .route("/dashboard", get(redirect_dashboard))
        .route("/sessions", get(redirect_sessions))
        .route("/beacons", get(redirect_beacons))
        .route("/listeners", get(redirect_listeners))
        .route("/admin", get(redirect_admin))
        .route("/events", get(redirect_events))
        .route("/payloads", get(redirect_payloads))
        .route("/websites", get(redirect_websites))
        .route("/loot", get(redirect_loot))
        .route("/creds", get(redirect_creds))
        .route("/app", get(spa_shell))
        .route("/audit", get(redirect_audit))
        .route("/", get(root_redirect))
}

async fn root_redirect() -> Redirect {
    Redirect::to("/app")
}

#[derive(Deserialize)]
pub struct LoginForm {
    username: String,
    password: String,
}

async fn login_page(session: Session) -> impl IntoResponse {
    let auth = AuthSession { session };
    // If already logged in, redirect to dashboard
    if auth.authenticated_user().await.is_some() {
        return Redirect::to("/dashboard").into_response();
    }
    Html(include_str!("../../static/login.html")).into_response()
}

async fn login_handler(
    session: Session,
    State(state): State<AppState>,
    Form(form): Form<LoginForm>,
) -> impl IntoResponse {
    let auth = AuthSession { session };

    // Look up user
    let user = sqlx::query_as::<_, db::models::User>(
        "SELECT id, username, password_hash, role::text as role, disabled, created_at, updated_at FROM users WHERE username = $1 AND disabled = false",
    )
    .bind(&form.username)
    .fetch_optional(&state.pool)
    .await;

    match user {
        Ok(Some(user)) => match password::verify_password(&form.password, &user.password_hash) {
            Ok(true) => {
                let role = user.role.parse::<Role>().unwrap_or_else(|_| {
                    tracing::warn!(
                        "Unknown role '{}' for user '{}', downgrading to Viewer",
                        user.role,
                        user.username
                    );
                    Role::Viewer
                });
                auth.login(user.id, &user.username, &role).await;
                Redirect::to("/dashboard").into_response()
            }
            _ => {
                tracing::warn!("Password verification failed for user '{}'", form.username);
                let mut html = String::from(include_str!("../../static/login.html"));
                html = html.replace(
                    "<!-- ERROR_PLACEHOLDER -->",
                    r#"<div class="error-message">Invalid username or password</div>"#,
                );
                Html(html).into_response()
            }
        },
        _ => {
            tracing::warn!(
                "Login failed: user '{}' not found or DB error",
                form.username
            );
            let mut html = String::from(include_str!("../../static/login.html"));
            html = html.replace(
                "<!-- ERROR_PLACEHOLDER -->",
                r#"<div class="error-message">Invalid username or password</div>"#,
            );
            Html(html).into_response()
        }
    }
}

async fn logout_handler(session: Session) -> impl IntoResponse {
    let auth = AuthSession { session };
    auth.logout().await;
    Redirect::to("/login")
}


async fn redirect_dashboard(_user: AuthenticatedUserGuard) -> impl IntoResponse {
    Redirect::to("/app#/dashboard")
}
async fn redirect_sessions(_user: AuthenticatedUserGuard) -> impl IntoResponse {
    Redirect::to("/app#/sessions")
}
async fn redirect_beacons(_user: AuthenticatedUserGuard) -> impl IntoResponse {
    Redirect::to("/app#/beacons")
}
async fn redirect_listeners(_user: AuthenticatedUserGuard) -> impl IntoResponse {
    Redirect::to("/app#/listeners")
}
async fn redirect_admin(_user: AuthenticatedUserGuard) -> impl IntoResponse {
    Redirect::to("/app#/admin")
}
async fn redirect_events(_user: AuthenticatedUserGuard) -> impl IntoResponse {
    Redirect::to("/app#/events")
}
async fn redirect_payloads(_user: AuthenticatedUserGuard) -> impl IntoResponse {
    Redirect::to("/app#/payloads")
}
async fn redirect_websites(_user: AuthenticatedUserGuard) -> impl IntoResponse {
    Redirect::to("/app#/websites")
}
async fn redirect_loot(_user: AuthenticatedUserGuard) -> impl IntoResponse {
    Redirect::to("/app#/loot")
}
async fn redirect_creds(_user: AuthenticatedUserGuard) -> impl IntoResponse {
    Redirect::to("/app#/creds")
}
async fn redirect_audit(_user: AuthenticatedUserGuard) -> impl IntoResponse {
    Redirect::to("/app#/audit")
}
async fn spa_shell(_user: AuthenticatedUserGuard) -> impl IntoResponse {
    Html(include_str!("../../static/app.html"))
}
