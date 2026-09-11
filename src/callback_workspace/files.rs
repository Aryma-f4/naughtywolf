use axum::{
    Extension, Json,
    body::Body,
    extract::{Multipart, Path, Query, State},
    http::{HeaderMap, HeaderValue, header},
    response::Response,
};
use serde::Deserialize;
use tower_sessions::Session;

use super::tasks::{TaskView, require_csrf, require_visible_callback, task_view};
use crate::{
    AppError,
    auth::{middleware::AuthenticatedUserGuard, rbac::Role},
    callback_workspace::transfers::{TransferError, TransferStore},
    db::{
        models::{FileSnapshot, FileTransfer},
        repositories::Repository,
    },
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DownloadRequest {
    path: String,
    #[serde(default)]
    expected_size: Option<u64>,
    #[serde(default)]
    sha256: Option<String>,
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

pub async fn transfers(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repository): State<Repository>,
    Path(session_id): Path<String>,
) -> Result<Json<Vec<FileTransfer>>, AppError> {
    user.require(Role::Operator)?;
    require_visible_callback(&repository, &user, &session_id).await?;
    Ok(Json(repository.list_file_transfers(&session_id).await?))
}

pub async fn download(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repository): State<Repository>,
    session: Session,
    Path(session_id): Path<String>,
    headers: HeaderMap,
    Extension(store): Extension<TransferStore>,
    Json(request): Json<DownloadRequest>,
) -> Result<Json<FileTransfer>, AppError> {
    user.require(Role::Operator)?;
    require_visible_callback(&repository, &user, &session_id).await?;
    require_csrf(&session, &headers).await?;
    let path = normalized(&request.path)?;
    let transfer = store
        .queue_download(
            &session_id,
            &path,
            request.expected_size,
            request.sha256.as_deref(),
            &user.id,
            &user.username,
        )
        .await
        .map_err(transfer_app_error)?;
    Ok(Json(transfer))
}

pub async fn upload(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repository): State<Repository>,
    session: Session,
    Path(session_id): Path<String>,
    headers: HeaderMap,
    Extension(store): Extension<TransferStore>,
    mut multipart: Multipart,
) -> Result<Json<FileTransfer>, AppError> {
    user.require(Role::Operator)?;
    require_visible_callback(&repository, &user, &session_id).await?;
    require_csrf(&session, &headers).await?;
    let mut destination: Option<String> = None;
    let mut stage = None;
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|_| AppError::Validation("invalid multipart upload".to_owned()))?
    {
        match field.name() {
            Some("destination") if destination.is_none() => {
                let mut value = Vec::new();
                while let Some(chunk) = field
                    .chunk()
                    .await
                    .map_err(|_| AppError::Validation("invalid upload destination".to_owned()))?
                {
                    if value.len().saturating_add(chunk.len()) > 4096 {
                        return Err(AppError::Validation(
                            "upload destination is too long".to_owned(),
                        ));
                    }
                    value.extend_from_slice(&chunk);
                }
                destination = Some(String::from_utf8(value).map_err(|_| {
                    AppError::Validation("upload destination must be UTF-8".to_owned())
                })?);
            }
            Some("file") if stage.is_none() => {
                let mut pending = store.begin_upload_stage().map_err(transfer_app_error)?;
                while let Some(chunk) = field
                    .chunk()
                    .await
                    .map_err(|_| AppError::Validation("invalid multipart file".to_owned()))?
                {
                    pending.write(&chunk).map_err(transfer_app_error)?;
                }
                stage = Some(pending);
            }
            _ => {
                return Err(AppError::Validation(
                    "multipart upload requires one destination and one file".to_owned(),
                ));
            }
        }
    }
    let destination = normalized(
        destination
            .as_deref()
            .ok_or_else(|| AppError::Validation("upload destination is required".to_owned()))?,
    )?;
    let transfer = store
        .finish_upload_stage(
            stage.ok_or_else(|| AppError::Validation("upload file is required".to_owned()))?,
            &session_id,
            &destination,
            &user.id,
            &user.username,
        )
        .await
        .map_err(transfer_app_error)?;
    Ok(Json(transfer))
}

pub async fn download_completed(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repository): State<Repository>,
    Path((session_id, transfer_id)): Path<(String, String)>,
    Extension(store): Extension<TransferStore>,
) -> Result<Response, AppError> {
    user.require(Role::Operator)?;
    require_visible_callback(&repository, &user, &session_id).await?;
    let transfer = repository
        .file_transfer_for_session(&session_id, &transfer_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let bytes = store
        .read_completed(&transfer)
        .await
        .map_err(transfer_app_error)?;
    let disposition = HeaderValue::from_str(&format!(
        "attachment; filename=\"download-{}.bin\"",
        transfer.id
    ))
    .map_err(|_| AppError::Internal)?;
    let mut response = Response::new(Body::from(bytes));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    response
        .headers_mut()
        .insert(header::CONTENT_DISPOSITION, disposition);
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    Ok(response)
}

fn transfer_app_error(error: TransferError) -> AppError {
    match error {
        TransferError::SizeLimit { .. }
        | TransferError::ChecksumMismatch
        | TransferError::TotalMismatch
        | TransferError::MissingProtocolId
        | TransferError::ProtocolMismatch
        | TransferError::TaskMismatch
        | TransferError::OffsetGap { .. }
        | TransferError::DuplicateMismatch => AppError::Validation(error.to_string()),
        TransferError::NotActive | TransferError::NotDownloadable => {
            AppError::Conflict(error.to_string())
        }
        TransferError::UnsafeStorage | TransferError::Storage | TransferError::Repository => {
            AppError::Internal
        }
    }
}
