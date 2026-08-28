use axum::{
    Router,
    extract::{Form, State},
    http::{Request, StatusCode},
    routing::{get, post},
};
use naughtywolf::{
    auth::{
        authenticate,
        middleware::{AuthSession, AuthenticatedUserGuard},
        password::{hash_password, verify_password},
        rbac::Role,
    },
    db::{self, repositories::Repository},
};
use serde::Deserialize;
use tower::ServiceExt;
use tower_sessions::{Session, SessionManagerLayer};
use tower_sessions_sqlx_store::SqliteStore;

#[derive(Deserialize)]
struct LoginForm {
    username: String,
    password: String,
}

async fn login_handler(
    State(repository): State<Repository>,
    session: Session,
    Form(form): Form<LoginForm>,
) -> StatusCode {
    match authenticate(&repository, &form.username, &form.password).await {
        Ok(Some(user)) => match (AuthSession { session }).login(&user).await {
            Ok(()) => StatusCode::NO_CONTENT,
            Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
        },
        Ok(None) => StatusCode::UNAUTHORIZED,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

async fn protected_handler(_: AuthenticatedUserGuard) -> StatusCode {
    StatusCode::NO_CONTENT
}

async fn test_app_with_user(disabled: bool) -> (Router, sqlx::SqlitePool) {
    let pool = db::create_pool("sqlite::memory:").await.unwrap();
    db::run_migrations(&pool).await.unwrap();
    let password_hash = hash_password("secret").unwrap();
    sqlx::query(
        "INSERT INTO users (id, username, password_hash, role, disabled) VALUES (?, ?, ?, ?, ?)",
    )
    .bind("test-user")
    .bind("test-user")
    .bind(password_hash)
    .bind("operator")
    .bind(disabled)
    .execute(&pool)
    .await
    .unwrap();

    let session_store = SqliteStore::new(pool.clone());
    session_store.migrate().await.unwrap();
    let session_layer = SessionManagerLayer::new(session_store).with_secure(false);

    let app = Router::new()
        .route("/login", post(login_handler))
        .route("/protected", get(protected_handler))
        .with_state(Repository { pool: pool.clone() })
        .layer(session_layer);

    (app, pool)
}

async fn login(app: Router, username: &str, password: &str) -> axum::response::Response {
    app.oneshot(
        Request::builder()
            .method("POST")
            .uri("/login")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(axum::body::Body::from(format!(
                "username={username}&password={password}"
            )))
            .unwrap(),
    )
    .await
    .unwrap()
}

async fn protected(app: Router, cookie: &str) -> axum::response::Response {
    app.oneshot(
        Request::builder()
            .method("GET")
            .uri("/protected")
            .header("cookie", cookie)
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await
    .unwrap()
}

#[test]
fn operator_cannot_manage_users() {
    assert!(!Role::Operator.allows(Role::Admin));
}

#[test]
fn hashed_password_verifies_only_the_original_secret() {
    let hash = hash_password("correct horse battery staple").unwrap();
    assert!(verify_password("correct horse battery staple", &hash).unwrap());
    assert!(!verify_password("wrong", &hash).unwrap());
}

#[tokio::test]
async fn disabled_user_cannot_start_a_session() {
    let (app, _) = test_app_with_user(true).await;
    assert_eq!(
        login(app, "test-user", "secret").await.status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn user_disabled_after_login_cannot_use_an_existing_session() {
    let (app, pool) = test_app_with_user(false).await;
    let login_response = login(app.clone(), "test-user", "secret").await;
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

    sqlx::query("UPDATE users SET disabled = 1 WHERE id = ?")
        .bind("test-user")
        .execute(&pool)
        .await
        .unwrap();

    assert_eq!(
        protected(app, &cookie).await.status(),
        StatusCode::UNAUTHORIZED
    );
}
