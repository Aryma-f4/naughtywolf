use axum::{
    Extension, Form, Json, Router,
    extract::{FromRequest, Path, Query, Request, State},
    http::{HeaderValue, StatusCode, header},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use serde::Deserialize;
use tower::ServiceExt;
use tower_http::services::ServeFile;
use tower_sessions::Session;
use uuid::Uuid;

use crate::{
    AppError,
    auth::{
        AuthenticatedUser,
        middleware::{AuthSession, AuthenticatedUserGuard},
        rbac::Role,
    },
    db::{
        models::{Asset, AuditEvent, CheckRun, Operation, OperationStatus, RunState},
        repositories::Repository,
    },
    evidence::EvidenceStore,
    policy::authorize_operation,
};

pub mod templates;
mod workspace;

pub const CSRF_TOKEN_KEY: &str = "login_csrf_token";

#[derive(Default, Debug, PartialEq)]
pub struct DashboardSummary {
    pub operation_count: i64,
    pub asset_count: i64,
    pub run_count: i64,
    pub evidence_count: i64,
    pub audit_count: i64,
}

#[derive(Debug, PartialEq)]
pub struct OperationReportSummary {
    pub operation: Operation,
    pub asset_count: usize,
    pub queued_count: usize,
    pub running_count: usize,
    pub succeeded_count: usize,
    pub failed_count: usize,
    pub cancelled_count: usize,
    pub audit_count: usize,
}

pub async fn visible_operations(
    repo: &Repository,
    user: &AuthenticatedUser,
) -> Result<Vec<Operation>, AppError> {
    repo.list_operations_visible_to(&user.id, user.role == Role::Admin)
        .await
}

pub async fn dashboard_summary(
    repo: &Repository,
    user: &AuthenticatedUser,
) -> Result<DashboardSummary, AppError> {
    let is_admin = user.role == Role::Admin;

    Ok(DashboardSummary {
        operation_count: repo.count_operations_visible_to(&user.id, is_admin).await?,
        asset_count: repo.count_assets_visible_to(&user.id, is_admin).await?,
        run_count: repo.count_check_runs_visible_to(&user.id, is_admin).await?,
        evidence_count: repo.count_evidence_visible_to(&user.id, is_admin).await?,
        audit_count: repo
            .count_audit_events_visible_to(&user.id, is_admin)
            .await?,
    })
}

pub fn public_router() -> Router {
    Router::new()
        .route("/", get(landing_page))
        .route("/login", get(login_page))
        .route(
            "/healthz",
            get(|| async { axum::http::StatusCode::NO_CONTENT }),
        )
        .route("/static/admin.css", get(stylesheet))
        .route("/static/admin.js", get(script))
        .route("/static/payload-wizard.js", get(payload_wizard_script))
        .route("/static/module_studio.js", get(module_studio_script))
        .route("/static/motion.js", get(motion_script))
        .route("/static/workspace.js", get(workspace_script))
        .route("/static/workspace.css", get(workspace_style))
        .route("/static/anime.min.js", get(anime_script))
        .route("/payloads/download/{token}", get(public_download_payload))
}

/// Routes that always resolve the current local identity before rendering.
/// The binary supplies the repository state and session layer when composing
/// this router with the public routes.
pub fn authenticated_router() -> Router<Repository> {
    Router::new()
        .route("/dashboard", get(dashboard))
        .route("/guide", get(guide))
        .route("/topology", get(workspace::topology_page))
        .route("/topology/data", get(workspace::topology_data))
        .route("/recon", get(workspace::recon_page))
        .route("/recon/run", post(workspace::run_recon))
        .route("/operations", get(operations).post(create_operation))
        .route("/operations/new", get(new_operation))
        .route("/operations/{operation_id}/assets", post(create_asset))
        .route("/operations/{operation_id}/assets/new", get(new_asset))
        .route("/inventory", get(inventory))
        .route("/checks", get(checks))
        .route("/audit", get(audit))
        .route("/evidence", get(evidence))
        .route("/evidence/{evidence_id}/download", get(download_evidence))
        .route("/reports", get(reports))
        .route("/payloads", get(payloads))
        .route("/payloads/generate", post(generate_payload))
        .route("/payloads/edit/{file}", get(edit_payload))
        .route("/callbacks", get(callbacks))
        .route("/callbacks/{session_id}", get(callback_detail))
        .route("/c2/sessions/{session_id}/tasks", get(tasks_json))
        .route("/eventing", get(eventing).post(create_event_rule))
        .route("/events", get(events_feed))
        .route("/services", get(services))
        .route("/search", get(search))
        .route(
            "/operations/{operation_id}",
            get(operation_detail).post(update_operation),
        )
        .route("/admin", get(admin))
        .route("/admin/users", get(admin_users))
        .route("/admin/users/{user_id}/role", post(set_user_role))
        .route("/admin/users/{user_id}/disabled", post(set_user_disabled))
        .route("/logout", post(logout))
}

async fn dashboard(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repository): State<Repository>,
) -> Result<Html<String>, AppError> {
    let summary = dashboard_summary(&repository, &user).await?;
    Ok(Html(templates::dashboard_page(&user, &summary)))
}

