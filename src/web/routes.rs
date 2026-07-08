use axum::{
    extract::State,
    response::{Html, IntoResponse, Redirect},
    Form, Router,
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
use crate::web::templates;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/login", get(login_page).post(login_handler))
        .route("/logout", get(logout_handler))
        .route("/dashboard", get(dashboard_page))
        .route("/", get(root_redirect))
}

async fn root_redirect() -> Redirect {
    Redirect::to("/dashboard")
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
        "SELECT * FROM users WHERE username = $1 AND disabled = false"
    )
    .bind(&form.username)
    .fetch_optional(&state.pool)
    .await;

    match user {
        Ok(Some(user)) => {
            match password::verify_password(&form.password, &user.password_hash) {
                Ok(true) => {
                    let role = Role::from_str(&user.role).unwrap_or(Role::Viewer);
                    auth.login(user.id, &user.username, &role).await;
                    Redirect::to("/dashboard").into_response()
                }
                _ => {
                    let mut html = String::from(include_str!("../../static/login.html"));
                    html = html.replace("<!-- ERROR_PLACEHOLDER -->",
                        r#"<div class="error-message">Invalid username or password</div>"#);
                    Html(html).into_response()
                }
            }
        }
        _ => {
            let mut html = String::from(include_str!("../../static/login.html"));
            html = html.replace("<!-- ERROR_PLACEHOLDER -->",
                r#"<div class="error-message">Invalid username or password</div>"#);
            Html(html).into_response()
        }
    }
}

async fn logout_handler(session: Session) -> impl IntoResponse {
    let auth = AuthSession { session };
    auth.logout().await;
    Redirect::to("/login")
}

async fn dashboard_page(user: AuthenticatedUserGuard) -> impl IntoResponse {
    let ctx = templates::PageContext {
        title: "Dashboard".to_string(),
        current_page: "dashboard",
        username: user.0.username.clone(),
        role: user.0.role.to_string(),
        profile_name: None,
    };
    let content = r#"
<div class="stats-grid">
    <div class="card"><h3>Active Listeners</h3><div class="value" id="listener-count">—</div></div>
    <div class="card"><h3>Sessions</h3><div class="value" id="session-count">—</div></div>
    <div class="card"><h3>Beacons</h3><div class="value" id="beacon-count">—</div></div>
    <div class="card"><h3>Jobs</h3><div class="value" id="job-count">—</div></div>
</div>
<div class="card">
    <h3>Recent Events</h3>
    <div id="event-feed" style="padding:0.5rem 0; font-size:0.85rem; color:var(--text-muted);">
        Waiting for connection...
    </div>
</div>
<script>
    const evtSource = new EventSource('/api/events');
    const feed = document.getElementById('event-feed');
    evtSource.onmessage = function(e) {
        const line = document.createElement('div');
        line.textContent = e.data;
        feed.prepend(line);
        if (feed.children.length > 100) feed.removeChild(feed.lastChild);
    };
</script>
"#.to_string();
    templates::render_page(&ctx, &content)
}
