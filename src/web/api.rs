use axum::{
    extract::State,
    response::Json,
    Router,
};
use chrono::DateTime;
use serde::Serialize;
use uuid::Uuid;

use crate::actions::{beacons, listeners, sessions};
use crate::auth::{
    middleware::AuthenticatedUserGuard,
    rbac::Role,
};
use crate::db;
use crate::web::routes::AppState;

#[derive(Serialize)]
pub struct UserResponse {
    pub id: Uuid,
    pub username: String,
    pub role: String,
    pub disabled: bool,
    pub created_at: String,
}

#[derive(Serialize)]
pub struct DashboardStats {
    pub active_listeners: u32,
    pub sessions: u32,
    pub beacons: u32,
    pub jobs: u32,
}

#[derive(Serialize)]
pub struct SessionResponse {
    pub id: String,
    pub name: String,
    pub hostname: String,
    pub username: String,
    pub transport: String,
    pub last_checkin: String,
    pub status: String,
}

#[derive(Serialize)]
pub struct BeaconResponse {
    pub id: String,
    pub name: String,
    pub hostname: String,
    pub username: String,
    pub transport: String,
    pub remote_address: String,
    pub last_checkin: String,
    pub next_checkin: String,
    pub interval: String,
    pub jitter: String,
    pub status: String,
}

#[derive(Serialize)]
pub struct ListenerResponse {
    pub id: u32,
    pub name: String,
    pub protocol: String,
    pub port: u32,
    pub description: String,
    pub domains: Vec<String>,
    pub profile_name: String,
    pub bind: String,
    pub status: String,
}

fn ts_to_string(ts: i64) -> String {
    if ts == 0 {
        return "N/A".to_string();
    }
    match DateTime::from_timestamp(ts, 0) {
        Some(dt) => dt.to_rfc3339(),
        None => "N/A".to_string(),
    }
}

pub fn api_routes() -> Router<AppState> {
    Router::new()
        .route("/api/users", axum::routing::get(list_users))
        .route("/api/dashboard/stats", axum::routing::get(dashboard_stats))
        .route("/api/sessions", axum::routing::get(list_sessions))
        .route("/api/beacons", axum::routing::get(list_beacons))
        .route("/api/listeners", axum::routing::get(list_listeners))
}

async fn list_users(
    user: AuthenticatedUserGuard,
    State(state): State<AppState>,
) -> Result<Json<Vec<UserResponse>>, (axum::http::StatusCode, String)> {
    if user.0.role != Role::Admin {
        return Err((axum::http::StatusCode::FORBIDDEN, "Admin only".to_string()));
    }
    let users = sqlx::query_as::<_, db::models::User>(
        "SELECT * FROM users ORDER BY created_at"
    )
    .fetch_all(&state.pool)
    .await
    .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let response: Vec<UserResponse> = users.into_iter().map(|u| UserResponse {
        id: u.id,
        username: u.username,
        role: u.role,
        disabled: u.disabled,
        created_at: u.created_at.to_rfc3339(),
    }).collect();

    Ok(Json(response))
}

async fn dashboard_stats(
    State(state): State<AppState>,
    _user: AuthenticatedUserGuard,
) -> Result<Json<DashboardStats>, (axum::http::StatusCode, String)> {
    let mut guard = state.sliver.lock().await;
    let conn = match guard.as_mut() {
        Some(c) => c,
        None => {
            return Ok(Json(DashboardStats {
                active_listeners: 0,
                sessions: 0,
                beacons: 0,
                jobs: 0,
            }));
        }
    };

    let jobs = match listeners::list_jobs(conn).await {
        Ok(j) => j.len() as u32,
        Err(_) => 0,
    };
    let sessions = match sessions::list_sessions(conn).await {
        Ok(s) => s.len() as u32,
        Err(_) => 0,
    };
    let beacons = match beacons::list_beacons(conn).await {
        Ok(b) => b.len() as u32,
        Err(_) => 0,
    };

    Ok(Json(DashboardStats {
        active_listeners: jobs,
        sessions,
        beacons,
        jobs,
    }))
}

async fn list_sessions(
    State(state): State<AppState>,
    _user: AuthenticatedUserGuard,
) -> Result<Json<Vec<SessionResponse>>, (axum::http::StatusCode, String)> {
    let mut guard = state.sliver.lock().await;
    let conn = guard.as_mut().ok_or_else(|| {
        (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "Sliver not connected".to_string(),
        )
    })?;

    let sliver_sessions = sessions::list_sessions(conn)
        .await
        .map_err(|e| (axum::http::StatusCode::BAD_GATEWAY, e))?;

    let response: Vec<SessionResponse> = sliver_sessions
        .into_iter()
        .map(|s| SessionResponse {
            id: s.id,
            name: s.name,
            hostname: s.hostname,
            username: s.username,
            transport: s.transport,
            last_checkin: ts_to_string(s.last_checkin),
            status: if s.is_dead { "Dead".to_string() } else { "Active".to_string() },
        })
        .collect();

    Ok(Json(response))
}

async fn list_beacons(
    State(state): State<AppState>,
    _user: AuthenticatedUserGuard,
) -> Result<Json<Vec<BeaconResponse>>, (axum::http::StatusCode, String)> {
    let mut guard = state.sliver.lock().await;
    let conn = guard.as_mut().ok_or_else(|| {
        (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "Sliver not connected".to_string(),
        )
    })?;

    let sliver_beacons = beacons::list_beacons(conn)
        .await
        .map_err(|e| (axum::http::StatusCode::BAD_GATEWAY, e))?;

    let response: Vec<BeaconResponse> = sliver_beacons
        .into_iter()
        .map(|b| BeaconResponse {
            id: b.id,
            name: b.name,
            hostname: b.hostname,
            username: b.username,
            transport: b.transport,
            remote_address: b.remote_address,
            last_checkin: ts_to_string(b.last_checkin),
            next_checkin: ts_to_string(b.next_checkin),
            interval: format!("{}s", b.interval),
            jitter: format!("{}s", b.jitter),
            status: if b.is_dead { "Dead".to_string() } else { "Active".to_string() },
        })
        .collect();

    Ok(Json(response))
}

async fn list_listeners(
    State(state): State<AppState>,
    _user: AuthenticatedUserGuard,
) -> Result<Json<Vec<ListenerResponse>>, (axum::http::StatusCode, String)> {
    let mut guard = state.sliver.lock().await;
    let conn = guard.as_mut().ok_or_else(|| {
        (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "Sliver not connected".to_string(),
        )
    })?;

    let jobs = listeners::list_jobs(conn)
        .await
        .map_err(|e| (axum::http::StatusCode::BAD_GATEWAY, e))?;

    let response: Vec<ListenerResponse> = jobs
        .into_iter()
        .map(|j| ListenerResponse {
            id: j.id,
            name: j.name,
            protocol: j.protocol,
            port: j.port,
            description: j.description,
            domains: j.domains,
            profile_name: j.profile_name,
            bind: format!("0.0.0.0:{}", j.port),
            status: "Running".to_string(),
        })
        .collect();

    Ok(Json(response))
}