async fn guide(AuthenticatedUserGuard(user): AuthenticatedUserGuard) -> Html<String> {
    Html(templates::guide_page(&user))
}

async fn operations(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repository): State<Repository>,
) -> Result<Html<String>, AppError> {
    let operations = visible_operations(&repository, &user).await?;
    Ok(Html(templates::operations_page(&user, &operations)))
}

#[derive(Default, Deserialize)]
struct OperationForm {
    #[serde(default)]
    name: String,
    #[serde(default)]
    purpose: String,
    csrf_token: Option<String>,
}

#[derive(Default, Deserialize)]
struct PayloadForm {
    #[serde(default)]
    name: String,
    #[serde(default)]
    lhost: String,
    #[serde(default)]
    lport: u16,
    #[serde(default = "default_protocol")]
    protocol: String,
    #[serde(default)]
    gsocket_secret: Option<String>,
    #[serde(default)]
    gsocket_local_port: Option<u16>,
    #[serde(default = "default_interval")]
    interval_ms: u64,
    #[serde(default = "default_jitter")]
    jitter_ms: u64,
    #[serde(default)]
    os: String,
    #[serde(default)]
    arch: String,
    /// Rust target triple; empty = host build.
    #[serde(default)]
    target: String,
    csrf_token: Option<String>,
}

fn default_protocol() -> String {
    "http".into()
}
const fn default_interval() -> u64 {
    1000
}
const fn default_jitter() -> u64 {
    200
}

#[derive(Default, Deserialize)]
struct AssetForm {
    #[serde(default)]
    name: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    owner: String,
    #[serde(default)]
    address: String,
    csrf_token: Option<String>,
}

async fn new_operation(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    session: Session,
) -> Result<Html<String>, AppError> {
    user.require(Role::Admin)?;
    let csrf_token = issue_csrf_token(&session).await?;
    Ok(Html(templates::operation_form_page(
        &user,
        &csrf_token,
        None,
        "",
        "",
    )))
}

async fn create_operation(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repository): State<Repository>,
    session: Session,
    request: Request,
) -> Result<Response, AppError> {
    user.require(Role::Admin)?;
    let form = match Form::<OperationForm>::from_request(request, &repository).await {
        Ok(Form(form)) => form,
        Err(_) => {
            return invalid_operation_form_response(&user, &session, &OperationForm::default())
                .await;
        }
    };
    if !csrf_token_matches(&session, form.csrf_token.as_deref()).await?
        || !valid_form_values(&[&form.name, &form.purpose])
    {
        return invalid_operation_form_response(&user, &session, &form).await;
    }

    repository
        .create_operation_with_audit(
            form.name.trim(),
            form.purpose.trim(),
            &user.id,
            &Uuid::new_v4().to_string(),
        )
        .await?;
    Ok(Redirect::to("/operations").into_response())
}

async fn new_asset(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repository): State<Repository>,
    Path(operation_id): Path<String>,
    session: Session,
) -> Result<Html<String>, AppError> {
    authorize_operation(&repository, &user, &operation_id, Role::Operator).await?;
    let csrf_token = issue_csrf_token(&session).await?;
    Ok(Html(templates::asset_form_page(
        &user,
        &operation_id,
        &csrf_token,
        None,
        ["", "", "", ""],
    )))
}

