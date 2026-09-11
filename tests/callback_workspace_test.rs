use axum::{
    Router,
    body::{Body, to_bytes},
    extract::Extension,
    http::{Request, StatusCode},
    response::Response,
    routing::post,
};
use futures::StreamExt;
use naughtywolf::{
    auth::{AuthenticatedUser, middleware::AuthSession, rbac::Role},
    db::{self, repositories::Repository},
    portal,
};
use serde_json::Value;
use tower::ServiceExt;
use tower_sessions::{MemoryStore, Session, SessionManagerLayer};

async fn test_repository() -> Repository {
    let pool = db::create_pool("sqlite::memory:").await.unwrap();
    db::run_migrations(&pool).await.unwrap();
    Repository { pool }
}

async fn create_user(repo: &Repository, id: &str, role: Role) -> AuthenticatedUser {
    let username = format!("{id}-user");
    sqlx::query("INSERT INTO users (id, username, password_hash, role) VALUES (?, ?, ?, ?)")
        .bind(id)
        .bind(&username)
        .bind("test-password-hash")
        .bind(role.to_string())
        .execute(&repo.pool)
        .await
        .unwrap();
    AuthenticatedUser {
        id: id.to_owned(),
        username,
        role,
    }
}

async fn create_scoped_callback(repo: &Repository, user_id: &str, session_id: &str) {
    let operation = repo
        .create_operation("Task API Lab", "Pagination tests")
        .await
        .unwrap();
    sqlx::query("INSERT INTO operation_members (operation_id, user_id) VALUES (?, ?)")
        .bind(&operation.id)
        .bind(user_id)
        .execute(&repo.pool)
        .await
        .unwrap();
    repo.upsert_callback(
        session_id,
        "task-api-host",
        "operator",
        "linux",
        "x86_64",
        42,
        "test-session-key",
    )
    .await
    .unwrap();
    sqlx::query("UPDATE callbacks SET operation_id = ? WHERE id = ?")
        .bind(&operation.id)
        .bind(session_id)
        .execute(&repo.pool)
        .await
        .unwrap();
}

fn process_snapshot_fixture(captured_at: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "schema": "nw.process-list.v1",
        "captured_at": captured_at,
        "processes": [{
            "pid": 7331,
            "parent_pid": 1,
            "name": "safe-worker",
            "executable": "/opt/lab/safe-worker",
            "user": "operator",
            "architecture": "x86_64",
            "cpu_percent": 2.5,
            "memory_bytes": 8192,
            "started_at": "2026-09-10T11:00:00Z"
        }]
    }))
    .unwrap()
}

fn filesystem_snapshot_fixture(path: &str, captured_at: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "schema": "nw.fs-list.v1",
        "captured_at": captured_at,
        "path": path,
        "entries": [{
            "name": "<img src=x onerror=alert(1)>.txt",
            "path": format!("{path}/<img src=x onerror=alert(1)>.txt"),
            "kind": "file",
            "size": 4,
            "modified_at": "2026-09-10T11:00:00Z",
            "permissions": "0644",
            "owner": "operator"
        }]
    }))
    .unwrap()
}

async fn login(session: Session, Extension(user): Extension<AuthenticatedUser>) -> StatusCode {
    AuthSession { session }.login(&user).await.unwrap();
    StatusCode::NO_CONTENT
}

