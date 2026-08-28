use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use naughtywolf::{
    auth::{AuthenticatedUser, rbac::Role},
    db::{self, repositories::Repository},
    portal::{DashboardSummary, dashboard_summary, public_router, templates, visible_operations},
};
use tower::ServiceExt;

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
