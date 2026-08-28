use axum::{
    Extension, Router,
    http::header,
    response::{IntoResponse, Response},
    routing::get,
};
use tower_sessions::Session;
use uuid::Uuid;

use crate::{
    AppError,
    auth::{AuthenticatedUser, rbac::Role},
    db::{models::Operation, repositories::Repository},
};

pub mod templates;

pub const CSRF_TOKEN_KEY: &str = "login_csrf_token";

#[derive(Default, Debug, PartialEq)]
pub struct DashboardSummary {
    pub operation_count: i64,
    pub asset_count: i64,
    pub run_count: i64,
    pub evidence_count: i64,
    pub audit_count: i64,
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
    let csrf_token = Uuid::new_v4().to_string();
    session
        .insert(CSRF_TOKEN_KEY, &csrf_token)
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
