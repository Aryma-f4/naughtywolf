use std::sync::Arc;
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::{
    Router,
    extract::{Extension, Request},
    http::StatusCode,
    middleware::{self, Next},
    response::Response,
    routing::post,
};
use naughtywolf::{
    application,
    auth::{AuthenticatedUser, middleware::AuthSession, rbac::Role},
    callback_workspace::transfers::TransferStore,
    db::{self, repositories::Repository},
    evidence::EvidenceStore,
    portal,
};
use nw_implant::{
    runtime::{BeaconRuntime, Profile},
    transport::Transport,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::Notify;
use tower_sessions::{MemoryStore, Session, SessionManagerLayer};
use uuid::Uuid;

const TOTAL: usize = 3072;
const MAX_TRANSFER_BYTES: u64 = 4096;

fn install_download_fixture() -> Vec<u8> {
    (0..TOTAL).map(|index| (index % 251) as u8).collect()
}

fn upload_fixture() -> Vec<u8> {
    vec![0x31; TOTAL]
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

async fn test_repository(db_url: &str) -> Repository {
    let pool = db::create_pool(db_url).await.unwrap();
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

async fn create_operation_member_scoping(repo: &Repository, user_id: &str, session_id: &str) {
    let operation = repo
        .create_operation("Production Restart Lab", "Transfer restart E2E")
        .await
        .unwrap();
    sqlx::query("INSERT INTO operation_members (operation_id, user_id) VALUES (?, ?)")
        .bind(&operation.id)
        .bind(user_id)
        .execute(&repo.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE callbacks SET operation_id = ? WHERE id = ?")
        .bind(&operation.id)
        .bind(session_id)
        .execute(&repo.pool)
        .await
        .unwrap();
}

async fn wait_for_callback(repo: &Repository, host: &str) -> String {
    let mut waited = Duration::ZERO;
    loop {
        let session_id: Option<String> =
            sqlx::query_scalar("SELECT id FROM callbacks WHERE host = ?")
                .bind(host)
                .fetch_optional(&repo.pool)
                .await
                .unwrap();
        if let Some(session_id) = session_id {
            return session_id;
        }
        assert!(
            waited < Duration::from_secs(15),
            "implant never registered for {host}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
        waited += Duration::from_millis(50);
    }
}

async fn login_cookie(addr: std::net::SocketAddr) -> String {
    let client = reqwest::Client::new();
    let response = client
        .post(format!("http://{addr}/test/login"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}

fn csrf_token(body: &str) -> String {
    body.split("name=\"csrf_token\" value=\"")
        .nth(1)
        .and_then(|remainder| remainder.split('"').next())
        .unwrap()
        .to_owned()
}

async fn fetch_csrf(addr: std::net::SocketAddr, cookie: &str, session_id: &str) -> String {
    let client = reqwest::Client::new();
    let page = client
        .get(format!("http://{addr}/callbacks/{session_id}"))
        .header("cookie", cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    csrf_token(&page.text().await.unwrap())
}

async fn queue_download(
    addr: std::net::SocketAddr,
    cookie: &str,
    csrf: &str,
    session_id: &str,
    path: &str,
    sha256: &str,
) -> String {
    let client = reqwest::Client::new();
    let response = client
        .post(format!(
            "http://{addr}/api/callbacks/{session_id}/files/download"
        ))
        .header("cookie", cookie)
        .header("x-csrf-token", csrf)
        .json(&serde_json::json!({
            "path": path,
            "expected_size": TOTAL as u64,
            "sha256": sha256,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let view: Value = response.json().await.unwrap();
    view["id"].as_str().unwrap().to_owned()
}

async fn queue_upload(
    addr: std::net::SocketAddr,
    cookie: &str,
    csrf: &str,
    session_id: &str,
    destination: &str,
    bytes: Vec<u8>,
) -> String {
    let client = reqwest::Client::new();
    let form = reqwest::multipart::Form::new()
        .text("destination", destination.to_owned())
        .part(
            "file",
            reqwest::multipart::Part::bytes(bytes)
                .file_name("payload.bin")
                .mime_str("application/octet-stream")
                .unwrap(),
        );
    let response = client
        .post(format!(
            "http://{addr}/api/callbacks/{session_id}/files/upload"
        ))
        .header("cookie", cookie)
        .header("x-csrf-token", csrf)
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let view: Value = response.json().await.unwrap();
    view["id"].as_str().unwrap().to_owned()
}

async fn transfer_row(repo: &Repository, transfer_id: &str) -> (String, i64) {
    let (status, received): (String, i64) =
        sqlx::query_as("SELECT status, received_bytes FROM c2_file_transfers WHERE id = ?")
            .bind(transfer_id)
            .fetch_one(&repo.pool)
            .await
            .unwrap();
    (status, received)
}

async fn dump_row(repo: &Repository, transfer_id: &str) {
    let row: (String, String, i64, i64, Option<String>, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT direction, status, received_bytes, expected_size, error, sha256, task_id FROM c2_file_transfers WHERE id = ?",
    )
    .bind(transfer_id)
    .fetch_one(&repo.pool)
    .await
    .unwrap();
    let (direction, status, received, expected, error, sha, task_id) = row;
    let task_status: Option<String> =
        sqlx::query_scalar("SELECT status FROM c2_tasks WHERE id = ?")
            .bind(&task_id)
            .fetch_optional(&repo.pool)
            .await
            .unwrap();
    println!(
        "DB {transfer_id} dir={direction} status={status} recv={received} exp={expected} err={error:?} sha={} task={task_status:?}",
        sha.as_deref().unwrap_or("-"),
    );
}

async fn storage_key(repo: &Repository, transfer_id: &str) -> String {
    sqlx::query_scalar("SELECT storage_key FROM c2_file_transfers WHERE id = ?")
        .bind(transfer_id)
        .fetch_one(&repo.pool)
        .await
        .unwrap()
}

async fn wait_for_transfer_completed(repo: &Repository, transfer_id: &str) {
    let mut waited = Duration::ZERO;
    loop {
        let (status, _) = transfer_row(repo, transfer_id).await;
        if status == "completed" {
            return;
        }
        if waited >= Duration::from_secs(10) {
            dump_row(repo, transfer_id).await;
            panic!("transfer {transfer_id} did not complete: {status}");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
        waited += Duration::from_millis(25);
    }
}

async fn wait_for_terminal(repo: &Repository, transfer_ids: &[&str], task_ids: &[&str]) {
    let mut waited = Duration::ZERO;
    loop {
        let mut remaining = vec![false; transfer_ids.len()];
        for (index, transfer_id) in transfer_ids.iter().enumerate() {
            let (status, _) = transfer_row(repo, transfer_id).await;
            remaining[index] = status == "completed";
        }
        if remaining.iter().all(|done| *done) {
            for task_id in task_ids {
                let status: String = sqlx::query_scalar("SELECT status FROM c2_tasks WHERE id = ?")
                    .bind(task_id)
                    .fetch_one(&repo.pool)
                    .await
                    .unwrap();
                if status != "completed" {
                    for transfer_id in transfer_ids {
                        dump_row(repo, transfer_id).await;
                    }
                    panic!("task {task_id} not completed: {status}");
                }
            }
            return;
        }
        assert!(
            waited < Duration::from_secs(20),
            "transfers did not reach terminal state in time"
        );
        for transfer_id in transfer_ids {
            let (status, _) = transfer_row(repo, transfer_id).await;
            if status != "completed" {
                dump_row(repo, transfer_id).await;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        waited += Duration::from_millis(50);
    }
}

#[derive(Clone)]
struct RestartState {
    repo: Repository,
    session_id: Arc<RwLock<Option<String>>>,
    fired: Arc<AtomicBool>,
    trigger: Arc<Notify>,
    release: Arc<Notify>,
}

impl RestartState {
    fn new(repo: Repository, already_fired: bool) -> Self {
        Self {
            repo,
            session_id: Arc::new(RwLock::new(None)),
            fired: Arc::new(AtomicBool::new(already_fired)),
            trigger: Arc::new(Notify::new()),
            release: Arc::new(Notify::new()),
        }
    }
}

async fn sever_mid_transfer(
    Extension(state): Extension<RestartState>,
    request: Request,
    next: Next,
) -> Response {
    if request.uri().path() != "/c2/checkin" {
        return next.run(request).await;
    }
    let response = next.run(request).await;
    let Some(session_id) = state.session_id.read().unwrap().clone() else {
        return response;
    };
    if state.fired.load(Ordering::SeqCst) {
        return response;
    }
    let transfers = state
        .repo
        .list_file_transfers(&session_id)
        .await
        .unwrap_or_default();
    let mut upload = 0i64;
    let mut download = 0i64;
    for transfer in &transfers {
        match transfer.direction.trim() {
            "upload" => upload = upload.max(transfer.received_bytes),
            "download" => download = download.max(transfer.received_bytes),
            _ => {}
        }
    }
    // The C2 reply budget (32 KiB over HTTP) pushes a 3 KiB upload in one
    // reply, so the upload durably lands at its full size in a single ack.
    // Sever only the download: it is paced one 1024-byte chunk per poll, so
    // the server durably records dl=1024 here and the stream must resume
    // from that offset after the restart.
    if download > 0 && download < TOTAL as i64 && upload >= 1 {
        state.fired.store(true, Ordering::SeqCst);
        state.trigger.notify_waiters();
        state.release.notified().await;
    }
    response
}

async fn test_login(session: Session, Extension(user): Extension<AuthenticatedUser>) -> StatusCode {
    (AuthSession { session }).login(&user).await.unwrap();
    StatusCode::OK
}

fn test_app(
    repository: Repository,
    operator: AuthenticatedUser,
    psk: Arc<Vec<u8>>,
    transfer_store: TransferStore,
    evidence_store: EvidenceStore,
    restart: RestartState,
) -> Router<()> {
    Router::<Repository>::new()
        .route("/test/login", post(test_login))
        .merge(application::router(
            psk,
            evidence_store,
            transfer_store,
            MAX_TRANSFER_BYTES,
        ))
        .layer(Extension(operator))
        .with_state(repository)
        .merge(portal::public_router())
        .layer(SessionManagerLayer::new(MemoryStore::default()).with_secure(false))
        .layer(middleware::from_fn(sever_mid_transfer))
        .layer(axum::Extension(restart.clone()))
}

async fn serve(listener: tokio::net::TcpListener, app: Router<()>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    })
}

#[tokio::test]
async fn production_stack_restart_resumes_partial_download() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .try_init();
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("workspace");
    let evidence_dir = workspace.join("evidence");
    let transfer_root = evidence_dir.join("callback-transfers");
    let implant_dir = workspace.join("implant");
    let implant_dest = implant_dir.join("pushed.bin");
    let download_source = implant_dir.join("pulled-source.bin");
    std::fs::create_dir_all(&implant_dir).unwrap();

    let db_path = temp.path().join("restart.db");
    std::fs::File::create(&db_path).unwrap();
    let db_url = format!("sqlite://{}", db_path.display());
    let psk = Arc::new(b"production-restart-psk".to_vec());
    let endpoint_hostname = format!("restart-host-{}", Uuid::new_v4());

    let repo = test_repository(&db_url).await;
    let operator = create_user(&repo, "operator", Role::Operator).await;

    let evidence_store = EvidenceStore::new(repo.clone(), evidence_dir.clone());
    let transfer_store = TransferStore::new(repo.clone(), evidence_dir.clone(), MAX_TRANSFER_BYTES)
        .expect("transfer store opens");

    let restart = RestartState::new(repo.clone(), false);
    let app = test_app(
        repo.clone(),
        operator.clone(),
        psk.clone(),
        transfer_store.clone(),
        evidence_store.clone(),
        restart.clone(),
    );

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let server = serve(listener, app).await;

    let download_bytes = install_download_fixture();
    let download_sha = sha256_hex(&download_bytes);
    std::fs::write(&download_source, &download_bytes).unwrap();
    std::fs::write(download_source.with_extension("b"), &download_bytes).unwrap();

    let runtime = Arc::new(BeaconRuntime::new(
        Profile {
            endpoint: format!("http://{addr}"),
            interval: Duration::from_millis(40),
            jitter: Duration::ZERO,
            hostname: endpoint_hostname.clone(),
            username: "restart-tester".into(),
            os: "test-os".into(),
            arch: "test-arch".into(),
            pid: 1234,
            addr: "127.0.0.1".into(),
        },
        (*psk).clone(),
    ));
    let beacon_runtime = runtime.clone();
    let beacon = tokio::spawn(async move { beacon_runtime.run().await });

    let session_id = wait_for_callback(&repo, &endpoint_hostname).await;
    *restart.session_id.write().unwrap() = Some(session_id.clone());
    create_operation_member_scoping(&repo, &operator.id, &session_id).await;

    let cookie = login_cookie(addr).await;
    let csrf = fetch_csrf(addr, &cookie, &session_id).await;

    // Phase 1: a single upload completes so the durable upload offset is
    // terminal before the download ever starts. Uploads and downloads are
    // independent FIFO queues, so queueing both up-front would let the
    // download stream to completion before the upload ack lands (observed
    // 0 upload progress during the download's three chunks).
    let upload_a = queue_upload(
        addr,
        &cookie,
        &csrf,
        &session_id,
        &implant_dest.to_string_lossy(),
        upload_fixture(),
    )
    .await;
    wait_for_transfer_completed(&repo, &upload_a).await;

    let download_a = queue_download(
        addr,
        &cookie,
        &csrf,
        &session_id,
        &download_source.to_string_lossy(),
        &download_sha,
    )
    .await;
    let upload_b = queue_upload(
        addr,
        &cookie,
        &csrf,
        &session_id,
        &implant_dest.with_extension("b").to_string_lossy(),
        upload_fixture(),
    )
    .await;
    let download_b = queue_download(
        addr,
        &cookie,
        &csrf,
        &session_id,
        &download_source.with_extension("b").to_string_lossy(),
        &download_sha,
    )
    .await;

    tokio::time::timeout(Duration::from_secs(15), restart.trigger.notified())
        .await
        .expect("severance never fired");

    let (upload_a_status, upload_a_received) = transfer_row(&repo, &upload_a).await;
    let (download_a_status, download_a_received) = transfer_row(&repo, &download_a).await;
    let download_b_received = {
        let (_, download_b_received) = transfer_row(&repo, &download_b).await;
        download_b_received
    };
    println!(
        "server-severed durable offsets: upload={upload_a_received} download={download_a_received} \
         upload-a={upload_a_status} download-a={download_a_status}"
    );
    assert_eq!(
        upload_a_received, TOTAL as i64,
        "upload must be durably complete"
    );
    assert!(
        download_a_received > 0 && download_a_received < TOTAL as i64,
        "download must be severed mid-stream (got {download_a_received})"
    );
    assert_eq!(
        download_b_received, 0,
        "the download follower must not start until the head download is terminal"
    );

    server.abort();
    let _ = server.await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let transport = Transport::from_endpoint(&format!("http://{addr}")).unwrap();
    match transport.exchange(&[0u8; 16]).await {
        Ok(_) => panic!("client must observe a real socket error against the severed endpoint"),
        Err(reason) => println!("socket error observed: {reason}"),
    }
    restart.release.notify_waiters();

    std::mem::drop(restart);
    std::mem::drop(transfer_store);
    std::mem::drop(evidence_store);
    std::mem::drop(repo);

    // Reopen the same SQLite database, rebuild the stores and serve the same
    // endpoint again while the implant is still alive and keeps polling.
    let reopened = test_repository(&db_url).await;
    let reopened_store =
        TransferStore::new(reopened.clone(), evidence_dir.clone(), MAX_TRANSFER_BYTES)
            .expect("reopened transfer store");
    let reopened_evidence = EvidenceStore::new(reopened.clone(), evidence_dir.clone());
    let restarted = RestartState::new(reopened.clone(), true);
    let app2 = test_app(
        reopened.clone(),
        operator.clone(),
        psk.clone(),
        reopened_store,
        reopened_evidence,
        restarted,
    );
    let listener2 = tokio::net::TcpListener::bind(addr).await.unwrap();
    let server2 = serve(listener2, app2).await;

    let task_ids = task_ids_for(&reopened, &[&upload_a, &download_a, &upload_b, &download_b]).await;
    let transfer_ids: Vec<&str> = vec![&upload_a, &download_a, &upload_b, &download_b];
    let task_refs: Vec<&str> = task_ids.iter().map(String::as_str).collect();
    wait_for_terminal(&reopened, &transfer_ids, &task_refs).await;

    let upload_bytes = upload_fixture();
    let upload_sha = sha256_hex(&upload_bytes);
    for transfer_id in [&upload_a, &download_a, &upload_b, &download_b] {
        let direction: String =
            sqlx::query_scalar("SELECT direction FROM c2_file_transfers WHERE id = ?")
                .bind(transfer_id)
                .fetch_one(&reopened.pool)
                .await
                .unwrap();
        let (expected, expected_sha) = if direction == "upload" {
            (&upload_bytes, &upload_sha)
        } else {
            (&download_bytes, &download_sha)
        };
        assert_artifact(
            &reopened,
            &transfer_root,
            transfer_id,
            expected,
            expected_sha,
        )
        .await;
    }

    assert_no_partials(&transfer_root);
    assert_no_implant_partials(&implant_dir);

    let completed_order =
        transfer_order(&reopened, &[&upload_a, &download_a, &upload_b, &download_b]).await;
    assert_eq!(completed_order.len(), 4, "all transfers must complete");
    for direction in ["upload", "download"] {
        let direction_rows: Vec<_> = completed_order
            .iter()
            .filter(|(dir, _, _)| dir.as_str() == direction)
            .collect();
        for pair in direction_rows.windows(2) {
            assert!(
                pair[0].2 <= pair[1].2,
                "{direction} transfer {} must finish before {}",
                pair[0].1,
                pair[1].1
            );
        }
    }

    // Every nw/upload|nw/download task is terminal with exactly one result.
    for transfer_id in [&upload_a, &download_a, &upload_b, &download_b] {
        let task_id: String =
            sqlx::query_scalar("SELECT task_id FROM c2_file_transfers WHERE id = ?")
                .bind(transfer_id)
                .fetch_one(&reopened.pool)
                .await
                .unwrap();
        let result_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM c2_task_results WHERE task_id = ?")
                .bind(&task_id)
                .fetch_one(&reopened.pool)
                .await
                .unwrap();
        assert_eq!(result_count, 1, "task {task_id} result not converged");
    }

    runtime.trigger_stop();
    let _ = beacon.await;
    server2.abort();
    let _ = server2.await;
}

async fn task_ids_for(repo: &Repository, transfer_ids: &[&str]) -> Vec<String> {
    let mut result = Vec::new();
    for transfer_id in transfer_ids {
        let task_id: Option<String> =
            sqlx::query_scalar("SELECT task_id FROM c2_file_transfers WHERE id = ?")
                .bind(transfer_id)
                .fetch_one(&repo.pool)
                .await
                .unwrap();
        result.push(task_id.unwrap());
    }
    result
}

async fn assert_artifact(
    repo: &Repository,
    transfer_root: &std::path::Path,
    transfer_id: &str,
    expected: &[u8],
    expected_sha: &str,
) {
    let transfer_key = storage_key(repo, transfer_id).await;
    let artifact = transfer_root.join(&transfer_key);
    let contents: Vec<String> = std::fs::read_dir(transfer_root)
        .unwrap()
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    assert_eq!(
        std::fs::read(&artifact).unwrap_or_else(|e| {
            panic!(
                "read artifact {transfer_id} ({artifact:?}): {e}; transfer_root has {} entries {contents:?}",
                contents.len()
            )
        }),
        expected
    );
    let row_sha: Option<String> =
        sqlx::query_scalar("SELECT sha256 FROM c2_file_transfers WHERE id = ?")
            .bind(transfer_id)
            .fetch_one(&repo.pool)
            .await
            .unwrap();
    assert_eq!(row_sha.as_deref(), Some(expected_sha));
}

async fn transfer_order(repo: &Repository, ids: &[&str]) -> Vec<(String, String, String)> {
    let mut rows = Vec::new();
    for transfer_id in ids {
        let completed_at: Option<String> =
            sqlx::query_scalar("SELECT completed_at FROM c2_file_transfers WHERE id = ?")
                .bind(transfer_id)
                .fetch_one(&repo.pool)
                .await
                .unwrap();
        let direction: String =
            sqlx::query_scalar("SELECT direction FROM c2_file_transfers WHERE id = ?")
                .bind(transfer_id)
                .fetch_one(&repo.pool)
                .await
                .unwrap();
        rows.push((
            direction,
            (*transfer_id).to_owned(),
            completed_at.unwrap_or_default(),
        ));
    }
    rows
}

fn assert_no_partials(transfer_root: &std::path::Path) {
    let part_like: Vec<_> = std::fs::read_dir(transfer_root)
        .unwrap()
        .flatten()
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.ends_with(".part")
        })
        .collect();
    assert!(
        part_like.is_empty(),
        "server-side partials remain: {part_like:?}"
    );
}

fn assert_no_implant_partials(implant_dir: &std::path::Path) {
    let partials: Vec<_> = std::fs::read_dir(implant_dir)
        .unwrap()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().contains("nwpart"))
        .collect();
    assert!(
        partials.is_empty(),
        "implant-side partials remain: {partials:?}"
    );
}