async fn create_asset(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repository): State<Repository>,
    Path(operation_id): Path<String>,
    session: Session,
    request: Request,
) -> Result<Response, AppError> {
    authorize_operation(&repository, &user, &operation_id, Role::Operator).await?;
    let form = match Form::<AssetForm>::from_request(request, &repository).await {
        Ok(Form(form)) => form,
        Err(_) => {
            return invalid_asset_form_response(
                &user,
                &session,
                &operation_id,
                &AssetForm::default(),
            )
            .await;
        }
    };
    if !csrf_token_matches(&session, form.csrf_token.as_deref()).await?
        || !valid_form_values(&[&form.name, &form.kind, &form.owner, &form.address])
    {
        return invalid_asset_form_response(&user, &session, &operation_id, &form).await;
    }

    repository
        .create_asset_with_audit(
            &operation_id,
            form.name.trim(),
            form.kind.trim(),
            form.owner.trim(),
            form.address.trim(),
            &user.id,
            &Uuid::new_v4().to_string(),
        )
        .await?;
    Ok(Redirect::to("/operations").into_response())
}

async fn invalid_operation_form_response(
    user: &AuthenticatedUser,
    session: &Session,
    form: &OperationForm,
) -> Result<Response, AppError> {
    let csrf_token = issue_csrf_token(session).await?;
    Ok((
        StatusCode::BAD_REQUEST,
        Html(templates::operation_form_page(
            user,
            &csrf_token,
            Some("The request is invalid."),
            &form.name,
            &form.purpose,
        )),
    )
        .into_response())
}

async fn invalid_asset_form_response(
    user: &AuthenticatedUser,
    session: &Session,
    operation_id: &str,
    form: &AssetForm,
) -> Result<Response, AppError> {
    let csrf_token = issue_csrf_token(session).await?;
    Ok((
        StatusCode::BAD_REQUEST,
        Html(templates::asset_form_page(
            user,
            operation_id,
            &csrf_token,
            Some("The request is invalid."),
            [&form.name, &form.kind, &form.owner, &form.address],
        )),
    )
        .into_response())
}

fn valid_form_values(values: &[&str]) -> bool {
    values.iter().all(|value| {
        let value = value.trim();
        !value.is_empty() && value.chars().count() <= 160
    })
}

async fn issue_csrf_token(session: &Session) -> Result<String, AppError> {
    let csrf_token = Uuid::new_v4().to_string();
    session
        .insert(CSRF_TOKEN_KEY, &csrf_token)
        .await
        .map_err(|_| AppError::Internal)?;
    Ok(csrf_token)
}

async fn csrf_token_matches(session: &Session, submitted: Option<&str>) -> Result<bool, AppError> {
    let expected: Option<String> = session
        .get(CSRF_TOKEN_KEY)
        .await
        .map_err(|_| AppError::Internal)?;
    Ok(expected.as_deref() == submitted && submitted.is_some())
}

async fn inventory(AuthenticatedUserGuard(user): AuthenticatedUserGuard) -> Html<String> {
    Html(templates::empty_page(
        "Inventory",
        &user,
        "inventory",
        "No scoped inventory yet",
    ))
}

async fn checks(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repository): State<Repository>,
) -> Result<Html<String>, AppError> {
    let runs = repository
        .list_check_runs_visible_to(&user.id, user.role == Role::Admin)
        .await?;
    Ok(Html(templates::checks_page(&user, &runs)))
}

async fn audit(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repository): State<Repository>,
) -> Result<Html<String>, AppError> {
    let events = repository
        .list_audit_events_visible_to(&user.id, user.role == Role::Admin)
        .await?;
    Ok(Html(templates::audit_page(&user, &events)))
}

async fn evidence(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repository): State<Repository>,
) -> Result<Html<String>, AppError> {
    let records = repository
        .list_evidence_visible_to(&user.id, user.role == Role::Admin)
        .await?;
    Ok(Html(templates::evidence_page(&user, &records)))
}

async fn download_evidence(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repository): State<Repository>,
    Path(evidence_id): Path<String>,
    evidence_store: Option<Extension<EvidenceStore>>,
) -> Result<Response, AppError> {
    let evidence = repository
        .find_evidence_visible_to(&evidence_id, &user.id, user.role == Role::Admin)
        .await?
        .ok_or(AppError::NotFound)?;
    let Extension(evidence_store) = evidence_store.ok_or(AppError::Internal)?;
    let bytes = evidence_store.read_verified(&evidence).await?;
    let content_type = HeaderValue::try_from(&evidence.content_type)
        .map_err(|_| AppError::Validation("stored evidence content type is invalid".to_owned()))?;
    let mut response = bytes.into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, content_type);
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=\"evidence\""),
    );
    Ok(response)
}

