use axum::{
    Json,
    extract::{Path, Query, State},
    http::HeaderMap,
};
use serde::Deserialize;
use tower_sessions::Session;

use super::tasks::{TaskView, require_csrf, require_visible_callback, task_view};
use crate::{
    AppError,
    auth::{middleware::AuthenticatedUserGuard, rbac::Role},
    db::{models::FileSnapshot, repositories::Repository},
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PathRequest {
    path: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MoveRequest {
    source: String,
    destination: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeleteRequest {
    path: String,
    recursive: bool,
}

#[derive(Debug, Deserialize)]
pub struct SnapshotQuery {
    path: String,
}

fn normalized(path: &str) -> Result<String, AppError> {
    nw_profile::control::normalize_remote_path(path)
        .map_err(|_| AppError::Validation("invalid absolute filesystem path".to_owned()))
}

async fn enqueue(
    repository: &Repository,
    session_id: &str,
    user: &crate::auth::AuthenticatedUser,
    command: &str,
    arguments: Vec<String>,
    audit_action: &str,
    audit_target: String,
) -> Result<Json<TaskView>, AppError> {
    let task_id = repository
        .enqueue_filesystem_task_with_audit(
            session_id,
            command,
            &arguments,
            &user.id,
            &user.username,
            audit_action,
            &audit_target,
        )
        .await?;
    Ok(Json(task_view(repository, session_id, &task_id).await?))
}

pub async fn snapshot(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repository): State<Repository>,
    Path(session_id): Path<String>,
    Query(query): Query<SnapshotQuery>,
) -> Result<Json<Option<FileSnapshot>>, AppError> {
    user.require(Role::Operator)?;
    require_visible_callback(&repository, &user, &session_id).await?;
    let path = normalized(&query.path)?;
    Ok(Json(
        repository.latest_file_snapshot(&session_id, &path).await?,
    ))
}

pub async fn list(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repository): State<Repository>,
    session: Session,
    Path(session_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<PathRequest>,
) -> Result<Json<TaskView>, AppError> {
    user.require(Role::Operator)?;
    require_visible_callback(&repository, &user, &session_id).await?;
    require_csrf(&session, &headers).await?;
    let path = normalized(&request.path)?;
    enqueue(
        &repository,
        &session_id,
        &user,
        "nw/fs-list",
        vec![path.clone()],
        "filesystem_list_enqueued",
        format!("exact path {path}"),
    )
    .await
}

pub async fn mkdir(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repository): State<Repository>,
    session: Session,
    Path(session_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<PathRequest>,
) -> Result<Json<TaskView>, AppError> {
    user.require(Role::Operator)?;
    require_visible_callback(&repository, &user, &session_id).await?;
    require_csrf(&session, &headers).await?;
    let path = normalized(&request.path)?;
    enqueue(
        &repository,
        &session_id,
        &user,
        "nw/fs-mkdir",
        vec![path.clone()],
        "filesystem_mkdir_enqueued",
        format!("exact path {path}"),
    )
    .await
}

pub async fn move_path(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repository): State<Repository>,
    session: Session,
    Path(session_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<MoveRequest>,
) -> Result<Json<TaskView>, AppError> {
    user.require(Role::Operator)?;
    require_visible_callback(&repository, &user, &session_id).await?;
    require_csrf(&session, &headers).await?;
    let source = normalized(&request.source)?;
    let destination = normalized(&request.destination)?;
    enqueue(
        &repository,
        &session_id,
        &user,
        "nw/fs-move",
        vec![source.clone(), destination.clone()],
        "filesystem_move_enqueued",
        format!("exact source {source}; exact destination {destination}"),
    )
    .await
}

pub async fn delete(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repository): State<Repository>,
    session: Session,
    Path(session_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<DeleteRequest>,
) -> Result<Json<TaskView>, AppError> {
    user.require(Role::Operator)?;
    require_visible_callback(&repository, &user, &session_id).await?;
    require_csrf(&session, &headers).await?;
    let path = normalized(&request.path)?;
    enqueue(
        &repository,
        &session_id,
        &user,
        "nw/fs-delete",
        vec![path.clone(), request.recursive.to_string()],
        "filesystem_delete_enqueued",
        format!("exact path {path}; recursive {}", request.recursive),
    )
    .await
}
