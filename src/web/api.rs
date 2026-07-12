use axum::{Router, extract::{Path, State}, response::Json};
use chrono::DateTime;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::actions::{admin, agents, beacons, creds, hosts, listeners, loot, modules, payloads, pivots, reports, sessions, sliver, stagers, websites};
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
    pub first_agent_ts: Option<String>,
    pub operation_seconds: i64,
    pub recent_event_count: u32,
}

#[derive(Serialize)]
pub struct TopTarget {
    pub hostname: String,
    pub agent_count: u32,
    pub last_seen: Option<String>,
}

#[derive(Serialize)]
pub struct DashboardEnriched {
    pub stats: DashboardStats,
    pub top_targets: Vec<TopTarget>,
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
        .route("/api/agents", axum::routing::get(list_agents_handler))
        .route("/api/agents/{id}", axum::routing::get(get_agent_handler))
        .route("/api/listeners", axum::routing::get(list_listeners).post(create_listener))
        .route("/api/listeners/kill/{id}", axum::routing::post(kill_listener))
        .route("/api/payloads", axum::routing::get(list_payloads))
        .route("/api/payloads/generate", axum::routing::post(generate_payload_handler))
        .route("/api/payloads/regenerate/{name}", axum::routing::post(regenerate_payload_handler))
        .route("/api/payloads/download/{name}", axum::routing::get(download_payload_handler))
        .route("/api/websites", axum::routing::get(list_websites))
        .route("/api/loot", axum::routing::get(list_loot))
        .route("/api/creds", axum::routing::get(list_creds_handler).post(create_cred_handler))
        .route("/api/creds/{id}", axum::routing::put(update_cred_handler).delete(delete_cred_handler))
        .route("/api/creds/{id}/crack", axum::routing::post(crack_cred_handler))
        .route("/api/hosts", axum::routing::get(list_hosts_handler))
        .route("/api/sliver/connect", axum::routing::post(sliver_connect))
        .route("/api/sliver/disconnect", axum::routing::post(sliver_disconnect))
        .route("/api/sliver/status", axum::routing::get(sliver_status))
        .route("/api/shell/exec", axum::routing::post(shell_exec_handler))
        .route("/api/agents/{id}/tasks/shell", axum::routing::post(task_shell_handler))
        .route("/api/agents/{id}/tasks/execute", axum::routing::post(task_execute_handler))
        .route("/api/agents/{id}/tasks", axum::routing::get(list_tasks_handler))
        .route("/api/agents/{id}/fs/ls", axum::routing::post(agent_ls_handler))
        .route("/api/agents/{id}/fs/download", axum::routing::get(agent_download_handler))
        .route("/api/agents/{id}/fs/upload", axum::routing::post(agent_upload_handler))
        .route("/api/modules", axum::routing::get(list_modules_handler))
        .route("/api/agents/{id}/modules/{name}/exec", axum::routing::post(exec_module_handler))
        .route("/api/stagers", axum::routing::get(list_stagers_handler).post(create_stager_handler))
        .route("/api/stagers/{id}", axum::routing::put(update_stager_handler).delete(delete_stager_handler))
        .route("/api/stagers/{id}/generate", axum::routing::post(generate_stager_handler))
        .route("/api/pivots", axum::routing::get(list_pivots_handler))
        .route("/api/pivots/graph", axum::routing::get(pivot_graph_handler))
        .route("/api/pivots/start", axum::routing::post(start_pivot_handler))
        .route("/api/pivots/stop", axum::routing::post(stop_pivot_handler))
        .route("/api/agents/{id}/portfwd", axum::routing::post(create_portfwd_handler))
        .route("/api/agents/{id}/socks", axum::routing::post(start_socks_handler))
        .route("/api/reports/sessions", axum::routing::get(report_sessions_handler))
        .route("/api/reports/credentials", axum::routing::get(report_credentials_handler))
        .route("/api/reports/hosts", axum::routing::get(report_hosts_handler))
        .route("/api/reports/timeline", axum::routing::get(report_timeline_handler))
        .route("/api/admin/users/{id}", axum::routing::put(admin_update_user_handler).delete(admin_delete_user_handler))
        .route("/api/admin/settings", axum::routing::get(admin_list_settings_handler))
        .route("/api/admin/settings/{key}", axum::routing::put(admin_upsert_setting_handler))
        .route("/api/health", axum::routing::get(health_handler))
        .route("/api/meta", axum::routing::get(meta_handler))
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
) -> Result<Json<DashboardEnriched>, (axum::http::StatusCode, String)> {
    let mut stats = DashboardStats {
        active_listeners: 0,
        sessions: 0,
        beacons: 0,
        jobs: 0,
        first_agent_ts: None,
        operation_seconds: 0,
        recent_event_count: 0,
    };

    // Sliver-backed stats
    if let Ok(mut guard) = state.sliver.try_lock() {
        if let Some(conn) = guard.as_mut() {
            if let Ok(j) = listeners::list_jobs(conn).await { stats.active_listeners = j.len() as u32; stats.jobs = stats.active_listeners; }
            if let Ok(s) = sessions::list_sessions(conn).await { stats.sessions = s.len() as u32; }
            if let Ok(b) = beacons::list_beacons(conn).await { stats.beacons = b.len() as u32; }

            // Earliest last_checkin across all sessions/beacons for operation timer
            // Sessions are kept in-memory by Sliver, not persisted to our DB.
            // So we compute the operation timer from local audit log instead.
        }
    }

    // Operation timer from earliest audit_events row
    if let Ok((first_ts,)) = sqlx::query_as::<_, (Option<i64>,)>(
        "SELECT EXTRACT(EPOCH FROM MIN(created_at))::bigint FROM audit_events"
    ).fetch_one(&state.pool).await {
        if let Some(ts) = first_ts {
            stats.first_agent_ts = Some(ts_to_string(ts));
            stats.operation_seconds = chrono::Utc::now().timestamp().max(ts) - ts;
        }
    }

    // DB-backed stats
    if let Ok((c,)) = sqlx::query_as::<_, (i64,)>(
        "SELECT COUNT(*) FROM audit_events WHERE created_at > now() - interval '1 hour'"
    ).fetch_one(&state.pool).await {
        stats.recent_event_count = c as u32;
    }

    // Top targets: group by hostname in credentials
    let top_rows: Result<Vec<(String, i64, Option<String>)>, _> = sqlx::query_as(
        "SELECT host, COUNT(*)::bigint, MAX(created_at)::text
         FROM credentials WHERE host != '' GROUP BY host
         ORDER BY COUNT(*) DESC, MAX(created_at) DESC LIMIT 5",
    ).fetch_all(&state.pool).await;
    let top_targets = top_rows.unwrap_or_default().into_iter().map(|r| TopTarget {
        hostname: r.0, agent_count: r.1 as u32, last_seen: r.2,
    }).collect();

    Ok(Json(DashboardEnriched { stats, top_targets }))
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

async fn list_agents_handler(
    State(state): State<AppState>,
    _user: AuthenticatedUserGuard,
) -> Result<Json<Vec<agents::AgentResponse>>, (axum::http::StatusCode, String)> {
    let mut guard = state.sliver.lock().await;
    let conn = guard.as_mut().ok_or_else(|| {
        (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "Sliver not connected".to_string(),
        )
    })?;

    let all = agents::list_agents(conn)
        .await
        .map_err(|e| (axum::http::StatusCode::BAD_GATEWAY, e))?;

    Ok(Json(all))
}

async fn get_agent_handler(
    State(state): State<AppState>,
    _user: AuthenticatedUserGuard,
    Path(id): Path<String>,
) -> Result<Json<agents::AgentResponse>, (axum::http::StatusCode, String)> {
    let mut guard = state.sliver.lock().await;
    let conn = guard.as_mut().ok_or_else(|| {
        (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "Sliver not connected".to_string(),
        )
    })?;

    let all = agents::list_agents(conn)
        .await
        .map_err(|e| (axum::http::StatusCode::BAD_GATEWAY, e))?;

    all.into_iter()
        .find(|a| a.id == id)
        .ok_or_else(|| {
            (
                axum::http::StatusCode::NOT_FOUND,
                format!("Agent '{}' not found", id),
            )
        })
        .map(Json)
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

async fn list_hosts_handler(
    State(state): State<AppState>,
    _user: AuthenticatedUserGuard,
) -> Result<Json<Vec<hosts::HostResponse>>, (axum::http::StatusCode, String)> {
    let mut guard = state.sliver.lock().await;
    let conn = guard.as_mut().ok_or_else(|| {
        (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "Sliver not connected".to_string(),
        )
    })?;

    let hosts = hosts::list_hosts(conn)
        .await
        .map_err(|e| (axum::http::StatusCode::BAD_GATEWAY, e))?;

    Ok(Json(hosts))
}

async fn list_payloads(
    State(state): State<AppState>,
    _user: AuthenticatedUserGuard,
) -> Json<Vec<payloads::ImplantBuildResponse>> {
    let mut builds = Vec::new();

    // Try gRPC if connected
    if let Ok(mut guard) = state.sliver.try_lock()
        && let Some(conn) = guard.as_mut()
        && let Ok(server_builds) = payloads::list_builds(conn).await
    {
        builds.extend(server_builds);
    }

    // Scan local payloads directory for CLI-generated files
    if let Ok(local) = payloads::list_local_payloads().await {
        builds.extend(local);
    }

    Json(builds)
}

async fn generate_payload_handler(
    State(state): State<AppState>,
    _user: AuthenticatedUserGuard,
    axum::Json(req): axum::Json<payloads::GeneratePayloadRequest>,
) -> Json<payloads::GenerateResponse> {
    let name = req.name.clone();
    // Spawn in background so HTTP returns immediately; brief mutex lock
    let sliver = state.sliver.clone();
    tokio::spawn(async move {
        let mut guard = sliver.lock().await;
        if let Some(conn) = guard.as_mut() {
            let result = payloads::generate_implant(conn, req).await;
            tracing::info!("Generate done: {:?}", result);
        }
    });

    Json(payloads::GenerateResponse {
        success: true,
        message: format!("Generation started for '{}' — check payloads page shortly", name),
        implant_name: Some(name),
        output_path: None,
    })
}

async fn regenerate_payload_handler(
    State(state): State<AppState>,
    _user: AuthenticatedUserGuard,
    Path(name): Path<String>,
) -> Json<payloads::GenerateResponse> {
    let sliver = state.sliver.clone();
    tokio::spawn(async move {
        let mut guard = sliver.lock().await;
        if let Some(conn) = guard.as_mut() {
            let result = payloads::regenerate_implant(conn, name).await;
            tracing::info!("Regenerate done: {:?}", result);
        }
    });
    Json(payloads::GenerateResponse {
        success: true,
        message: "Regeneration started".into(),
        implant_name: None,
        output_path: None,
    })
}

async fn download_payload_handler(
    Path(name): Path<String>,
    _user: AuthenticatedUserGuard,
) -> Result<axum::response::Response<axum::body::Body>, (axum::http::StatusCode, String)> {
    use axum::body::Body;
    use axum::response::Response;
    use tokio::fs;

    // Search ./payloads directory for matching file
    let save_dir = std::path::Path::new("./payloads");
    if !save_dir.exists() {
        return Err((axum::http::StatusCode::NOT_FOUND, "No payloads directory".to_string()));
    }

    let mut entries = fs::read_dir(save_dir)
        .await
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut found_path = None;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        let fname = entry.file_name().to_string_lossy().to_string();
        if fname == name || fname.starts_with(&name) {
            found_path = Some(entry.path());
            break;
        }
    }

    match found_path {
        Some(path) => {
            let data = fs::read(&path)
                .await
                .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            let filename = path.file_name().unwrap_or_default().to_string_lossy().to_string();
            let body = Body::from(data);
            let _headers = [
                ("Content-Type", "application/octet-stream"),
                ("Content-Disposition", &format!("attachment; filename=\"{}\"", filename)),
            ];
            Ok(Response::builder()
                .status(200)
                .header("Content-Type", "application/octet-stream")
                .header("Content-Disposition", format!("attachment; filename=\"{}\"", filename))
                .body(body)
                .unwrap())
        }
        None => Err((axum::http::StatusCode::NOT_FOUND, format!("Payload '{}' not found in ./payloads", name))),
    }
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

async fn list_creds_handler(
    State(state): State<AppState>,
    _user: AuthenticatedUserGuard,
) -> Result<Json<Vec<creds::CredResponse>>, (axum::http::StatusCode, String)> {
    let mut results = Vec::new();

    // Fetch from local DB
    if let Ok(local) = creds::list_local_creds(&state.pool).await {
        results.extend(local);
    }

    // Also fetch from Sliver gRPC if connected
    if let Ok(mut guard) = state.sliver.try_lock() {
        if let Some(conn) = guard.as_mut() {
            if let Ok(server_creds) = creds::list_creds(conn).await {
                results.extend(server_creds);
            }
        }
    }

    Ok(Json(results))
}

async fn create_cred_handler(
    State(state): State<AppState>,
    user: AuthenticatedUserGuard,
    axum::Json(req): axum::Json<creds::CreateCredRequest>,
) -> Result<Json<creds::CredResponse>, (axum::http::StatusCode, String)> {
    if user.0.role != Role::Admin && user.0.role != Role::Operator {
        return Err((axum::http::StatusCode::FORBIDDEN, "Operator or Admin role required".to_string()));
    }
    let result = creds::create_cred(&state.pool, req).await
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;
    crate::actions::audit_action(
        &state.pool,
        &user.0,
        None,
        "create_cred",
        "credential",
        Some(result.id.clone()),
        None,
        "success",
    ).await;
    Ok(Json(result))
}

async fn update_cred_handler(
    State(state): State<AppState>,
    user: AuthenticatedUserGuard,
    Path(id): Path<String>,
    axum::Json(req): axum::Json<creds::UpdateCredRequest>,
) -> Result<Json<creds::CredResponse>, (axum::http::StatusCode, String)> {
    if user.0.role != Role::Admin && user.0.role != Role::Operator {
        return Err((axum::http::StatusCode::FORBIDDEN, "Operator or Admin role required".to_string()));
    }
    let result = creds::update_cred(&state.pool, &id, req).await
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;
    crate::actions::audit_action(
        &state.pool,
        &user.0,
        None,
        "update_cred",
        "credential",
        Some(id.clone()),
        None,
        "success",
    ).await;
    Ok(Json(result))
}

async fn delete_cred_handler(
    State(state): State<AppState>,
    user: AuthenticatedUserGuard,
    Path(id): Path<String>,
) -> Result<axum::http::StatusCode, (axum::http::StatusCode, String)> {
    if user.0.role != Role::Admin {
        return Err((axum::http::StatusCode::FORBIDDEN, "Admin role required".to_string()));
    }
    creds::delete_cred(&state.pool, &id).await
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;
    crate::actions::audit_action(
        &state.pool,
        &user.0,
        None,
        "delete_cred",
        "credential",
        Some(id.clone()),
        None,
        "success",
    ).await;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct CrackRequest {
    plaintext: String,
}

async fn crack_cred_handler(
    State(state): State<AppState>,
    user: AuthenticatedUserGuard,
    Path(id): Path<String>,
    axum::Json(req): axum::Json<CrackRequest>,
) -> Result<Json<creds::CredResponse>, (axum::http::StatusCode, String)> {
    if user.0.role != Role::Admin && user.0.role != Role::Operator {
        return Err((axum::http::StatusCode::FORBIDDEN, "Operator or Admin role required".to_string()));
    }
    if req.plaintext.is_empty() {
        return Err((axum::http::StatusCode::BAD_REQUEST, "plaintext is required".to_string()));
    }
    let result = creds::mark_cracked(&state.pool, &id, &req.plaintext).await
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;
    crate::actions::audit_action(
        &state.pool,
        &user.0,
        None,
        "crack_cred",
        "credential",
        Some(id.clone()),
        None,
        "success",
    ).await;
    Ok(Json(result))
}

async fn sliver_connect(
    State(state): State<AppState>,
    _user: AuthenticatedUserGuard,
    axum::Json(req): axum::Json<sliver::ConnectRequest>,
) -> Result<axum::Json<sliver::SliverStatusResponse>, (axum::http::StatusCode, String)> {
    if req.config_path.is_empty() {
        return Err((axum::http::StatusCode::BAD_REQUEST, "config_path is required".to_string()));
    }
    let status = sliver::connect(&state.sliver, &req.config_path, state.event_tx)
        .await
        .map_err(|e| (axum::http::StatusCode::BAD_GATEWAY, e))?;
    Ok(axum::Json(status))
}

async fn sliver_disconnect(
    State(state): State<AppState>,
    _user: AuthenticatedUserGuard,
) -> axum::Json<sliver::SliverStatusResponse> {
    let status = sliver::disconnect(&state.sliver, state.event_tx).await;
    axum::Json(status)
}

async fn sliver_status(
    State(state): State<AppState>,
    _user: AuthenticatedUserGuard,
) -> axum::Json<sliver::SliverStatusResponse> {
    let status = sliver::status(&state.sliver);
    axum::Json(status)
}

// ---- Agent Task Execution Handlers ----

async fn task_shell_handler(
    State(state): State<AppState>,
    _user: AuthenticatedUserGuard,
    Path(id): Path<String>,
    axum::Json(req): axum::Json<agents::TaskRequest>,
) -> Result<Json<agents::TaskResponse>, (axum::http::StatusCode, String)> {
    let mut guard = state.sliver.lock().await;
    let conn = guard.as_mut().ok_or_else(|| {
        (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "Sliver not connected".to_string(),
        )
    })?;

    // Override action to "shell" since this is the shell route
    let task_req = agents::TaskRequest {
        action: "shell".to_string(),
        args: req.args,
        exec_path: req.exec_path,
    };

    let resp = agents::exec_task(conn, &id, task_req)
        .await
        .map_err(|e| (axum::http::StatusCode::BAD_GATEWAY, e))?;

    Ok(Json(resp))
}

async fn task_execute_handler(
    State(state): State<AppState>,
    _user: AuthenticatedUserGuard,
    Path(id): Path<String>,
    axum::Json(req): axum::Json<agents::TaskRequest>,
) -> Result<Json<agents::TaskResponse>, (axum::http::StatusCode, String)> {
    let mut guard = state.sliver.lock().await;
    let conn = guard.as_mut().ok_or_else(|| {
        (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "Sliver not connected".to_string(),
        )
    })?;

    // Override action to "execute" since this is the execute route
    let task_req = agents::TaskRequest {
        action: "execute".to_string(),
        args: req.args,
        exec_path: req.exec_path,
    };

    let resp = agents::exec_task(conn, &id, task_req)
        .await
        .map_err(|e| (axum::http::StatusCode::BAD_GATEWAY, e))?;

    Ok(Json(resp))
}

async fn list_tasks_handler(
    State(state): State<AppState>,
    _user: AuthenticatedUserGuard,
    Path(id): Path<String>,
) -> Result<Json<Vec<agents::TaskResponse>>, (axum::http::StatusCode, String)> {
    let mut guard = state.sliver.lock().await;
    let conn = guard.as_mut().ok_or_else(|| {
        (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "Sliver not connected".to_string(),
        )
    })?;

    let tasks = agents::list_session_tasks(conn, &id)
        .await
        .map_err(|e| (axum::http::StatusCode::BAD_GATEWAY, e))?;

    Ok(Json(tasks))
}

#[derive(Deserialize)]
struct ShellCommand {
    command: String,
}

#[derive(Serialize)]
struct ShellResponse {
    success: bool,
    stdout: String,
    stderr: String,
    exit_code: i32,
}

#[derive(Deserialize)]
struct LsRequest {
    path: String,
}

async fn agent_ls_handler(
    State(state): State<AppState>,
    _user: AuthenticatedUserGuard,
    Path(id): Path<String>,
    axum::Json(req): axum::Json<LsRequest>,
) -> Result<Json<agents::DirListResponse>, (axum::http::StatusCode, String)> {
    let mut guard = state.sliver.lock().await;
    let conn = guard.as_mut().ok_or_else(|| {
        (axum::http::StatusCode::SERVICE_UNAVAILABLE, "Sliver not connected".to_string())
    })?;
    let resp = agents::list_dir(conn, &id, &req.path)
        .await
        .map_err(|e| (axum::http::StatusCode::BAD_GATEWAY, e))?;
    Ok(Json(resp))
}

async fn agent_download_handler(
    State(state): State<AppState>,
    _user: AuthenticatedUserGuard,
    Path(id): Path<String>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<(axum::http::StatusCode, [(String, String); 2], Vec<u8>), (axum::http::StatusCode, String)> {
    let path = params.get("path").ok_or_else(|| {
        (axum::http::StatusCode::BAD_REQUEST, "path parameter required".to_string())
    })?;
    let mut guard = state.sliver.lock().await;
    let conn = guard.as_mut().ok_or_else(|| {
        (axum::http::StatusCode::SERVICE_UNAVAILABLE, "Sliver not connected".to_string())
    })?;
    let resp = agents::download_file_from_session(conn, &id, path)
        .await
        .map_err(|e| (axum::http::StatusCode::BAD_GATEWAY, e))?;
    Ok((
        axum::http::StatusCode::OK,
        [
            ("Content-Type".to_string(), "application/octet-stream".to_string()),
            ("Content-Disposition".to_string(), format!("attachment; filename=\"{}\"", resp.file_name)),
        ],
        resp.data,
    ))
}

async fn agent_upload_handler(
    State(state): State<AppState>,
    _user: AuthenticatedUserGuard,
    Path(id): Path<String>,
    axum::Json(req): axum::Json<agents::UploadRequest>,
) -> Result<Json<agents::UploadResponse>, (axum::http::StatusCode, String)> {
    let mut guard = state.sliver.lock().await;
    let conn = guard.as_mut().ok_or_else(|| {
        (axum::http::StatusCode::SERVICE_UNAVAILABLE, "Sliver not connected".to_string())
    })?;
    let resp = agents::upload_file_to_session(conn, &id, req)
        .await
        .map_err(|e| (axum::http::StatusCode::BAD_GATEWAY, e))?;
    Ok(Json(resp))
}

async fn list_modules_handler(
    State(state): State<AppState>,
    _user: AuthenticatedUserGuard,
) -> Result<Json<Vec<modules::ModuleInfo>>, (axum::http::StatusCode, String)> {
    let mut guard = state.sliver.lock().await;
    let conn = guard.as_mut().ok_or_else(|| {
        (axum::http::StatusCode::SERVICE_UNAVAILABLE, "Sliver not connected".to_string())
    })?;
    let mods = modules::list_modules(conn).await
        .map_err(|e| (axum::http::StatusCode::BAD_GATEWAY, e))?;
    Ok(Json(mods))
}

async fn exec_module_handler(
    State(state): State<AppState>,
    user: AuthenticatedUserGuard,
    Path((id, name)): Path<(String, String)>,
    axum::Json(req): axum::Json<modules::ExecModuleRequest>,
) -> Result<Json<modules::ExecModuleResponse>, (axum::http::StatusCode, String)> {
    if user.0.role != Role::Admin && user.0.role != Role::Operator {
        return Err((axum::http::StatusCode::FORBIDDEN, "Operator or Admin role required".to_string()));
    }
    let mut guard = state.sliver.lock().await;
    let conn = guard.as_mut().ok_or_else(|| {
        (axum::http::StatusCode::SERVICE_UNAVAILABLE, "Sliver not connected".to_string())
    })?;
    let resp = modules::exec_module(conn, &id, &name, req).await
        .map_err(|e| (axum::http::StatusCode::BAD_GATEWAY, e))?;
    crate::actions::audit_action(
        &state.pool,
        &user.0,
        None,
        "exec_module",
        "module",
        Some(name.clone()),
        None,
        "success",
    ).await;
    Ok(Json(resp))
}

async fn list_stagers_handler(
    State(state): State<AppState>,
    _user: AuthenticatedUserGuard,
) -> Result<Json<Vec<stagers::StagerResponse>>, (axum::http::StatusCode, String)> {
    let rows = stagers::list_stagers(&state.pool).await
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(rows))
}

async fn create_stager_handler(
    State(state): State<AppState>,
    user: AuthenticatedUserGuard,
    axum::Json(req): axum::Json<stagers::CreateStagerRequest>,
) -> Result<Json<stagers::StagerResponse>, (axum::http::StatusCode, String)> {
    if user.0.role != Role::Admin && user.0.role != Role::Operator {
        return Err((axum::http::StatusCode::FORBIDDEN, "Operator or Admin role required".to_string()));
    }
    if req.name.is_empty() {
        return Err((axum::http::StatusCode::BAD_REQUEST, "name is required".to_string()));
    }
    let result = stagers::create_stager(&state.pool, req).await
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;
    crate::actions::audit_action(
        &state.pool,
        &user.0,
        None,
        "create_stager",
        "stager",
        Some(result.id.clone()),
        None,
        "success",
    ).await;
    Ok(Json(result))
}

async fn update_stager_handler(
    State(state): State<AppState>,
    user: AuthenticatedUserGuard,
    Path(id): Path<String>,
    axum::Json(req): axum::Json<stagers::UpdateStagerRequest>,
) -> Result<Json<stagers::StagerResponse>, (axum::http::StatusCode, String)> {
    if user.0.role != Role::Admin && user.0.role != Role::Operator {
        return Err((axum::http::StatusCode::FORBIDDEN, "Operator or Admin role required".to_string()));
    }
    let result = stagers::update_stager(&state.pool, &id, req).await
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(result))
}

async fn delete_stager_handler(
    State(state): State<AppState>,
    user: AuthenticatedUserGuard,
    Path(id): Path<String>,
) -> Result<axum::http::StatusCode, (axum::http::StatusCode, String)> {
    if user.0.role != Role::Admin {
        return Err((axum::http::StatusCode::FORBIDDEN, "Admin role required".to_string()));
    }
    stagers::delete_stager(&state.pool, &id).await
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;
    crate::actions::audit_action(
        &state.pool,
        &user.0,
        None,
        "delete_stager",
        "stager",
        Some(id),
        None,
        "success",
    ).await;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

async fn generate_stager_handler(
    State(state): State<AppState>,
    user: AuthenticatedUserGuard,
    Path(id): Path<String>,
    axum::Json(req): axum::Json<stagers::GenerateFromStagerRequest>,
) -> Result<Json<payloads::GenerateResponse>, (axum::http::StatusCode, String)> {
    if user.0.role != Role::Admin && user.0.role != Role::Operator {
        return Err((axum::http::StatusCode::FORBIDDEN, "Operator or Admin role required".to_string()));
    }
    if req.lhost.is_empty() || req.lport == 0 {
        return Err((axum::http::StatusCode::BAD_REQUEST, "lhost and lport are required".to_string()));
    }
    let result = stagers::generate_from_stager(&state.pool, &id, req).await
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;
    crate::actions::audit_action(
        &state.pool,
        &user.0,
        None,
        "generate_stager",
        "stager",
        Some(id),
        None,
        if result.success { "success" } else { "failed" },
    ).await;
    Ok(Json(result))
}

async fn list_pivots_handler(
    State(state): State<AppState>,
    _user: AuthenticatedUserGuard,
) -> Result<Json<Vec<pivots::PivotListener>>, (axum::http::StatusCode, String)> {
    let mut guard = state.sliver.lock().await;
    let conn = guard.as_mut().ok_or_else(|| {
        (axum::http::StatusCode::SERVICE_UNAVAILABLE, "Sliver not connected".to_string())
    })?;
    let p = pivots::list_pivots(conn).await
        .map_err(|e| (axum::http::StatusCode::BAD_GATEWAY, e))?;
    Ok(Json(p))
}

async fn pivot_graph_handler(
    State(state): State<AppState>,
    _user: AuthenticatedUserGuard,
) -> Result<Json<Vec<pivots::PivotNode>>, (axum::http::StatusCode, String)> {
    let mut guard = state.sliver.lock().await;
    let conn = guard.as_mut().ok_or_else(|| {
        (axum::http::StatusCode::SERVICE_UNAVAILABLE, "Sliver not connected".to_string())
    })?;
    let g = pivots::get_pivot_graph(conn).await
        .map_err(|e| (axum::http::StatusCode::BAD_GATEWAY, e))?;
    Ok(Json(g))
}

async fn start_pivot_handler(
    State(state): State<AppState>,
    user: AuthenticatedUserGuard,
    axum::Json(req): axum::Json<pivots::CreatePivotRequest>,
) -> Result<Json<pivots::CreatePivotResponse>, (axum::http::StatusCode, String)> {
    if user.0.role != Role::Admin && user.0.role != Role::Operator {
        return Err((axum::http::StatusCode::FORBIDDEN, "Operator or Admin role required".to_string()));
    }
    let mut guard = state.sliver.lock().await;
    let conn = guard.as_mut().ok_or_else(|| {
        (axum::http::StatusCode::SERVICE_UNAVAILABLE, "Sliver not connected".to_string())
    })?;
    let resp = pivots::start_pivot(conn, req).await
        .map_err(|e| (axum::http::StatusCode::BAD_GATEWAY, e))?;
    Ok(Json(resp))
}

async fn stop_pivot_handler(
    State(state): State<AppState>,
    user: AuthenticatedUserGuard,
    axum::Json(req): axum::Json<pivots::StopPivotRequest>,
) -> Result<Json<pivots::StopPivotResponse>, (axum::http::StatusCode, String)> {
    if user.0.role != Role::Admin && user.0.role != Role::Operator {
        return Err((axum::http::StatusCode::FORBIDDEN, "Operator or Admin role required".to_string()));
    }
    let mut guard = state.sliver.lock().await;
    let conn = guard.as_mut().ok_or_else(|| {
        (axum::http::StatusCode::SERVICE_UNAVAILABLE, "Sliver not connected".to_string())
    })?;
    let resp = pivots::stop_pivot(conn, req).await
        .map_err(|e| (axum::http::StatusCode::BAD_GATEWAY, e))?;
    Ok(Json(resp))
}

async fn create_portfwd_handler(
    State(state): State<AppState>,
    user: AuthenticatedUserGuard,
    Path(id): Path<String>,
    axum::Json(mut req): axum::Json<pivots::CreatePortFwdRequest>,
) -> Result<Json<pivots::CreatePortFwdResponse>, (axum::http::StatusCode, String)> {
    if user.0.role != Role::Admin && user.0.role != Role::Operator {
        return Err((axum::http::StatusCode::FORBIDDEN, "Operator or Admin role required".to_string()));
    }
    req.session_id = id.clone();
    let mut guard = state.sliver.lock().await;
    let conn = guard.as_mut().ok_or_else(|| {
        (axum::http::StatusCode::SERVICE_UNAVAILABLE, "Sliver not connected".to_string())
    })?;
    let resp = pivots::create_portfwd(conn, req).await
        .map_err(|e| (axum::http::StatusCode::BAD_GATEWAY, e))?;
    Ok(Json(resp))
}

async fn start_socks_handler(
    State(state): State<AppState>,
    user: AuthenticatedUserGuard,
    Path(id): Path<String>,
    axum::Json(mut req): axum::Json<pivots::StartSocksRequest>,
) -> Result<Json<pivots::StartSocksResponse>, (axum::http::StatusCode, String)> {
    if user.0.role != Role::Admin && user.0.role != Role::Operator {
        return Err((axum::http::StatusCode::FORBIDDEN, "Operator or Admin role required".to_string()));
    }
    req.session_id = id.clone();
    let mut guard = state.sliver.lock().await;
    let conn = guard.as_mut().ok_or_else(|| {
        (axum::http::StatusCode::SERVICE_UNAVAILABLE, "Sliver not connected".to_string())
    })?;
    let resp = pivots::start_socks(conn, req).await
        .map_err(|e| (axum::http::StatusCode::BAD_GATEWAY, e))?;
    Ok(Json(resp))
}

/// Helper to render a report as either JSON or CSV based on query param `format`.
fn format_response<T: serde::Serialize>(
    q: &reports::ReportQuery,
    rows: Vec<T>,
    name: &str,
) -> (axum::http::StatusCode, [(String, String); 2], Vec<u8>) {
    if q.format.as_deref() == Some("csv") {
        let body = reports::to_csv(&rows).unwrap_or_default();
        let filename = format!("{}.csv", name);
        (
            axum::http::StatusCode::OK,
            [
                ("Content-Type".to_string(), "text/csv".to_string()),
                (
                    "Content-Disposition".to_string(),
                    format!("attachment; filename=\"{}\"", filename),
                ),
            ],
            body.into_bytes(),
        )
    } else {
        let body = serde_json::to_string(&rows).unwrap_or_else(|_| "[]".to_string());
        (
            axum::http::StatusCode::OK,
            [
                ("Content-Type".to_string(), "application/json".to_string()),
                ("X-Content-Type-Options".to_string(), "nosniff".to_string()),
            ],
            body.into_bytes(),
        )
    }
}

async fn report_sessions_handler(
    State(state): State<AppState>,
    _user: AuthenticatedUserGuard,
    axum::extract::Query(q): axum::extract::Query<reports::ReportQuery>,
) -> Result<(axum::http::StatusCode, [(String, String); 2], Vec<u8>), (axum::http::StatusCode, String)> {
    let mut guard = state.sliver.lock().await;
    let conn = guard.as_mut().ok_or_else(|| {
        (axum::http::StatusCode::SERVICE_UNAVAILABLE, "Sliver not connected".to_string())
    })?;
    let rows = reports::report_sessions(conn, q.clone()).await
        .map_err(|e| (axum::http::StatusCode::BAD_GATEWAY, e))?;
    Ok(format_response(&q, rows, "sessions_report"))
}

async fn report_credentials_handler(
    State(state): State<AppState>,
    _user: AuthenticatedUserGuard,
    axum::extract::Query(q): axum::extract::Query<reports::ReportQuery>,
) -> Result<(axum::http::StatusCode, [(String, String); 2], Vec<u8>), (axum::http::StatusCode, String)> {
    let rows = reports::report_credentials(&state.pool, q.clone()).await
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(format_response(&q, rows, "credentials_report"))
}

async fn report_hosts_handler(
    State(state): State<AppState>,
    _user: AuthenticatedUserGuard,
    axum::extract::Query(q): axum::extract::Query<reports::ReportQuery>,
) -> Result<(axum::http::StatusCode, [(String, String); 2], Vec<u8>), (axum::http::StatusCode, String)> {
    let rows = reports::report_hosts(&state.pool, q.clone()).await
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(format_response(&q, rows, "hosts_report"))
}

async fn admin_update_user_handler(
    State(state): State<AppState>,
    user: AuthenticatedUserGuard,
    Path(id): Path<String>,
    axum::Json(req): axum::Json<admin::UpdateUserRequest>,
) -> Result<Json<admin::UserAdmin>, (axum::http::StatusCode, String)> {
    if user.0.role != Role::Admin {
        return Err((axum::http::StatusCode::FORBIDDEN, "Admin only".to_string()));
    }
    let updated = admin::update_user(&state.pool, &id, req).await
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;
    crate::actions::audit_action(
        &state.pool,
        &user.0,
        None,
        "update_user",
        "user",
        Some(id.clone()),
        None,
        "success",
    ).await;
    Ok(Json(updated))
}

async fn admin_delete_user_handler(
    State(state): State<AppState>,
    user: AuthenticatedUserGuard,
    Path(id): Path<String>,
) -> Result<axum::http::StatusCode, (axum::http::StatusCode, String)> {
    if user.0.role != Role::Admin {
        return Err((axum::http::StatusCode::FORBIDDEN, "Admin only".to_string()));
    }
    admin::delete_user(&state.pool, &id).await
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;
    crate::actions::audit_action(
        &state.pool,
        &user.0,
        None,
        "delete_user",
        "user",
        Some(id.clone()),
        None,
        "success",
    ).await;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

async fn admin_list_settings_handler(
    State(state): State<AppState>,
    _user: AuthenticatedUserGuard,
) -> Result<Json<Vec<admin::SettingEntry>>, (axum::http::StatusCode, String)> {
    let rows = admin::list_settings(&state.pool).await
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(rows))
}

async fn admin_upsert_setting_handler(
    State(state): State<AppState>,
    user: AuthenticatedUserGuard,
    Path(key): Path<String>,
    axum::Json(req): axum::Json<admin::UpsertSettingRequest>,
) -> Result<Json<admin::SettingEntry>, (axum::http::StatusCode, String)> {
    if user.0.role != Role::Admin {
        return Err((axum::http::StatusCode::FORBIDDEN, "Admin only".to_string()));
    }
    let updated = admin::upsert_setting(&state.pool, &key, req, user.0.id).await
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(updated))
}

async fn health_handler(
    State(state): State<AppState>,
) -> Result<Json<admin::HealthReport>, (axum::http::StatusCode, String)> {
    let db_ok = admin::db_ok(&state.pool).await;
    let (sliver_connected, sessions, listeners) = {
        let mut s = 0;
        let mut l = 0;
        let mut connected = false;
        if let Ok(mut guard) = state.sliver.try_lock() {
            if let Some(c) = guard.as_mut() {
                connected = true;
                if let Ok(se) = sessions::list_sessions(c).await { s = se.len() as u32; }
                if let Ok(li) = listeners::list_jobs(c).await { l = li.len() as u32; }
            }
        }
        (connected, s, l)
    };
    Ok(Json(admin::HealthReport {
        status: if db_ok && sliver_connected { "ok" } else if db_ok { "degraded" } else { "down" }.to_string(),
        db_ok,
        sliver_connected,
        active_sessions: sessions,
        active_listeners: listeners,
        uptime_seconds: 0,
    }))
}

async fn meta_handler() -> Json<admin::MetaInfo> {
    Json(admin::build_meta())
}

async fn report_timeline_handler(
    State(state): State<AppState>,
    _user: AuthenticatedUserGuard,
    axum::extract::Query(q): axum::extract::Query<reports::ReportQuery>,
) -> Result<(axum::http::StatusCode, [(String, String); 2], Vec<u8>), (axum::http::StatusCode, String)> {
    let rows = reports::report_timeline(&state.pool, q.clone()).await
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(format_response(&q, rows, "timeline_report"))
}

async fn shell_exec_handler(
    _user: AuthenticatedUserGuard,
    axum::Json(req): axum::Json<ShellCommand>,
) -> axum::Json<ShellResponse> {
    let output = tokio::process::Command::new("bash")
        .arg("-c")
        .arg(&req.command)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await;

    match output {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let code = output.status.code().unwrap_or(-1);
            axum::Json(ShellResponse {
                success: output.status.success(),
                stdout,
                stderr,
                exit_code: code,
            })
        }
        Err(e) => axum::Json(ShellResponse {
            success: false,
            stdout: String::new(),
            stderr: format!("Failed to execute: {e}"),
            exit_code: -1,
        }),
    }
}