async fn authenticated_app(repository: Repository, user: AuthenticatedUser) -> Router {
    let app = Router::<Repository>::new()
        .route("/test/login", post(login))
        .merge(portal::authenticated_router())
        .with_state(repository)
        .layer(Extension(user))
        .layer(SessionManagerLayer::new(MemoryStore::default()).with_secure(false));
    let response = app
        .clone()
        .oneshot(Request::post("/test/login").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let cookie = response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    Router::new().fallback_service(
        tower::ServiceBuilder::new()
            .map_request(move |mut request: Request<Body>| {
                request
                    .headers_mut()
                    .insert("cookie", cookie.parse().unwrap());
                request
            })
            .service(app),
    )
}

async fn json(response: Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
}

async fn response_text(response: Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

fn csrf_token(page: &str) -> String {
    page.split("name=\"csrf_token\" value=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("callback page csrf token")
        .to_owned()
}

#[tokio::test]
async fn task_api_paginates_on_created_at_and_id_without_overlap() {
    let repo = test_repository().await;
    let operator = create_user(&repo, "page-operator", Role::Operator).await;
    let session_id = uuid::Uuid::new_v4().to_string();
    create_scoped_callback(&repo, &operator.id, &session_id).await;

    for index in 0..35 {
        let id = repo
            .enqueue_task(
                &session_id,
                &format!("command-{index:02}"),
                &serde_json::json!([index]),
                30_000,
            )
            .await
            .unwrap();
        sqlx::query("UPDATE c2_tasks SET created_at = ?, updated_at = ? WHERE id = ?")
            .bind(format!("2026-09-10T12:{index:02}:00.000Z"))
            .bind(format!("2026-09-10T12:{index:02}:00.000Z"))
            .bind(id)
            .execute(&repo.pool)
            .await
            .unwrap();
    }

    let app = authenticated_app(repo, operator).await;
    let first = app
        .clone()
        .oneshot(
            Request::get(format!("/api/callbacks/{session_id}/tasks?limit=20"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let first = json(first).await;
    assert_eq!(first["tasks"].as_array().unwrap().len(), 20);
    assert_eq!(first["tasks"][0]["command"], "command-34");
    let before = first["next_before"].as_str().expect("first page cursor");
    assert!(!before.contains("2026-09-10"), "cursor must be opaque");

    let second = app
        .oneshot(
            Request::get(format!(
                "/api/callbacks/{session_id}/tasks?limit=20&before={before}"
            ))
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::OK);
    let second = json(second).await;
    assert_eq!(second["tasks"].as_array().unwrap().len(), 15);
    assert!(second["next_before"].is_null());
    let first_ids = first["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|task| task["id"].as_str().unwrap())
        .collect::<std::collections::HashSet<_>>();
    assert!(
        second["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|task| !first_ids.contains(task["id"].as_str().unwrap()))
    );
}

#[tokio::test]
async fn process_list_result_validates_and_persists_projection_atomically() {
    let repo = test_repository().await;
    let operator = create_user(&repo, "process-projection", Role::Operator).await;
    let session_id = uuid::Uuid::new_v4().to_string();
    create_scoped_callback(&repo, &operator.id, &session_id).await;
    let task_id = repo
        .enqueue_task(
            &session_id,
            "nw/process-list",
            &serde_json::json!([]),
            30_000,
        )
        .await
        .unwrap();
    let stdout = process_snapshot_fixture("2026-09-10T12:34:56Z");

    assert!(
        repo.store_task_result_for_session(&session_id, &task_id, true, &stdout, &[], 0)
            .await
            .unwrap()
    );
    let snapshot = repo
        .latest_process_snapshot(&session_id)
        .await
        .unwrap()
        .expect("persisted process projection");
    assert_eq!(snapshot.task_id, task_id);
    assert_eq!(snapshot.schema_version, "nw.process-list.v1");
    assert_eq!(snapshot.captured_at, "2026-09-10T12:34:56Z");
    assert_eq!(snapshot.snapshot_json["processes"][0]["pid"], 7331);
}

#[tokio::test]
async fn process_list_result_rejects_unknown_schema_without_storing_any_result() {
    let repo = test_repository().await;
    let operator = create_user(&repo, "process-invalid", Role::Operator).await;
    let session_id = uuid::Uuid::new_v4().to_string();
    create_scoped_callback(&repo, &operator.id, &session_id).await;
    let task_id = repo
        .enqueue_task(
            &session_id,
            "nw/process-list",
            &serde_json::json!([]),
            30_000,
        )
        .await
        .unwrap();
    let invalid =
        br#"{"schema":"nw.process-list.v2","captured_at":"2026-09-10T12:34:56Z","processes":[]}"#;

    let error = repo
        .store_task_result_for_session(&session_id, &task_id, true, invalid, &[], 0)
        .await
        .unwrap_err();
    assert!(matches!(error, naughtywolf::AppError::Validation(_)));
    let result_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM c2_task_results WHERE task_id = ?")
            .bind(&task_id)
            .fetch_one(&repo.pool)
            .await
            .unwrap();
    let snapshot_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM c2_process_snapshots WHERE session_id = ?")
            .bind(&session_id)
            .fetch_one(&repo.pool)
            .await
            .unwrap();
    assert_eq!((result_count, snapshot_count), (0, 0));
}

#[tokio::test]
async fn older_process_snapshot_result_cannot_replace_newer_projection() {
    let repo = test_repository().await;
    let operator = create_user(&repo, "process-order", Role::Operator).await;
    let session_id = uuid::Uuid::new_v4().to_string();
    create_scoped_callback(&repo, &operator.id, &session_id).await;
    let newer_task = repo
        .enqueue_task(
            &session_id,
            "nw/process-list",
            &serde_json::json!([]),
            30_000,
        )
        .await
        .unwrap();
    let older_task = repo
        .enqueue_task(
            &session_id,
            "nw/process-list",
            &serde_json::json!([]),
            30_000,
        )
        .await
        .unwrap();
    repo.store_task_result_for_session(
        &session_id,
        &newer_task,
        true,
        &process_snapshot_fixture("2026-09-10T12:35:00Z"),
        &[],
        0,
    )
    .await
    .unwrap();
    repo.store_task_result_for_session(
        &session_id,
        &older_task,
        true,
        &process_snapshot_fixture("2026-09-10T12:34:00Z"),
        &[],
        0,
    )
    .await
    .unwrap();

    let snapshot = repo
        .latest_process_snapshot(&session_id)
        .await
        .unwrap()
        .expect("newer process projection");
    assert_eq!(snapshot.task_id, newer_task);
    assert_eq!(snapshot.captured_at, "2026-09-10T12:35:00Z");
}

#[tokio::test]
async fn filesystem_list_result_atomically_upserts_normalized_monotonic_projection() {
    let repo = test_repository().await;
    let operator = create_user(&repo, "filesystem-projection", Role::Operator).await;
    let session_id = uuid::Uuid::new_v4().to_string();
    create_scoped_callback(&repo, &operator.id, &session_id).await;
    let newer_task = repo
        .enqueue_task(
            &session_id,
            "nw/fs-list",
            &serde_json::json!(["/srv//lab/../files/"]),
            30_000,
        )
        .await
        .unwrap();
    let older_task = repo
        .enqueue_task(
            &session_id,
            "nw/fs-list",
            &serde_json::json!(["/srv/files"]),
            30_000,
        )
        .await
        .unwrap();

    repo.store_task_result_for_session(
        &session_id,
        &newer_task,
        true,
        &filesystem_snapshot_fixture("/srv/files", "2026-09-10T12:35:00Z"),
        &[],
        0,
    )
    .await
    .unwrap();
    repo.store_task_result_for_session(
        &session_id,
        &older_task,
        true,
        &filesystem_snapshot_fixture("/srv/files", "2026-09-10T12:34:00Z"),
        &[],
        0,
    )
    .await
    .unwrap();

    let snapshot = repo
        .latest_file_snapshot(&session_id, "/srv/files")
        .await
        .unwrap()
        .expect("persisted filesystem projection");
    assert_eq!(snapshot.path, "/srv/files");
    assert_eq!(snapshot.task_id, newer_task);
    assert_eq!(snapshot.schema_version, "nw.fs-list.v1");
    assert_eq!(snapshot.captured_at, "2026-09-10T12:35:00Z");
    assert_eq!(snapshot.snapshot_json["entries"][0]["size"], 4);
}

#[tokio::test]
async fn filesystem_list_result_rejects_path_mismatch_without_any_result_write() {
    let repo = test_repository().await;
    let operator = create_user(&repo, "filesystem-invalid", Role::Operator).await;
    let session_id = uuid::Uuid::new_v4().to_string();
    create_scoped_callback(&repo, &operator.id, &session_id).await;
    let task_id = repo
        .enqueue_task(
            &session_id,
            "nw/fs-list",
            &serde_json::json!(["/srv/files"]),
            30_000,
        )
        .await
        .unwrap();

    let error = repo
        .store_task_result_for_session(
            &session_id,
            &task_id,
            true,
            &filesystem_snapshot_fixture("/srv/other", "2026-09-10T12:35:00Z"),
            &[],
            0,
        )
        .await
        .unwrap_err();
    assert!(matches!(error, naughtywolf::AppError::Validation(_)));
    let result_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM c2_task_results WHERE task_id = ?")
            .bind(&task_id)
            .fetch_one(&repo.pool)
            .await
            .unwrap();
    let snapshot_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM c2_file_snapshots WHERE session_id = ?")
            .bind(&session_id)
            .fetch_one(&repo.pool)
            .await
            .unwrap();
    assert_eq!((result_count, snapshot_count), (0, 0));
}

#[tokio::test]
async fn filesystem_mutation_result_must_match_the_exact_task_target_atomically() {
    let repo = test_repository().await;
    let operator = create_user(&repo, "filesystem-result", Role::Operator).await;
    let session_id = uuid::Uuid::new_v4().to_string();
    create_scoped_callback(&repo, &operator.id, &session_id).await;
    let task_id = repo
        .enqueue_task(
            &session_id,
            "nw/fs-delete",
            &serde_json::json!(["/srv/exact", "true"]),
            30_000,
        )
        .await
        .unwrap();
    let mismatched = br#"{"schema":"nw.fs-mutation.v1","action":"delete","path":"/srv/other","destination":null,"recursive":true,"completed_at":"2026-09-10T12:35:00Z"}"#;

    let error = repo
        .store_task_result_for_session(&session_id, &task_id, true, mismatched, &[], 0)
        .await
        .unwrap_err();
    assert!(matches!(error, naughtywolf::AppError::Validation(_)));
    let result_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM c2_task_results WHERE task_id = ?")
            .bind(&task_id)
            .fetch_one(&repo.pool)
            .await
            .unwrap();
    assert_eq!(result_count, 0);

    let valid_task = repo
        .enqueue_task(
            &session_id,
            "nw/fs-delete",
            &serde_json::json!(["/srv/exact", "true"]),
            30_000,
        )
        .await
        .unwrap();
    let valid = br#"{"schema":"nw.fs-mutation.v1","action":"delete","path":"/srv/exact","destination":null,"recursive":true,"completed_at":"2026-09-10T12:35:01Z"}"#;
    assert!(
        repo.store_task_result_for_session(&session_id, &valid_task, true, valid, &[], 0)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn filesystem_routes_require_role_scope_csrf_and_audit_exact_paths() {
    let repo = test_repository().await;
    let operator = create_user(&repo, "filesystem-owner", Role::Operator).await;
    let outsider = create_user(&repo, "filesystem-outsider", Role::Operator).await;
    let viewer = create_user(&repo, "filesystem-viewer", Role::Viewer).await;
    let session_id = uuid::Uuid::new_v4().to_string();
    create_scoped_callback(&repo, &operator.id, &session_id).await;
    let owner_app = authenticated_app(repo.clone(), operator).await;

    let missing_csrf = owner_app
        .clone()
        .oneshot(
            Request::post(format!("/api/callbacks/{session_id}/files/mkdir"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"path":"/srv/exact <target>"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing_csrf.status(), StatusCode::BAD_REQUEST);

    let hidden = authenticated_app(repo.clone(), outsider)
        .await
        .oneshot(
            Request::post(format!("/api/callbacks/{session_id}/files/delete"))
                .header("content-type", "application/json")
                .header("x-csrf-token", "not-disclosed")
                .body(Body::from(
                    r#"{"path":"/srv/exact <target>","recursive":false}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(hidden.status(), StatusCode::NOT_FOUND);

    let viewer_denied = authenticated_app(repo.clone(), viewer)
        .await
        .oneshot(
            Request::post(format!("/api/callbacks/{session_id}/files/list"))
                .header("content-type", "application/json")
                .header("x-csrf-token", "not-disclosed")
                .body(Body::from(r#"{"path":"/srv"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(viewer_denied.status(), StatusCode::FORBIDDEN);

    let page = owner_app
        .clone()
        .oneshot(
            Request::get(format!("/callbacks/{session_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let csrf = csrf_token(&response_text(page).await);
    let cases = [
        (
            "list",
            r#"{"path":"/srv/files"}"#,
            "nw/fs-list",
            serde_json::json!(["/srv/files"]),
        ),
        (
            "mkdir",
            r#"{"path":"/srv/exact <target>"}"#,
            "nw/fs-mkdir",
            serde_json::json!(["/srv/exact <target>"]),
        ),
        (
            "move",
            r#"{"source":"/srv/exact <target>","destination":"/srv/moved & safe"}"#,
            "nw/fs-move",
            serde_json::json!(["/srv/exact <target>", "/srv/moved & safe"]),
        ),
        (
            "delete",
            r#"{"path":"/srv/moved & safe","recursive":true}"#,
            "nw/fs-delete",
            serde_json::json!(["/srv/moved & safe", "true"]),
        ),
    ];
    for (route, body, command, arguments) in cases {
        let response = owner_app
            .clone()
            .oneshot(
                Request::post(format!("/api/callbacks/{session_id}/files/{route}"))
                    .header("content-type", "application/json")
                    .header("x-csrf-token", &csrf)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{route}");
        let task = json(response).await;
        assert_eq!(task["command"], command);
        assert_eq!(task["arguments"], arguments);
    }
    let audits: Vec<(String, String)> = sqlx::query_as(
        "SELECT action, details FROM c2_audit WHERE target_session = ? AND action LIKE 'filesystem_%' ORDER BY id",
    )
    .bind(&session_id)
    .fetch_all(&repo.pool)
    .await
    .unwrap();
    assert_eq!(audits.len(), 4);
    assert!(
        audits
            .iter()
            .any(|(action, details)| action == "filesystem_mkdir_enqueued"
                && details.contains("exact path /srv/exact <target>"))
    );
    assert!(
        audits
            .iter()
            .any(|(action, details)| action == "filesystem_move_enqueued"
                && details.contains("source /srv/exact <target>")
                && details.contains("destination /srv/moved & safe"))
    );
    assert!(
        audits
            .iter()
            .any(|(action, details)| action == "filesystem_delete_enqueued"
                && details.contains("exact path /srv/moved & safe")
                && details.contains("recursive true"))
    );
}

#[tokio::test]
async fn filesystem_routes_reject_invalid_paths_and_non_boolean_recursion_before_enqueue() {
    let repo = test_repository().await;
    let operator = create_user(&repo, "filesystem-validation", Role::Operator).await;
    let session_id = uuid::Uuid::new_v4().to_string();
    create_scoped_callback(&repo, &operator.id, &session_id).await;
    let app = authenticated_app(repo.clone(), operator).await;
    let page = app
        .clone()
        .oneshot(
            Request::get(format!("/callbacks/{session_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let csrf = csrf_token(&response_text(page).await);

    for body in [
        r#"{"path":""}"#,
        r#"{"path":"bad\u0000path"}"#,
        r#"{"path":"relative/path"}"#,
        r#"{"path":"/srv/files","recursive":"true"}"#,
        r#"{"path":"/srv/files","recursive":1}"#,
    ] {
        let route = if body.contains("recursive") {
            "delete"
        } else {
            "mkdir"
        };
        let response = app
            .clone()
            .oneshot(
                Request::post(format!("/api/callbacks/{session_id}/files/{route}"))
                    .header("content-type", "application/json")
                    .header("x-csrf-token", &csrf)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            matches!(
                response.status(),
                StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY
            ),
            "{body}: {}",
            response.status()
        );
    }
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM c2_tasks")
        .fetch_one(&repo.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn process_kill_success_enqueues_a_linked_refresh_after_completion() {
    let repo = test_repository().await;
    let operator = create_user(&repo, "process-refresh", Role::Operator).await;
    let session_id = uuid::Uuid::new_v4().to_string();
    create_scoped_callback(&repo, &operator.id, &session_id).await;
    let kill_id = repo
        .enqueue_task_with_audit(
            &session_id,
            "nw/process-kill",
            &serde_json::json!([7331]),
            30_000,
            &operator.id,
            &operator.username,
            None,
            "process_kill_enqueued",
        )
        .await
        .unwrap();
    let kill_result = br#"{"schema":"nw.process-kill.v1","pid":7331,"name":"safe-worker","terminated":true,"terminated_at":"2026-09-10T12:35:00Z"}"#;

    repo.store_task_result_for_session(&session_id, &kill_id, true, kill_result, &[], 0)
        .await
        .unwrap();

    let refresh: (String, serde_json::Value, String, Option<String>) = sqlx::query_as(
        "SELECT command, args_json, status, parent_task_id FROM c2_tasks WHERE parent_task_id = ?",
    )
    .bind(&kill_id)
    .fetch_one(&repo.pool)
    .await
    .unwrap();
    assert_eq!(refresh.0, "nw/process-list");
    assert_eq!(refresh.1, serde_json::json!([]));
    assert_eq!(refresh.2, "pending");
    assert_eq!(refresh.3.as_deref(), Some(kill_id.as_str()));
}

#[tokio::test]
async fn process_control_mutations_require_scope_and_csrf_and_audit_exact_pid() {
    let repo = test_repository().await;
    let operator = create_user(&repo, "process-owner", Role::Operator).await;
    let outsider = create_user(&repo, "process-outsider", Role::Operator).await;
    let session_id = uuid::Uuid::new_v4().to_string();
    create_scoped_callback(&repo, &operator.id, &session_id).await;
    let owner_app = authenticated_app(repo.clone(), operator).await;

    let missing_csrf = owner_app
        .clone()
        .oneshot(
            Request::post(format!("/api/callbacks/{session_id}/processes/refresh"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing_csrf.status(), StatusCode::BAD_REQUEST);

    let missing_kill_csrf = owner_app
        .clone()
        .oneshot(
            Request::post(format!("/api/callbacks/{session_id}/processes/7331/kill"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing_kill_csrf.status(), StatusCode::BAD_REQUEST);

    let outsider_app = authenticated_app(repo.clone(), outsider).await;
    let hidden = outsider_app
        .oneshot(
            Request::post(format!("/api/callbacks/{session_id}/processes/7331/kill"))
                .header("x-csrf-token", "not-disclosed")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(hidden.status(), StatusCode::NOT_FOUND);

    let page = owner_app
        .clone()
        .oneshot(
            Request::get(format!("/callbacks/{session_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let csrf = csrf_token(&response_text(page).await);
    let kill = owner_app
        .oneshot(
            Request::post(format!("/api/callbacks/{session_id}/processes/7331/kill"))
                .header("x-csrf-token", csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(kill.status(), StatusCode::OK);
    let kill = json(kill).await;
    assert_eq!(kill["command"], "nw/process-kill");
    assert_eq!(kill["arguments"], serde_json::json!(["7331"]));
    let details: String = sqlx::query_scalar(
        "SELECT details FROM c2_audit WHERE target_session = ? AND action = 'process_kill_enqueued'",
    )
    .bind(&session_id)
    .fetch_one(&repo.pool)
    .await
    .unwrap();
    assert!(details.contains("PID 7331"), "audit details: {details}");
}

#[tokio::test]
async fn generic_task_endpoints_reject_reserved_structured_commands_and_retries() {
    let repo = test_repository().await;
    let operator = create_user(&repo, "reserved-process", Role::Operator).await;
    let session_id = uuid::Uuid::new_v4().to_string();
    create_scoped_callback(&repo, &operator.id, &session_id).await;
    let app = authenticated_app(repo.clone(), operator).await;
    let page = app
        .clone()
        .oneshot(
            Request::get(format!("/callbacks/{session_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let csrf = csrf_token(&response_text(page).await);

    for command in [
        "nw/process-list",
        "nw/process-kill",
        "nw/fs-list",
        "nw/fs-stat",
        "nw/fs-mkdir",
        "nw/fs-move",
        "nw/fs-delete",
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::post(format!("/api/callbacks/{session_id}/tasks"))
                    .header("content-type", "application/json")
                    .header("x-csrf-token", &csrf)
                    .body(Body::from(format!(
                        r#"{{"command":"{command}","arguments":[]}}"#
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{command}");
    }

    let old_task = repo
        .enqueue_task(
            &session_id,
            "nw/process-kill",
            &serde_json::json!(["7331"]),
            30_000,
        )
        .await
        .unwrap();
    let response = app
        .oneshot(
            Request::post(format!(
                "/api/callbacks/{session_id}/tasks/{old_task}/retry"
            ))
            .header("x-csrf-token", &csrf)
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM c2_tasks")
        .fetch_one(&repo.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn task_api_returns_not_found_for_a_callback_outside_operator_scope() {
    let repo = test_repository().await;
    let owner = create_user(&repo, "scope-owner", Role::Operator).await;
    let outsider = create_user(&repo, "scope-outsider", Role::Operator).await;
    let session_id = uuid::Uuid::new_v4().to_string();
    create_scoped_callback(&repo, &owner.id, &session_id).await;
    let app = authenticated_app(repo, outsider).await;

    for path in [
        format!("/api/callbacks/{session_id}/tasks"),
        format!("/callbacks/{session_id}"),
    ] {
        let response = app
            .clone()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}

#[tokio::test]
async fn task_api_propagates_repository_failures_as_server_errors() {
    let repo = test_repository().await;
    let operator = create_user(&repo, "failure-operator", Role::Operator).await;
    let session_id = uuid::Uuid::new_v4().to_string();
    create_scoped_callback(&repo, &operator.id, &session_id).await;
    sqlx::query("DROP TABLE c2_tasks")
        .execute(&repo.pool)
        .await
        .unwrap();
    let app = authenticated_app(repo, operator).await;

    let response = app
        .oneshot(
            Request::get(format!("/api/callbacks/{session_id}/tasks"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn task_api_mutations_require_csrf_and_persist_operator_audit_links() {
    let repo = test_repository().await;
    let operator = create_user(&repo, "mutation-operator", Role::Operator).await;
    let session_id = uuid::Uuid::new_v4().to_string();
    create_scoped_callback(&repo, &operator.id, &session_id).await;
    let app = authenticated_app(repo.clone(), operator.clone()).await;

    let missing_csrf = app
        .clone()
        .oneshot(
            Request::post(format!("/api/callbacks/{session_id}/tasks"))
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"command":"whoami","arguments":[],"timeout_ms":30000}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing_csrf.status(), StatusCode::BAD_REQUEST);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM c2_tasks")
        .fetch_one(&repo.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);

    let page = app
        .clone()
        .oneshot(
            Request::get(format!("/callbacks/{session_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let csrf = csrf_token(&response_text(page).await);
    let enqueue = app
        .clone()
        .oneshot(
            Request::post(format!("/api/callbacks/{session_id}/tasks"))
                .header("content-type", "application/json")
                .header("x-csrf-token", &csrf)
                .body(Body::from(
                    r#"{"command":"whoami","arguments":["--all"],"timeout_ms":30000}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(enqueue.status(), StatusCode::OK);
    let original = json(enqueue).await;
    assert_eq!(original["operator_id"], operator.id);
    assert_eq!(original["operator_name"], operator.username);

    let retry = app
        .clone()
        .oneshot(
            Request::post(format!(
                "/api/callbacks/{session_id}/tasks/{}/retry",
                original["id"].as_str().unwrap()
            ))
            .header("x-csrf-token", &csrf)
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(retry.status(), StatusCode::OK);
    let retry = json(retry).await;
    assert_eq!(retry["parent_task_id"], original["id"]);

    let cancel = app
        .oneshot(
            Request::post(format!(
                "/api/callbacks/{session_id}/tasks/{}/cancel",
                retry["id"].as_str().unwrap()
            ))
            .header("x-csrf-token", &csrf)
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cancel.status(), StatusCode::OK);
    assert_eq!(json(cancel).await["status"], "cancelled");

    let actions: Vec<String> =
        sqlx::query_scalar("SELECT action FROM c2_audit WHERE target_session = ? ORDER BY rowid")
            .bind(&session_id)
            .fetch_all(&repo.pool)
            .await
            .unwrap();
    assert_eq!(actions, ["task_enqueued", "task_retried", "task_cancelled"]);
}

#[tokio::test]
async fn enqueue_and_audit_roll_back_together_when_the_audit_write_fails() {
    let repo = test_repository().await;
    let operator = create_user(&repo, "rollback-operator", Role::Operator).await;
    let session_id = uuid::Uuid::new_v4().to_string();
    create_scoped_callback(&repo, &operator.id, &session_id).await;
    let app = authenticated_app(repo.clone(), operator).await;
    let page = app
        .clone()
        .oneshot(
            Request::get(format!("/callbacks/{session_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let csrf = csrf_token(&response_text(page).await);
    sqlx::query("DROP TABLE c2_audit")
        .execute(&repo.pool)
        .await
        .unwrap();

    let response = app
        .oneshot(
            Request::post(format!("/api/callbacks/{session_id}/tasks"))
                .header("content-type", "application/json")
                .header("x-csrf-token", csrf)
                .body(Body::from(r#"{"command":"id","arguments":[]}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM c2_tasks")
        .fetch_one(&repo.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn task_events_begin_with_an_authoritative_complete_snapshot() {
    let repo = test_repository().await;
    let operator = create_user(&repo, "events-operator", Role::Operator).await;
    let session_id = uuid::Uuid::new_v4().to_string();
    create_scoped_callback(&repo, &operator.id, &session_id).await;
    let task_id = repo
        .enqueue_task_with_audit(
            &session_id,
            "hostname",
            &serde_json::json!(["--fqdn"]),
            30_000,
            &operator.id,
            &operator.username,
            None,
            "task_enqueued",
        )
        .await
        .unwrap();
    let app = authenticated_app(repo, operator).await;

    let response = app
        .oneshot(
            Request::get(format!("/api/callbacks/{session_id}/events"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/event-stream")
    );
    let mut stream = response.into_body().into_data_stream();
    let first = tokio::time::timeout(std::time::Duration::from_secs(1), stream.next())
        .await
        .expect("initial SSE snapshot")
        .expect("SSE body item")
        .expect("SSE bytes");
    let event = String::from_utf8(first.to_vec()).unwrap();
    assert!(event.contains("event: task"));
    assert!(event.contains(&task_id));
    assert!(event.contains("\"arguments\":[\"--fqdn\"]"));
    assert!(event.contains("\"status\":\"pending\""));
    assert!(event.contains("\"operator_name\":\"events-operator-user\""));
}

#[tokio::test]
async fn task_events_terminate_when_reconciliation_reads_fail() {
    let repo = test_repository().await;
    let operator = create_user(&repo, "stream-failure-operator", Role::Operator).await;
    let session_id = uuid::Uuid::new_v4().to_string();
    create_scoped_callback(&repo, &operator.id, &session_id).await;
    repo.enqueue_task(&session_id, "pwd", &serde_json::json!([]), 30_000)
        .await
        .unwrap();
    let app = authenticated_app(repo.clone(), operator).await;
    let response = app
        .oneshot(
            Request::get(format!("/api/callbacks/{session_id}/events"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let mut stream = response.into_body().into_data_stream();
    tokio::time::timeout(std::time::Duration::from_secs(1), stream.next())
        .await
        .expect("initial task snapshot")
        .expect("initial body item")
        .expect("initial event bytes");
    sqlx::query("DROP TABLE c2_tasks")
        .execute(&repo.pool)
        .await
        .unwrap();

    let next = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
        .await
        .expect("repository failure must not leave a stale stream connected");
    assert!(next.is_none(), "repository failure must terminate SSE");
}

#[tokio::test]
async fn tasking_tab_exposes_persistent_controls_and_local_assets() {
    let repo = test_repository().await;
    let operator = create_user(&repo, "tab-operator", Role::Operator).await;
    let session_id = uuid::Uuid::new_v4().to_string();
    create_scoped_callback(&repo, &operator.id, &session_id).await;
    let app = authenticated_app(repo, operator).await;

    let response = app
        .oneshot(
            Request::get(format!("/callbacks/{session_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let page = response_text(response).await;
    assert!(page.contains("/static/callback-workspace.css"));
    assert!(page.contains("/static/callback-workspace.js"));
    assert!(page.contains("data-callback-tasking"));
    assert!(page.contains(&format!(
        "data-tasks-endpoint=\"/api/callbacks/{session_id}/tasks\""
    )));
    assert!(page.contains(&format!(
        "data-events-endpoint=\"/api/callbacks/{session_id}/events\""
    )));
    assert!(page.contains(&format!(
        "data-process-panel data-processes-endpoint=\"/api/callbacks/{session_id}/processes\""
    )));
    assert!(page.contains("data-process-capable=\"false\""));
    for label in [
        "Tasking",
        "Search task history",
        "State",
        "Errors",
        "Load older",
        "Connection",
    ] {
        assert!(page.contains(label), "missing tasking label {label}");
    }
}

#[tokio::test]
async fn filesystem_workspace_renders_url_navigation_controls_and_disabled_transfers() {
    let repo = test_repository().await;
    let operator = create_user(&repo, "filesystem-page", Role::Operator).await;
    let session_id = uuid::Uuid::new_v4().to_string();
    create_scoped_callback(&repo, &operator.id, &session_id).await;
    let app = authenticated_app(repo, operator).await;

    let response = app
        .oneshot(
            Request::get(format!(
                "/callbacks/{session_id}?tab=files&path=%2Fsrv%2Flab"
            ))
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let page = response_text(response).await;
    for marker in [
        "data-file-panel",
        "data-file-path-form",
        "data-file-breadcrumbs",
        "data-file-parent",
        "data-file-refresh",
        "data-file-mkdir-form",
        "data-file-table-body",
        "data-file-confirm",
        "data-file-task-link",
        "data-file-snapshot-age",
    ] {
        assert!(page.contains(marker), "missing {marker}");
    }
    assert!(page.contains("?tab=files&amp;path=%2F"));
    assert_eq!(
        page.matches("Transfer support is being initialized")
            .count(),
        2
    );
    assert!(page.contains("data-file-upload disabled"));
    assert!(page.contains("data-file-download disabled"));
}
