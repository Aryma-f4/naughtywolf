use axum::{
    Extension, Form, Router,
    extract::{Path, State},
    http::{StatusCode, header},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use serde::Deserialize;
use tower_sessions::Session;
use uuid::Uuid;

use crate::{
    AppError,
    auth::{
        AuthenticatedUser,
        middleware::{AuthSession, AuthenticatedUserGuard},
        rbac::Role,
    },
    db::{models::Operation, repositories::Repository},
    policy::authorize_operation,
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

/// Routes that always resolve the current local identity before rendering.
/// The binary supplies the repository state and session layer when composing
/// this router with the public routes.
pub fn authenticated_router() -> Router<Repository> {
    Router::new()
        .route("/dashboard", get(dashboard))
        .route("/operations", get(operations).post(create_operation))
        .route("/operations/new", get(new_operation))
        .route("/operations/{operation_id}/assets", post(create_asset))
        .route("/operations/{operation_id}/assets/new", get(new_asset))
        .route("/inventory", get(inventory))
        .route("/checks", get(checks))
        .route("/audit", get(audit))
        .route("/evidence", get(evidence))
        .route("/reports", get(reports))
        .route("/admin", get(admin))
        .route("/logout", post(logout))
}

async fn dashboard(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repository): State<Repository>,
) -> Result<Html<String>, AppError> {
    let summary = dashboard_summary(&repository, &user).await?;
    Ok(Html(templates::dashboard_page(&user, &summary)))
}

async fn operations(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repository): State<Repository>,
) -> Result<Html<String>, AppError> {
    let operations = visible_operations(&repository, &user).await?;
    Ok(Html(templates::operations_page(&user, &operations)))
}

#[derive(Deserialize)]
struct OperationForm {
    #[serde(default)]
    name: String,
    #[serde(default)]
    purpose: String,
    csrf_token: Option<String>,
}

#[derive(Deserialize)]
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
    Form(form): Form<OperationForm>,
) -> Result<Response, AppError> {
    user.require(Role::Admin)?;
    if !csrf_token_matches(&session, form.csrf_token.as_deref()).await?
        || !valid_form_values(&[&form.name, &form.purpose])
    {
        let csrf_token = issue_csrf_token(&session).await?;
        return Ok((
            StatusCode::BAD_REQUEST,
            Html(templates::operation_form_page(
                &user,
                &csrf_token,
                Some("The request is invalid."),
                &form.name,
                &form.purpose,
            )),
        )
            .into_response());
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
    Form(form): Form<AssetForm>,
) -> Result<Response, AppError> {
    authorize_operation(&repository, &user, &operation_id, Role::Operator).await?;
    if !csrf_token_matches(&session, form.csrf_token.as_deref()).await?
        || !valid_form_values(&[&form.name, &form.kind, &form.owner, &form.address])
    {
        let csrf_token = issue_csrf_token(&session).await?;
        return Ok((
            StatusCode::BAD_REQUEST,
            Html(templates::asset_form_page(
                &user,
                &operation_id,
                &csrf_token,
                Some("The request is invalid."),
                [&form.name, &form.kind, &form.owner, &form.address],
            )),
        )
            .into_response());
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

async fn checks(AuthenticatedUserGuard(user): AuthenticatedUserGuard) -> Html<String> {
    Html(templates::empty_page(
        "Checks",
        &user,
        "checks",
        "No scoped checks yet",
    ))
}

async fn audit(AuthenticatedUserGuard(user): AuthenticatedUserGuard) -> Html<String> {
    Html(templates::empty_page(
        "Audit",
        &user,
        "audit",
        "No scoped audit records yet",
    ))
}

async fn evidence(AuthenticatedUserGuard(user): AuthenticatedUserGuard) -> Html<String> {
    Html(templates::empty_page(
        "Evidence",
        &user,
        "evidence",
        "No scoped evidence yet",
    ))
}

async fn reports(AuthenticatedUserGuard(user): AuthenticatedUserGuard) -> Html<String> {
    Html(templates::empty_page(
        "Reports",
        &user,
        "reports",
        "No scoped reports yet",
    ))
}

async fn admin(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
) -> Result<Html<String>, AppError> {
    user.require(Role::Admin)?;
    Ok(Html(templates::admin_page(&user)))
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