async fn reports(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repository): State<Repository>,
) -> Result<Html<String>, AppError> {
    let is_admin = user.role == Role::Admin;
    let operations = repository
        .list_operations_visible_to(&user.id, is_admin)
        .await?;
    let assets = repository
        .list_assets_visible_to(&user.id, is_admin)
        .await?;
    let runs = repository
        .list_check_runs_visible_to(&user.id, is_admin)
        .await?;
    let events = repository
        .list_audit_events_visible_to(&user.id, is_admin)
        .await?;
    let summaries = operation_report_summaries(operations, &assets, &runs, &events);
    Ok(Html(templates::reports_page(&user, &summaries)))
}

async fn payloads(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    session: Session,
) -> Result<Html<String>, AppError> {
    user.require(Role::Operator)?;
    let csrf_token = issue_csrf_token(&session).await?;
    let builds = crate::payload::list().unwrap_or_default();
    let building = crate::payload::building_jobs();
    let errors = crate::payload::recent_errors();
    Ok(Html(templates::payloads_page(
        &user,
        &builds,
        &csrf_token,
        None,
        None,
        &building,
        &errors,
    )))
}

async fn generate_payload(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    Extension(c2_psk): Extension<std::sync::Arc<Vec<u8>>>,
    session: Session,
    request: Request,
) -> Result<Response, AppError> {
    user.require(Role::Operator)?;
    let form = match Form::<PayloadForm>::from_request(request, &user).await {
        Ok(Form(form)) => form,
        Err(_) => {
            let csrf_token = issue_csrf_token(&session).await?;
            return Ok((
                StatusCode::BAD_REQUEST,
                Html(templates::payloads_page(
                    &user,
                    &crate::payload::list().unwrap_or_default(),
                    &csrf_token,
                    Some("The request is invalid."),
                    None,
                    &crate::payload::building_jobs(),
                    &crate::payload::recent_errors(),
                )),
            )
                .into_response());
        }
    };
    let supported_protocol = matches!(
        form.protocol.as_str(),
        "http" | "https" | "tcp" | "gs" | "dns"
    );
    let valid_gsocket = form.protocol != "gs"
        || (form
            .gsocket_secret
            .as_deref()
            .is_some_and(|secret| !secret.trim().is_empty())
            && form.gsocket_local_port.is_some_and(|port| port > 0));
    if !csrf_token_matches(&session, form.csrf_token.as_deref()).await?
        || form.lhost.trim().is_empty()
        || form.name.trim().is_empty()
        || form.lport == 0
        || !supported_protocol
        || !valid_gsocket
    {
        let csrf_token = issue_csrf_token(&session).await?;
        return Ok((
            StatusCode::BAD_REQUEST,
            Html(templates::payloads_page(
                &user,
                &crate::payload::list().unwrap_or_default(),
                &csrf_token,
                Some("Provide a valid name, callback, protocol, and GSocket secret when selected."),
                None,
                &crate::payload::building_jobs(),
                &crate::payload::recent_errors(),
            )),
        )
            .into_response());
    }

    let req = crate::payload::BuildRequest {
        name: form.name,
        lhost: form.lhost,
        lport: form.lport,
        protocol: form.protocol,
        gsocket_secret: form.gsocket_secret,
        gsocket_local_port: form.gsocket_local_port,
        os: form.os,
        arch: form.arch,
        interval_ms: form.interval_ms,
        jitter_ms: form.jitter_ms,
        target: form.target,
    };
    tokio::spawn(async move {
        let file = crate::payload::predict_file(&req);
        match crate::payload::build(&req, c2_psk.as_slice()).await {
            Ok(meta) => tracing::info!(payload = %meta.file, size = meta.size, "implant built"),
            Err(e) => {
                crate::payload::record_build_error(&file, &e.to_string());
                tracing::error!(error = %e, "implant build failed");
            }
        }
    });
    Ok(Redirect::to("/payloads").into_response())
}

