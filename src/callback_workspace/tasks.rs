use axum::{
    Json,
    extract::{Path, Query, State},
    http::HeaderMap,
    response::sse::{Event, KeepAlive, Sse},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    AppError,
    auth::{middleware::AuthenticatedUserGuard, rbac::Role},
    db::{
        models::{TaskCancellation, TaskRecord},
        repositories::Repository,
    },
    portal::CSRF_TOKEN_KEY,
};
use std::{
    collections::{HashMap, VecDeque},
    convert::Infallible,
    time::Duration,
};
use tower_sessions::Session;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskView {
    pub id: String,
    pub session_id: String,
    pub command: String,
    pub arguments: Value,
    pub timeout_ms: i64,
    pub status: String,
    pub state_label: String,
    pub state_class: String,
    pub operator_id: Option<String>,
    pub operator_name: Option<String>,
    pub parent_task_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub processing_at: Option<String>,
    pub completed_at: Option<String>,
    pub cancellation_requested_at: Option<String>,
    pub stdout: Option<String>,
    pub stderr: Option<String>,
    pub exit_code: Option<i32>,
}

impl From<TaskRecord> for TaskView {
    fn from(task: TaskRecord) -> Self {
        let (state_label, state_class) =
            crate::db::models::TaskStatus::label_class_from_str(&task.status);
        Self {
            id: task.id,
            session_id: task.session_id,
            command: task.command,
            arguments: task.args_json,
            timeout_ms: task.timeout_ms,
            status: task.status,
            state_label: state_label.to_owned(),
            state_class: state_class.to_owned(),
            operator_id: task.operator_id,
            operator_name: task.operator_name,
            parent_task_id: task.parent_task_id,
            created_at: task.created_at,
            updated_at: task.updated_at,
            processing_at: task.processing_at,
            completed_at: task.completed_at,
            cancellation_requested_at: task.cancellation_requested_at,
            stdout: task
                .result_stdout
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned()),
            stderr: task
                .result_stderr
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned()),
            exit_code: task.result_exit_code,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TaskPage {
    pub tasks: Vec<TaskView>,
    pub next_before: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TaskPageQuery {
    limit: Option<usize>,
    before: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct TaskCursor {
    created_at: String,
    id: String,
}

fn decode_cursor(encoded: &str) -> Result<TaskCursor, AppError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| AppError::Validation("invalid task cursor".to_owned()))?;
    serde_json::from_slice(&bytes)
        .map_err(|_| AppError::Validation("invalid task cursor".to_owned()))
}

fn encode_cursor(task: &TaskView) -> Result<String, AppError> {
    let bytes = serde_json::to_vec(&TaskCursor {
        created_at: task.created_at.clone(),
        id: task.id.clone(),
    })
    .map_err(|_| AppError::Internal)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

pub async fn list_tasks(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repository): State<Repository>,
    Path(session_id): Path<String>,
    Query(query): Query<TaskPageQuery>,
) -> Result<Json<TaskPage>, AppError> {
    user.require(Role::Operator)?;
    repository
        .find_callback_visible_to(&session_id, &user.id, user.role == Role::Admin)
        .await?
        .ok_or(AppError::NotFound)?;

    let limit = query.limit.unwrap_or(20).clamp(1, 100);
    let cursor = query.before.as_deref().map(decode_cursor).transpose()?;
    let before = cursor
        .as_ref()
        .map(|cursor| (cursor.created_at.as_str(), cursor.id.as_str()));
    let mut tasks = repository
        .list_task_page(&session_id, before, limit + 1)
        .await?
        .into_iter()
        .map(TaskView::from)
        .collect::<Vec<_>>();
    let next_before = if tasks.len() > limit {
        tasks.truncate(limit);
        tasks.last().map(encode_cursor).transpose()?
    } else {
        None
    };
    Ok(Json(TaskPage { tasks, next_before }))
}

#[derive(Debug, Deserialize)]
pub struct EnqueueTaskRequest {
    command: String,
    #[serde(default, alias = "args")]
    arguments: Vec<String>,
    timeout_ms: Option<u64>,
}

fn is_reserved_structured_command(command: &str) -> bool {
    matches!(
        command,
        "nw/process-list"
            | "nw/process-kill"
            | "nw/fs-list"
            | "nw/fs-stat"
            | "nw/fs-mkdir"
            | "nw/fs-move"
            | "nw/fs-delete"
    )
}

pub(super) async fn require_csrf(session: &Session, headers: &HeaderMap) -> Result<(), AppError> {
    let expected: Option<String> = session
        .get(CSRF_TOKEN_KEY)
        .await
        .map_err(|_| AppError::Internal)?;
    let submitted = headers
        .get("x-csrf-token")
        .and_then(|value| value.to_str().ok());
    if expected.as_deref() != submitted || submitted.is_none() {
        return Err(AppError::Validation("invalid CSRF token".to_owned()));
    }
    Ok(())
}

pub(super) async fn require_visible_callback(
    repository: &Repository,
    user: &crate::auth::AuthenticatedUser,
    session_id: &str,
) -> Result<(), AppError> {
    repository
        .find_callback_visible_to(session_id, &user.id, user.role == Role::Admin)
        .await?
        .map(|_| ())
        .ok_or(AppError::NotFound)
}

pub(super) async fn task_view(
    repository: &Repository,
    session_id: &str,
    task_id: &str,
) -> Result<TaskView, AppError> {
    repository
        .find_task_record(session_id, task_id)
        .await?
        .map(TaskView::from)
        .ok_or(AppError::NotFound)
}

pub async fn enqueue_task(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repository): State<Repository>,
    session: Session,
    Path(session_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<EnqueueTaskRequest>,
) -> Result<Json<TaskView>, AppError> {
    user.require(Role::Operator)?;
    require_visible_callback(&repository, &user, &session_id).await?;
    require_csrf(&session, &headers).await?;
    let command = request.command.trim();
    if command.is_empty() || command.chars().count() > 256 {
        return Err(AppError::Validation("invalid command".to_owned()));
    }
    if is_reserved_structured_command(command) {
        return Err(AppError::Validation(
            "structured controls must use their typed endpoints".to_owned(),
        ));
    }
    let timeout_ms = request.timeout_ms.unwrap_or(30_000).clamp(1, 600_000);
    let task_id = repository
        .enqueue_task_with_audit(
            &session_id,
            command,
            &serde_json::json!(request.arguments),
            timeout_ms,
            &user.id,
            &user.username,
            None,
            "task_enqueued",
        )
        .await?;
    Ok(Json(task_view(&repository, &session_id, &task_id).await?))
}

pub async fn retry_task(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repository): State<Repository>,
    session: Session,
    Path((session_id, task_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<TaskView>, AppError> {
    user.require(Role::Operator)?;
    require_visible_callback(&repository, &user, &session_id).await?;
    require_csrf(&session, &headers).await?;
    let original_task = repository
        .find_task_record(&session_id, &task_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if is_reserved_structured_command(&original_task.command) {
        return Err(AppError::Validation(
            "structured controls must use their typed endpoints".to_owned(),
        ));
    }
    let retry_id = repository
        .retry_task_with_audit(&session_id, &task_id, &user.id, &user.username)
        .await?;
    Ok(Json(task_view(&repository, &session_id, &retry_id).await?))
}

pub async fn cancel_task(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repository): State<Repository>,
    session: Session,
    Path((session_id, task_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<TaskView>, AppError> {
    user.require(Role::Operator)?;
    require_visible_callback(&repository, &user, &session_id).await?;
    require_csrf(&session, &headers).await?;
    match repository
        .request_task_cancellation(&session_id, &task_id, &user.id, &user.username)
        .await?
    {
        TaskCancellation::AlreadyTerminal => {
            return Err(AppError::Conflict("task is already terminal".to_owned()));
        }
        TaskCancellation::Cancelled | TaskCancellation::Requested { .. } => {}
    }
    Ok(Json(task_view(&repository, &session_id, &task_id).await?))
}

pub async fn task_events(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repository): State<Repository>,
    Path(session_id): Path<String>,
) -> Result<Sse<impl futures::Stream<Item = Result<Event, Infallible>> + Send>, AppError> {
    user.require(Role::Operator)?;
    require_visible_callback(&repository, &user, &session_id).await?;

    let all_tasks = repository.list_all_task_records(&session_id).await?;
    let mut known = HashMap::new();
    let mut known_transfers = HashMap::new();
    let mut queued = VecDeque::new();
    for (index, record) in all_tasks.into_iter().enumerate() {
        let view = TaskView::from(record);
        let serialized = serde_json::to_string(&view).map_err(|_| AppError::Internal)?;
        known.insert(view.id.clone(), serialized.clone());
        if index < 20 {
            queued.push_back(("task", serialized));
        }
    }
    for transfer in repository.list_file_transfers(&session_id).await? {
        let view = crate::callback_workspace::transfers::TransferView::from(transfer);
        let serialized = serde_json::to_string(&view).map_err(|_| AppError::Internal)?;
        known_transfers.insert(view.id.clone(), serialized.clone());
        queued.push_back(("transfer", serialized));
    }

    let stream = futures::stream::unfold(
        (repository, session_id, known, known_transfers, queued),
        |(repository, session_id, mut known, mut known_transfers, mut queued)| async move {
            loop {
                if let Some((kind, snapshot)) = queued.pop_front() {
                    let event = Event::default().event(kind).data(snapshot);
                    return Some((
                        Ok(event),
                        (repository, session_id, known, known_transfers, queued),
                    ));
                }

                tokio::time::sleep(Duration::from_millis(500)).await;
                let records = match repository.list_all_task_records(&session_id).await {
                    Ok(records) => records,
                    Err(_) => return None,
                };
                for record in records.into_iter().rev() {
                    let view = TaskView::from(record);
                    let Ok(serialized) = serde_json::to_string(&view) else {
                        continue;
                    };
                    if known.get(&view.id) != Some(&serialized) {
                        known.insert(view.id, serialized.clone());
                        queued.push_back(("task", serialized));
                    }
                }
                let transfers = match repository.list_file_transfers(&session_id).await {
                    Ok(transfers) => transfers,
                    Err(_) => return None,
                };
                for transfer in transfers.into_iter().rev() {
                    let view = crate::callback_workspace::transfers::TransferView::from(transfer);
                    let Ok(serialized) = serde_json::to_string(&view) else {
                        continue;
                    };
                    if known_transfers.get(&view.id) != Some(&serialized) {
                        known_transfers.insert(view.id, serialized.clone());
                        queued.push_back(("transfer", serialized));
                    }
                }
            }
        },
    );

    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keepalive"),
    ))
}
