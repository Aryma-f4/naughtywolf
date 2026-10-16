//! gRPC carrier for the C2 check-in.
//!
//! The C2 protocol is unchanged: a check-in is still one sealed (AEAD)
//! envelope blob produced by `channels::process_sealed`, exactly as on
//! `/c2/checkin` and `/c2/ws`. `proto/c2.proto` therefore carries a single
//! opaque `bytes sealed` field, so no protocol type is duplicated in protobuf
//! and HTTP/WebSocket/gRPC all share one implementation.

use std::net::SocketAddr;

use tonic::{Request, Response, Status};

use crate::channels::{C2Error, process_sealed};
use crate::server::ServerState;

/// Generated from `proto/c2.proto`.
pub mod pb {
    #![allow(clippy::all)]
    tonic::include_proto!("nw");
}

use pb::Frame;
use pb::c2_server::{C2, C2Server};

pub struct C2Service {
    state: ServerState,
}

impl C2Service {
    pub fn new(state: ServerState) -> Self {
        Self { state }
    }
}

fn to_status(err: C2Error) -> Status {
    match err {
        C2Error::BadRequest => Status::invalid_argument("bad request"),
        C2Error::Unauthorized => Status::unauthenticated("unauthorized"),
        C2Error::Internal(_) => Status::internal("internal error"),
    }
}

#[tonic::async_trait]
impl C2 for C2Service {
    async fn checkin(&self, request: Request<Frame>) -> Result<Response<Frame>, Status> {
        let sealed = request.into_inner().sealed;
        let reply = process_sealed(&self.state, &sealed)
            .await
            .map_err(to_status)?;
        Ok(Response::new(Frame { sealed: reply }))
    }
}

/// Bind the gRPC listener. Kept separate from the axum router because a tonic
/// server owns its own listener and does not compose into an axum `Router`.
pub async fn serve(
    state: ServerState,
    addr: SocketAddr,
) -> anyhow::Result<tokio::task::JoinHandle<()>> {
    Ok(tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(C2Server::new(C2Service::new(state)))
            .serve(addr)
            .await
            .expect("grpc listener failed");
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The carrier must reject malformed frames instead of panicking or
    /// hanging. Both rejection paths get pinned, since they map to different
    /// gRPC codes and a wrong mapping would leak parsing vs auth behaviour.
    #[tokio::test]
    async fn malformed_frames_are_rejected_before_auth() {
        let (state, _q) = crate::channels::test_state();
        let svc = C2Service::new(state);

        // Invalid UTF-8 cannot be an envelope at all -> bad request.
        let err = svc
            .checkin(Request::new(Frame {
                sealed: vec![0xff, 0xfe],
            }))
            .await
            .expect_err("invalid utf-8 must be rejected");
        assert_eq!(err.code(), tonic::Code::InvalidArgument);

        // Empty is valid UTF-8, so it parses but cannot authenticate.
        let err = svc
            .checkin(Request::new(Frame { sealed: Vec::new() }))
            .await
            .expect_err("empty frame must not authenticate");
        assert_eq!(err.code(), tonic::Code::Unauthenticated);
    }
}
