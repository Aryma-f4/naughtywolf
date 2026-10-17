//! Cancelling a task the operator can see stop.
//!
//! Two things are asserted here, because they are the two different promises:
//!   * the ledger and the audit trail are right whether the kill went out on the
//!     live channel or had to wait for a beacon;
//!   * a kill that was pushed is never handed to the callback a second time on
//!     its next poll, which would run it twice.

use axum::{
    Router,
    body::Body,
    extract::Extension,
    http::{Request, StatusCode},
    response::IntoResponse,
    routing::post,
};
use base64::Engine;
use naughtywolf::{
    auth::{AuthenticatedUser, middleware::AuthSession, rbac::Role},
    c2_live::{LiveRegistry, LiveSink},
    db::{self, repositories::Repository},
    portal,
};
use tower::ServiceExt;
use tower_sessions::{MemoryStore, Session, SessionManagerLayer};
use uuid::Uuid;

async fn login(session: Session, Extension(user): Extension<AuthenticatedUser>) -> StatusCode {
    AuthSession { session }.login(&user).await.unwrap();
    StatusCode::NO_CONTENT
}

async fn csrf(session: Session) -> axum::response::Response {
    match portal::issue_csrf_token(&session).await {
        Ok(token) => (StatusCode::OK, token).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

fn operator() -> AuthenticatedUser {
    AuthenticatedUser {
        id: "operator-1".into(),
        username: "operator".into(),
        role: Role::Operator,
    }
}

/// One callback, one operator, and a signed-in session with a CSRF token.
async fn fixture(live: bool) -> (Router, Repository, LiveRegistry, Uuid, String) {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    db::run_migrations(&pool).await.unwrap();
    let repository = Repository { pool };
    let user = operator();
    sqlx::query(
        "INSERT INTO users (id, username, password_hash, role, disabled, created_at) \
         VALUES (?, ?, 'x', 'operator', 0, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
    )
    .bind(&user.id)
    .bind(&user.username)
    .execute(&repository.pool)
    .await
    .unwrap();

    let operation = repository
        .create_operation("Kill Lab", "Cancellation delivery")
        .await
        .unwrap();
    sqlx::query("INSERT INTO operation_members (operation_id, user_id) VALUES (?, ?)")
        .bind(&operation.id)
        .bind(&user.id)
        .execute(&repository.pool)
        .await
        .unwrap();

    let session = Uuid::new_v4();
    let key = base64::engine::general_purpose::STANDARD.encode([9u8; 32]);
    repository
        .upsert_callback(
            &session.to_string(),
            "lab-host",
            "operator",
            "linux",
            "x86_64",
            4242,
            &key,
        )
        .await
        .unwrap();
    sqlx::query("UPDATE callbacks SET operation_id = ? WHERE id = ?")
        .bind(&operation.id)
        .bind(session.to_string())
        .execute(&repository.pool)
        .await
        .unwrap();
    // The transport-facing row the live channel resolves the session key from.
    sqlx::query(
        "INSERT INTO c2_sessions (id, hostname, username, os, arch, pid, addr, session_key, last_seen) \
         VALUES (?, 'lab-host', 'operator', 'linux', 'x86_64', 4242, '127.0.0.1', ?, \
                 strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
    )
    .bind(session.to_string())
    .bind(vec![9u8; 32])
    .execute(&repository.pool)
    .await
    .ok();

    let registry = LiveRegistry::new();
    if live {
        let (frames, reader) = tokio::sync::mpsc::unbounded_channel();
        registry.attach(
            session,
            LiveSink::new(frames, std::sync::Arc::new(tokio::sync::Notify::new())),
        );
        // The socket task owns the receiving end in production; here it has to
        // outlive `fixture`, or the socket reads as closed and every push fails.
        std::mem::forget(reader);
    }

    let inner = Router::<Repository>::new()
        .route("/test/login", post(login))
        .route("/test/csrf", post(csrf))
        .merge(portal::authenticated_router())
        .with_state(repository.clone())
        .layer(Extension(registry.clone()))
        .layer(Extension(user))
        .layer(SessionManagerLayer::new(MemoryStore::default()).with_secure(false));
    let response = inner
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
    let issued = inner
        .clone()
        .oneshot(
            Request::post("/test/csrf")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let token = String::from_utf8(
        axum::body::to_bytes(issued.into_body(), 512)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    let app = Router::new().fallback_service(
        tower::ServiceBuilder::new()
            .map_request(move |mut request: Request<Body>| {
                request
                    .headers_mut()
                    .insert("cookie", cookie.parse().unwrap());
                request
            })
            .service(inner),
    );
    (app, repository, registry, session, token)
}

/// A task that is already running on the callback.
async fn running_task(repository: &Repository, session: Uuid) -> String {
    let task = repository
        .enqueue_task(
            &session.to_string(),
            "sleep",
            &serde_json::json!(["30"]),
            120_000,
        )
        .await
        .unwrap();
    repository
        .tasks_for_delivery(&session.to_string())
        .await
        .unwrap();
    repository
        .acknowledge_tasks(&session.to_string(), &[Uuid::parse_str(&task).unwrap()])
        .await
        .unwrap();
    task
}

#[tokio::test]
async fn a_pushed_kill_reports_an_immediate_stop_and_is_never_delivered_twice() {
    let (app, repository, registry, session, token) = fixture(true).await;
    assert!(
        registry.is_live(session),
        "the fixture registers a live socket for this session"
    );
    let task = running_task(&repository, session).await;

    let response = app
        .clone()
        .oneshot(
            Request::post(format!("/api/callbacks/{session}/tasks/{task}/cancel"))
                .header("x-csrf-token", &token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let raw = axum::body::to_bytes(response.into_body(), 8192)
        .await
        .unwrap();
    assert_eq!(
        status,
        StatusCode::OK,
        "body: {}",
        String::from_utf8_lossy(&raw)
    );
    let body: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(
        body["delivery"], "live",
        "the operator is told it stopped now"
    );
    assert_eq!(
        body["status"], "processing",
        "the kill itself is confirmed by the callback's next result"
    );

    let control_id = body["control_task_id"].as_str().expect("control task");
    let status: String = sqlx::query_scalar("SELECT status FROM c2_tasks WHERE id = ?")
        .bind(control_id)
        .fetch_one(&repository.pool)
        .await
        .unwrap();
    assert_eq!(
        status, "delivered",
        "a kill already on the socket must not be handed out again"
    );

    // The next poll would have delivered a pending control task; there is none.
    let pending: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM c2_tasks WHERE session_id = ? AND status = 'pending'",
    )
    .bind(session.to_string())
    .fetch_one(&repository.pool)
    .await
    .unwrap();
    assert_eq!(pending, 0);

    let actions: Vec<String> =
        sqlx::query_scalar("SELECT action FROM c2_audit WHERE target_session = ? ORDER BY rowid")
            .bind(session.to_string())
            .fetch_all(&repository.pool)
            .await
            .unwrap();
    assert!(
        actions.contains(&"task_cancellation_requested".to_owned()),
        "{actions:?}"
    );
    assert!(
        actions.contains(&"task_pushed_live".to_owned()),
        "the channel the kill went out on is recorded: {actions:?}"
    );
}

#[tokio::test]
async fn without_a_live_socket_the_kill_waits_for_the_next_beacon() {
    let (app, repository, _registry, session, token) = fixture(false).await;
    let task = running_task(&repository, session).await;

    let response = app
        .oneshot(
            Request::post(format!("/api/callbacks/{session}/tasks/{task}/cancel"))
                .header("x-csrf-token", &token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 8192)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        body["delivery"], "queued",
        "no socket means the operator waits for the next check-in"
    );

    // The control task is pending, and the next poll picks it up first.
    let control_id = body["control_task_id"].as_str().expect("control task");
    let delivered = repository
        .tasks_for_delivery(&session.to_string())
        .await
        .unwrap();
    let commands: Vec<String> = delivered.iter().map(|task| task.command.clone()).collect();
    assert_eq!(
        commands.first().map(String::as_str),
        Some("nw/killtask"),
        "a kill goes out ahead of ordinary pending work: {commands:?}"
    );
    let status: String = sqlx::query_scalar("SELECT status FROM c2_tasks WHERE id = ?")
        .bind(control_id)
        .fetch_one(&repository.pool)
        .await
        .unwrap();
    assert_eq!(status, "delivered");
}

#[tokio::test]
async fn a_task_that_never_left_the_server_is_stopped_without_asking_the_callback() {
    let (app, repository, registry, session, token) = fixture(true).await;
    let task = repository
        .enqueue_task(
            &session.to_string(),
            "sleep",
            &serde_json::json!(["30"]),
            120_000,
        )
        .await
        .unwrap();

    let response = app
        .oneshot(
            Request::post(format!("/api/callbacks/{session}/tasks/{task}/cancel"))
                .header("x-csrf-token", &token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 8192)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(body["status"], "cancelled");
    assert_eq!(
        body["delivery"], "before_delivery",
        "nothing was running, so there was nothing to interrupt"
    );
    assert!(
        body.get("control_task_id").is_none(),
        "no kill is queued for a task that never ran"
    );
    let controls: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM c2_tasks WHERE command = 'nw/killtask'")
            .fetch_one(&repository.pool)
            .await
            .unwrap();
    assert_eq!(controls, 0);
    let _ = registry;
}
