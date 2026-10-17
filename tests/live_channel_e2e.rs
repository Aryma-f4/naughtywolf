//! End-to-end proof of the live channel: the real implant against the real
//! portal over a WebSocket, with a beacon interval long enough that nothing here
//! can be passing because a poll happened to arrive.
//!
//! What each assertion establishes:
//!   * the callback's socket is registered as a live channel;
//!   * a shell pushed over that socket answers a command, with no beacon in
//!     between and no task ever entering the queue;
//!   * a task pushed over the same socket is stopped by a kill pushed after it,
//!     and the callback confirms the stop.

use std::time::Duration;

use base64::Engine;
use naughtywolf::{c2_live::LiveRegistry, db::repositories::Repository, shell_hub::ShellHub};
use nw_implant::runtime::BeaconRuntime;
use nw_profile::config::GSocketConfig;
use nw_profile::msgs::Live;
use uuid::Uuid;

/// No beacon in this test is anywhere near fast enough to explain a result.
const BEACON_INTERVAL: Duration = Duration::from_secs(120);
/// Generous, so a slow machine fails on behaviour rather than on timing.
const PATIENCE: Duration = Duration::from_secs(30);

struct Harness {
    repository: Repository,
    live: LiveRegistry,
    hub: ShellHub,
    address: std::net::SocketAddr,
    server: tokio::task::JoinHandle<()>,
}

fn init_tracing() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let filter = tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,nw_implant=debug"));
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_test_writer()
            .try_init();
    });
}

async fn harness(psk: Vec<u8>) -> Harness {
    init_tracing();
    let pool = naughtywolf::db::create_pool("sqlite::memory:")
        .await
        .unwrap();
    naughtywolf::db::run_migrations(&pool).await.unwrap();
    let repository = Repository { pool };
    let live = LiveRegistry::new();
    let hub = ShellHub::new();
    let app = naughtywolf::c2::router(std::sync::Arc::new(psk.clone()))
        .layer(axum::Extension(live.clone()))
        .layer(axum::Extension(hub.clone()))
        .layer(axum::Extension(
            naughtywolf::callback_workspace::transfers::TransferStore::configured(
                repository.clone(),
            )
            .unwrap(),
        ))
        .with_state(repository.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app.into_make_service())
            .await
            .expect("portal c2 router serves");
    });
    Harness {
        repository,
        live,
        hub,
        address,
        server,
    }
}

async fn session_key(repository: &Repository, session_id: &Uuid) -> [u8; 32] {
    let encoded = repository
        .session_key_for(&session_id.to_string())
        .await
        .unwrap()
        .expect("the callback stored its session key");
    let mut key = [0u8; 32];
    key.copy_from_slice(
        &base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap(),
    );
    key
}

