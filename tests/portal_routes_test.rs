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
    db::{self, repositories::Repository},
    portal::{DashboardSummary, dashboard_summary, public_router, templates, visible_operations},
};
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

async fn authenticated_app(repository: Repository) -> Router {
    Router::<Repository>::new()
        .merge(naughtywolf::portal::authenticated_router())
        .with_state(repository)
        .merge(public_router())
        .layer(SessionManagerLayer::new(MemoryStore::default()).with_secure(false))
}

async fn test_login(session: Session, Extension(user): Extension<AuthenticatedUser>) -> StatusCode {
    AuthSession { session }.login(&user).await.unwrap();
    StatusCode::NO_CONTENT
}

async fn app_with_logged_in_user(
    role: Role,
) -> impl tower::Service<Request<Body>, Response = Response, Error = std::convert::Infallible> + Clone
{
    let repository = test_repository().await;
    let user = create_user(&repository, "logged-in", role).await;
    let app = Router::<Repository>::new()
        .route("/test/login", post(test_login))
        .merge(naughtywolf::portal::authenticated_router())
        .with_state(repository)
        .merge(public_router())
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

#[tokio::test]
async fn public_pages_link_local_styles_and_do_not_expose_operator_console_copy() {
    let app = public_router();
    let response = app
        .oneshot(Request::get("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let body = String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();

    assert!(body.contains("/static/admin.css"));
    assert!(!body.to_lowercase().contains("sliver"));
    assert!(!body.to_lowercase().contains("payload"));
}

#[tokio::test]
async fn login_page_has_labeled_credentials_and_local_styles() {
    let response = public_router()
        .oneshot(Request::get("/login").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let body = String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();

    assert!(body.contains("<label"));
    assert!(body.contains("autocomplete=\"username\""));
    assert!(body.contains("/static/admin.css"));
}

#[test]
fn invalid_login_template_is_generic_and_never_echoes_a_username() {
    let body = templates::login_page(Some("Invalid username or password"), "csrf-token");

    assert!(body.contains("Invalid username or password"));
    assert!(!body.contains("missing-user"));
}

#[tokio::test]
async fn operator_summary_excludes_an_operation_without_membership() {
    let repo = test_repository().await;
    let allowed = repo.create_operation("Allowed", "lab").await.unwrap();
    let hidden = repo.create_operation("Hidden", "lab").await.unwrap();
    let operator = create_user(&repo, "op", Role::Operator).await;
    repo.add_member(&allowed.id, &operator.id).await.unwrap();

    let operations = visible_operations(&repo, &operator).await.unwrap();

    assert_eq!(
        operations.iter().map(|item| &item.id).collect::<Vec<_>>(),
        vec![&allowed.id]
    );
    assert!(!operations.iter().any(|item| item.id == hidden.id));
}

#[tokio::test]
async fn empty_database_produces_zero_dashboard_summary() {
    let repo = test_repository().await;
    let viewer = create_user(&repo, "viewer", Role::Viewer).await;

    assert_eq!(
        dashboard_summary(&repo, &viewer).await.unwrap(),
        DashboardSummary::default()
    );
}

#[tokio::test]
async fn anonymous_dashboard_request_is_rejected() {
    let response = authenticated_app(test_repository().await)
        .await
        .oneshot(Request::get("/dashboard").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn viewer_cannot_open_admin_page_even_if_they_request_its_url() {
    let app = app_with_logged_in_user(Role::Viewer).await;
    let response = app
        .oneshot(Request::get("/admin").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}
