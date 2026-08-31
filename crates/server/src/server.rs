use std::path::PathBuf;
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

/// Build ServerState from a ServerConfig, initializing SQLite persistence if
/// NW_DB is set. An explicitly configured database is a durability contract,
/// so initialization errors are returned instead of silently using memory.
pub async fn build_state(config: &crate::config::ServerConfig) -> anyhow::Result<ServerState> {
    let psk = config.psk.clone().into_bytes();
    let downloads = PathBuf::from(&config.downloads_dir);

    let (registry, queue) = if let Some(db) = config.db_path() {
        let pool = crate::persist::open_pool(db).await?;
        tracing::info!(db, "c2 sqlite persistence enabled");
        let registry = crate::session::SessionRegistry::with_sqlite(pool.clone());
        let queue = crate::queue::TaskQueue::with_sqlite(pool);
        (Arc::new(registry), Arc::new(queue))
    } else {
        (
            Arc::new(crate::session::SessionRegistry::new()),
            Arc::new(crate::queue::TaskQueue::new()),
        )
    };

    Ok(ServerState {
        registry,
        queue,
        psk: Arc::new(psk),
        files: FileStore::new(downloads),
        uploads: UploadStore::default(),
    })
}

/// Build a server and return the exact SQLite pool shared by sessions, tasks,
/// operators, and audit. The console uses this to avoid separate in-memory
/// databases when `NW_DB` is unset.
pub async fn build_state_with_pool(
    config: &crate::config::ServerConfig,
) -> anyhow::Result<(ServerState, sqlx::SqlitePool)> {
    let db = config.db_path().unwrap_or(":memory:");
    let pool = crate::persist::open_pool(db).await?;
    let state = ServerState {
        registry: Arc::new(crate::session::SessionRegistry::with_sqlite(pool.clone())),
        queue: Arc::new(crate::queue::TaskQueue::with_sqlite(pool.clone())),
        psk: Arc::new(config.psk.clone().into_bytes()),
        files: FileStore::new(PathBuf::from(&config.downloads_dir)),
        uploads: UploadStore::default(),
    };
    Ok((state, pool))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn configured_database_failure_is_not_silently_downgraded_to_memory() {
        let dir = tempfile::tempdir().unwrap();
        let missing_parent = dir.path().join("missing").join("c2.sqlite");
        let config = crate::config::ServerConfig {
            bind: "127.0.0.1:0".into(),
            tcp_bind: None,
            dns_bind: None,
            callback_host: "http://127.0.0.1:0".into(),
            psk: "test-psk".into(),
            downloads_dir: dir.path().join("downloads").display().to_string(),
            db_path: Some(missing_parent.display().to_string()),
            admin_password: None,
        };

        assert!(build_state(&config).await.is_err());
    }
}
