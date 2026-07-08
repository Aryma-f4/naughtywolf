use axum::{
    extract::State,
    response::Json,
    Router,
};
use serde::Serialize;
use uuid::Uuid;

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
    _user: AuthenticatedUserGuard,
) -> Result<Json<DashboardStats>, (axum::http::StatusCode, String)> {
    Ok(Json(DashboardStats {
        active_listeners: 0,
        sessions: 0,
        beacons: 0,
        jobs: 0,
    }))
}

async fn list_sessions(
    _user: AuthenticatedUserGuard,
) -> Result<Json<Vec<SessionResponse>>, (axum::http::StatusCode, String)> {
    Ok(Json(vec![]))
}

async fn list_beacons(
    _user: AuthenticatedUserGuard,
) -> Result<Json<Vec<SessionResponse>>, (axum::http::StatusCode, String)> {
    Ok(Json(vec![]))
}

async fn list_listeners(
    _user: AuthenticatedUserGuard,
) -> Result<Json<Vec<serde_json::Value>>, (axum::http::StatusCode, String)> {
    Ok(Json(vec![]))
}
