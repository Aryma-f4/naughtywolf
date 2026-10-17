//! gRPC carrier for the portal's own C2.
//!
//! The portal mounts its C2 handler directly rather than running the
//! standalone `nw-server`, so it needs its own gRPC listener too: the payload
//! wizard offers `grpc://`/`grpcs://`, and without this a payload built from
//! the portal would have no server able to terminate it.
//!
//! The carrier is deliberately the same shape as `crates/server/src/grpc.rs` —
//! one opaque `bytes sealed` field over `process_sealed` — so a beacon behaves
//! identically whichever C2 process it checks in against.

use std::sync::Arc;

use tonic::{Request, Response, Status, transport::Server};

use crate::db::repositories::Repository;

/// Generated from `proto/c2.proto`.
pub mod pb {
    #![allow(clippy::all)]
    tonic::include_proto!("nw");
}

use pb::Frame;
use pb::c2_server::{C2, C2Server};

pub struct C2GrpcService {
    repository: Repository,
    psk: Arc<Vec<u8>>,
    /// Protocol label recorded against callbacks created over this transport,
    /// so the operator UI attributes the session to gRPC rather than HTTP.
    protocol: String,
}

impl C2GrpcService {
    pub fn new(repository: Repository, psk: Arc<Vec<u8>>, protocol: impl Into<String>) -> Self {
        Self {
            repository,
            psk,
            protocol: protocol.into(),
        }
    }
}

#[tonic::async_trait]
impl C2 for C2GrpcService {
    async fn checkin(&self, request: Request<Frame>) -> Result<Response<Frame>, Status> {
        let sealed = request.into_inner().sealed;
        let reply = crate::c2::process_sealed(&self.repository, &self.psk, &sealed, &self.protocol)
            .await
            .map_err(|error| match error {
                crate::c2::C2Error::BadRequest => Status::invalid_argument("bad request"),
                crate::c2::C2Error::Unauthorized => Status::unauthenticated("unauthorized"),
                crate::c2::C2Error::Internal => Status::internal("internal error"),
            })?;
        Ok(Response::new(Frame { sealed: reply }))
    }
}

/// Serve the portal's C2 over gRPC until the process exits.
///
/// A tonic server owns its own listener, so it cannot be folded into the axum
/// `Router`; the caller spawns this alongside the HTTP and TCP listeners.
pub async fn serve(
    repository: Repository,
    psk: Arc<Vec<u8>>,
    protocol: String,
    addr: std::net::SocketAddr,
) -> Result<(), tonic::transport::Error> {
    Server::builder()
        .add_service(C2Server::new(C2GrpcService::new(repository, psk, protocol)))
        .serve(addr)
        .await
}