async fn edit_payload(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    session: Session,
    Path(file): Path<String>,
) -> Result<Html<String>, AppError> {
    user.require(Role::Operator)?;
    let csrf_token = issue_csrf_token(&session).await?;
    let builds = crate::payload::list().unwrap_or_default();
    let editor = builds.iter().find(|b| b.file == file).cloned();
    let building = crate::payload::building_jobs();
    let errors = crate::payload::recent_errors();
    Ok(Html(templates::payloads_page(
        &user,
        &builds,
        &csrf_token,
        None,
        editor.as_ref(),
        &building,
        &errors,
    )))
}

async fn public_download_payload(Path(token): Path<String>) -> Result<Response, AppError> {
    let Some((path, filename)) = crate::payload::public_download(&token) else {
        return Err(AppError::NotFound);
    };
    let served = ServeFile::new(path)
        .oneshot(Request::new(axum::body::Body::empty()))
        .await
        .map_err(|_| AppError::Internal)?;
    if served.status() == StatusCode::NOT_FOUND {
        return Err(AppError::NotFound);
    }
    let (parts, body) = served.into_parts();
    let mut response = Response::from_parts(parts, axum::body::Body::new(body));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        content_disposition(&filename).map_err(|_| AppError::Internal)?,
    );
    Ok(response)
}

fn content_disposition(
    filename: &str,
) -> Result<HeaderValue, axum::http::header::InvalidHeaderValue> {
    let fallback = filename
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_' | ' ') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    let encoded = filename
        .as_bytes()
        .iter()
        .map(|byte| {
            if byte.is_ascii_alphanumeric()
                || matches!(
                    *byte,
                    b'!' | b'#'
                        | b'$'
                        | b'&'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
            {
                char::from(*byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect::<String>();
    HeaderValue::from_str(&format!(
        "attachment; filename=\"{fallback}\"; filename*=UTF-8''{encoded}"
    ))
}

async fn callbacks(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repository): State<Repository>,
) -> Result<Html<String>, AppError> {
    user.require(Role::Operator)?;
    let callbacks = repository
        .list_callbacks_visible_to(&user.id, user.role == Role::Admin)
        .await?;
    Ok(Html(templates::callbacks_page(&user, &callbacks)))
}

/// Mythic-style callback detail/interact page.
/// Shows callback metadata, a tasking panel, and a task history table.
/// Task results stream in real-time via SSE on the companion endpoint.
async fn callback_detail(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repository): State<Repository>,
    Path(session_id): Path<String>,
) -> Result<Html<String>, AppError> {
    user.require(Role::Operator)?;
    let callback = repository
        .find_callback(&session_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let tasks = repository
        .list_tasks_for_session(&session_id)
        .await
        .unwrap_or_default();
    let csrf_token = Uuid::new_v4().to_string();
    Ok(Html(templates::callback_detail_page(
        &user,
        &callback,
        &tasks,
        &csrf_token,
    )))
}

/// JSON API for listing tasks for a callback session.
async fn tasks_json(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repository): State<Repository>,
    Path(session_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    user.require(Role::Operator)?;
    let tasks = repository
        .list_tasks_for_session(&session_id)
        .await
        .unwrap_or_default();
    let arr: Vec<serde_json::Value> = tasks
        .iter()
        .map(|t| {
            let (label, cls) = crate::db::models::TaskStatus::label_class_from_str(&t.status);
            serde_json::json!({
                "id": t.id,
                "command": t.command,
                "args": t.args_json,
                "status": t.status,
                "state_label": label,
                "state_class": cls,
                "created_at": t.created_at,
                "processing_at": t.processing_at,
                "completed_at": t.completed_at,
                "result_output": t.result_output,
                "result_ok": t.result_ok,
                "result_exit_code": t.result_exit_code,
            })
        })
        .collect();
    Ok(Json(serde_json::json!({ "tasks": arr })))
}

/// Mythic-style event feed page showing operation-wide audit events.
async fn events_feed(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repository): State<Repository>,
) -> Result<Html<String>, AppError> {
    user.require(Role::Operator)?;
    let events = repository
        .list_audit_events_visible_to(&user.id, user.role == Role::Admin)
        .await?;
    Ok(Html(templates::event_feed_page(&user, &events)))
}

async fn eventing(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repository): State<Repository>,
    session: Session,
) -> Result<Html<String>, AppError> {
    user.require(Role::Operator)?;
    let csrf_token = issue_csrf_token(&session).await?;
    let rules = repository.list_event_rules().await?;
    Ok(Html(templates::eventing_page(
        &user,
        &rules,
        &csrf_token,
        None,
    )))
}

async fn create_event_rule(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repository): State<Repository>,
    session: Session,
    request: Request,
) -> Result<Response, AppError> {
    user.require(Role::Operator)?;
    let form = match Form::<EventRuleForm>::from_request(request, &repository).await {
        Ok(Form(form)) => form,
        Err(_) => {
            let csrf_token = issue_csrf_token(&session).await?;
            let rules = repository.list_event_rules().await?;
            return Ok((
                StatusCode::BAD_REQUEST,
                Html(templates::eventing_page(
                    &user,
                    &rules,
                    &csrf_token,
                    Some("The request is invalid."),
                )),
            )
                .into_response());
        }
    };
    if !csrf_token_matches(&session, form.csrf_token.as_deref()).await?
        || !valid_form_values(&[&form.name, &form.trigger, &form.command])
    {
        let csrf_token = issue_csrf_token(&session).await?;
        let rules = repository.list_event_rules().await?;
        return Ok((
            StatusCode::BAD_REQUEST,
            Html(templates::eventing_page(
                &user,
                &rules,
                &csrf_token,
                Some("Provide a rule name, trigger, and command."),
            )),
        )
            .into_response());
    }
    repository
        .create_event_rule(
            form.name.trim(),
            form.trigger.trim(),
            form.command.trim(),
            if form.target.is_empty() {
                "all"
            } else {
                form.target.trim()
            },
            &user.id,
            &Uuid::new_v4().to_string(),
        )
        .await?;
    Ok(Redirect::to("/eventing").into_response())
}

async fn services(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repository): State<Repository>,
) -> Result<Html<String>, AppError> {
    user.require(Role::Operator)?;
    let services = repository
        .list_services_visible_to(&user.id, user.role == Role::Admin)
        .await?;
    Ok(Html(templates::services_page(&user, &services)))
}

