use std::sync::Arc;

use axum::{Extension, Router};

use crate::{
    c2, callback_workspace::transfers::TransferStore, db::repositories::Repository,
    evidence::EvidenceStore, portal,
};

/// The production router composition shared by the binary and the restart
/// tests: the portal operator routes and the C2 fabric, with the C2 pre-shared
/// key and the evidence/transfer stores attached as extensions.
///
/// The identity-providing login route, the stateless public landing routes,
/// `.with_state`, and the session layer all stay with the caller because they
/// need a `Repository` (or the unit state) before the router can be served.
pub fn router(
    c2_psk: Arc<Vec<u8>>,
    evidence_store: EvidenceStore,
    transfer_store: TransferStore,
    max_transfer_bytes: u64,
) -> Router<Repository> {
    Router::<Repository>::new()
        .merge(portal::authenticated_router_with_transfer_limit(
            max_transfer_bytes,
        ))
        .merge(c2::router(c2_psk.clone()))
        .layer(Extension(c2_psk))
        .layer(axum::Extension(evidence_store))
        .layer(axum::Extension(transfer_store))
}
