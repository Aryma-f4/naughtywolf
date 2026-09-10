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