async fn search(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repository): State<Repository>,
    Query(params): Query<SearchParams>,
) -> Result<Html<String>, AppError> {
    user.require(Role::Operator)?;
    let query = params.q.unwrap_or_default().trim().to_owned();
    if query.is_empty() {
        return Ok(Html(templates::search_page(&user, "", &[], &[], &[])));
    }
    let (operations, assets, callbacks) = repository
        .search_portal(&user.id, user.role == Role::Admin, &query)
        .await?;
    Ok(Html(templates::search_page(
        &user,
        &query,
        &operations,
        &assets,
        &callbacks,
    )))
}

async fn operation_detail(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repository): State<Repository>,
    Path(operation_id): Path<String>,
    session: Session,
) -> Result<Html<String>, AppError> {
    authorize_operation(&repository, &user, &operation_id, Role::Operator).await?;
    let operation = repository
        .find_operation(&operation_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let csrf_token = issue_csrf_token(&session).await?;
    Ok(Html(templates::operation_detail_page(
        &user,
        &operation,
        &csrf_token,
        None,
    )))
}

async fn update_operation(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repository): State<Repository>,
    Path(operation_id): Path<String>,
    session: Session,
    request: Request,
) -> Result<Response, AppError> {
    authorize_operation(&repository, &user, &operation_id, Role::Operator).await?;
    let form = match Form::<OperationUpdateForm>::from_request(request, &repository).await {
        Ok(Form(form)) => form,
        Err(_) => {
            let csrf_token = issue_csrf_token(&session).await?;
            let operation = repository
                .find_operation(&operation_id)
                .await?
                .ok_or(AppError::NotFound)?;
            return Ok((
                StatusCode::BAD_REQUEST,
                Html(templates::operation_detail_page(
                    &user,
                    &operation,
                    &csrf_token,
                    Some("The request is invalid."),
                )),
            )
                .into_response());
        }
    };
    if !csrf_token_matches(&session, form.csrf_token.as_deref()).await?
        || !valid_form_values(&[&form.name, &form.purpose])
    {
        let csrf_token = issue_csrf_token(&session).await?;
        let operation = repository
            .find_operation(&operation_id)
            .await?
            .ok_or(AppError::NotFound)?;
        return Ok((
            StatusCode::BAD_REQUEST,
            Html(templates::operation_detail_page(
                &user,
                &operation,
                &csrf_token,
                Some("Name and purpose are required."),
            )),
        )
            .into_response());
    }
    let status = match form.status.as_str() {
        "active" => OperationStatus::Active,
        "closed" => OperationStatus::Closed,
        _ => OperationStatus::Planned,
    };
    repository
        .update_operation(
            &operation_id,
            form.name.trim(),
            form.purpose.trim(),
            form.scope_note.trim(),
            status,
            &user.id,
            &Uuid::new_v4().to_string(),
        )
        .await?;
    Ok(Redirect::to("/operations").into_response())
}

