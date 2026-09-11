use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use tower_sessions::Session;

use super::tasks::{TaskView, require_csrf, require_visible_callback, task_view};
use crate::{
    AppError,
    auth::{middleware::AuthenticatedUserGuard, rbac::Role},
    db::{models::ProcessSnapshot, repositories::Repository},
};

pub async fn snapshot(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repository): State<Repository>,
    Path(session_id): Path<String>,
) -> Result<Json<Option<ProcessSnapshot>>, AppError> {
    user.require(Role::Operator)?;
    require_visible_callback(&repository, &user, &session_id).await?;
    Ok(Json(repository.latest_process_snapshot(&session_id).await?))
}

pub async fn refresh(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repository): State<Repository>,
    session: Session,
    Path(session_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<TaskView>, AppError> {
    user.require(Role::Operator)?;
    require_visible_callback(&repository, &user, &session_id).await?;
    require_csrf(&session, &headers).await?;
    let task_id = repository
        .enqueue_task_with_audit(
            &session_id,
            "nw/process-list",
            &serde_json::json!([]),
            30_000,
            &user.id,
            &user.username,
            None,
            "process_refresh_enqueued",
        )
        .await?;
    Ok(Json(task_view(&repository, &session_id, &task_id).await?))
}

pub async fn kill(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repository): State<Repository>,
    session: Session,
    Path((session_id, pid)): Path<(String, u32)>,
    headers: HeaderMap,
) -> Result<Json<TaskView>, AppError> {
    user.require(Role::Operator)?;
    require_visible_callback(&repository, &user, &session_id).await?;
    require_csrf(&session, &headers).await?;
    let task_id = repository
        .enqueue_process_kill_with_audit(&session_id, pid, &user.id, &user.username)
        .await?;
    Ok(Json(task_view(&repository, &session_id, &task_id).await?))
}
