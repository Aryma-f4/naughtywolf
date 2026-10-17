use axum::{
    Router,
    body::{Body, to_bytes},
    extract::Extension,
    http::{Request, StatusCode},
    response::Response,
    routing::post,
};
use naughtywolf::{
    auth::{AuthenticatedUser, middleware::AuthSession, rbac::Role},
    db::{self, models::Callback, repositories::Repository},
};
use tower::ServiceExt;
use tower_sessions::{MemoryStore, SessionManagerLayer};

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
        .bind("not-a-real-password-hash")
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

fn post_form(path: &str, body: impl Into<Body>) -> Request<Body> {
    Request::post(path)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(body.into())
        .unwrap()
}

fn csrf_token(body: &str) -> String {
    body.split("name=\"csrf_token\" value=\"")
        .nth(1)
        .and_then(|remainder| remainder.split('"').next())
        .unwrap()
        .to_owned()
}

async fn body_string(response: Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

async fn app_with_user_and_repository(
    repository: Repository,
    user: AuthenticatedUser,
) -> impl tower::Service<Request<Body>, Response = Response, Error = std::convert::Infallible> + Clone
{
    let app = Router::<Repository>::new()
        .route("/test/login", post(test_login))
        .merge(naughtywolf::portal::authenticated_router())
        .with_state(repository)
        .merge(naughtywolf::portal::public_router())
        .layer(Extension(user))
        .layer(SessionManagerLayer::new(MemoryStore::default()).with_secure(false));
    let login_response = app
        .clone()
        .oneshot(Request::post("/test/login").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let cookie = login_response
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();

    tower::ServiceBuilder::new()
        .map_request(move |mut request: Request<Body>| {
            request
                .headers_mut()
                .insert("cookie", cookie.parse().unwrap());
            request
        })
        .service(app)
}

async fn test_login(
    session: tower_sessions::Session,
    Extension(user): Extension<AuthenticatedUser>,
) -> StatusCode {
    AuthSession { session }.login(&user).await.unwrap();
    StatusCode::NO_CONTENT
}

/// `register_callback` writes the callback row and the `c2_sessions` row with
/// the same id, and `c2_tasks.session_id` has a foreign key into `c2_sessions`.
/// A fixture that only creates the callback row produces a target that can
/// never receive a task.
async fn insert_callback(repo: &Repository, id: &str, operation_id: Option<&str>, host: &str) {
    sqlx::query(
        "INSERT INTO callbacks (id, operation_id, host, user_name, os, arch, process, protocol, status, session_key, last_seen) \
         VALUES (?, ?, ?, 'root', 'linux', 'x86_64', 'nw-implant', 'http', 'active', ?, strftime('%Y-%m-%dT%H:%M:%fZ','now'))",
    )
    .bind(id)
    .bind(operation_id)
    .bind(host)
    .bind(format!("key-{id}"))
    .execute(&repo.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO c2_sessions (id, hostname, username, os, arch, pid, addr, session_key, last_seen) \
         VALUES (?, ?, 'root', 'linux', 'x86_64', 42, '127.0.0.1', ?, strftime('%Y-%m-%dT%H:%M:%fZ','now'))",
    )
    .bind(id)
    .bind(host)
    .bind(format!("key-{id}"))
    .execute(&repo.pool)
    .await
    .unwrap();
}

async fn count_tasks(repo: &Repository, session_id: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM c2_tasks WHERE session_id = ? AND command = ?",
    )
    .bind(session_id)
    .bind("nw/process-list")
    .fetch_one(&repo.pool)
    .await
    .unwrap()
}

/// A group must never be able to hold a callback from another operation. This
/// is the constraint that stops a fan-out from crossing a scope boundary.
#[tokio::test]
async fn a_group_cannot_absorb_a_callback_from_another_operation() {
    let repo = test_repository().await;
    let allowed = repo.create_operation("Allowed", "in scope").await.unwrap();
    let hidden = repo
        .create_operation("Hidden", "out of scope")
        .await
        .unwrap();
    insert_callback(&repo, "cb-allowed", Some(&allowed.id), "allowed-host").await;
    insert_callback(&repo, "cb-hidden", Some(&hidden.id), "hidden-host").await;

    let actor = create_user(&repo, "operator", Role::Operator).await;
    repo.add_member(&allowed.id, &actor.id).await.unwrap();

    let group = repo
        .create_callback_group(
            &allowed.id,
            "workstations",
            "lab fleet",
            &actor.id,
            "corr-1",
        )
        .await
        .unwrap();
    assert!(
        repo.add_callback_to_group(&group.id, "cb-allowed", &actor.id, "corr-2")
            .await
            .is_ok()
    );
    assert!(
        matches!(
            repo.add_callback_to_group(&group.id, "cb-hidden", &actor.id, "corr-3")
                .await,
            Err(naughtywolf::AppError::Forbidden)
        ),
        "a cross-operation callback must be refused"
    );
    assert_eq!(
        repo.resolve_group_targets(&group.id, &actor.id, false)
            .await
            .unwrap()
            .len(),
        1
    );
}

/// Callbacks carry a nullable operation_id. A group dispatch must never reach
/// an unattached callback just because its host matched something.
#[tokio::test]
async fn an_unattached_callback_is_never_a_dispatch_target() {
    let repo = test_repository().await;
    let operation = repo.create_operation("Lab", "scope").await.unwrap();
    insert_callback(&repo, "cb-scoped", Some(&operation.id), "scoped-host").await;
    insert_callback(&repo, "cb-orphan", None, "orphan-host").await;

    let actor = create_user(&repo, "operator", Role::Operator).await;
    repo.add_member(&operation.id, &actor.id).await.unwrap();
    let group = repo
        .create_callback_group(&operation.id, "fleet", "", &actor.id, "corr-1")
        .await
        .unwrap();
    repo.add_callback_to_group(&group.id, "cb-scoped", &actor.id, "corr-2")
        .await
        .unwrap();
    // Bypass the guard directly to prove the resolver is defence in depth.
    sqlx::query("INSERT INTO callback_group_members (group_id, callback_id) VALUES (?, ?)")
        .bind(&group.id)
        .bind("cb-orphan")
        .execute(&repo.pool)
        .await
        .unwrap();

    let targets = repo
        .resolve_group_targets(&group.id, &actor.id, false)
        .await
        .unwrap();
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].id, "cb-scoped");
}

