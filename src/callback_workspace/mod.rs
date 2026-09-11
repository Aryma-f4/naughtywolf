use axum::{
    Router,
    routing::{get, post},
};

use crate::db::repositories::Repository;

pub mod files;
pub mod processes;
pub mod tasks;
pub mod transfers;

pub fn router() -> Router<Repository> {
    Router::new()
        .route(
            "/api/callbacks/{session_id}/tasks",
            get(tasks::list_tasks).post(tasks::enqueue_task),
        )
        .route(
            "/api/callbacks/{session_id}/tasks/{task_id}/retry",
            post(tasks::retry_task),
        )
        .route(
            "/api/callbacks/{session_id}/tasks/{task_id}/cancel",
            post(tasks::cancel_task),
        )
        .route(
            "/api/callbacks/{session_id}/events",
            get(tasks::task_events),
        )
        .route(
            "/api/callbacks/{session_id}/processes",
            get(processes::snapshot),
        )
        .route(
            "/api/callbacks/{session_id}/processes/refresh",
            post(processes::refresh),
        )
        .route(
            "/api/callbacks/{session_id}/processes/{pid}/kill",
            post(processes::kill),
        )
        .route("/api/callbacks/{session_id}/files", get(files::snapshot))
        .route("/api/callbacks/{session_id}/files/list", post(files::list))
        .route(
            "/api/callbacks/{session_id}/files/mkdir",
            post(files::mkdir),
        )
        .route(
            "/api/callbacks/{session_id}/files/move",
            post(files::move_path),
        )
        .route(
            "/api/callbacks/{session_id}/files/delete",
            post(files::delete),
        )
        .route(
            "/api/callbacks/{session_id}/files/upload",
            post(files::upload),
        )
        .route(
            "/api/callbacks/{session_id}/files/download",
            post(files::download),
        )
        .route(
            "/api/callbacks/{session_id}/transfers",
            get(files::transfers),
        )
        .route(
            "/api/callbacks/{session_id}/transfers/{transfer_id}/download",
            get(files::download_completed),
        )
}
