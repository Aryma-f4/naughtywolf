use axum::{Router, extract::{Path, State}, response::Json};
use chrono::DateTime;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::actions::{beacons, creds, listeners, loot, payloads, sessions, websites};
use crate::auth::{middleware::AuthenticatedUserGuard, rbac::Role};
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
        .route("/api/listeners", axum::routing::get(list_listeners).post(create_listener))
        .route("/api/listeners/kill/{id}", axum::routing::post(kill_listener))
        .route("/api/payloads", axum::routing::get(list_payloads))
        .route("/api/websites", axum::routing::get(list_websites))
        .route("/api/loot", axum::routing::get(list_loot))
        .route("/api/creds", axum::routing::get(list_creds))
}

async fn list_users(
    user: AuthenticatedUserGuard,
    State(state): State<AppState>,
) -> Result<Json<Vec<UserResponse>>, (axum::http::StatusCode, String)> {
    if user.0.role != Role::Admin {
        return Err((axum::http::StatusCode::FORBIDDEN, "Admin only".to_string()));
    }
    let users = sqlx::query_as::<_, db::models::User>("SELECT id, username, password_hash, role::text as role, disabled, created_at, updated_at FROM users ORDER BY created_at")
        .fetch_all(&state.pool)
        .await
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let response: Vec<UserResponse> = users
        .into_iter()
        .map(|u| UserResponse {
            id: u.id,
            username: u.username,
            role: u.role,
            disabled: u.disabled,
            created_at: u.created_at.to_rfc3339(),
        })
        .collect();

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
            status: if s.is_dead {
                "Dead".to_string()
            } else {
                "Active".to_string()
            },
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
            status: if b.is_dead {
                "Dead".to_string()
            } else {
                "Active".to_string()
            },
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

#[derive(Deserialize)]
pub struct CreateListenerRequest {
    pub protocol: String, // "mtls" | "http"
    pub host: String,
    pub port: u32,
    pub domain: Option<String>,
}

async fn create_listener(
    State(state): State<AppState>,
    _user: AuthenticatedUserGuard,
    axum::Json(req): axum::Json<CreateListenerRequest>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    if req.host.is_empty() {
        return Err((axum::http::StatusCode::BAD_REQUEST, "host is required".to_string()));
    }
    if req.port == 0 {
        return Err((axum::http::StatusCode::BAD_REQUEST, "port must be > 0".to_string()));
    }

    let mut guard = state.sliver.lock().await;
    let conn = guard.as_mut().ok_or_else(|| {
        (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "Sliver not connected".to_string(),
        )
    })?;

    match req.protocol.as_str() {
        "mtls" => {
            let job = listeners::start_mtls(conn, &req.host, req.port)
                .await
                .map_err(|e| (axum::http::StatusCode::BAD_GATEWAY, e))?;
            Ok(Json(serde_json::json!({
                "success": true, "message": "mTLS listener started",
                "job_id": job.id, "protocol": "mtls"
            })))
        }
        "http" => {
            let domain = req.domain.as_deref().unwrap_or("");
            let job = listeners::start_http(conn, &req.host, req.port, domain)
                .await
                .map_err(|e| (axum::http::StatusCode::BAD_GATEWAY, e))?;
            Ok(Json(serde_json::json!({
                "success": true, "message": "HTTP listener started",
                "job_id": job.id, "protocol": "http"
            })))
        }
        _ => Err((axum::http::StatusCode::BAD_REQUEST, format!("unsupported protocol: {}", req.protocol))),
    }
}

async fn kill_listener(
    State(state): State<AppState>,
    _user: AuthenticatedUserGuard,
    Path(id): Path<u32>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    let mut guard = state.sliver.lock().await;
    let conn = guard.as_mut().ok_or_else(|| {
        (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "Sliver not connected".to_string(),
        )
    })?;

    listeners::kill_job(conn, id)
        .await
        .map_err(|e| (axum::http::StatusCode::BAD_GATEWAY, e))?;

    Ok(Json(serde_json::json!({
        "success": true, "message": format!("Job {} killed", id)
    })))
}

async fn list_payloads(
    State(state): State<AppState>,
    _user: AuthenticatedUserGuard,
) -> Result<
    Json<Vec<payloads::ImplantBuildResponse>>,
    (axum::http::StatusCode, String),
> {
    let mut guard = state.sliver.lock().await;
    let conn = guard.as_mut().ok_or_else(|| {
        (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "Sliver not connected".to_string(),
        )
    })?;

    let builds = payloads::list_builds(conn)
        .await
        .map_err(|e| (axum::http::StatusCode::BAD_GATEWAY, e))?;

    Ok(Json(builds))
}

async fn list_websites(
    State(state): State<AppState>,
    _user: AuthenticatedUserGuard,
) -> Result<
    Json<Vec<websites::WebsiteResponse>>,
    (axum::http::StatusCode, String),
> {
    let mut guard = state.sliver.lock().await;
    let conn = guard.as_mut().ok_or_else(|| {
        (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "Sliver not connected".to_string(),
        )
    })?;

    let sites = websites::list_websites(conn)
        .await
        .map_err(|e| (axum::http::StatusCode::BAD_GATEWAY, e))?;

    Ok(Json(sites))
}

async fn list_loot(
    State(state): State<AppState>,
    _user: AuthenticatedUserGuard,
) -> Result<
    Json<Vec<loot::LootResponse>>,
    (axum::http::StatusCode, String),
> {
    let mut guard = state.sliver.lock().await;
    let conn = guard.as_mut().ok_or_else(|| {
        (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "Sliver not connected".to_string(),
        )
    })?;

    let items = loot::list_loot(conn)
        .await
        .map_err(|e| (axum::http::StatusCode::BAD_GATEWAY, e))?;

    Ok(Json(items))
}

async fn list_creds(
    State(state): State<AppState>,
    _user: AuthenticatedUserGuard,
) -> Result<
    Json<Vec<creds::CredResponse>>,
    (axum::http::StatusCode, String),
> {
    let mut guard = state.sliver.lock().await;
    let conn = guard.as_mut().ok_or_else(|| {
        (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "Sliver not connected".to_string(),
        )
    })?;

    let credentials = creds::list_creds(conn)
        .await
        .map_err(|e| (axum::http::StatusCode::BAD_GATEWAY, e))?;

    Ok(Json(credentials))
}
