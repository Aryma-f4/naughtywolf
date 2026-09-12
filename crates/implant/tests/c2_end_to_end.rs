//! End-to-end C2 round trip: start a real server listener on an ephemeral
//! port, connect an in-process BeaconRuntime implant, queue a task, and assert
//! the result comes back over the real HTTP channel.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use axum::{
    Router,
    body::Bytes,
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
    routing::post,
};
use nw_implant::runtime::{BeaconRuntime, Profile};
use nw_profile::{
    crypto,
    envelope::{Envelope, Kind},
    msgs::{PollReply, PollRequest, Register, RegisterAck, Task},
};
use nw_server::{ServerState, queue::TaskQueue, server, session::SessionRegistry};
use uuid::Uuid;

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[derive(Clone)]
struct DropPartialTransferResponse {
    watched: PathBuf,
    expected: u64,
    dropped: Arc<AtomicBool>,
}

async fn drop_partial_transfer_response(
    State(state): State<DropPartialTransferResponse>,
    request: Request,
    next: Next,
) -> Response {
    let response = next.run(request).await;
    let bytes = if state.watched.is_dir() {
        std::fs::read_dir(&state.watched)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.ok()?.metadata().ok().map(|metadata| metadata.len()))
            .max()
            .unwrap_or(0)
    } else {
        let direct = std::fs::metadata(&state.watched)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        // Upload receivers keep the final destination untouched until
        // publication; observe the transfer-specific sidecar instead.
        let prefix = state
            .watched
            .file_name()
            .map(|name| format!("{}{}.nwpart-", name.to_string_lossy(), ""));
        let staged = prefix
            .and_then(|prefix| {
                state.watched.parent().and_then(|parent| {
                    std::fs::read_dir(parent).ok().map(|entries| {
                        entries
                            .flatten()
                            .filter(|entry| {
                                entry.file_name().to_string_lossy().starts_with(&prefix)
                            })
                            .filter_map(|entry| entry.metadata().ok().map(|m| m.len()))
                            .max()
                            .unwrap_or(0)
                    })
                })
            })
            .unwrap_or(0);
        direct.max(staged)
    };
    if bytes > 0 && bytes < state.expected && !state.dropped.swap(true, Ordering::SeqCst) {
        return StatusCode::BAD_GATEWAY.into_response();
    }
    response
}

type SessionState = Option<(Uuid, [u8; crypto::KEY_LEN])>;

#[derive(Clone)]
struct AckTestServer {
    psk: Arc<Vec<u8>>,
    session: Arc<Mutex<SessionState>>,
    task: Task,
    polls: Arc<AtomicUsize>,
    result_deliveries: Arc<AtomicUsize>,
    accepted_seen: Arc<AtomicBool>,
    result_ack_sent: Arc<AtomicBool>,
    empty_after_ack: Arc<AtomicBool>,
}

async fn ack_test_checkin(
    State(state): State<AckTestServer>,
    body: Bytes,
) -> Result<Bytes, StatusCode> {
    let wire = std::str::from_utf8(&body).map_err(|_| StatusCode::BAD_REQUEST)?;
    if Envelope::routing_id(wire).is_none() {
        let psk_key = crypto::derive_key(&state.psk, b"nw-m1-salt");
        let env = Envelope::open(&psk_key, wire).map_err(|_| StatusCode::UNAUTHORIZED)?;
        let plaintext = crypto::decrypt(&psk_key, env.id, &env.encrypted)
            .map_err(|_| StatusCode::UNAUTHORIZED)?;
        let register: Register =
            serde_json::from_slice(&plaintext).map_err(|_| StatusCode::BAD_REQUEST)?;
        let implant_pub = {
            use base64::Engine;
            let raw = base64::engine::general_purpose::STANDARD
                .decode(register.session_key)
                .map_err(|_| StatusCode::BAD_REQUEST)?;
            <[u8; 32]>::try_from(raw.as_slice()).map_err(|_| StatusCode::BAD_REQUEST)?
        };
        let server_keys = crypto::KeyPair::generate();
        let shared = server_keys
            .shared_secret(&implant_pub)
            .map_err(|_| StatusCode::UNAUTHORIZED)?;
        let session_key = crypto::derive_key(&shared, crypto::SESSION_SALT);
        let session_id = Uuid::new_v4();
        *state.session.lock().unwrap() = Some((session_id, session_key));
        let ack = RegisterAck {
            session_id,
            server_pub: {
                use base64::Engine;
                base64::engine::general_purpose::STANDARD.encode(server_keys.public_key())
            },
        };
        let reply_id = env.id + 1;
        let encrypted = crypto::encrypt(
            &psk_key,
            reply_id,
            &serde_json::to_vec(&ack).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let reply = Envelope::new(Kind::RegisterAck, reply_id, Some(session_id), encrypted)
            .seal(&psk_key)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        return Ok(Bytes::from(reply));
    }

    let (session_id, key) = state
        .session
        .lock()
        .unwrap()
        .as_ref()
        .copied()
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let env = Envelope::open(&key, wire).map_err(|_| StatusCode::UNAUTHORIZED)?;
    let plaintext =
        crypto::decrypt(&key, env.id, &env.encrypted).map_err(|_| StatusCode::UNAUTHORIZED)?;
    let request: PollRequest =
        serde_json::from_slice(&plaintext).map_err(|_| StatusCode::BAD_REQUEST)?;
    if request.accepted_task_ids.contains(&state.task.id) {
        state.accepted_seen.store(true, Ordering::SeqCst);
    }
    let mut result_acks = Vec::new();
    if request
        .results
        .iter()
        .any(|result| result.task_id == state.task.id)
    {
        let delivery = state.result_deliveries.fetch_add(1, Ordering::SeqCst) + 1;
        if delivery >= 2 {
            result_acks.push(state.task.id);
            state.result_ack_sent.store(true, Ordering::SeqCst);
        }
    } else if state.result_ack_sent.load(Ordering::SeqCst) {
        state.empty_after_ack.store(true, Ordering::SeqCst);
    }
    let poll = state.polls.fetch_add(1, Ordering::SeqCst) + 1;
    let tasks = if poll <= 2 {
        vec![state.task.clone()]
    } else {
        Vec::new()
    };
    let reply = PollReply {
        tasks,
        result_acks,
        acks: Vec::new(),
        push_chunks: Vec::new(),
    };
    let reply_id = env.id + 1;
    let encrypted = crypto::encrypt(
        &key,
        reply_id,
        &serde_json::to_vec(&reply).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    )
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let wire = Envelope::new(Kind::Heartbeat, reply_id, Some(session_id), encrypted)
        .seal(&key)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Bytes::from(wire))
}