/// Event rules store a command, so an operator must not read another
/// operation's rules.
#[tokio::test]
async fn event_rules_are_scoped_to_operation_membership() {
    let repo = test_repository().await;
    let allowed = repo.create_operation("Allowed", "in scope").await.unwrap();
    let hidden = repo
        .create_operation("Hidden", "out of scope")
        .await
        .unwrap();
    let actor = create_user(&repo, "operator", Role::Operator).await;
    let other = create_user(&repo, "other", Role::Operator).await;
    repo.add_member(&allowed.id, &actor.id).await.unwrap();

    repo.create_event_rule(
        &allowed.id,
        "mine",
        "new callback",
        "allowed-command",
        "all",
        &actor.id,
        "corr-1",
    )
    .await
    .unwrap();
    repo.create_event_rule(
        &hidden.id,
        "theirs",
        "new callback",
        "secret-command",
        "all",
        &other.id,
        "corr-2",
    )
    .await
    .unwrap();

    let visible = repo
        .list_event_rules_visible_to(&actor.id, false)
        .await
        .unwrap();
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].name, "mine");

    let admin_view = repo
        .list_event_rules_visible_to(&actor.id, true)
        .await
        .unwrap();
    assert_eq!(admin_view.len(), 2);
}

/// The dispatch confirmation must match the resolved set, so a membership change
/// between preview and submit cannot silently widen the blast radius.
#[tokio::test]
async fn dispatch_requires_the_confirmed_target_count() {
    let repo = test_repository().await;
    let operation = repo.create_operation("Lab", "scope").await.unwrap();
    sqlx::query("UPDATE operations SET status = 'active' WHERE id = ?")
        .bind(&operation.id)
        .execute(&repo.pool)
        .await
        .unwrap();
    insert_callback(&repo, "cb-1", Some(&operation.id), "host-1").await;
    let actor = create_user(&repo, "operator", Role::Operator).await;
    repo.add_member(&operation.id, &actor.id).await.unwrap();
    let group = repo
        .create_callback_group(&operation.id, "fleet", "", &actor.id, "corr-1")
        .await
        .unwrap();
    repo.add_callback_to_group(&group.id, "cb-1", &actor.id, "corr-2")
        .await
        .unwrap();

    let app = app_with_user_and_repository(repo.clone(), actor).await;
    let page = app
        .clone()
        .oneshot(Request::get("/eventing").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let token = csrf_token(&body_string(page).await);

    // A wrong confirmed count is rejected and nothing is sent.
    let response = app
        .clone()
        .oneshot(post_form(
            "/eventing/dispatch",
            format!(
                "group_id={}&command=nw/process-list&confirmed_count=9&csrf_token={token}",
                group.id
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(count_tasks(&repo, "cb-1").await, 0);

    // A rejected form rotates the CSRF token, so re-read it before retrying.
    let page = app
        .clone()
        .oneshot(Request::get("/eventing").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let token = csrf_token(&body_string(page).await);

    // The matching count dispatches exactly one task.
    let response = app
        .clone()
        .oneshot(post_form(
            "/eventing/dispatch",
            format!(
                "group_id={}&command=nw/process-list&confirmed_count=1&csrf_token={token}",
                group.id
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(count_tasks(&repo, "cb-1").await, 1);
}

/// An empty group must not silently dispatch to nothing and report success.
#[tokio::test]
async fn dispatching_to_an_empty_group_is_refused() {
    let repo = test_repository().await;
    let operation = repo.create_operation("Lab", "scope").await.unwrap();
    sqlx::query("UPDATE operations SET status = 'active' WHERE id = ?")
        .bind(&operation.id)
        .execute(&repo.pool)
        .await
        .unwrap();
    let actor = create_user(&repo, "operator", Role::Operator).await;
    repo.add_member(&operation.id, &actor.id).await.unwrap();
    let group = repo
        .create_callback_group(&operation.id, "empty", "", &actor.id, "corr-1")
        .await
        .unwrap();

    let app = app_with_user_and_repository(repo.clone(), actor).await;
    let page = app
        .clone()
        .oneshot(Request::get("/eventing").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let token = csrf_token(&body_string(page).await);
    let response = app
        .oneshot(post_form(
            "/eventing/dispatch",
            format!(
                "group_id={}&command=nw/process-list&confirmed_count=0&csrf_token={token}",
                group.id
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

/// The preview must name the exact callbacks, so the operator is not guessing
/// at how wide the fan-out is.
#[tokio::test]
async fn the_dispatch_preview_lists_every_target() {
    let repo = test_repository().await;
    let operation = repo.create_operation("Lab", "scope").await.unwrap();
    insert_callback(&repo, "cb-1", Some(&operation.id), "finance-01").await;
    insert_callback(&repo, "cb-2", Some(&operation.id), "finance-02").await;
    insert_callback(&repo, "cb-other", None, "unrelated-host").await;
    let actor = create_user(&repo, "operator", Role::Operator).await;
    repo.add_member(&operation.id, &actor.id).await.unwrap();
    let group = repo
        .create_callback_group(&operation.id, "finance", "", &actor.id, "corr-1")
        .await
        .unwrap();
    for callback in ["cb-1", "cb-2"] {
        repo.add_callback_to_group(&group.id, callback, &actor.id, "corr-2")
            .await
            .unwrap();
    }

    let app = app_with_user_and_repository(repo.clone(), actor).await;
    let response = app
        .clone()
        .oneshot(
            Request::get(format!("/eventing?preview={}", group.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;
    assert!(body.contains("finance-01"));
    assert!(body.contains("finance-02"));
    assert!(!body.contains("unrelated-host"));
    assert!(body.contains("Send to 2 callback(s)"));
    assert!(body.contains("name=\"confirmed_count\" value=\"2\""));
}

/// A viewer must not be able to reach any dispatch route.
#[tokio::test]
async fn a_viewer_cannot_dispatch_or_manage_groups() {
    let repo = test_repository().await;
    let operation = repo.create_operation("Lab", "scope").await.unwrap();
    let actor = create_user(&repo, "viewer", Role::Viewer).await;
    repo.add_member(&operation.id, &actor.id).await.unwrap();
    let app = app_with_user_and_repository(repo.clone(), actor).await;

    for (path, body) in [
        (
            "/eventing/groups",
            format!("operation_id={}&name=x&csrf_token=x", operation.id),
        ),
        (
            "/eventing/dispatch",
            "group_id=x&command=x&confirmed_count=0&csrf_token=x".to_owned(),
        ),
    ] {
        let response = app.clone().oneshot(post_form(path, body)).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{path}");
    }

    let groups: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM callback_groups")
        .fetch_one(&repo.pool)
        .await
        .unwrap();
    assert_eq!(groups, 0);
}

/// Group names are unique per operation and the collision is reported, not
/// swallowed into a 500.
#[tokio::test]
async fn duplicate_group_names_in_one_operation_are_rejected_cleanly() {
    let repo = test_repository().await;
    let operation = repo.create_operation("Lab", "scope").await.unwrap();
    let actor = create_user(&repo, "operator", Role::Operator).await;
    repo.add_member(&operation.id, &actor.id).await.unwrap();
    repo.create_callback_group(&operation.id, "fleet", "", &actor.id, "corr-1")
        .await
        .unwrap();
    assert!(matches!(
        repo.create_callback_group(&operation.id, "fleet", "", &actor.id, "corr-2")
            .await,
        Err(naughtywolf::AppError::Conflict(_))
    ));
}

#[tokio::test]
async fn callbacks_are_returned_in_a_stable_order_for_repeatable_previews() {
    let repo = test_repository().await;
    let operation = repo.create_operation("Lab", "scope").await.unwrap();
    insert_callback(&repo, "cb-b", Some(&operation.id), "host-b").await;
    insert_callback(&repo, "cb-a", Some(&operation.id), "host-a").await;
    let actor = create_user(&repo, "operator", Role::Operator).await;
    repo.add_member(&operation.id, &actor.id).await.unwrap();
    let group = repo
        .create_callback_group(&operation.id, "fleet", "", &actor.id, "corr-1")
        .await
        .unwrap();
    for callback in ["cb-b", "cb-a"] {
        repo.add_callback_to_group(&group.id, callback, &actor.id, "corr-2")
            .await
            .unwrap();
    }
    let first: Vec<Callback> = repo
        .resolve_group_targets(&group.id, &actor.id, false)
        .await
        .unwrap();
    let second = repo
        .resolve_group_targets(&group.id, &actor.id, false)
        .await
        .unwrap();
    assert_eq!(
        first.iter().map(|c| c.id.clone()).collect::<Vec<_>>(),
        second.iter().map(|c| c.id.clone()).collect::<Vec<_>>()
    );
}