/// Poll until `condition` holds, or fail naming what was being waited for.
async fn wait_for(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + PATIENCE;
    while !condition() {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Start the implant against the in-process server and return its session id.
async fn start_implant(harness: &Harness, psk: Vec<u8>) -> Uuid {
    let profile = nw_implant::runtime::discover_profile(
        format!("ws://{}/c2/ws", harness.address),
        BEACON_INTERVAL,
        Duration::from_millis(0),
    );
    let runtime = std::sync::Arc::new(BeaconRuntime::new_with_gsocket(
        profile,
        psk,
        None::<GSocketConfig>,
    ));
    tokio::spawn(async move {
        let _ = runtime.run().await;
    });
    // The registration handshake is what mints the session id; wait for the
    // portal to record it rather than reaching into the implant.
    let deadline = std::time::Instant::now() + PATIENCE;
    loop {
        let rows: Vec<(String,)> = sqlx::query_as("SELECT id FROM callbacks LIMIT 1")
            .fetch_all(&harness.repository.pool)
            .await
            .unwrap();
        if let Some((id,)) = rows.into_iter().next()
            && let Ok(parsed) = Uuid::parse_str(&id)
        {
            return parsed;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the implant never registered"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Connect an implant and wait until its socket is a usable live channel.
async fn connect(harness: &Harness) -> (Uuid, [u8; 32]) {
    let psk = b"live-channel-e2e-psk".to_vec();
    let session = start_implant(harness, psk).await;
    wait_for("the callback's socket to register", || {
        harness.live.is_live(session)
    })
    .await;
    let key = session_key(&harness.repository, &session).await;
    (session, key)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_attached_shell_answers_a_command_without_a_beacon() {
    let harness = harness(b"live-channel-e2e-psk".to_vec()).await;
    let (session, key) = connect(&harness).await;

    let mut stream = harness.hub.subscribe(&session.to_string());
    let shell = Uuid::new_v4();
    harness
        .live
        .push(
            &key,
            session,
            Live::ShellOpen {
                shell_id: shell,
                program: String::new(),
                args: vec![],
            },
        )
        .await
        .expect("the shell opens on the open socket");

    // The implant starts a real terminal and says so.
    wait_for("the implant to report the shell running", || {
        harness
            .hub
            .shells(&session.to_string())
            .iter()
            .any(|view| view.id == shell && view.state == "running")
    })
    .await;

    harness
        .live
        .push(
            &key,
            session,
            Live::ShellInput {
                shell_id: shell,
                data: "echo LIVE_CHANNEL_PROBE\n".to_owned(),
            },
        )
        .await
        .expect("keystrokes are pushed");

    // Collect output until the probe comes back.
    let mut seen = String::new();
    let deadline = std::time::Instant::now() + PATIENCE;
    while !seen.contains("LIVE_CHANNEL_PROBE") {
        assert!(
            std::time::Instant::now() < deadline,
            "the shell never answered; saw {seen:?}"
        );
        let message = tokio::time::timeout(PATIENCE, stream.recv())
            .await
            .ok()
            .flatten()
            .expect("the shell stream stays open");
        if let naughtywolf::shell_hub::ShellMessage::Output { data, .. } = message {
            let raw = base64::engine::general_purpose::STANDARD
                .decode(data)
                .unwrap_or_default();
            seen.push_str(&String::from_utf8_lossy(&raw));
        }
    }
    assert!(
        seen.contains("LIVE_CHANNEL_PROBE"),
        "the command ran on the callback, not on the server: {seen:?}"
    );

    harness
        .live
        .push(&key, session, Live::ShellClose { shell_id: shell })
        .await
        .expect("the shell closes");
    harness.server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_running_task_is_stopped_by_a_pushed_kill_and_confirmed() {
    let harness = harness(b"live-channel-e2e-psk".to_vec()).await;
    let (session, key) = connect(&harness).await;

    // A task that would still be running when the test ends. It is delivered on
    // the live channel, so nothing is queued and no beacon delivers it either.
    let victim = Uuid::new_v4();
    harness
        .live
        .push(
            &key,
            session,
            Live::Task {
                task: nw_profile::msgs::Task {
                    id: victim,
                    command: "sleep".to_owned(),
                    args: vec!["30".to_owned()],
                    timeout_ms: 120_000,
                },
            },
        )
        .await
        .expect("the task is pushed to the open socket");

    // Let it start before stopping it; killing something that never began would
    // prove nothing.
    tokio::time::sleep(Duration::from_millis(750)).await;

    harness
        .live
        .push(&key, session, Live::Kill { task_id: victim })
        .await
        .expect("the kill is pushed to the open socket");

    // The implant acknowledges on the same channel, which the portal records.
    let deadline = std::time::Instant::now() + PATIENCE;
    loop {
        let confirmed: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM c2_audit WHERE action = 'task_kill_confirmed' AND details = ?",
        )
        .bind(format!("task {victim}"))
        .fetch_one(&harness.repository.pool)
        .await
        .unwrap();
        if confirmed > 0 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the callback never confirmed the kill"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    // And the outcome is in the audit trail next to the operator's request.
    let actions: Vec<String> =
        sqlx::query_scalar("SELECT action FROM c2_audit WHERE target_session = ? ORDER BY rowid")
            .bind(session.to_string())
            .fetch_all(&harness.repository.pool)
            .await
            .unwrap();
    assert!(
        actions.contains(&"task_kill_confirmed".to_owned()),
        "the confirmation is recorded: {actions:?}"
    );
    harness.server.abort();
}
