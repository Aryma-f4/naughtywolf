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
    app_with_user_and_repository(repository, user).await
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

#[tokio::test]
async fn viewer_cannot_submit_an_operation_form() {
    let app = app_with_logged_in_user(Role::Viewer).await;
    let response = app
        .oneshot(post_form("/operations", "name=Lab&purpose=Practice"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn operator_cannot_open_or_submit_an_operation_form() {
    let app = app_with_logged_in_user(Role::Operator).await;
    let open = app
        .clone()
        .oneshot(Request::get("/operations/new").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let submit = app
        .oneshot(post_form("/operations", "name=Lab&purpose=Practice"))
        .await
        .unwrap();

    assert_eq!(open.status(), StatusCode::FORBIDDEN);
    assert_eq!(submit.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn operation_form_is_labeled_csrf_protected_and_trims_values() {
    let app = app_with_logged_in_user(Role::Admin).await;
    let form_response = app
        .clone()
        .oneshot(Request::get("/operations/new").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(form_response.status(), StatusCode::OK);
    let form_body = String::from_utf8(
        to_bytes(form_response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(form_body.contains("<label for=\"operation-name\""));
    assert!(form_body.contains("<label for=\"operation-purpose\""));
    assert!(form_body.contains("method=\"post\" action=\"/operations\""));
    let token = csrf_token(&form_body);

    let submit = app
        .clone()
        .oneshot(post_form(
            "/operations",
            format!("name=%20Lab%20&purpose=%20Practice%20&csrf_token={token}"),
        ))
        .await
        .unwrap();
    assert_eq!(submit.status(), StatusCode::SEE_OTHER);
    assert_eq!(submit.headers().get("location").unwrap(), "/operations");

    let list = app
        .oneshot(Request::get("/operations").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let list_body = String::from_utf8(
        to_bytes(list.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(list_body.contains("<h2>Lab</h2>"));
    assert!(list_body.contains("<p>Practice</p>"));
}

#[tokio::test]
async fn invalid_operation_form_rerenders_with_a_generic_accessible_error() {
    let app = app_with_logged_in_user(Role::Admin).await;
    let form_response = app
        .clone()
        .oneshot(Request::get("/operations/new").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let form_body = String::from_utf8(
        to_bytes(form_response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    let token = csrf_token(&form_body);

    let response = app
        .oneshot(post_form(
            "/operations",
            format!("name=%20%20&purpose=Practice&csrf_token={token}"),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(body.contains("The request is invalid."));
    assert!(body.contains("role=\"alert\" id=\"operation-form-error\""));
    assert!(body.contains("aria-describedby=\"operation-form-error\""));
    assert!(!body.contains("name is required"));
}

#[tokio::test]
async fn operation_form_rejects_a_missing_csrf_token() {
    let app = app_with_logged_in_user(Role::Admin).await;
    let response = app
        .oneshot(post_form("/operations", "name=Lab&purpose=Practice"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn operation_form_rerenders_when_a_required_field_is_missing() {
    let app = app_with_logged_in_user(Role::Admin).await;
    let form_response = app
        .clone()
        .oneshot(Request::get("/operations/new").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let form_body = String::from_utf8(
        to_bytes(form_response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    let token = csrf_token(&form_body);

    let response = app
        .oneshot(post_form(
            "/operations",
            format!("purpose=Practice&csrf_token={token}"),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(body.contains("The request is invalid."));
    assert!(body.contains("<form class=\"record-form\""));
}

#[tokio::test]
async fn viewer_never_sees_mutation_links() {
    let app = app_with_logged_in_user(Role::Viewer).await;
    let response = app
        .oneshot(Request::get("/operations").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let body = String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();

    assert!(!body.contains("/operations/new"));
    assert!(!body.contains("/assets/new"));
}

#[tokio::test]
async fn operator_cannot_add_an_asset_outside_their_operation_scope() {
    let repository = test_repository().await;
    let operation = repository
        .create_operation("Lab", "Practice")
        .await
        .unwrap();
    let operator = create_user(&repository, "operator", Role::Operator).await;
    let app = app_with_user_and_repository(repository, operator).await;

    let open = app
        .clone()
        .oneshot(
            Request::get(format!("/operations/{}/assets/new", operation.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let submit = app
        .oneshot(post_form(
            &format!("/operations/{}/assets", operation.id),
            "name=web-01&kind=web&owner=Lab&address=127.0.0.1",
        ))
        .await
        .unwrap();

    assert_eq!(open.status(), StatusCode::FORBIDDEN);
    assert_eq!(submit.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn authorized_operator_can_submit_the_labeled_asset_form() {
    let repository = test_repository().await;
    let operation = repository
        .create_operation("Lab", "Practice")
        .await
        .unwrap();
    let operator = create_user(&repository, "operator", Role::Operator).await;
    repository
        .add_member(&operation.id, &operator.id)
        .await
        .unwrap();
    let app = app_with_user_and_repository(repository.clone(), operator).await;
    let form_response = app
        .clone()
        .oneshot(
            Request::get(format!("/operations/{}/assets/new", operation.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(form_response.status(), StatusCode::OK);
    let form_body = String::from_utf8(
        to_bytes(form_response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    for field in ["asset-name", "asset-kind", "asset-owner", "asset-address"] {
        assert!(form_body.contains(&format!("<label for=\"{field}\"")));
    }
    let token = csrf_token(&form_body);

    let submit = app
        .oneshot(post_form(
            &format!("/operations/{}/assets", operation.id),
            format!(
                "name=%20web-01%20&kind=%20web%20&owner=%20Lab%20&address=%20127.0.0.1%20&csrf_token={token}"
            ),
        ))
        .await
        .unwrap();
    assert_eq!(submit.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        submit.headers().get("location").unwrap(),
        format!("/operations/{}", operation.id).as_str()
    );
    let assets = repository.list_assets(&operation.id).await.unwrap();
    assert_eq!(assets.len(), 1);
    assert_eq!(assets[0].name, "web-01");
    assert_eq!(assets[0].kind, "web");
    assert_eq!(assets[0].owner, "Lab");
    assert_eq!(assets[0].address, "127.0.0.1");
    assert_eq!(repository.count_audit_events().await.unwrap(), 1);
}

#[tokio::test]
async fn asset_form_rejects_each_value_longer_than_160_characters() {
    for field in ["name", "kind", "owner", "address"] {
        let repository = test_repository().await;
        let operation = repository
            .create_operation("Lab", "Practice")
            .await
            .unwrap();
        let operator = create_user(&repository, "operator", Role::Operator).await;
        repository
            .add_member(&operation.id, &operator.id)
            .await
            .unwrap();
        let app = app_with_user_and_repository(repository.clone(), operator).await;
        let form_response = app
            .clone()
            .oneshot(
                Request::get(format!("/operations/{}/assets/new", operation.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let form_body = String::from_utf8(
            to_bytes(form_response.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        let token = csrf_token(&form_body);
        let mut values = [
            ("name", "web-01".to_owned()),
            ("kind", "web".to_owned()),
            ("owner", "Lab".to_owned()),
            ("address", "127.0.0.1".to_owned()),
        ];
        values
            .iter_mut()
            .find(|(name, _)| *name == field)
            .unwrap()
            .1 = "x".repeat(161);
        let body = format!(
            "name={}&kind={}&owner={}&address={}&csrf_token={token}",
            values[0].1, values[1].1, values[2].1, values[3].1
        );

        let response = app
            .oneshot(post_form(
                &format!("/operations/{}/assets", operation.id),
                body,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "field {field}");
        assert!(
            repository
                .list_assets(&operation.id)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(repository.count_audit_events().await.unwrap(), 0);
    }
}