#[tokio::test]
async fn duplicate_delivery_executes_once_and_results_wait_for_ack() {
    let port = free_port();
    let endpoint = format!("http://127.0.0.1:{port}");
    let side_effect = std::env::temp_dir().join(format!(
        "nw-ack-once-{}-{}",
        std::process::id(),
        Uuid::new_v4()
    ));
    let _ = std::fs::remove_file(&side_effect);
    let state = AckTestServer {
        psk: Arc::new(b"ack-e2e-psk".to_vec()),
        session: Arc::new(Mutex::new(None)),
        task: Task {
            id: Uuid::new_v4(),
            command: "sh".into(),
            args: vec![
                "-c".into(),
                format!("printf x >> '{}'", side_effect.display()),
            ],
            timeout_ms: 5_000,
        },
        polls: Arc::new(AtomicUsize::new(0)),
        result_deliveries: Arc::new(AtomicUsize::new(0)),
        accepted_seen: Arc::new(AtomicBool::new(false)),
        result_ack_sent: Arc::new(AtomicBool::new(false)),
        empty_after_ack: Arc::new(AtomicBool::new(false)),
    };
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .unwrap();
    let app = Router::new()
        .route("/c2/checkin", post(ack_test_checkin))
        .with_state(state.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let runtime = Arc::new(BeaconRuntime::new(
        Profile {
            endpoint,
            interval: Duration::from_millis(40),
            jitter: Duration::ZERO,
            hostname: "acklab".into(),
            username: "tester".into(),
            os: "test-os".into(),
            arch: "test-arch".into(),
            pid: 123,
            addr: "127.0.0.1".into(),
        },
        state.psk.as_ref().clone(),
    ));
    let beacon_runtime = runtime.clone();
    let beacon = tokio::spawn(async move { beacon_runtime.run().await });

    let mut waited = Duration::ZERO;
    while !state.empty_after_ack.load(Ordering::SeqCst) {
        assert!(
            waited < Duration::from_secs(10),
            "result was not retried and acknowledged"
        );
        tokio::time::sleep(Duration::from_millis(40)).await;
        waited += Duration::from_millis(40);
    }
    runtime.trigger_stop();
    let _ = beacon.await;
    server.abort();

    assert!(state.accepted_seen.load(Ordering::SeqCst));
    assert!(state.result_deliveries.load(Ordering::SeqCst) >= 2);
    assert_eq!(std::fs::read(&side_effect).unwrap(), b"x");
    std::fs::remove_file(side_effect).ok();
}

#[tokio::test]
async fn http_c2_round_trip() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .try_init();
    let psk: Vec<u8> = b"e2e-psk".to_vec();
    let port = free_port();
    let endpoint = format!("http://127.0.0.1:{}", port);

    let registry = Arc::new(SessionRegistry::new());
    let queue = Arc::new(TaskQueue::new());
    let state = ServerState {
        registry: registry.clone(),
        queue: queue.clone(),
        psk: Arc::new(psk.clone()),
        files: nw_server::filestore::FileStore::default(),
        uploads: nw_server::uploadstore::UploadStore::default(),
        creds: Arc::new(nw_server::creds::CredentialStore::new_in_memory()),
    };

    let bind = format!("127.0.0.1:{}", port);
    let server_handle = tokio::spawn(async move { server::serve_with_bind(state, &bind).await });

    // Give the listener a beat to bind.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let profile = Profile {
        endpoint: endpoint.clone(),
        interval: Duration::from_millis(100),
        jitter: Duration::ZERO,
        hostname: "testlab".into(),
        username: "tester".into(),
        os: "test-os".into(),
        arch: "test-arch".into(),
        pid: 1234,
        addr: "127.0.0.1".into(),
    };
    let runtime = Arc::new(BeaconRuntime::new(profile, psk.clone()));
    let implanted = runtime.clone();
    let beacon = tokio::spawn(async move { implanted.run().await });

    // The implant registers on its first tick; give it time, then queue a task.
    wait_for_session(&registry).await;
    let sid = registry.list().await[0].id;
    let task_id = queue
        .push(&sid, "printf".into(), vec!["roundtrip-ok".into()], 5000)
        .await
        .expect("queued");

    // Poll the queue until the result is delivered by the implant.
    let mut waited = Duration::ZERO;
    let result = loop {
        if let Some(r) = queue.take_result(&sid, &task_id).await {
            break r;
        }
        assert!(
            waited < Duration::from_secs(10),
            "timed out waiting for task result"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
        waited += Duration::from_millis(100);
    };

    assert!(
        result.ok,
        "stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&result.stdout), "roundtrip-ok");

    // Stop the implant, then drop the server.
    runtime.trigger_stop();
    let _ = beacon.await;
    server_handle.abort();
}

#[tokio::test]
async fn download_streams_a_remote_file_to_the_server() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .try_init();
    let psk: Vec<u8> = b"dl-e2e-psk".to_vec();
    let port = free_port();
    let endpoint = format!("http://127.0.0.1:{}", port);

    // A source file on the "implant" side.
    let fixture_dir = tempfile::tempdir().unwrap();
    let src = fixture_dir.path().join("nw-dl-src.dat");
    let payload: Vec<u8> = (0..=255).cycle().take(3072).collect();
    std::fs::write(&src, &payload).unwrap();

    // A dedicated downloads dir so we can assert the file landed.
    let dl_dir = fixture_dir.path().join("downloads");

    let registry = Arc::new(SessionRegistry::new());
    let queue = Arc::new(TaskQueue::new());
    let state = ServerState {
        registry: registry.clone(),
        queue: queue.clone(),
        psk: Arc::new(psk.clone()),
        files: nw_server::filestore::FileStore::new(dl_dir.clone()),
        uploads: nw_server::uploadstore::UploadStore::default(),
        creds: Arc::new(nw_server::creds::CredentialStore::new_in_memory()),
    };

    let dropped = Arc::new(AtomicBool::new(false));
    let drop_state = DropPartialTransferResponse {
        watched: dl_dir.clone(),
        expected: 3072,
        dropped: dropped.clone(),
    };
    let app = nw_server::channels::application(state).layer(axum::middleware::from_fn_with_state(
        drop_state,
        drop_partial_transfer_response,
    ));
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .unwrap();
    let server_handle = tokio::spawn(async move { axum::serve(listener, app).await });
    tokio::time::sleep(Duration::from_millis(200)).await;

    let profile = Profile {
        endpoint: endpoint.clone(),
        interval: Duration::from_millis(50),
        jitter: Duration::ZERO,
        hostname: "dllab".into(),
        username: "tester".into(),
        os: "test-os".into(),
        arch: "test-arch".into(),
        pid: 999,
        addr: "127.0.0.1".into(),
    };
    let runtime = Arc::new(BeaconRuntime::new(profile, psk.clone()));
    let implanted = runtime.clone();
    let beacon = tokio::spawn(async move { implanted.run().await });

    wait_for_session(&registry).await;
    let sid = registry.list().await[0].id;
    let task_id = queue
        .push(
            &sid,
            "nw/download".into(),
            vec![
                src.to_str().unwrap().to_string(),
                Uuid::new_v4().to_string(),
            ],
            60_000,
        )
        .await
        .expect("queued");

    let result = wait_for_result(&queue, &sid, &task_id).await;
    assert!(
        result.ok,
        "stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(String::from_utf8_lossy(&result.stdout).contains("downloaded"));

    // The server should have the full file under its downloads dir.
    let out_path = dl_dir.join(format!("{}-nw-dl-src.dat", sid));
    let written = std::fs::read(&out_path).unwrap_or_default();
    assert_eq!(written, payload, "downloaded bytes must match source");
    use sha2::Digest as _;
    assert_eq!(
        sha2::Sha256::digest(&written),
        sha2::Sha256::digest(&payload)
    );
    assert!(
        dropped.load(Ordering::SeqCst),
        "test must lose a mid-transfer response"
    );

    runtime.trigger_stop();
    let _ = beacon.await;
    server_handle.abort();
}

#[tokio::test]
async fn upload_streams_a_local_file_to_the_implant() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .try_init();
    let psk: Vec<u8> = b"up-e2e-psk".to_vec();
    let port = free_port();
    let endpoint = format!("http://127.0.0.1:{}", port);

    // A source file on the "server/operator" side to push out.
    let fixture_dir = tempfile::tempdir().unwrap();
    let src = fixture_dir.path().join("nw-up-src.dat");
    let payload: Vec<u8> = (0..=255).cycle().take(3072).collect();
    std::fs::write(&src, &payload).unwrap();

    let dest = fixture_dir.path().join("nw-up-dest.dat");

    let registry = Arc::new(SessionRegistry::new());
    let queue = Arc::new(TaskQueue::new());
    let uploads = nw_server::uploadstore::UploadStore::default();
    let state = ServerState {
        registry: registry.clone(),
        queue: queue.clone(),
        psk: Arc::new(psk.clone()),
        files: nw_server::filestore::FileStore::default(),
        uploads: uploads.clone(),
        creds: Arc::new(nw_server::creds::CredentialStore::new_in_memory()),
    };

    let dropped = Arc::new(AtomicBool::new(false));
    let drop_state = DropPartialTransferResponse {
        watched: dest.clone(),
        expected: 3072,
        dropped: dropped.clone(),
    };
    let app = nw_server::channels::application(state).layer(axum::middleware::from_fn_with_state(
        drop_state,
        drop_partial_transfer_response,
    ));
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .unwrap();
    let server_handle = tokio::spawn(async move { axum::serve(listener, app).await });
    tokio::time::sleep(Duration::from_millis(200)).await;

    // The operator uses a Dispatch over the SAME queue/uploads so the upload job
    // lands where the server's poll handler reads it.
    let sys_op = nw_server::operators::Operator {
        id: uuid::Uuid::nil(),
        username: "system".into(),
        role: nw_server::operators::Role::Admin,
    };
    let dispatcher = nw_server::Dispatcher::new(
        registry.clone(),
        queue.clone(),
        uploads,
        Arc::new(nw_server::creds::CredentialStore::new_in_memory()),
        sys_op,
    );

    let profile = Profile {
        endpoint: endpoint.clone(),
        interval: Duration::from_millis(50),
        jitter: Duration::ZERO,
        hostname: "uplab".into(),
        username: "tester".into(),
        os: "test-os".into(),
        arch: "test-arch".into(),
        pid: 888,
        addr: "127.0.0.1".into(),
    };
    let runtime = Arc::new(BeaconRuntime::new(profile, psk.clone()));
    let implanted = runtime.clone();
    let beacon = tokio::spawn(async move { implanted.run().await });

    wait_for_session(&registry).await;
    let sid = registry.list().await[0].id;
    dispatcher.set_interacted(Some(sid));
    let queued = match dispatcher
        .parse(&format!(
            "upload {} {}",
            src.to_str().unwrap(),
            dest.to_str().unwrap()
        ))
        .await
    {
        nw_server::dispatch::Outcome::TaskQueued { task_id, .. } => task_id,
        other => panic!("upload not queued: {other:?}"),
    };

    let result = wait_for_result(queue.as_ref(), &sid, &queued).await;
    assert!(
        result.ok,
        "stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(String::from_utf8_lossy(&result.stdout).contains("uploaded"));

    let written = std::fs::read(&dest).unwrap_or_default();
    assert_eq!(
        written, payload,
        "implant-side file must match the pushed source"
    );
    use sha2::Digest as _;
    assert_eq!(
        sha2::Sha256::digest(&written),
        sha2::Sha256::digest(&payload)
    );
    assert!(
        dropped.load(Ordering::SeqCst),
        "test must lose a mid-transfer response"
    );

    runtime.trigger_stop();
    let _ = beacon.await;
    server_handle.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn socks5_proxy_relays_traffic_through_the_implant() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .try_init();
    let psk: Vec<u8> = b"socks-e2e-psk".to_vec();
    let c2_port = free_port();
    let endpoint = format!("http://127.0.0.1:{}", c2_port);

    // A TCP echo target "on the internet" the operator reaches via the proxy.
    let echo = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let echo_port = echo.local_addr().unwrap().port();
    let echo_task = tokio::spawn(async move {
        let (mut c, _) = echo.accept().await.unwrap();
        let mut b = [0u8; 64];
        let n = c.read(&mut b).await.unwrap();
        c.write_all(&b[..n]).await.unwrap();
    });

    let registry = Arc::new(SessionRegistry::new());
    let queue = Arc::new(TaskQueue::new());
    let state = ServerState {
        registry: registry.clone(),
        queue: queue.clone(),
        psk: Arc::new(psk.clone()),
        files: nw_server::filestore::FileStore::default(),
        uploads: nw_server::uploadstore::UploadStore::default(),
        creds: Arc::new(nw_server::creds::CredentialStore::new_in_memory()),
    };
    let bind = format!("127.0.0.1:{}", c2_port);
    let server_handle = tokio::spawn(async move { server::serve_with_bind(state, &bind).await });
    tokio::time::sleep(Duration::from_millis(200)).await;

    let profile = Profile {
        endpoint: endpoint.clone(),
        interval: Duration::from_millis(50),
        jitter: Duration::ZERO,
        hostname: "sockslab".into(),
        username: "tester".into(),
        os: "test-os".into(),
        arch: "test-arch".into(),
        pid: 777,
        addr: "127.0.0.1".into(),
    };
    let runtime = Arc::new(BeaconRuntime::new(profile, psk.clone()));
    let implanted = runtime.clone();
    let beacon = tokio::spawn(async move { implanted.run().await });

    wait_for_session(&registry).await;
    let sid = registry.list().await[0].id;

    let socks_port = free_port();
    let task_id = queue
        .push(
            &sid,
            "nw/socks".into(),
            vec![socks_port.to_string()],
            10_000,
        )
        .await
        .expect("queued socks");
    let result = wait_for_result(&queue, &sid, &task_id).await;
    assert!(
        result.ok,
        "stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );

    // Drive the implant's SOCKS5 proxy like a real client: no-auth handshake,
    // then CONNECT to the echo target, relay one message.
    let mut client = std::net::TcpStream::connect(("127.0.0.1", socks_port)).unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    client
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    client.write_all(&[0x05, 0x01, 0x00]).unwrap();
    let mut h = [0u8; 2];
    client.read_exact(&mut h).unwrap();
    assert_eq!(h, [0x05, 0x00]);
    client
        .write_all(&[
            0x05,
            0x01,
            0x00,
            0x01,
            127,
            0,
            0,
            1,
            (echo_port >> 8) as u8,
            (echo_port & 0xff) as u8,
        ])
        .unwrap();
    let mut ack = [0u8; 10];
    client.read_exact(&mut ack).unwrap();
    assert_eq!(ack[1], 0x00, "CONNECT must succeed");
    client.write_all(b"via-socks").unwrap();
    let mut echoed = [0u8; 9];
    client.read_exact(&mut echoed).unwrap();
    assert_eq!(&echoed, b"via-socks");

    runtime.trigger_stop();
    let _ = beacon.await;
    server_handle.abort();
    echo_task.abort();
}

#[tokio::test]
async fn hashes_returns_sha256_of_a_remote_file() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .try_init();
    let psk: Vec<u8> = b"hashes-e2e-psk".to_vec();
    let c2_port = free_port();
    let endpoint = format!("http://127.0.0.1:{}", c2_port);

    let registry = Arc::new(SessionRegistry::new());
    let queue = Arc::new(TaskQueue::new());
    let state = ServerState {
        registry: registry.clone(),
        queue: queue.clone(),
        psk: Arc::new(psk.clone()),
        files: nw_server::filestore::FileStore::default(),
        uploads: nw_server::uploadstore::UploadStore::default(),
        creds: Arc::new(nw_server::creds::CredentialStore::new_in_memory()),
    };
    let bind = format!("127.0.0.1:{}", c2_port);
    let server_handle = tokio::spawn(async move { server::serve_with_bind(state, &bind).await });
    tokio::time::sleep(Duration::from_millis(200)).await;

    let profile = Profile {
        endpoint: endpoint.clone(),
        interval: Duration::from_millis(50),
        jitter: Duration::ZERO,
        hostname: "hashlab".into(),
        username: "tester".into(),
        os: "test-os".into(),
        arch: "test-arch".into(),
        pid: 555,
        addr: "127.0.0.1".into(),
    };
    let runtime = Arc::new(BeaconRuntime::new(profile, psk.clone()));
    let implanted = runtime.clone();
    let beacon = tokio::spawn(async move { implanted.run().await });

    wait_for_session(&registry).await;
    let sid = registry.list().await[0].id;

    // A file on the "implant" (same process in this e2e) with known bytes.
    let data = b"hello from naughtywolf hashes test\n";
    let dir = std::env::temp_dir().join(format!("nw-hashes-{}", std::process::id()));
    std::fs::create_dir_all(&dir).ok();
    let path = dir.join("target.bin");
    std::fs::write(&path, data).unwrap();

    let task_id = queue
        .push(
            &sid,
            "nw/hashes".into(),
            vec![path.to_str().unwrap().into()],
            30_000,
        )
        .await
        .expect("queued hashes");
    let result = wait_for_result(&queue, &sid, &task_id).await;
    assert!(
        result.ok,
        "stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );

    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(data);
    let expect_hex: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.starts_with(&expect_hex), "stdout was: {stdout}");

    runtime.trigger_stop();
    let _ = beacon.await;
    server_handle.abort();
    std::fs::remove_file(&path).ok();
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn script_queues_task_sequence_to_a_session() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .try_init();
    let psk: Vec<u8> = b"script-e2e-psk".to_vec();
    let port = free_port();
    let endpoint = format!("http://127.0.0.1:{}", port);

    let registry = Arc::new(SessionRegistry::new());
    let queue = Arc::new(TaskQueue::new());
    let uploads = nw_server::uploadstore::UploadStore::default();
    let state = ServerState {
        registry: registry.clone(),
        queue: queue.clone(),
        psk: Arc::new(psk.clone()),
        files: nw_server::filestore::FileStore::default(),
        uploads: uploads.clone(),
        creds: Arc::new(nw_server::creds::CredentialStore::new_in_memory()),
    };

    let bind = format!("127.0.0.1:{}", port);
    let server_handle = tokio::spawn(async move { server::serve_with_bind(state, &bind).await });
    tokio::time::sleep(Duration::from_millis(200)).await;

    let profile = Profile {
        endpoint: endpoint.clone(),
        interval: Duration::from_millis(50),
        jitter: Duration::ZERO,
        hostname: "scriptlab".into(),
        username: "tester".into(),
        os: "test-os".into(),
        arch: "test-arch".into(),
        pid: 444,
        addr: "127.0.0.1".into(),
    };
    let runtime = Arc::new(BeaconRuntime::new(profile, psk.clone()));
    let implanted = runtime.clone();
    let beacon = tokio::spawn(async move { implanted.run().await });

    wait_for_session(&registry).await;
    let sid = registry.list().await[0].id;

    // Write a script file. Also create a temp file for the hashes step.
    let hash_src = std::env::temp_dir().join(format!("nw-script-hash-{}.bin", std::process::id()));
    std::fs::write(&hash_src, b"test-data-for-hashes").unwrap();

    let script_text = r#"
        script: recon-chain
        step recon {
            cmd: printf
            args: recon-done
        }
        step check {
            cmd: nw/hashes
            args: {{HASH_PATH}}
            depends_on: [recon]
            require: recon-done
        }
    "#
    .replace("{{HASH_PATH}}", hash_src.to_str().unwrap());
    let script_path = std::env::temp_dir().join(format!("nw-script-{}.txt", std::process::id()));
    std::fs::write(&script_path, script_text).unwrap();

    let sys_op = nw_server::operators::Operator {
        id: uuid::Uuid::nil(),
        username: "system".into(),
        role: nw_server::operators::Role::Admin,
    };
    let dispatcher = nw_server::Dispatcher::new(
        registry.clone(),
        queue.clone(),
        uploads,
        Arc::new(nw_server::creds::CredentialStore::new_in_memory()),
        sys_op,
    );
    dispatcher.set_interacted(Some(sid));

    let outcome = dispatcher
        .parse(&format!("script {}", script_path.to_str().unwrap()))
        .await;
    match &outcome {
        nw_server::dispatch::Outcome::Local { message } => {
            assert!(
                message.contains("queued 2 step(s)"),
                "message was: {message}"
            );
        }
        other => panic!("script command should return Local, got: {other:?}"),
    }

    // Both tasks should be queued on the session.
    let statuses = queue.statuses(&sid).await;
    assert_eq!(statuses.len(), 2, "expected 2 queued tasks");

    // Wait for task results.
    let mut waited = Duration::ZERO;
    loop {
        let results = queue.results(&sid).await;
        if results.len() >= 2 {
            break;
        }
        assert!(
            waited < Duration::from_secs(10),
            "timed out waiting for script results"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
        waited += Duration::from_millis(100);
    }

    let results = queue.results(&sid).await;
    for r in &results {
        assert!(
            r.ok,
            "task result failed: {}",
            String::from_utf8_lossy(&r.stderr)
        );
    }

    runtime.trigger_stop();
    let _ = beacon.await;
    server_handle.abort();
    std::fs::remove_file(&script_path).ok();
    std::fs::remove_file(&hash_src).ok();
}

#[tokio::test]
async fn redirect_delivers_its_result_to_the_second_listener() {
    let psk: Vec<u8> = b"redirect-e2e-psk".to_vec();
    let first_port = free_port();
    let second_port = free_port();
    let first_endpoint = format!("http://127.0.0.1:{}", first_port);
    let second_endpoint = format!("http://127.0.0.1:{}", second_port);

    let registry = Arc::new(SessionRegistry::new());
    let queue = Arc::new(TaskQueue::new());
    let first = FirstListenerState {
        total: Arc::new(AtomicUsize::new(0)),
        polls: Arc::new(AtomicUsize::new(0)),
        register_ready: Arc::new(tokio::sync::Notify::new()),
        release_register: Arc::new(tokio::sync::Notify::new()),
    };
    let first_app =
        nw_server::channels::router(registry.clone(), queue.clone(), psk.clone()).layer(
            axum::middleware::from_fn_with_state(first.clone(), reject_followup_polls),
        );
    let first_bind = format!("127.0.0.1:{}", first_port);
    let first_listener = tokio::net::TcpListener::bind(&first_bind).await.unwrap();
    let first_handle = tokio::spawn(async move { axum::serve(first_listener, first_app).await });

    let second_state = ServerState {
        registry: registry.clone(),
        queue: queue.clone(),
        psk: Arc::new(psk.clone()),
        files: nw_server::filestore::FileStore::default(),
        uploads: nw_server::uploadstore::UploadStore::default(),
        creds: Arc::new(nw_server::creds::CredentialStore::new_in_memory()),
    };
    let second_bind = format!("127.0.0.1:{}", second_port);
    let second_handle =
        tokio::spawn(async move { server::serve_with_bind(second_state, &second_bind).await });

    let profile = Profile {
        endpoint: first_endpoint,
        interval: Duration::from_millis(50),
        jitter: Duration::ZERO,
        hostname: "redirect-testlab".into(),
        username: "tester".into(),
        os: "test-os".into(),
        arch: "test-arch".into(),
        pid: 4321,
        addr: "127.0.0.1".into(),
    };
    let runtime = Arc::new(BeaconRuntime::new(profile, psk));
    let implanted = runtime.clone();
    let beacon = tokio::spawn(async move { implanted.run().await });

    tokio::time::timeout(Duration::from_secs(3), first.register_ready.notified())
        .await
        .expect("register did not reach the first listener");
    let sid = registry.list().await[0].id;
    let task_id = queue
        .push(
            &sid,
            "nw/sethost".into(),
            vec![second_endpoint.clone()],
            5000,
        )
        .await
        .expect("queued redirect");
    first.release_register.notify_one();

    let result = wait_for_result(&queue, &sid, &task_id).await;
    assert!(
        result.ok,
        "stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(String::from_utf8_lossy(&result.stdout).contains(&second_endpoint));
    assert_eq!(first.polls.load(Ordering::SeqCst), 1);

    runtime.trigger_stop();
    let _ = beacon.await;
    first_handle.abort();
    second_handle.abort();
}

#[derive(Clone)]
struct FirstListenerState {
    total: Arc<AtomicUsize>,
    polls: Arc<AtomicUsize>,
    register_ready: Arc<tokio::sync::Notify>,
    release_register: Arc<tokio::sync::Notify>,
}

async fn reject_followup_polls(
    State(first): State<FirstListenerState>,
    request: Request,
    next: Next,
) -> Response {
    if request.uri().path() == "/c2/checkin" {
        // First checkin on the listener is the register; everything after is a
        // poll. Reject the 2nd+ poll so the implant's redirect lands on the
        // second listener.
        let first_here = first.total.fetch_add(1, Ordering::SeqCst) == 0;
        if first_here {
            let resp = next.run(request).await;
            first.register_ready.notify_one();
            first.release_register.notified().await;
            return resp;
        } else if first.polls.fetch_add(1, Ordering::SeqCst) > 0 {
            return axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
    }
    next.run(request).await
}

async fn wait_for_session(registry: &SessionRegistry) {
    let mut waited = Duration::ZERO;
    while registry.list().await.is_empty() {
        assert!(waited < Duration::from_secs(10), "implant never registered");
        tokio::time::sleep(Duration::from_millis(100)).await;
        waited += Duration::from_millis(100);
    }
}

async fn wait_for_result(
    queue: &TaskQueue,
    sid: &uuid::Uuid,
    task_id: &uuid::Uuid,
) -> nw_profile::msgs::TaskResult {
    let mut waited = Duration::ZERO;
    loop {
        if let Some(result) = queue.take_result(sid, task_id).await {
            return result;
        }
        assert!(
            waited < Duration::from_secs(3),
            "timed out waiting for task result"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
        waited += Duration::from_millis(50);
    }
}

#[tokio::test]
async fn raw_tcp_c2_round_trip() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .try_init();
    let psk: Vec<u8> = b"tcp-e2e-psk".to_vec();
    let port = free_port();
    let endpoint = format!("tcp://127.0.0.1:{}", port);

    let registry = Arc::new(SessionRegistry::new());
    let queue = Arc::new(TaskQueue::new());
    let state = ServerState {
        registry: registry.clone(),
        queue: queue.clone(),
        psk: Arc::new(psk.clone()),
        files: nw_server::filestore::FileStore::default(),
        uploads: nw_server::uploadstore::UploadStore::default(),
        creds: Arc::new(nw_server::creds::CredentialStore::new_in_memory()),
    };

    let bind = format!("127.0.0.1:{}", port);
    let server_handle = tokio::spawn(async move { nw_server::serve_tcp(state, bind).await });

    tokio::time::sleep(Duration::from_millis(200)).await;

    let profile = Profile {
        endpoint: endpoint.clone(),
        interval: Duration::from_millis(100),
        jitter: Duration::ZERO,
        hostname: "tcplab".into(),
        username: "tester".into(),
        os: "test-os".into(),
        arch: "test-arch".into(),
        pid: 777,
        addr: "127.0.0.1".into(),
    };
    let runtime = Arc::new(BeaconRuntime::new(profile, psk.clone()));
    let implanted = runtime.clone();
    let beacon = tokio::spawn(async move { implanted.run().await });

    wait_for_session(&registry).await;
    let sid = registry.list().await[0].id;
    let task_id = queue
        .push(&sid, "printf".into(), vec!["tcp-roundtrip-ok".into()], 5000)
        .await
        .expect("queued");

    let result = wait_for_result(&queue, &sid, &task_id).await;
    assert!(
        result.ok,
        "stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&result.stdout), "tcp-roundtrip-ok");

    runtime.trigger_stop();
    let _ = beacon.await;
    server_handle.abort();
}

#[tokio::test]
async fn raw_dns_c2_round_trip() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .try_init();
    let psk: Vec<u8> = b"dns-e2e-psk".to_vec();
    let port = free_port();
    let endpoint = format!("dns://127.0.0.1:{}", port);

    let registry = Arc::new(SessionRegistry::new());
    let queue = Arc::new(TaskQueue::new());
    let bind = format!("127.0.0.1:{}", port);
    let server_handle = tokio::spawn({
        let registry = registry.clone();
        let queue = queue.clone();
        let psk = psk.clone();
        async move { nw_server::serve_dns(registry, queue, psk, bind).await }
    });

    tokio::time::sleep(Duration::from_millis(200)).await;

    let profile = Profile {
        endpoint: endpoint.clone(),
        interval: Duration::from_millis(100),
        jitter: Duration::ZERO,
        hostname: "dnslab".into(),
        username: "tester".into(),
        os: "test-os".into(),
        arch: "test-arch".into(),
        pid: 888,
        addr: "127.0.0.1".into(),
    };
    let runtime = Arc::new(BeaconRuntime::new(profile, psk));
    let implanted = runtime.clone();
    let beacon = tokio::spawn(async move { implanted.run().await });

    wait_for_session(&registry).await;
    let sid = registry.list().await[0].id;
    let task_id = queue
        .push(&sid, "printf".into(), vec!["dns-roundtrip-ok".into()], 5000)
        .await
        .expect("queued");

    let result = wait_for_result(&queue, &sid, &task_id).await;
    assert!(
        result.ok,
        "stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&result.stdout), "dns-roundtrip-ok");

    runtime.trigger_stop();
    let _ = beacon.await;
    server_handle.abort();
}

#[tokio::test]
async fn killjob_cancels_an_in_flight_task() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .try_init();
    let psk: Vec<u8> = b"kill-e2e-psk".to_vec();
    let port = free_port();
    let endpoint = format!("http://127.0.0.1:{}", port);

    let registry = Arc::new(SessionRegistry::new());
    let queue = Arc::new(TaskQueue::new());
    let state = ServerState {
        registry: registry.clone(),
        queue: queue.clone(),
        psk: Arc::new(psk.clone()),
        files: nw_server::filestore::FileStore::default(),
        uploads: nw_server::uploadstore::UploadStore::default(),
        creds: Arc::new(nw_server::creds::CredentialStore::new_in_memory()),
    };

    let bind = format!("127.0.0.1:{}", port);
    let server_handle = tokio::spawn(async move { server::serve_with_bind(state, &bind).await });
    tokio::time::sleep(Duration::from_millis(200)).await;

    let profile = Profile {
        endpoint: endpoint.clone(),
        interval: Duration::from_millis(100),
        jitter: Duration::ZERO,
        hostname: "killlab".into(),
        username: "tester".into(),
        os: "test-os".into(),
        arch: "test-arch".into(),
        pid: 999,
        addr: "127.0.0.1".into(),
    };
    let runtime = Arc::new(BeaconRuntime::new(profile, psk));
    let implanted = runtime.clone();
    let beacon = tokio::spawn(async move { implanted.run().await });

    wait_for_session(&registry).await;
    let sid = registry.list().await[0].id;

    // Queue a long-running task; once Delivered it is in-flight on the implant.
    let victim = queue
        .push(&sid, "sleep".into(), vec!["30".into()], 60_000)
        .await
        .expect("queued");
    let mut waited = Duration::ZERO;
    while queue.status(&sid, &victim).await != Some(nw_server::queue::TaskStatus::Delivered) {
        assert!(
            waited < Duration::from_secs(10),
            "long task never delivered"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
        waited += Duration::from_millis(50);
    }

    // Queue the kill; expect a result reporting the child was terminated.
    let kill_tid = queue
        .push(&sid, "nw/killtask".into(), vec![victim.to_string()], 10_000)
        .await
        .expect("queued");
    let result = wait_for_result(&queue, &sid, &kill_tid).await;
    eprintln!(
        "kill result: ok={} stdout={:?} stderr={:?}",
        result.ok,
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        result.ok,
        "stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );

    runtime.trigger_stop();
    let _ = beacon.await;
    server_handle.abort();
}

#[tokio::test]
async fn m2_auth_audit_and_restart_persistence() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("m2.sqlite");
    let pool = nw_server::persist::open_pool(db.to_str().unwrap())
        .await
        .unwrap();
    let registry = Arc::new(SessionRegistry::with_sqlite(pool.clone()));
    let queue = Arc::new(TaskQueue::with_sqlite(pool.clone()));
    let operators = nw_server::operators::OperatorStore::new(pool.clone());
    operators
        .create("admin", "admin-password", nw_server::operators::Role::Admin)
        .await
        .unwrap();
    operators
        .create(
            "viewer",
            "viewer-password",
            nw_server::operators::Role::Viewer,
        )
        .await
        .unwrap();
    let admin = operators
        .authenticate("admin", "admin-password")
        .await
        .unwrap();
    let viewer = operators
        .authenticate("viewer", "viewer-password")
        .await
        .unwrap();
    let audit = Arc::new(nw_server::audit_log::AuditLog::new(pool.clone()));

    let psk = b"m2-e2e-psk".to_vec();
    let port = free_port();
    let endpoint = format!("http://127.0.0.1:{port}");
    let state = ServerState {
        registry: registry.clone(),
        queue: queue.clone(),
        psk: Arc::new(psk.clone()),
        files: nw_server::filestore::FileStore::default(),
        uploads: nw_server::uploadstore::UploadStore::default(),
        creds: Arc::new(nw_server::creds::CredentialStore::new_in_memory()),
    };
    let bind = format!("127.0.0.1:{port}");
    let server_handle = tokio::spawn(async move { server::serve_with_bind(state, &bind).await });
    tokio::time::sleep(Duration::from_millis(100)).await;

    let profile = Profile {
        endpoint,
        interval: Duration::from_millis(50),
        jitter: Duration::from_millis(10),
        hostname: "m2-host".into(),
        username: "tester".into(),
        os: "test-os".into(),
        arch: "test-arch".into(),
        pid: 2026,
        addr: "127.0.0.1".into(),
    };
    let runtime = Arc::new(BeaconRuntime::new(profile, psk));
    let implanted = runtime.clone();
    let beacon = tokio::spawn(async move { implanted.run().await });

    wait_for_session(&registry).await;
    let sid = registry.list().await[0].id;

    let viewer_dispatcher = nw_server::Dispatcher::new(
        registry.clone(),
        queue.clone(),
        nw_server::uploadstore::UploadStore::default(),
        Arc::new(nw_server::creds::CredentialStore::new_in_memory()),
        viewer,
    )
    .with_audit(audit.clone());
    viewer_dispatcher.set_interacted(Some(sid));
    assert!(matches!(
        viewer_dispatcher.parse("shell printf forbidden").await,
        nw_server::Outcome::Error(_)
    ));

    let admin_dispatcher = nw_server::Dispatcher::new(
        registry.clone(),
        queue.clone(),
        nw_server::uploadstore::UploadStore::default(),
        Arc::new(nw_server::creds::CredentialStore::new_in_memory()),
        admin,
    )
    .with_audit(audit.clone());
    assert!(matches!(
        admin_dispatcher.parse(&format!("interact {sid}")).await,
        nw_server::Outcome::Local { .. }
    ));
    let task_id = match admin_dispatcher.parse("shell printf m2-complete").await {
        nw_server::Outcome::TaskQueued { task_id, role, .. } => {
            assert_eq!(role, nw_server::operators::Role::Admin);
            task_id
        }
        other => panic!("shell task was not queued: {other:?}"),
    };

    let mut waited = Duration::ZERO;
    while queue.status(&sid, &task_id).await != Some(nw_server::queue::TaskStatus::Completed) {
        assert!(waited < Duration::from_secs(10), "M2 task never completed");
        tokio::time::sleep(Duration::from_millis(50)).await;
        waited += Duration::from_millis(50);
    }

    runtime.trigger_stop();
    let _ = beacon.await;
    server_handle.abort();
    let _ = server_handle.await;
    drop(viewer_dispatcher);
    drop(admin_dispatcher);
    drop(audit);
    drop(operators);
    drop(queue);
    drop(registry);
    pool.close().await;

    let restarted_pool = nw_server::persist::open_pool(db.to_str().unwrap())
        .await
        .unwrap();
    let restarted_registry = SessionRegistry::with_sqlite(restarted_pool.clone());
    let restarted_queue = TaskQueue::with_sqlite(restarted_pool.clone());
    let restarted_operators = nw_server::operators::OperatorStore::new(restarted_pool.clone());
    let restarted_audit = nw_server::audit_log::AuditLog::new(restarted_pool);

    assert_eq!(
        restarted_registry.get(&sid).await.unwrap().hostname,
        "m2-host"
    );
    assert_eq!(
        restarted_queue.status(&sid, &task_id).await,
        Some(nw_server::queue::TaskStatus::Completed)
    );
    assert_eq!(
        restarted_queue
            .take_result(&sid, &task_id)
            .await
            .unwrap()
            .stdout,
        b"m2-complete"
    );
    assert!(
        restarted_operators
            .authenticate("admin", "admin-password")
            .await
            .is_some()
    );
    let entries = restarted_audit.list(20).await;
    assert!(
        entries
            .iter()
            .any(|entry| entry.action == "task.shell" && entry.succeeded)
    );
    assert!(
        entries
            .iter()
            .any(|entry| entry.action == "task.shell" && !entry.succeeded)
    );
}
