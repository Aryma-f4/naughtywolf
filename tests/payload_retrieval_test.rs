//! Retrieval ledger and build randomizer, exercised through the real routes.
use axum::{
    Router,
    body::Body,
    extract::Extension,
    http::{Request, StatusCode},
    response::Response,
    routing::post,
};
use naughtywolf::{
    auth::{AuthenticatedUser, middleware::AuthSession, rbac::Role},
    db::{self, repositories::Repository},
    portal::templates,
};
use tower::ServiceExt;
use tower_sessions::{MemoryStore, SessionManagerLayer};

/// `payload` resolves `./payloads` relative to the process working directory,
/// so tests that exercise it must not run concurrently with each other.
static CWD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Serializes a test and moves into `dir`, restoring the previous working
/// directory when the returned guard drops.
fn enter(dir: &std::path::Path) -> CwdGuard {
    let lock = CWD_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let original = std::env::current_dir().expect("a current directory");
    std::env::set_current_dir(dir).expect("enter temp dir");
    CwdGuard {
        _lock: lock,
        original: Some(original),
    }
}

struct CwdGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    original: Option<std::path::PathBuf>,
}

impl Drop for CwdGuard {
    fn drop(&mut self) {
        if let Some(original) = self.original.take() {
            let _ = std::env::set_current_dir(original);
        }
    }
}

async fn test_repository() -> Repository {
    let pool = db::create_pool("sqlite::memory:").await.unwrap();
    db::run_migrations(&pool).await.unwrap();
    Repository { pool }
}

async fn operator(repo: &Repository) -> AuthenticatedUser {
    sqlx::query("INSERT INTO users (id, username, password_hash, role) VALUES (?, ?, ?, ?)")
        .bind("op")
        .bind("op-user")
        .bind("not-a-real-password-hash")
        .bind(Role::Operator.to_string())
        .execute(&repo.pool)
        .await
        .unwrap();
    AuthenticatedUser {
        id: "op".to_owned(),
        username: "op-user".to_owned(),
        role: Role::Operator,
    }
}

async fn test_login(
    session: tower_sessions::Session,
    Extension(user): Extension<AuthenticatedUser>,
) -> StatusCode {
    AuthSession { session }.login(&user).await.unwrap();
    StatusCode::NO_CONTENT
}

