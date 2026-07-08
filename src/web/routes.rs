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
use crate::sliver::events::SliverEvent;
use crate::sliver::connection::SliverConnection;
use crate::web::templates;
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex};

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
        .route("/dashboard", get(dashboard_page))
        .route("/sessions", get(sessions_page))
        .route("/beacons", get(beacons_page))
        .route("/listeners", get(listeners_page))
        .route("/admin", get(admin_page))
        .route("/events", get(events_page))
        .route("/audit", get(audit_page))
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
                    let role = Role::from_str(&user.role).unwrap_or_else(|| {
                        tracing::warn!("Unknown role '{}' for user '{}', downgrading to Viewer", user.role, user.username);
                        Role::Viewer
                    });
                    auth.login(user.id, &user.username, &role).await;
                    Redirect::to("/dashboard").into_response()
                }
                _ => {
                    tracing::warn!("Password verification failed for user '{}'", form.username);
                    let mut html = String::from(include_str!("../../static/login.html"));
                    html = html.replace("<!-- ERROR_PLACEHOLDER -->",
                        r#"<div class="error-message">Invalid username or password</div>"#);
                    Html(html).into_response()
                }
            }
        }
        _ => {
            tracing::warn!("Login failed: user '{}' not found or DB error", form.username);
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

async fn sessions_page(user: AuthenticatedUserGuard) -> impl IntoResponse {
    let ctx = templates::PageContext {
        title: "Sessions".to_string(),
        current_page: "sessions",
        username: user.0.username.clone(),
        role: user.0.role.to_string(),
        profile_name: None,
    };
    let content = r#"
<div class="card">
    <h3>Sessions</h3>
    <p>Active Sliver sessions will be listed here in a later task.</p>
    <table class="data-table">
        <thead><tr><th>ID</th><th>Name</th><th>Hostname</th><th>Transport</th><th>Last Checkin</th><th>Status</th></tr></thead>
        <tbody id="sessions-tbody"></tbody>
    </table>
</div>
<script>
    fetch('/api/sessions').then(r=>r.json()).then(sessions=>{
        const tbody=document.getElementById('sessions-tbody');
        sessions.forEach(s=>{
            const tr=document.createElement('tr');
            tr.innerHTML=`<td>${s.id}</td><td>${s.name}</td><td>${s.hostname}</td><td>${s.transport}</td><td>${s.last_checkin}</td><td>${s.status}</td>`;
            tbody.appendChild(tr);
        });
    });
</script>
"#.to_string();
    templates::render_page(&ctx, &content)
}

async fn beacons_page(user: AuthenticatedUserGuard) -> impl IntoResponse {
    let ctx = templates::PageContext {
        title: "Beacons".to_string(),
        current_page: "beacons",
        username: user.0.username.clone(),
        role: user.0.role.to_string(),
        profile_name: None,
    };
    let content = r#"
<div class="card">
    <h3>Beacons</h3>
    <p>Active Sliver beacons will be listed here in a later task.</p>
    <table class="data-table">
        <thead><tr><th>ID</th><th>Name</th><th>Hostname</th><th>Transport</th><th>Last Checkin</th><th>Status</th></tr></thead>
        <tbody id="beacons-tbody"></tbody>
    </table>
</div>
<script>
    fetch('/api/beacons').then(r=>r.json()).then(beacons=>{
        const tbody=document.getElementById('beacons-tbody');
        beacons.forEach(b=>{
            const tr=document.createElement('tr');
            tr.innerHTML=`<td>${b.id}</td><td>${b.name}</td><td>${b.hostname}</td><td>${b.transport}</td><td>${b.last_checkin}</td><td>${b.status}</td>`;
            tbody.appendChild(tr);
        });
    });
</script>
"#.to_string();
    templates::render_page(&ctx, &content)
}

async fn listeners_page(user: AuthenticatedUserGuard) -> impl IntoResponse {
    let can_act = user.0.role.can_perform(&Role::Operator);
    let ctx = templates::PageContext {
        title: "Listeners".to_string(),
        current_page: "listeners",
        username: user.0.username.clone(),
        role: user.0.role.to_string(),
        profile_name: None,
    };
    let content = format!(r#"
<div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:1rem;">
    <h3 style="color:var(--text-muted);font-size:0.85rem;text-transform:uppercase;letter-spacing:0.5px;">Active Listeners</h3>
    {action_btn}
</div>
<div class="card">
    <table class="data-table">
        <thead><tr><th>ID</th><th>Protocol</th><th>Bind Address</th><th>Status</th></tr></thead>
        <tbody id="listeners-tbody"></tbody>
    </table>
</div>
<script>
    fetch('/api/listeners').then(r=>r.json()).then(listeners=>{{
        const tbody=document.getElementById('listeners-tbody');
        listeners.forEach(l=>{{
            const tr=document.createElement('tr');
            tr.innerHTML=`<td>${{l.id}}</td><td>${{l.protocol}}</td><td>${{l.bind}}</td><td>${{l.status}}</td>`;
            tbody.appendChild(tr);
        }});
    }});
</script>
"#,
        action_btn = if can_act {
            r#"<button class="btn btn-primary btn-sm" onclick="alert('Listener creation coming in a later task')">+ New Listener</button>"#
        } else {
            r#"<span style="color:var(--text-muted);font-size:0.85rem;" title="Read-only">Actions disabled</span>"#
        }
    );
    templates::render_page(&ctx, &content)
}

async fn admin_page(user: AuthenticatedUserGuard) -> impl IntoResponse {
    if user.0.role == Role::Viewer {
        let ctx = templates::PageContext {
            title: "Admin".to_string(),
            current_page: "admin",
            username: user.0.username.clone(),
            role: user.0.role.to_string(),
            profile_name: None,
        };
        let content = r#"<div class="card"><p class="error-message">Access denied. Admin or Operator role required.</p></div>"#.to_string();
        return templates::render_page(&ctx, &content).into_response();
    }

    let ctx = templates::PageContext {
        title: "Admin".to_string(),
        current_page: "admin",
        username: user.0.username.clone(),
        role: user.0.role.to_string(),
        profile_name: None,
    };
    let can_manage_users = if user.0.role == Role::Admin { "true" } else { "false" };
    let content = format!(r#"
<div class="card">
    <h3>User Management</h3>
    <p>Manage NaughtyWolf users.</p>
    <table id="user-table" class="data-table">
        <thead><tr><th>Username</th><th>Role</th><th>Status</th><th>Actions</th></tr></thead>
        <tbody></tbody>
    </table>
</div>
<script>
    const canManage = {can_manage_users};
    fetch('/api/users').then(r=>r.json()).then(users=>{{
        const tbody=document.querySelector('#user-table tbody');
        users.forEach(u=>{{
            const tr=document.createElement('tr');
            const actions = canManage
                ? '<button class="btn-sm" onclick="alert(\'Toggle user: '+u.username+'\')">Toggle Disabled</button>'
                : '—';
            tr.innerHTML=`<td>${{u.username}}</td><td>${{u.role}}</td><td>${{u.disabled ? 'Disabled' : 'Active'}}</td><td>${{actions}}</td>`;
            tbody.appendChild(tr);
        }});
    }});
</script>
"#, can_manage_users = can_manage_users);
    templates::render_page(&ctx, &content).into_response()
}

async fn events_page(user: AuthenticatedUserGuard) -> impl IntoResponse {
    let ctx = templates::PageContext {
        title: "Events".to_string(),
        current_page: "events",
        username: user.0.username.clone(),
        role: user.0.role.to_string(),
        profile_name: None,
    };
    let content = r#"
<div class="card">
    <h3>Live Event Stream</h3>
    <div id="event-feed" style="padding:0.5rem 0; font-size:0.85rem; height:400px; overflow-y:auto; background:var(--bg-secondary); border-radius:4px; font-family:monospace;">
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
        if (feed.children.length > 200) feed.removeChild(feed.lastChild);
    };
</script>
"#.to_string();
    templates::render_page(&ctx, &content)
}

async fn audit_page(user: AuthenticatedUserGuard) -> impl IntoResponse {
    let ctx = templates::PageContext {
        title: "Audit Log".to_string(),
        current_page: "audit",
        username: user.0.username.clone(),
        role: user.0.role.to_string(),
        profile_name: None,
    };
    let content = r#"
<div class="card">
    <h3>Audit Log</h3>
    <p>Audit trail of actions performed through NaughtyWolf.</p>
    <table class="data-table">
        <thead><tr><th>Time</th><th>User</th><th>Action</th><th>Target</th><th>Status</th></tr></thead>
        <tbody><tr><td colspan="5" style="text-align:center;color:var(--text-muted);">Audit log not yet implemented.</td></tr></tbody>
    </table>
</div>
"#.to_string();
    templates::render_page(&ctx, &content)
}
