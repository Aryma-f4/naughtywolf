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
    db::{
        self,
        models::{C2TaskWithResult, Callback, CallbackStatus, EventRule, RunState},
        repositories::Repository,
    },
    evidence::EvidenceStore,
    payload::PayloadMeta,
    portal::{DashboardSummary, dashboard_summary, public_router, templates, visible_operations},
};
use tower::ServiceExt;
use tower_sessions::{MemoryStore, Session, SessionManagerLayer};

struct PayloadArtifactCleanup {
    paths: Vec<std::path::PathBuf>,
}

impl Drop for PayloadArtifactCleanup {
    fn drop(&mut self) {
        for path in &self.paths {
            let _ = std::fs::remove_file(path);
        }
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

#[tokio::test]
async fn public_payload_download_requires_the_matching_uuid_not_a_login() {
    let token = uuid::Uuid::new_v4().to_string();
    let wrong_token = "550e8400-e29b-41d4-a716-446655440001";
    let file = format!("public-download-route-test-{token}.bin");
    let dir = std::path::Path::new("payloads");
    std::fs::create_dir_all(dir).unwrap();
    let artifact = dir.join(&file);
    let sidecar = dir.join(format!("{file}.json"));
    let _cleanup = PayloadArtifactCleanup {
        paths: vec![artifact.clone(), sidecar.clone()],
    };
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&artifact)
        .and_then(|mut output| std::io::Write::write_all(&mut output, b"public-payload-bytes"))
        .unwrap();
    std::fs::write(
        &sidecar,
        serde_json::to_vec(&serde_json::json!({
            "file": file,
            "name": "public-download-route-test",
            "os": "linux",
            "arch": "amd64",
            "protocol": "https",
            "lhost": "gateofbabylon.space",
            "lport": 443,
            "psk": "test-only",
            "interval_ms": 5000,
            "jitter_ms": 1000,
            "target": "x86_64-unknown-linux-musl",
            "size": 20,
            "built_at": "2026-09-10T00:00:00Z",
            "public_id": token
        }))
        .unwrap(),
    )
    .unwrap();

    let response = public_router()
        .oneshot(
            Request::get(format!("/payloads/download/{token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    assert!(
        response.headers()["content-disposition"]
            .to_str()
            .unwrap()
            .contains(&file)
    );
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX).await.unwrap(),
        "public-payload-bytes"
    );

    for unavailable in [&file, wrong_token] {
        let response = public_router()
            .oneshot(
                Request::get(format!("/payloads/download/{unavailable}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
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

async fn app_with_user_repository_and_store(
    repository: Repository,
    user: AuthenticatedUser,
    evidence_store: EvidenceStore,
) -> impl tower::Service<Request<Body>, Response = Response, Error = std::convert::Infallible> + Clone
{
    let app = Router::<Repository>::new()
        .route("/test/login", post(test_login))
        .merge(naughtywolf::portal::authenticated_router())
        .with_state(repository)
        .merge(public_router())
        .layer(Extension(evidence_store))
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

struct ScopedPortalFixture {
    repository: Repository,
    viewer: AuthenticatedUser,
    allowed_operation_id: String,
    hidden_operation_id: String,
    allowed_evidence_id: String,
    hidden_evidence_id: String,
}

async fn scoped_portal_fixture() -> ScopedPortalFixture {
    let repository = test_repository().await;
    let viewer = create_user(&repository, "viewer", Role::Viewer).await;
    let allowed = repository
        .create_operation("Allowed operation", "Visible portal records")
        .await
        .unwrap();
    let hidden = repository
        .create_operation("Hidden operation", "Out-of-scope portal records")
        .await
        .unwrap();
    let first_allowed_asset = repository
        .create_asset(&allowed.id, "allowed-one", "host", "lab", "10.0.0.1")
        .await
        .unwrap();
    repository
        .create_asset(&allowed.id, "allowed-two", "host", "lab", "10.0.0.2")
        .await
        .unwrap();
    let hidden_asset = repository
        .create_asset(&hidden.id, "hidden", "host", "lab", "10.0.0.3")
        .await
        .unwrap();
    repository
        .add_member(&allowed.id, &viewer.id)
        .await
        .unwrap();
    repository
        .ensure_builtin_check("allowed-check", "Allowed check", "viewer", 30, 4_096)
        .await
        .unwrap();
    repository
        .ensure_builtin_check("hidden-check", "Hidden check", "viewer", 30, 4_096)
        .await
        .unwrap();
    let queued_run = repository
        .create_run(
            "allowed-check",
            &first_allowed_asset.id,
            &allowed.id,
            Some(&viewer.id),
            &serde_json::json!({}),
        )
        .await
        .unwrap();
    let succeeded_run = repository
        .create_run(
            "allowed-check",
            &first_allowed_asset.id,
            &allowed.id,
            Some(&viewer.id),
            &serde_json::json!({}),
        )
        .await
        .unwrap();
    repository
        .finish_run(
            &succeeded_run.id,
            RunState::Succeeded,
            Some(&serde_json::json!({"finding": "recorded"})),
            None,
            false,
        )
        .await
        .unwrap();
    let hidden_run = repository
        .create_run(
            "hidden-check",
            &hidden_asset.id,
            &hidden.id,
            Some(&viewer.id),
            &serde_json::json!({}),
        )
        .await
        .unwrap();
    let allowed_evidence = repository
        .create_evidence(
            &queued_run.id,
            &format!("{}/allowed.json", queued_run.id),
            "application/json",
            17,
            "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
        )
        .await
        .unwrap();
    let hidden_evidence = repository
        .create_evidence(
            &hidden_run.id,
            &format!("{}/hidden.json", hidden_run.id),
            "text/plain",
            99,
            "9876543210abcdef9876543210abcdef9876543210abcdef9876543210abcdef",
        )
        .await
        .unwrap();
    for (id, operation_id, action) in [
        ("allowed-audit", &allowed.id, "allowed.reviewed"),
        ("hidden-audit", &hidden.id, "hidden.reviewed"),
    ] {
        sqlx::query(
            "INSERT INTO audit_events \
             (id, actor_id, operation_id, action, target_type, target_id, outcome, correlation_id) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(&viewer.id)
        .bind(operation_id)
        .bind(action)
        .bind("operation")
        .bind(operation_id)
        .bind("success")
        .bind(format!("{id}-correlation"))
        .execute(&repository.pool)
        .await
        .unwrap();
    }

    ScopedPortalFixture {
        repository,
        viewer,
        allowed_operation_id: allowed.id,
        hidden_operation_id: hidden.id,
        allowed_evidence_id: allowed_evidence.id,
        hidden_evidence_id: hidden_evidence.id,
    }
}

// ── Task 1: Login template contract ──────────────────────────────────────────

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

#[test]
fn login_template_uses_the_operator_layout_without_external_assets() {
    let body = templates::login_page(Some("Invalid username or password"), "csrf-token");

    for class_name in [
        "login-shell",
        "login-brand-panel",
        "login-form-panel",
        "login-card",
        "brand-mark",
        "trust-list",
        "form-field",
    ] {
        assert!(body.contains(class_name), "missing {class_name}");
    }
    assert!(body.contains("Authorized lab access only"));
    assert!(body.contains("name=\"csrf_token\" value=\"csrf-token\""));
    assert!(body.contains("autocomplete=\"username\""));
    assert!(body.contains("autocomplete=\"current-password\""));
    assert!(body.contains("role=\"alert\""));
    assert!(!body.contains("https://"));
}

// ── Task 2: Authenticated shell contract ─────────────────────────────────────

#[test]
fn authenticated_shell_uses_top_navigation_and_post_logout() {
    let user = AuthenticatedUser {
        id: "operator-id".into(),
        username: "operator-user".into(),
        role: Role::Operator,
    };
    let body = templates::app_page("Dashboard", &user, "dashboard", "<p>body</p>");

    assert!(body.contains("class=\"portal-shell\""));
    assert!(body.contains("class=\"portal-topbar\""));
    assert!(body.contains("class=\"primary-nav\""));
    assert!(body.contains("aria-label=\"Primary navigation\""));
    assert!(body.contains("href=\"/dashboard\" aria-current=\"page\""));
    assert!(body.contains("method=\"post\" action=\"/logout\""));
    assert!(body.contains("operator-user"));
    assert!(!body.contains("sidebar"));
}

#[test]
fn admin_navigation_remains_role_scoped() {
    let viewer = AuthenticatedUser {
        id: "viewer-id".into(),
        username: "viewer-user".into(),
        role: Role::Viewer,
    };
    let admin = AuthenticatedUser {
        id: "admin-id".into(),
        username: "admin-user".into(),
        role: Role::Admin,
    };

    assert!(
        !templates::app_page("Dashboard", &viewer, "dashboard", "")
            .contains("href=\"/admin/users\"")
    );
    assert!(
        templates::app_page("Admin", &admin, "admin", "")
            .contains("href=\"/admin/users\" aria-current=\"page\"")
    );
}

#[test]
fn payloads_navigation_remains_role_scoped() {
    let viewer = AuthenticatedUser {
        id: "viewer-id".into(),
        username: "viewer-user".into(),
        role: Role::Viewer,
    };
    let operator = AuthenticatedUser {
        id: "op-id".into(),
        username: "op-user".into(),
        role: Role::Operator,
    };

    assert!(
        !templates::app_page("Payloads", &viewer, "payloads", "").contains("href=\"/payloads\"")
    );
    let operator_page = templates::app_page("Payloads", &operator, "payloads", "");
    assert!(operator_page.contains("href=\"/payloads\""));
    assert!(operator_page.contains(">Create Payload</span>"));
    assert!(operator_page.contains("nav-link-featured"));
}

#[test]
fn new_features_navigation_remains_role_scoped() {
    let viewer = AuthenticatedUser {
        id: "viewer-id".into(),
        username: "viewer-user".into(),
        role: Role::Viewer,
    };
    let operator = AuthenticatedUser {
        id: "op-id".into(),
        username: "op-user".into(),
        role: Role::Operator,
    };

    for (href, active) in [
        ("/callbacks", "callbacks"),
        ("/eventing", "eventing"),
        ("/services", "services"),
        ("/search", "search"),
    ] {
        let viewer_body = templates::app_page("Nav", &viewer, active, "");
        assert!(
            !viewer_body.contains(&format!("href=\"{href}\"")),
            "{href} visible to viewer"
        );
        let operator_body = templates::app_page("Nav", &operator, active, "");
        assert!(
            operator_body.contains(&format!("href=\"{href}\"")),
            "{href} hidden from operator"
        );
    }
}

#[test]
fn eventing_page_renders_rule_form() {
    let user = AuthenticatedUser {
        id: "op-id".into(),
        username: "op-user".into(),
        role: Role::Operator,
    };
    let rules = vec![EventRule {
        id: "r1".into(),
        name: "quarantine".into(),
        trigger: "new callback".into(),
        command: "collect state".into(),
        target: "all".into(),
        enabled: true,
        requested_by: None,
        created_at: "2026-08-30T00:00:00Z".into(),
    }];
    let body = templates::eventing_page(&user, &rules, "csrf-token", None);

    assert!(body.contains("action=\"/eventing\""));
    assert!(body.contains("quarantine"));
    assert!(body.contains("Enabled"));
}

#[test]
fn payloads_page_renders_build_form_and_built_rows() {
    let user = AuthenticatedUser {
        id: "op-id".into(),
        username: "op-user".into(),
        role: Role::Operator,
    };
    let metas = vec![PayloadMeta {
        file: "linux-amd64.bin".into(),
        name: "linux-implant".into(),
        os: "linux".into(),
        arch: "amd64".into(),
        protocol: "http".into(),
        lhost: "10.0.0.1".into(),
        lport: 8081,
        interval_ms: 1000,
        jitter_ms: 200,
        target: String::new(),
        public_id: "550e8400-e29b-41d4-a716-446655440000".into(),
        size: 42,
        built_at: "2026-08-30T00:00:00Z".into(),
    }];
    let body = templates::payloads_page(&user, &metas, "csrf-token", None, None, &[], &[]);

    assert!(body.contains("action=\"/payloads/generate\""));
    assert!(body.contains("name=\"csrf_token\" value=\"csrf-token\""));
    assert!(body.contains("linux-implant"));
    assert!(body.contains("10.0.0.1:8081"));
    assert!(body.contains("href=\"/payloads/download/550e8400-e29b-41d4-a716-446655440000\""));
    assert!(!body.contains("href=\"/payloads/download/linux-amd64.bin\""));
    assert!(body.contains("NW_PSK"));
    assert!(body.contains("active server secret automatically"));
    assert!(!body.contains("name=\"psk\""));
    assert!(!body.contains("payload-psk-random"));
    assert!(body.contains("data-payload-wizard"));
    assert!(body.contains("aria-label=\"Payload creation progress\""));
    assert_eq!(body.matches("data-payload-step=").count(), 4);
    assert!(body.contains("data-payload-review"));
    assert!(body.contains("data-gsocket-fields"));
    assert!(body.contains("name=\"gsocket_secret\""));
    assert!(body.contains("name=\"gsocket_local_port\""));
    assert!(!body.contains("relay-secret-from-test"));
    assert!(body.contains("A DNS name such as"));
    assert!(body.contains("c2.lab.example"));
    assert!(body.contains("or an IPv4 or IPv6 address"));
    assert!(body.contains("Choose a port allowed by your lab firewall"));
    assert!(body.contains("Keep jitter below the base interval"));
}

#[test]
fn operator_guide_walks_through_the_complete_workflow() {
    let user = AuthenticatedUser {
        id: "op-id".into(),
        username: "op-user".into(),
        role: Role::Operator,
    };
    let body = templates::guide_page(&user);

    assert!(body.contains("data-guide-page"));
    assert!(body.contains("1. Define scope"));
    assert!(body.contains("2. Add authorized assets"));
    assert!(body.contains("3. Run reconnaissance"));
    assert!(body.contains("4. Build a payload"));
    assert!(body.contains("5. Interact with callbacks"));
    assert!(body.contains("6. Preserve evidence"));
    assert!(body.contains("href=\"/payloads\""));
    assert!(body.contains("href=\"/callbacks\""));
}

#[test]
fn callbacks_page_presents_active_session_workspace() {
    let user = AuthenticatedUser {
        id: "op-id".into(),
        username: "op-user".into(),
        role: Role::Operator,
    };
    let callbacks = vec![Callback {
        id: "callback-1".into(),
        asset_id: Some("asset-1".into()),
        operation_id: Some("operation-1".into()),
        host: "LAB-WS-01".into(),
        user_name: "analyst".into(),
        process: "nw-implant".into(),
        arch: "amd64".into(),
        os: "linux".into(),
        protocol: "http".into(),
        status: CallbackStatus::Active,
        last_seen: "2026-09-09T10:00:00Z".into(),
        created_at: "2026-09-09T09:00:00Z".into(),
    }];

    let body = templates::callbacks_page(&user, &callbacks);

    assert!(body.contains("data-callback-workspace"));
    assert!(body.contains("data-callback-summary"));
    assert!(body.contains("class=\"callback-row"));
    assert!(body.contains("href=\"/payloads\""));
    assert!(body.contains("Active Callbacks"));
}

#[test]
fn callback_detail_separates_session_history_output_and_command_dock() {
    let user = AuthenticatedUser {
        id: "op-id".into(),
        username: "op-user".into(),
        role: Role::Operator,
    };
    let callback = Callback {
        id: "callback-1".into(),
        asset_id: Some("asset-1".into()),
        operation_id: Some("operation-1".into()),
        host: "LAB-WS-01".into(),
        user_name: "analyst".into(),
        process: "nw-implant".into(),
        arch: "amd64".into(),
        os: "linux".into(),
        protocol: "http".into(),
        status: CallbackStatus::Active,
        last_seen: "2026-09-09T10:00:00Z".into(),
        created_at: "2026-09-09T09:00:00Z".into(),
    };

    let body = templates::callback_detail_page(&user, &callback, &[], "csrf-token");

    assert!(body.contains("callback-console-grid"));
    assert!(body.contains("command-dock"));
    assert!(body.contains("data-live-output"));
    assert!(body.contains("Session context"));
    assert!(body.contains("name=\"csrf_token\" value=\"csrf-token\""));
    assert!(body.contains("data-module-studio"));
    assert!(body.contains("data-module-command=\"nw/user-enum\""));
    assert!(body.contains("data-module-command=\"nw/peas-audit\""));
    assert!(body.contains("data-custom-code-form"));
    assert!(body.contains("Automatic exploitation stays disabled"));
}

#[test]
fn callback_detail_rehydrates_completed_output_and_command_history() {
    use base64::Engine;

    let user = AuthenticatedUser {
        id: "op-id".into(),
        username: "op-user".into(),
        role: Role::Operator,
    };
    let callback = Callback {
        id: "callback-1".into(),
        asset_id: None,
        operation_id: None,
        host: "LAB-WS-01".into(),
        user_name: "analyst".into(),
        process: "nw-implant".into(),
        arch: "amd64".into(),
        os: "linux".into(),
        protocol: "http".into(),
        status: CallbackStatus::Active,
        last_seen: "2026-09-10T05:22:24Z".into(),
        created_at: "2026-09-10T05:20:00Z".into(),
    };
    let tasks = vec![C2TaskWithResult {
        id: "task-persisted-1".into(),
        session_id: callback.id.clone(),
        command: "ls".into(),
        args_json: serde_json::json!([]),
        status: "completed".into(),
        created_at: "2026-09-10T05:22:22Z".into(),
        processing_at: Some("2026-09-10T05:22:23Z".into()),
        completed_at: Some("2026-09-10T05:22:24Z".into()),
        result_output: Some(base64::engine::general_purpose::STANDARD.encode(b"one\ntwo\n")),
        result_ok: Some(true),
        result_exit_code: Some(0),
    }];

    let body = templates::callback_detail_page(&user, &callback, &tasks, "csrf-token");

    assert!(body.contains("data-task-id=\"task-persisted-1\""));
    assert!(body.contains("data-task-command=\"ls\""));
    assert!(body.contains("one\ntwo\n"));
    assert!(!body.contains("No output yet"));
}

// ── Task 3: Component contracts ───────────────────────────────────────────────

#[test]
fn dashboard_renders_five_real_metric_cards() {
    let user = AuthenticatedUser {
        id: "viewer-id".into(),
        username: "viewer-user".into(),
        role: Role::Viewer,
    };
    let summary = DashboardSummary {
        operation_count: 1,
        asset_count: 2,
        run_count: 3,
        evidence_count: 4,
        audit_count: 5,
    };
    let body = templates::dashboard_page(&user, &summary);

    assert_eq!(body.matches("class=\"metric-card\"").count(), 5);
    for value in ["1", "2", "3", "4", "5"] {
        assert!(body.contains(&format!(">{value}</strong>")));
    }
    assert!(!body.contains("chart"));
}

// ── Task 4: Accessibility contract ───────────────────────────────────────────

#[test]
fn form_pages_keep_labels_errors_and_descriptions() {
    let user = AuthenticatedUser {
        id: "admin-id".into(),
        username: "admin-user".into(),
        role: Role::Admin,
    };
    let body = templates::operation_form_page(
        &user,
        "csrf-token",
        Some("The request is invalid."),
        "Retained name",
        "Retained purpose",
    );

    assert!(body.contains("role=\"alert\""));
    assert!(body.contains("aria-describedby=\"operation-form-error\""));
    assert!(body.contains("value=\"Retained name\""));
    assert!(body.contains("value=\"Retained purpose\""));
    assert_eq!(body.matches("class=\"form-field\"").count(), 2);
}

// ── Existing tests (unchanged) ────────────────────────────────────────────────

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
async fn module_studio_script_is_served_as_javascript() {
    let response = public_router()
        .oneshot(
            Request::get("/static/module_studio.js")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "application/javascript; charset=utf-8"
    );
}

#[tokio::test]
async fn signed_in_viewer_can_open_operator_guide() {
    let response = app_with_logged_in_user(Role::Viewer)
        .await
        .oneshot(Request::get("/guide").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(body_string(response).await.contains("data-guide-page"));
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
async fn payloads_require_a_non_viewer_role() {
    let viewer = app_with_logged_in_user(Role::Viewer).await;
    let view_response = viewer
        .oneshot(Request::get("/payloads").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(view_response.status(), StatusCode::FORBIDDEN);

    let operator = app_with_logged_in_user(Role::Operator).await;
    let op_response = operator
        .oneshot(Request::get("/payloads").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(op_response.status(), StatusCode::OK);
}

#[tokio::test]
async fn payload_feature_routes_require_a_non_viewer_role() {
    let viewer = app_with_logged_in_user(Role::Viewer).await;
    for path in ["/callbacks", "/eventing", "/services", "/search"] {
        let response = viewer
            .clone()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "{path} open to viewer"
        );
    }

    let operator = app_with_logged_in_user(Role::Operator).await;
    for path in ["/callbacks", "/eventing", "/services", "/search"] {
        let op_response = operator
            .clone()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(
            op_response.status(),
            StatusCode::OK,
            "{path} not open to operator"
        );
    }
}

#[tokio::test]
async fn operator_can_create_an_event_rule_with_audit() {
    let repository = test_repository().await;
    let operator = create_user(&repository, "op", Role::Operator).await;
    let app = app_with_user_and_repository(repository.clone(), operator).await;
    let page = app
        .clone()
        .oneshot(Request::get("/eventing").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let token = csrf_token(&body_string(page).await);

    let response = app
        .oneshot(post_form(
            "/eventing",
            format!(
                "name=quarantine&trigger=new+callback&command=collect+state&target=all&csrf_token={token}"
            ),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers().get("location").unwrap(), "/eventing");
    let rule: String = sqlx::query_scalar("SELECT name FROM event_rules WHERE name = ?")
        .bind("quarantine")
        .fetch_one(&repository.pool)
        .await
        .unwrap();
    assert_eq!(rule, "quarantine");
    let action: String =
        sqlx::query_scalar("SELECT action FROM audit_events WHERE target_type = ?")
            .bind("event_rule")
            .fetch_one(&repository.pool)
            .await
            .unwrap();
    assert_eq!(action, "event_rule.created");
}

#[tokio::test]
async fn operator_cannot_change_another_users_role() {
    let app = app_with_logged_in_user(Role::Operator).await;
    let response = app
        .oneshot(post_form("/admin/users/target/role", "role=viewer"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn admin_user_table_never_renders_password_hashes() {
    let repository = test_repository().await;
    let admin = create_user(&repository, "admin", Role::Admin).await;
    sqlx::query("INSERT INTO users (id, username, password_hash, role) VALUES (?, ?, ?, ?)")
        .bind("target")
        .bind("target-user")
        .bind("secret-password-hash")
        .bind("viewer")
        .execute(&repository.pool)
        .await
        .unwrap();
    let app = app_with_user_and_repository(repository, admin).await;

    let response = app
        .oneshot(Request::get("/admin/users").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();

    assert!(body.contains("target-user"));
    assert!(!body.contains("password_hash"));
    assert!(!body.contains("secret-password-hash"));
    assert!(body.contains("class=\"data-table admin-users-table\""));
    assert!(body.contains("class=\"status-pill"));
    assert!(body.contains("Enabled") || body.contains("Disabled"));
}

#[tokio::test]
async fn viewer_cannot_download_evidence_outside_their_operation_scope() {
    let repository = test_repository().await;
    let operation = repository
        .create_operation("Hidden", "Scoped download fixture")
        .await
        .unwrap();
    let asset = repository
        .create_asset(&operation.id, "hidden-host", "host", "lab", "10.0.0.2")
        .await
        .unwrap();
    repository
        .ensure_builtin_check("download-test", "Download test", "viewer", 30, 4_096)
        .await
        .unwrap();
    let run = repository
        .create_run(
            "download-test",
            &asset.id,
            &operation.id,
            None,
            &serde_json::json!({}),
        )
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO evidence (id, check_run_id, storage_path, content_type, byte_len, sha256) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind("not-visible")
    .bind(&run.id)
    .bind(format!("{}/hidden.json", run.id))
    .bind("application/json")
    .bind(0_i64)
    .bind("hidden")
    .execute(&repository.pool)
    .await
    .unwrap();
    let viewer = create_user(&repository, "viewer", Role::Viewer).await;
    let app = app_with_user_and_repository(repository, viewer).await;

    let response = app
        .oneshot(
            Request::get("/evidence/not-visible/download")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(body, "The requested resource was not found.");
}

#[tokio::test]
async fn admin_role_change_requires_csrf_and_writes_an_audit_event() {
    let repository = test_repository().await;
    let admin = create_user(&repository, "admin", Role::Admin).await;
    create_user(&repository, "target", Role::Operator).await;
    let app = app_with_user_and_repository(repository.clone(), admin).await;
    let page = app
        .clone()
        .oneshot(Request::get("/admin/users").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let page_body = String::from_utf8(
        to_bytes(page.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    let token = csrf_token(&page_body);

    let response = app
        .oneshot(post_form(
            "/admin/users/target/role",
            format!("role=viewer&csrf_token={token}"),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers().get("location").unwrap(), "/admin/users");
    let role: String = sqlx::query_scalar("SELECT role FROM users WHERE id = ?")
        .bind("target")
        .fetch_one(&repository.pool)
        .await
        .unwrap();
    assert_eq!(role, "viewer");
    let action: String = sqlx::query_scalar("SELECT action FROM audit_events WHERE target_id = ?")
        .bind("target")
        .fetch_one(&repository.pool)
        .await
        .unwrap();
    assert_eq!(action, "user.role_changed");
}

#[tokio::test]
async fn admin_can_disable_another_account_with_an_audit_event() {
    let repository = test_repository().await;
    let admin = create_user(&repository, "admin", Role::Admin).await;
    create_user(&repository, "target", Role::Viewer).await;
    let app = app_with_user_and_repository(repository.clone(), admin).await;
    let page = app
        .clone()
        .oneshot(Request::get("/admin/users").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let token = csrf_token(
        &String::from_utf8(
            to_bytes(page.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap(),
    );

    let response = app
        .oneshot(post_form(
            "/admin/users/target/disabled",
            format!("disabled=true&csrf_token={token}"),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let disabled: bool = sqlx::query_scalar("SELECT disabled FROM users WHERE id = ?")
        .bind("target")
        .fetch_one(&repository.pool)
        .await
        .unwrap();
    assert!(disabled);
    let action: String = sqlx::query_scalar("SELECT action FROM audit_events WHERE target_id = ?")
        .bind("target")
        .fetch_one(&repository.pool)
        .await
        .unwrap();
    assert_eq!(action, "user.disabled");
}

#[tokio::test]
async fn admin_cannot_change_their_own_role() {
    let repository = test_repository().await;
    let admin = create_user(&repository, "admin", Role::Admin).await;
    create_user(&repository, "target", Role::Viewer).await;
    let app = app_with_user_and_repository(repository.clone(), admin).await;
    let page = app
        .clone()
        .oneshot(Request::get("/admin/users").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let token = csrf_token(
        &String::from_utf8(
            to_bytes(page.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap(),
    );

    let response = app
        .oneshot(post_form(
            "/admin/users/admin/role",
            format!("role=viewer&csrf_token={token}"),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let role: String = sqlx::query_scalar("SELECT role FROM users WHERE id = ?")
        .bind("admin")
        .fetch_one(&repository.pool)
        .await
        .unwrap();
    assert_eq!(role, "admin");
    assert_eq!(repository.count_audit_events().await.unwrap(), 0);
}

#[tokio::test]
async fn admin_cannot_disable_their_own_account() {
    let repository = test_repository().await;
    let admin = create_user(&repository, "admin", Role::Admin).await;
    create_user(&repository, "target", Role::Viewer).await;
    let app = app_with_user_and_repository(repository.clone(), admin).await;
    let page = app
        .clone()
        .oneshot(Request::get("/admin/users").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let token = csrf_token(
        &String::from_utf8(
            to_bytes(page.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap(),
    );

    let response = app
        .oneshot(post_form(
            "/admin/users/admin/disabled",
            format!("disabled=true&csrf_token={token}"),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let disabled: bool = sqlx::query_scalar("SELECT disabled FROM users WHERE id = ?")
        .bind("admin")
        .fetch_one(&repository.pool)
        .await
        .unwrap();
    assert!(!disabled);
    assert_eq!(repository.count_audit_events().await.unwrap(), 0);
}

#[tokio::test]
async fn admin_role_change_rejects_unknown_roles() {
    let repository = test_repository().await;
    let admin = create_user(&repository, "admin", Role::Admin).await;
    create_user(&repository, "target", Role::Operator).await;
    let app = app_with_user_and_repository(repository.clone(), admin).await;
    let page = app
        .clone()
        .oneshot(Request::get("/admin/users").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let token = csrf_token(
        &String::from_utf8(
            to_bytes(page.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap(),
    );

    let response = app
        .oneshot(post_form(
            "/admin/users/target/role",
            format!("role=owner&csrf_token={token}"),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let role: String = sqlx::query_scalar("SELECT role FROM users WHERE id = ?")
        .bind("target")
        .fetch_one(&repository.pool)
        .await
        .unwrap();
    assert_eq!(role, "operator");
    assert_eq!(repository.count_audit_events().await.unwrap(), 0);
}

#[tokio::test]
async fn admin_account_mutation_rejects_a_missing_csrf_token() {
    let repository = test_repository().await;
    let admin = create_user(&repository, "admin", Role::Admin).await;
    create_user(&repository, "target", Role::Operator).await;
    let app = app_with_user_and_repository(repository.clone(), admin).await;

    let response = app
        .oneshot(post_form("/admin/users/target/role", "role=viewer"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let role: String = sqlx::query_scalar("SELECT role FROM users WHERE id = ?")
        .bind("target")
        .fetch_one(&repository.pool)
        .await
        .unwrap();
    assert_eq!(role, "operator");
    assert_eq!(repository.count_audit_events().await.unwrap(), 0);
}

#[tokio::test]
async fn admin_csrf_mismatch_rerenders_with_a_replacement_token() {
    let repository = test_repository().await;
    let admin = create_user(&repository, "admin", Role::Admin).await;
    create_user(&repository, "target", Role::Operator).await;
    let app = app_with_user_and_repository(repository.clone(), admin).await;
    let page = app
        .clone()
        .oneshot(Request::get("/admin/users").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let initial_token = csrf_token(&body_string(page).await);

    let response = app
        .oneshot(post_form(
            "/admin/users/target/role",
            format!("role=viewer&csrf_token={initial_token}-mismatch"),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = body_string(response).await;
    assert!(body.contains("The request is invalid."));
    assert!(body.contains("role=\"alert\""));
    assert_ne!(csrf_token(&body), initial_token);
    let role: String = sqlx::query_scalar("SELECT role FROM users WHERE id = ?")
        .bind("target")
        .fetch_one(&repository.pool)
        .await
        .unwrap();
    assert_eq!(role, "operator");
    assert_eq!(repository.count_audit_events().await.unwrap(), 0);
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
async fn viewer_malformed_operation_form_is_forbidden_before_form_parsing() {
    let repository = test_repository().await;
    let viewer = create_user(&repository, "viewer", Role::Viewer).await;
    let app = app_with_user_and_repository(repository.clone(), viewer).await;

    let response = app
        .oneshot(post_form(
            "/operations",
            "name=Lab&name=Duplicate&purpose=Practice",
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        repository
            .count_operations_visible_to("viewer", true)
            .await
            .unwrap(),
        0
    );
    assert_eq!(repository.count_audit_events().await.unwrap(), 0);
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
async fn authorized_malformed_operation_form_gets_generic_bad_request() {
    let repository = test_repository().await;
    let admin = create_user(&repository, "admin", Role::Admin).await;
    let app = app_with_user_and_repository(repository.clone(), admin).await;
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
            format!("name=Lab&name=Duplicate&purpose=Practice&csrf_token={token}"),
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
    assert_ne!(csrf_token(&body), token);
    assert_eq!(
        repository
            .count_operations_visible_to("admin", true)
            .await
            .unwrap(),
        0
    );
    assert_eq!(repository.count_audit_events().await.unwrap(), 0);
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
    assert!(body.contains("class=\"form-panel panel\""));
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
async fn out_of_scope_operator_malformed_asset_form_is_forbidden_before_form_parsing() {
    let repository = test_repository().await;
    let operation = repository
        .create_operation("Lab", "Practice")
        .await
        .unwrap();
    let operator = create_user(&repository, "operator", Role::Operator).await;
    let app = app_with_user_and_repository(repository.clone(), operator).await;

    let response = app
        .oneshot(post_form(
            &format!("/operations/{}/assets", operation.id),
            "name=web-01&name=duplicate&kind=web&owner=Lab&address=127.0.0.1",
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(
        repository
            .list_assets(&operation.id)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(repository.count_audit_events().await.unwrap(), 0);
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
    assert_eq!(submit.headers().get("location").unwrap(), "/operations");
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

#[tokio::test]
async fn check_history_renders_only_scoped_runs() {
    let fixture = scoped_portal_fixture().await;
    let app = app_with_user_and_repository(fixture.repository, fixture.viewer).await;

    let response = app
        .oneshot(Request::get("/checks").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;

    assert!(body.contains("allowed-check"));
    assert!(body.contains(&fixture.allowed_operation_id));
    assert!(!body.contains("hidden-check"));
    assert!(!body.contains(&fixture.hidden_operation_id));
}

#[tokio::test]
async fn audit_history_renders_only_scoped_events() {
    let fixture = scoped_portal_fixture().await;
    let app = app_with_user_and_repository(fixture.repository, fixture.viewer).await;

    let response = app
        .oneshot(Request::get("/audit").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;

    assert!(body.contains("allowed.reviewed"));
    assert!(!body.contains("hidden.reviewed"));
}

#[tokio::test]
async fn evidence_page_renders_safe_scoped_metadata_without_paths() {
    let fixture = scoped_portal_fixture().await;
    let app = app_with_user_and_repository(fixture.repository, fixture.viewer).await;

    let response = app
        .oneshot(Request::get("/evidence").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;

    assert!(body.contains(&fixture.allowed_evidence_id));
    assert!(body.contains("application/json"));
    assert!(body.contains("17 bytes"));
    assert!(body.contains("abcdef123456"));
    assert!(!body.contains("allowed.json"));
    assert!(!body.contains("abcdef1234567890abcdef1234567890"));
    assert!(!body.contains(&fixture.hidden_evidence_id));
    assert!(!body.contains("hidden.json"));
}

#[tokio::test]
async fn reports_page_counts_only_persisted_scoped_records() {
    let fixture = scoped_portal_fixture().await;
    let app = app_with_user_and_repository(fixture.repository, fixture.viewer).await;

    let response = app
        .oneshot(Request::get("/reports").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;

    assert!(body.contains("Allowed operation"));
    assert!(body.contains("Assets</dt><dd>2"));
    assert!(body.contains("Queued</dt><dd>1"));
    assert!(body.contains("Succeeded</dt><dd>1"));
    assert!(body.contains("Audit records</dt><dd>1"));
    assert!(!body.contains("Hidden operation"));
}

#[tokio::test]
async fn scoped_evidence_download_returns_verified_bytes_as_an_attachment() {
    let repository = test_repository().await;
    let viewer = create_user(&repository, "viewer", Role::Viewer).await;
    let operation = repository
        .create_operation("Download", "Verified download fixture")
        .await
        .unwrap();
    let asset = repository
        .create_asset(&operation.id, "host", "host", "lab", "127.0.0.1")
        .await
        .unwrap();
    repository
        .add_member(&operation.id, &viewer.id)
        .await
        .unwrap();
    repository
        .ensure_builtin_check(
            "verified-download",
            "Verified download",
            "viewer",
            30,
            4_096,
        )
        .await
        .unwrap();
    let run = repository
        .create_run(
            "verified-download",
            &asset.id,
            &operation.id,
            Some(&viewer.id),
            &serde_json::json!({}),
        )
        .await
        .unwrap();
    let directory = tempfile::TempDir::new().unwrap();
    let store = EvidenceStore::new(repository.clone(), directory.path());
    let evidence = store
        .write(&run.id, br#"{"safe":true}"#, "application/json")
        .await
        .unwrap();
    let app = app_with_user_repository_and_store(repository, viewer, store).await;

    let response = app
        .oneshot(
            Request::get(format!("/evidence/{}/download", evidence.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "application/json"
    );
    assert_eq!(
        response.headers().get("content-disposition").unwrap(),
        "attachment; filename=\"evidence\""
    );
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX).await.unwrap(),
        br#"{"safe":true}"#.as_slice()
    );
}

#[tokio::test]
async fn evidence_download_rejects_a_file_changed_after_storage() {
    let repository = test_repository().await;
    let viewer = create_user(&repository, "viewer", Role::Viewer).await;
    let operation = repository
        .create_operation("Download", "Checksum fixture")
        .await
        .unwrap();
    let asset = repository
        .create_asset(&operation.id, "host", "host", "lab", "127.0.0.1")
        .await
        .unwrap();
    repository
        .add_member(&operation.id, &viewer.id)
        .await
        .unwrap();
    repository
        .ensure_builtin_check(
            "checksum-download",
            "Checksum download",
            "viewer",
            30,
            4_096,
        )
        .await
        .unwrap();
    let run = repository
        .create_run(
            "checksum-download",
            &asset.id,
            &operation.id,
            Some(&viewer.id),
            &serde_json::json!({}),
        )
        .await
        .unwrap();
    let directory = tempfile::TempDir::new().unwrap();
    let store = EvidenceStore::new(repository.clone(), directory.path());
    let evidence = store
        .write(&run.id, br#"{"safe":true}"#, "application/json")
        .await
        .unwrap();
    tokio::fs::write(
        directory.path().join(&evidence.storage_path),
        br#"{"evil":true}"#,
    )
    .await
    .unwrap();
    let app = app_with_user_repository_and_store(repository, viewer, store).await;

    let response = app
        .oneshot(
            Request::get(format!("/evidence/{}/download", evidence.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX).await.unwrap(),
        "The request is invalid."
    );
}

#[tokio::test]
async fn topology_data_is_scoped_and_omits_callback_details_for_viewers() {
    let fixture = scoped_portal_fixture().await;
    sqlx::query("INSERT INTO callbacks (id, operation_id, host, session_key) VALUES ('test-callback', ?, 'sensitive-callback-host', 'test-session-secret')")
        .bind(&fixture.allowed_operation_id).execute(&fixture.repository.pool).await.unwrap();
    let app = app_with_user_and_repository(fixture.repository, fixture.viewer).await;
    let response = app
        .oneshot(Request::get("/topology/data").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;
    assert!(body.contains("Allowed operation"));
    assert!(body.contains("allowed-one"));
    assert!(!body.contains("Hidden operation"));
    assert!(!body.contains("session_key"));
    assert!(!body.contains("password_hash"));
    assert!(!body.contains("sensitive-callback-host"));
    assert!(!body.contains("test-session-secret"));
}

#[tokio::test]
async fn graph_and_recon_pages_render_without_javascript() {
    let app = app_with_logged_in_user(Role::Admin).await;
    for path in ["/topology", "/recon", "/recon?asset="] {
        let response = app
            .clone()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_string(response).await;
        assert!(body.contains("/static/workspace.js"));
        assert!(body.contains("No assets"));
    }
}

#[tokio::test]
async fn recon_execution_requires_operator_and_csrf() {
    let viewer = app_with_logged_in_user(Role::Viewer).await;
    let response = viewer
        .oneshot(post_form("/recon/run", "asset_id=unknown&mode=dns"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let admin = app_with_logged_in_user(Role::Admin).await;
    let response = admin
        .oneshot(post_form("/recon/run", "asset_id=unknown&mode=dns"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn recon_persists_observations_audit_and_topology_relationships() {
    let repo = test_repository().await;
    let admin = create_user(&repo, "recon-admin", Role::Admin).await;
    let op = repo
        .create_operation("Recon fixture", "Local test")
        .await
        .unwrap();
    sqlx::query("UPDATE operations SET status = 'active' WHERE id = ?")
        .bind(&op.id)
        .execute(&repo.pool)
        .await
        .unwrap();
    let asset = repo
        .create_asset(&op.id, "test-host", "host", "lab", "192.0.2.10")
        .await
        .unwrap();
    sqlx::query("INSERT INTO callbacks (id, operation_id, asset_id, host, session_key) VALUES ('test-callback', ?, ?, 'test-callback-host', 'never-expose-this-key')")
        .bind(&op.id).bind(&asset.id).execute(&repo.pool).await.unwrap();
    let app = app_with_user_and_repository(repo.clone(), admin).await;
    let page = app
        .clone()
        .oneshot(Request::get("/recon").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let token = csrf_token(&body_string(page).await);
    // A documentation IP is parsed locally; DNS-only mode opens no network connection.
    let response = app
        .clone()
        .oneshot(post_form(
            "/recon/run",
            format!("asset_id={}&mode=dns&csrf_token={token}", asset.id),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let runs = repo
        .list_check_runs_visible_to("recon-admin", true)
        .await
        .unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].state, RunState::Succeeded);
    assert_eq!(repo.count_audit_events().await.unwrap(), 2);
    let graph = body_string(
        app.clone()
            .oneshot(Request::get("/topology/data").body(Body::empty()).unwrap())
            .await
            .unwrap(),
    )
    .await;
    assert!(!graph.contains("never-expose-this-key"));
    let graph: serde_json::Value = serde_json::from_str(&graph).unwrap();
    assert_eq!(graph["nodes"].as_array().unwrap().len(), 4);
    let edges = graph["edges"].as_array().unwrap();
    assert!(
        edges
            .iter()
            .any(|e| e["source"] == format!("asset:{}", asset.id)
                && e["target"] == "callback:test-callback")
    );
    assert!(edges.iter().any(|e| e["label"] == "resolved to"));
    let history = body_string(
        app.oneshot(
            Request::get(format!("/recon?asset={}", asset.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap(),
    )
    .await;
    assert!(history.contains("dns.address"));
    assert!(history.contains("192.0.2.10"));
}

#[tokio::test]
async fn recon_rejects_out_of_scope_and_inactive_assets_before_creating_runs() {
    let repo = test_repository().await;
    let operator = create_user(&repo, "recon-operator", Role::Operator).await;
    let op = repo
        .create_operation("Restricted fixture", "Local test")
        .await
        .unwrap();
    let asset = repo
        .create_asset(&op.id, "test-host", "host", "lab", "192.0.2.10")
        .await
        .unwrap();
    let app = app_with_user_and_repository(repo.clone(), operator.clone()).await;
    let page = app
        .clone()
        .oneshot(Request::get("/recon").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let token = csrf_token(&body_string(page).await);
    let body = format!("asset_id={}&mode=dns&csrf_token={token}", asset.id);
    let response = app
        .clone()
        .oneshot(post_form("/recon/run", body.clone()))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    repo.add_member(&op.id, &operator.id).await.unwrap();
    let response = app
        .clone()
        .oneshot(post_form("/recon/run", body.clone()))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    sqlx::query("UPDATE operations SET status = 'active' WHERE id = ?")
        .bind(&op.id)
        .execute(&repo.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE assets SET status = 'retired' WHERE id = ?")
        .bind(&asset.id)
        .execute(&repo.pool)
        .await
        .unwrap();
    let response = app.oneshot(post_form("/recon/run", body)).await.unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert!(
        repo.list_check_runs_visible_to(&operator.id, true)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(repo.count_audit_events().await.unwrap(), 0);
}
