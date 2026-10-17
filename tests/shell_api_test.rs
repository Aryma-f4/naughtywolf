//! The operator-facing shell surface: who may attach, what is refused, and what
//! the response says about how the request will actually be delivered.

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
    shell_hub::ShellHub,
};
use tower::ServiceExt;
use tower_sessions::{MemoryStore, Session, SessionManagerLayer};
use uuid::Uuid;

async fn login(session: Session, Extension(user): Extension<AuthenticatedUser>) -> StatusCode {
    AuthSession { session }.login(&user).await.unwrap();
    StatusCode::NO_CONTENT
}

/// Mint this session's CSRF token, exactly as rendering a page does, and hand
/// it back so requests can submit what the browser would.
async fn csrf(session: Session) -> axum::response::Response {
    match portal::issue_csrf_token(&session).await {
        Ok(token) => (StatusCode::OK, token).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// The portal router with a logged-in operator, plus the two in-memory stores
/// the shell and live-channel endpoints read.
async fn app(
    repository: Repository,
    user: AuthenticatedUser,
    live: LiveRegistry,
    hub: ShellHub,
) -> (Router, String) {
    let inner = Router::<Repository>::new()
        .route("/test/login", post(login))
        .route("/test/csrf", post(csrf))
        .merge(portal::authenticated_router())
        .with_state(repository)
        .layer(Extension(live))
        .layer(Extension(hub))
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
    // Give the session its CSRF token, so the requests below are the same ones
    // an operator's browser would send from a rendered page.
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
    let csrf = String::from_utf8(
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
    (app, csrf)
}

/// A callback inside an operation the given user belongs to.
///
/// Scoping is the whole point of the workspace, so the fixture has to be a
/// callback its operator can actually reach.
async fn visible_callback(pool: &sqlx::SqlitePool, user_id: &str, id: &Uuid) {
    let repository = Repository { pool: pool.clone() };
    let operation = repository
        .create_operation("Shell Lab", "Interactive shell tests")
        .await
        .unwrap();
    sqlx::query("INSERT INTO operation_members (operation_id, user_id) VALUES (?, ?)")
        .bind(&operation.id)
        .bind(user_id)
        .execute(pool)
        .await
        .unwrap();
    let id = id.to_string();
    // A real session key is 32 bytes of base64: the live channel seals with it.
    let key = base64::engine::general_purpose::STANDARD.encode([7u8; 32]);
    repository
        .upsert_callback(&id, "lab-host", "operator", "linux", "x86_64", 4242, &key)
        .await
        .unwrap();
    sqlx::query("UPDATE callbacks SET operation_id = ? WHERE id = ?")
        .bind(&operation.id)
        .bind(&id)
        .execute(pool)
        .await
        .unwrap();
}

fn operator() -> AuthenticatedUser {
    AuthenticatedUser {
        id: "operator-1".into(),
        username: "operator".into(),
        role: Role::Operator,
    }
}

/// A migrated in-memory database holding the operator the test signs in as.
///
/// One connection, so every statement in a test sees the same in-memory
/// database, and the user row exists before anything references it.
async fn pool_with_callback(user: &AuthenticatedUser) -> sqlx::SqlitePool {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    db::run_migrations(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO users (id, username, password_hash, role, disabled, created_at) \
         VALUES (?, ?, 'x', ?, 0, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
    )
    .bind(&user.id)
    .bind(&user.username)
    .bind(user.role.to_string())
    .execute(&pool)
    .await
    .unwrap();
    pool
}

async fn harness_with(
    pool: sqlx::SqlitePool,
    user: AuthenticatedUser,
) -> (Router, LiveRegistry, ShellHub, String) {
    let live = LiveRegistry::new();
    let hub = ShellHub::new();
    let (app, csrf) = app(Repository { pool }, user, live.clone(), hub.clone()).await;
    (app, live, hub, csrf)
}

#[tokio::test]
async fn attaching_without_a_live_socket_is_accepted_but_reported_as_queued() {
    let pool = pool_with_callback(&operator()).await;
    let session = Uuid::new_v4();
    visible_callback(&pool, "operator-1", &session).await;
    let (app, _live, hub, csrf) = harness_with(pool, operator()).await;

    let response = app
        .clone()
        .oneshot(
            Request::post(format!("/api/callbacks/{session}/shell"))
                .header("content-type", "application/json")
                .header("x-csrf-token", &csrf)
                .body(Body::from(r#"{"program":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        body["delivery"], "queued",
        "the operator is told the shell waits for a beacon"
    );
    assert_eq!(
        hub.shells(&session.to_string()).len(),
        1,
        "the request is recorded"
    );
}

#[tokio::test]
async fn attaching_over_an_open_socket_reports_an_immediate_delivery() {
    let pool = pool_with_callback(&operator()).await;
    let session = Uuid::new_v4();
    visible_callback(&pool, "operator-1", &session).await;
    let (app, live, _hub, csrf) = harness_with(pool, operator()).await;
    let (frames, _reader) = tokio::sync::mpsc::unbounded_channel();
    live.attach(
        session,
        LiveSink::new(frames, std::sync::Arc::new(tokio::sync::Notify::new())),
    );

    let response = app
        .clone()
        .oneshot(
            Request::post(format!("/api/callbacks/{session}/shell"))
                .header("content-type", "application/json")
                .header("x-csrf-token", &csrf)
                .body(Body::from(r#"{"program":"/bin/bash"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(body["delivery"], "push");
    assert_eq!(body["shell"]["program"], "/bin/bash");
}

#[tokio::test]
async fn attaching_requires_a_csrf_token() {
    let pool = pool_with_callback(&operator()).await;
    let session = Uuid::new_v4();
    visible_callback(&pool, "operator-1", &session).await;
    let (app, _live, _hub, _csrf) = harness_with(pool, operator()).await;

    let response = app
        .oneshot(
            Request::post(format!("/api/callbacks/{session}/shell"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"program":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_shell_program_is_validated_before_it_can_reach_the_callback() {
    let pool = pool_with_callback(&operator()).await;
    let session = Uuid::new_v4();
    visible_callback(&pool, "operator-1", &session).await;
    let (app, _live, _hub, csrf) = harness_with(pool, operator()).await;

    for program in ["/bin/sh\nrm -rf /", &"x".repeat(300)] {
        let response = app
            .clone()
            .oneshot(
                Request::post(format!("/api/callbacks/{session}/shell"))
                    .header("content-type", "application/json")
                    .header("x-csrf-token", &csrf)
                    .body(Body::from(
                        serde_json::json!({ "program": program }).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{program:?}");
    }
}

#[tokio::test]
async fn typing_without_an_attached_shell_is_a_conflict_not_a_silent_drop() {
    let pool = pool_with_callback(&operator()).await;
    let session = Uuid::new_v4();
    visible_callback(&pool, "operator-1", &session).await;
    let (app, _live, _hub, csrf) = harness_with(pool, operator()).await;

    let response = app
        .oneshot(
            Request::post(format!("/api/callbacks/{session}/shell/input"))
                .header("content-type", "application/json")
                .header("x-csrf-token", &csrf)
                .body(Body::from(r#"{"data":"ls\r"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn a_viewer_may_not_attach_a_shell() {
    let viewer = AuthenticatedUser {
        id: "viewer-1".into(),
        username: "viewer".into(),
        role: Role::Viewer,
    };
    let pool = pool_with_callback(&viewer).await;
    let session = Uuid::new_v4();
    visible_callback(&pool, "viewer-1", &session).await;
    let (app, _live, _hub, csrf) = harness_with(pool, viewer).await;

    let response = app
        .oneshot(
            Request::post(format!("/api/callbacks/{session}/shell"))
                .header("content-type", "application/json")
                .header("x-csrf-token", &csrf)
                .body(Body::from(r#"{"program":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn an_unknown_callback_is_not_found() {
    let pool = pool_with_callback(&operator()).await;
    let (app, _live, _hub, csrf) = harness_with(pool, operator()).await;

    let response = app
        .oneshot(
            Request::post(format!("/api/callbacks/{}/shell", Uuid::new_v4()))
                .header("content-type", "application/json")
                .header("x-csrf-token", &csrf)
                .body(Body::from(r#"{"program":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_shell_list_reports_whether_this_callback_can_be_reached_instantly() {
    let pool = pool_with_callback(&operator()).await;
    let session = Uuid::new_v4();
    visible_callback(&pool, "operator-1", &session).await;
    let (app, live, _hub, _csrf) = harness_with(pool, operator()).await;
    let (frames, _reader) = tokio::sync::mpsc::unbounded_channel();
    live.attach(
        session,
        LiveSink::new(frames, std::sync::Arc::new(tokio::sync::Notify::new())),
    );

    let response = app
        .oneshot(
            Request::get(format!("/api/callbacks/{session}/shell"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        body["live"], true,
        "an open socket means attaches are instant"
    );
}
