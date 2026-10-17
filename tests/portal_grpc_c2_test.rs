//! The portal's own gRPC C2 carrier, driven by the real implant gRPC client.
//!
//! The heavy `payload_run_smoke_test` proves a built binary checks in, but it
//! is `#[ignore]`d because it spawns a toolchain build. These tests cover the
//! same portal gRPC path in the default suite: real tonic client, real tonic
//! server, real sealed-envelope handshake, no child processes.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use naughtywolf::db;
use naughtywolf::db::repositories::Repository;
use nw_profile::crypto;
use nw_profile::envelope::{Envelope, Kind};
use nw_profile::msgs::{PollReply, Register, RegisterAck};

fn b64(b: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(b)
}

/// The PSK-derived check-in key. Must match the portal's derivation exactly;
/// a plain hash here authenticates nothing and fails as `Unauthorized`.
fn session_key(psk: &[u8]) -> [u8; crypto::KEY_LEN] {
    crypto::derive_key(psk, b"nw-m1-salt")
}

async fn spawn_grpc_c2(
    repository: Repository,
    psk: Arc<Vec<u8>>,
) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = probe.local_addr().unwrap();
    drop(probe);
    let handle = tokio::spawn(async move {
        let _ = naughtywolf::c2_grpc::serve(repository, psk, "grpc".to_owned(), addr).await;
    });
    // Give the listener a beat to bind before the client dials.
    tokio::time::sleep(Duration::from_millis(250)).await;
    (addr, handle)
}

/// Register a beacon over the portal's gRPC C2 using the real implant client
/// transport, then assert the session landed in the database and is labelled
/// with the gRPC protocol.
#[tokio::test]
async fn implant_grpc_client_registers_against_the_portal_grpc_c2() {
    let pool = db::create_pool("sqlite::memory:").await.unwrap();
    db::run_migrations(&pool).await.unwrap();
    let repository = Repository { pool: pool.clone() };
    let psk: Vec<u8> = b"portal-grpc-test-psk".to_vec();
    let psk_key = session_key(&psk);

    let (addr, server) = spawn_grpc_c2(repository.clone(), Arc::new(psk.clone())).await;

    // The implant transport speaks its own scheme; it rewrites it for tonic.
    let endpoint = format!("http://{addr}");
    let cache = tokio::sync::Mutex::new(None);
    let exchange = |sealed: Vec<u8>| {
        let endpoint = endpoint.clone();
        let cache = &cache;
        async move { nw_implant::grpc_client::exchange(&endpoint, cache, &sealed).await }
    };

    // Register.
    let implant_kp = crypto::KeyPair::generate();
    let register = Register {
        hostname: "grpc-lab".into(),
        username: "operator".into(),
        os: "linux".into(),
        arch: "x86_64".into(),
        pid: 4242,
        addr: "127.0.0.1".into(),
        os_version: None,
        executable_path: None,
        local_addr: None,
        implant_version: None,
        interval_ms: None,
        jitter_ms: None,
        capabilities: None,
        session_key: b64(&implant_kp.public_key()),
    };
    let ct = crypto::encrypt(&psk_key, 1, &serde_json::to_vec(&register).unwrap()).unwrap();
    let wire = Envelope::new(Kind::Register, 1, None, ct)
        .seal(&psk_key)
        .unwrap()
        .into_bytes();
    let ack_wire = exchange(wire).await.expect("register over gRPC");
    let ack_env = Envelope::open(&psk_key, std::str::from_utf8(&ack_wire).unwrap()).unwrap();
    let ack: RegisterAck =
        serde_json::from_slice(&crypto::decrypt(&psk_key, ack_env.id, &ack_env.encrypted).unwrap())
            .unwrap();

    // The session exists and is attributed to gRPC, not HTTP.
    let protocol: String = sqlx::query_scalar("SELECT protocol FROM callbacks WHERE id = ?")
        .bind(ack.session_id.to_string())
        .fetch_one(&pool)
        .await
        .expect("gRPC registration created a callback");
    assert_eq!(protocol, "grpc");

    // Poll under the derived session key and confirm the channel works for
    // authenticated (non-register) frames too.
    use base64::Engine;
    let server_pub = base64::engine::general_purpose::STANDARD
        .decode(&ack.server_pub)
        .unwrap();
    let shared = implant_kp.shared_secret(&server_pub).unwrap();
    let key = crypto::derive_key(&shared, crypto::SESSION_SALT);

    let poll_ct = crypto::encrypt(
        &key,
        2,
        &serde_json::to_vec(&nw_profile::msgs::PollRequest::default()).unwrap(),
    )
    .unwrap();
    let poll_wire = Envelope::new(Kind::TaskResult, 2, Some(ack.session_id), poll_ct)
        .seal(&key)
        .unwrap()
        .into_bytes();
    let poll_reply_wire = exchange(poll_wire).await.expect("poll over gRPC");
    let poll_env = Envelope::open(&key, std::str::from_utf8(&poll_reply_wire).unwrap()).unwrap();
    let reply: PollReply =
        serde_json::from_slice(&crypto::decrypt(&key, poll_env.id, &poll_env.encrypted).unwrap())
            .unwrap();
    assert!(reply.tasks.is_empty(), "no task queued yet");

    server.abort();
}

/// A frame that is not a valid envelope must be rejected over the same carrier
/// without tearing the listener down, so one bad check-in cannot kill the
/// channel for other beacons.
#[tokio::test]
async fn portal_grpc_rejects_a_malformed_frame_without_dying() {
    let pool = db::create_pool("sqlite::memory:").await.unwrap();
    db::run_migrations(&pool).await.unwrap();
    let repository = Repository { pool: pool.clone() };
    let psk: Vec<u8> = b"portal-grpc-bad-psk".to_vec();

    let (addr, server) = spawn_grpc_c2(repository.clone(), Arc::new(psk.clone())).await;
    let cache = tokio::sync::Mutex::new(None);
    let endpoint = format!("http://{addr}");

    let err = nw_implant::grpc_client::exchange(&endpoint, &cache, &[0xff, 0xfe])
        .await
        .expect_err("invalid utf-8 frame must be rejected");
    assert!(err.contains("grpc checkin failed"), "{err}");

    // The listener still serves a good frame afterwards.
    let psk_key = session_key(&psk);
    let implant_kp = crypto::KeyPair::generate();
    let register = Register {
        hostname: "still-alive".into(),
        username: "operator".into(),
        os: "linux".into(),
        arch: "x86_64".into(),
        pid: 1,
        addr: "127.0.0.1".into(),
        os_version: None,
        executable_path: None,
        local_addr: None,
        implant_version: None,
        interval_ms: None,
        jitter_ms: None,
        capabilities: None,
        session_key: b64(&implant_kp.public_key()),
    };
    let ct = crypto::encrypt(&psk_key, 1, &serde_json::to_vec(&register).unwrap()).unwrap();
    let wire = Envelope::new(Kind::Register, 1, None, ct)
        .seal(&psk_key)
        .unwrap()
        .into_bytes();
    nw_implant::grpc_client::exchange(&endpoint, &cache, &wire)
        .await
        .expect("listener survives a malformed frame");

    server.abort();
}