#[derive(Default, Deserialize)]
struct EventRuleForm {
    #[serde(default)]
    name: String,
    #[serde(default)]
    trigger: String,
    #[serde(default)]
    command: String,
    #[serde(default)]
    target: String,
    csrf_token: Option<String>,
}

#[derive(Default, Deserialize)]
struct OperationUpdateForm {
    #[serde(default)]
    name: String,
    #[serde(default)]
    purpose: String,
    #[serde(default)]
    scope_note: String,
    #[serde(default)]
    status: String,
    csrf_token: Option<String>,
}

#[derive(Default, Deserialize)]
struct SearchParams {
    q: Option<String>,
}

fn operation_report_summaries(
    operations: Vec<Operation>,
    assets: &[Asset],
    runs: &[CheckRun],
    events: &[AuditEvent],
) -> Vec<OperationReportSummary> {
    operations
        .into_iter()
        .map(|operation| {
            let operation_id = operation.id.clone();
            let mut summary = OperationReportSummary {
                asset_count: assets
                    .iter()
                    .filter(|asset| asset.operation_id == operation_id)
                    .count(),
                audit_count: events
                    .iter()
                    .filter(|event| event.operation_id.as_deref() == Some(operation_id.as_str()))
                    .count(),
                operation,
                queued_count: 0,
                running_count: 0,
                succeeded_count: 0,
                failed_count: 0,
                cancelled_count: 0,
            };
            for run in runs.iter().filter(|run| run.operation_id == operation_id) {
                match run.state {
                    RunState::Queued => summary.queued_count += 1,
                    RunState::Running => summary.running_count += 1,
                    RunState::Succeeded => summary.succeeded_count += 1,
                    RunState::Failed => summary.failed_count += 1,
                    RunState::Cancelled => summary.cancelled_count += 1,
                }
            }
            summary
        })
        .collect()
}

async fn admin(AuthenticatedUserGuard(user): AuthenticatedUserGuard) -> Result<Redirect, AppError> {
    user.require(Role::Admin)?;
    Ok(Redirect::to("/admin/users"))
}

async fn admin_users(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repository): State<Repository>,
    session: Session,
) -> Result<Html<String>, AppError> {
    user.require(Role::Admin)?;
    let users = repository.list_users().await?;
    let csrf_token = issue_csrf_token(&session).await?;
    Ok(Html(templates::admin_users_page(
        &user,
        &users,
        &csrf_token,
        None,
    )))
}

#[derive(Deserialize)]
struct UserRoleForm {
    role: String,
    csrf_token: Option<String>,
}

#[derive(Deserialize)]
struct UserDisabledForm {
    disabled: bool,
    csrf_token: Option<String>,
}

async fn set_user_role(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repository): State<Repository>,
    Path(user_id): Path<String>,
    session: Session,
    request: Request,
) -> Result<Response, AppError> {
    user.require(Role::Admin)?;
    if user.id == user_id {
        return Err(AppError::Conflict(
            "administrators cannot change their own role".to_owned(),
        ));
    }
    let form = match Form::<UserRoleForm>::from_request(request, &repository).await {
        Ok(Form(form)) => form,
        Err(_) => return invalid_admin_users_response(&user, &repository, &session).await,
    };
    if !csrf_token_matches(&session, form.csrf_token.as_deref()).await? {
        return invalid_admin_users_response(&user, &repository, &session).await;
    }
    let role = form
        .role
        .parse::<Role>()
        .map_err(|_| AppError::Validation("invalid user role".to_owned()))?;

    repository
        .set_user_role_with_audit(&user_id, role, &user.id, &Uuid::new_v4().to_string())
        .await?;
    Ok(Redirect::to("/admin/users").into_response())
}