async fn app_with_user(
    repository: Repository,
    user: AuthenticatedUser,
) -> impl tower::Service<Request<Body>, Response = Response, Error = std::convert::Infallible> + Clone
{
    let app = Router::<Repository>::new()
        .route("/test/login", post(test_login))
        .merge(naughtywolf::portal::authenticated_router())
        .with_state(repository)
        .layer(Extension(user))
        .layer(Extension(std::sync::Arc::new(vec![0u8; 32])))
        .layer(SessionManagerLayer::new(MemoryStore::default()).with_secure(false));
    let login = app
        .clone()
        .oneshot(Request::post("/test/login").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let cookie = login
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

/// The whole point of the ledger: a payload nobody fetched must read as
/// "never retrieved", and fetching it must flip that.
#[tokio::test]
async fn fetching_a_payload_records_a_retrieval_and_a_fresh_one_reads_as_never() {
    let dir = tempfile::tempdir().unwrap();
    let payload_dir = dir.path().join("payloads");
    std::fs::create_dir_all(&payload_dir).unwrap();

    let file = "fleet-0000.linux.amd64";
    std::fs::write(payload_dir.join(file), b"artifact-bytes").unwrap();
    let public_id = uuid::Uuid::new_v4().to_string();
    std::fs::write(
        payload_dir.join(format!("{file}.json")),
        serde_json::to_vec(&naughtywolf::payload::PayloadMeta {
            file: file.to_owned(),
            name: "fleet-0000".to_owned(),
            os: "linux".to_owned(),
            arch: "amd64".to_owned(),
            protocol: "http".to_owned(),
            lhost: "192.0.2.10".to_owned(),
            lport: 8080,
            interval_ms: 5000,
            jitter_ms: 500,
            target: String::new(),
            public_id: public_id.clone(),
            size: 14,
            built_at: "2026-01-01T00:00:00Z".to_owned(),
            retrievals: 0,
            first_retrieved_at: None,
            last_retrieved_at: None,
            fingerprint: None,
        })
        .unwrap(),
    )
    .unwrap();

    // Run from the temp directory so the payload module resolves ./payloads.
    let cwd = enter(dir.path());

    let read_meta = || {
        std::fs::read(payload_dir.join(format!("{file}.json")))
            .ok()
            .and_then(|bytes| {
                serde_json::from_slice::<naughtywolf::payload::PayloadMeta>(&bytes).ok()
            })
            .expect("sidecar")
    };

    let before = read_meta();
    assert!(
        !before.was_retrieved(),
        "a freshly built payload has not been fetched"
    );

    // Drive the real ledger entry point.
    naughtywolf::payload::record_retrieval(file).unwrap();
    let after_first = read_meta();
    assert_eq!(after_first.retrievals, 1);
    let first_seen = after_first
        .first_retrieved_at
        .clone()
        .expect("first retrieval");

    naughtywolf::payload::record_retrieval(file).unwrap();
    let after = read_meta();
    assert!(after.was_retrieved());
    assert_eq!(after.retrievals, 2);
    // `first` must stay pinned to the first fetch while `last` advances.
    assert_eq!(after.first_retrieved_at.clone(), Some(first_seen.clone()));
    assert!(after.last_retrieved_at >= Some(first_seen));
    assert_eq!(
        after.public_id, public_id,
        "token must survive retrieval writes"
    );

    // The token still resolves to the same artifact afterwards.
    let (path, resolved) = naughtywolf::payload::public_download(&public_id).unwrap();
    assert_eq!(resolved, file);
    assert!(path.ends_with(file));

    drop(cwd);
}

/// A download for an artifact that does not exist must not fabricate a ledger
/// entry, and must not be able to write outside the payload directory.
#[tokio::test]
async fn retrieval_ledger_rejects_unknown_and_unsafe_names() {
    let dir = tempfile::tempdir().unwrap();
    let payload_dir = dir.path().join("payloads");
    std::fs::create_dir_all(&payload_dir).unwrap();
    let _cwd = enter(dir.path());

    assert!(naughtywolf::payload::record_retrieval("missing.bin").is_err());
    assert!(
        naughtywolf::payload::record_retrieval("../../etc/passwd").is_err(),
        "a traversal name must never be recorded"
    );
}

/// An unguessable token is the only way in; a wrong one is not found.
#[tokio::test]
async fn the_download_token_is_the_only_way_to_resolve_an_artifact() {
    let dir = tempfile::tempdir().unwrap();
    let payload_dir = dir.path().join("payloads");
    std::fs::create_dir_all(&payload_dir).unwrap();
    let file = "fleet-1111.linux.amd64";
    std::fs::write(payload_dir.join(file), b"artifact-bytes").unwrap();
    let public_id = uuid::Uuid::new_v4().to_string();
    std::fs::write(
        payload_dir.join(format!("{file}.json")),
        format!(
            r#"{{"file":"{file}","name":"fleet-1111","os":"linux","arch":"amd64","protocol":"http","lhost":"192.0.2.10","lport":8080,"interval_ms":0,"jitter_ms":0,"target":"","public_id":"{public_id}","size":14,"built_at":"2026-01-01T00:00:00Z"}}"#
        ),
    )
    .unwrap();

    let _cwd = enter(dir.path());

    assert!(naughtywolf::payload::public_download(&public_id).is_some());
    assert!(naughtywolf::payload::public_download(&uuid::Uuid::new_v4().to_string()).is_none());
    // Neither the filename nor a directory traversal resolves as a token.
    assert!(naughtywolf::payload::public_download(file).is_none());
    assert!(naughtywolf::payload::public_download("..").is_none());
    assert!(naughtywolf::payload::public_download("").is_none());
}

/// The operator-facing view is the retrieval signal, so it must be on the page.
#[tokio::test]
async fn the_payload_table_reports_retrieval_state() {
    let user = AuthenticatedUser {
        id: "op".into(),
        username: "op-user".into(),
        role: Role::Operator,
    };
    let base = naughtywolf::payload::PayloadMeta {
        file: "a.linux.amd64".into(),
        name: "a".into(),
        os: "linux".into(),
        arch: "amd64".into(),
        protocol: "http".into(),
        lhost: "192.0.2.10".into(),
        lport: 8080,
        interval_ms: 5000,
        jitter_ms: 500,
        target: String::new(),
        public_id: uuid::Uuid::new_v4().to_string(),
        size: 10,
        built_at: "2026-01-01T00:00:00Z".into(),
        retrievals: 0,
        first_retrieved_at: None,
        last_retrieved_at: None,
        fingerprint: None,
    };
    let mut fetched = base.clone();
    fetched.retrievals = 3;
    fetched.first_retrieved_at = Some("2026-01-02T00:00:00Z".into());
    fetched.last_retrieved_at = Some("2026-01-03T00:00:00Z".into());

    let body = templates::payloads_page(&user, &[base, fetched], "csrf", None, None, &[], &[]);
    assert!(body.contains("Never retrieved"));
    assert!(body.contains("3 fetch(es)"));
    assert!(body.contains("2026-01-03"));
    assert!(body.contains("5000ms"), "cadence is shown");
    assert!(body.contains("/payloads/randomize"));
}

/// The randomizer route must not accept a request without a valid CSRF token.
#[tokio::test]
async fn the_randomizer_route_requires_csrf() {
    let repo = test_repository().await;
    let user = operator(&repo).await;
    let app = app_with_user(repo.clone(), user).await;
    let response = app
        .oneshot(
            Request::post("/payloads/randomize")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("name=x&lhost=192.0.2.10&lport=8080&protocol=http&os=linux&arch=amd64&csrf_token=stale"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(response.status(), StatusCode::SEE_OTHER);
}

/// A viewer must not be able to queue builds.
#[tokio::test]
async fn a_viewer_cannot_queue_a_randomized_build() {
    let repo = test_repository().await;
    sqlx::query("INSERT INTO users (id, username, password_hash, role) VALUES (?, ?, ?, ?)")
        .bind("viewer")
        .bind("viewer-user")
        .bind("hash")
        .bind(Role::Viewer.to_string())
        .execute(&repo.pool)
        .await
        .unwrap();
    let viewer = AuthenticatedUser {
        id: "viewer".to_owned(),
        username: "viewer-user".to_owned(),
        role: Role::Viewer,
    };
    let app = app_with_user(repo, viewer).await;
    let response = app
        .oneshot(
            Request::post("/payloads/randomize")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("name=x&lhost=192.0.2.10&lport=8080&protocol=http&os=linux&arch=amd64&csrf_token=x"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}