async fn set_user_disabled(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repository): State<Repository>,
    Path(user_id): Path<String>,
    session: Session,
    request: Request,
) -> Result<Response, AppError> {
    user.require(Role::Admin)?;
    if user.id == user_id {
        return Err(AppError::Conflict(
            "administrators cannot disable their own account".to_owned(),
        ));
    }
    let form = match Form::<UserDisabledForm>::from_request(request, &repository).await {
        Ok(Form(form)) => form,
        Err(_) => return invalid_admin_users_response(&user, &repository, &session).await,
    };
    if !csrf_token_matches(&session, form.csrf_token.as_deref()).await? {
        return invalid_admin_users_response(&user, &repository, &session).await;
    }

    repository
        .set_user_disabled_with_audit(
            &user_id,
            form.disabled,
            &user.id,
            &Uuid::new_v4().to_string(),
        )
        .await?;
    Ok(Redirect::to("/admin/users").into_response())
}

async fn invalid_admin_users_response(
    user: &AuthenticatedUser,
    repository: &Repository,
    session: &Session,
) -> Result<Response, AppError> {
    let users = repository.list_users().await?;
    let csrf_token = issue_csrf_token(session).await?;
    Ok((
        StatusCode::BAD_REQUEST,
        Html(templates::admin_users_page(
            user,
            &users,
            &csrf_token,
            Some("The request is invalid."),
        )),
    )
        .into_response())
}

async fn logout(session: Session) -> Redirect {
    AuthSession { session }.logout().await;
    Redirect::to("/login")
}

async fn landing_page() -> axum::response::Html<String> {
    axum::response::Html(templates::public_landing())
}

async fn login_page(
    session: Option<Extension<Session>>,
) -> Result<axum::response::Html<String>, axum::http::StatusCode> {
    match session {
        Some(Extension(session)) => login_page_with_new_csrf(&session, None).await,
        None => Ok(axum::response::Html(templates::login_page(
            None,
            &Uuid::new_v4().to_string(),
        ))),
    }
}

pub async fn login_page_with_new_csrf(
    session: &Session,
    error: Option<&str>,
) -> Result<axum::response::Html<String>, axum::http::StatusCode> {
    let csrf_token = issue_csrf_token(session)
        .await
        .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(axum::response::Html(templates::login_page(
        error,
        &csrf_token,
    )))
}

pub async fn stylesheet() -> Response {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        include_str!("../static/admin.css"),
    )
        .into_response()
}

pub async fn script() -> Response {
    (
        [(
            header::CONTENT_TYPE,
            "application/javascript; charset=utf-8",
        )],
        include_str!("../static/admin.js"),
    )
        .into_response()
}

pub async fn payload_wizard_script() -> Response {
    (
        [(
            header::CONTENT_TYPE,
            "application/javascript; charset=utf-8",
        )],
        include_str!("../static/payload_wizard.js"),
    )
        .into_response()
}

pub async fn module_studio_script() -> Response {
    (
        [(
            header::CONTENT_TYPE,
            "application/javascript; charset=utf-8",
        )],
        include_str!("../static/module_studio.js"),
    )
        .into_response()
}

pub async fn anime_script() -> Response {
    (
        [(
            header::CONTENT_TYPE,
            "application/javascript; charset=utf-8",
        )],
        include_str!("../static/anime.min.js"),
    )
        .into_response()
}

async fn workspace_script() -> Response {
    (
        [(
            header::CONTENT_TYPE,
            "application/javascript; charset=utf-8",
        )],
        include_str!("../static/workspace.js"),
    )
        .into_response()
}

async fn motion_script() -> Response {
    (
        [(
            header::CONTENT_TYPE,
            "application/javascript; charset=utf-8",
        )],
        include_str!("../static/motion.js"),
    )
        .into_response()
}
async fn workspace_style() -> Response {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        include_str!("../static/workspace.css"),
    )
        .into_response()
}

#[cfg(test)]
mod public_download_tests {
    use super::*;

    #[test]
    fn content_disposition_cannot_inject_headers_from_a_legacy_filename() {
        let value = content_disposition("legacy\"\r\nX-Evil: yes-猫.bin")
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();

        assert!(!value.contains('\r'));
        assert!(!value.contains('\n'));
        assert!(!value.contains("X-Evil:"));
        assert!(value.starts_with("attachment; filename=\"legacy___X-Evil_ yes-_.bin\""));
        assert!(value.contains("filename*=UTF-8''legacy%22%0D%0AX-Evil%3A%20yes-%E7%8C%AB.bin"));
    }
}
