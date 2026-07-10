use std::path::Path;
use std::sync::Arc;
use tokio::sync::{Mutex, broadcast};

use serde::{Deserialize, Serialize};

use crate::sliver::connection::SliverConnection;
use crate::sliver::events::{self, ConnectionInfo, SliverEvent};
use crate::sliver::profiles::SliverCfg;

#[derive(Serialize)]
pub struct SliverStatusResponse {
    pub connected: bool,
    pub profile_name: Option<String>,
    pub operator: Option<String>,
    pub lhost: Option<String>,
    pub lport: Option<u32>,
}

#[derive(Deserialize)]
pub struct ConnectRequest {
    pub config_path: String,
}

/// Check current connection status.
pub fn status(
    conn: &Arc<Mutex<Option<SliverConnection>>>,
) -> SliverStatusResponse {
    let guard = conn.try_lock();
    match guard {
        Ok(guard) => match guard.as_ref() {
            Some(conn) => SliverStatusResponse {
                connected: true,
                profile_name: Some(conn.profile.profile_name()),
                operator: Some(conn.profile.operator.clone()),
                lhost: Some(conn.profile.lhost.clone()),
                lport: Some(conn.profile.lport.into()),
            },
            None => SliverStatusResponse {
                connected: false,
                profile_name: None,
                operator: None,
                lhost: None,
                lport: None,
            },
        },
        Err(_) => SliverStatusResponse {
            connected: false,
            profile_name: None,
            operator: None,
            lhost: None,
            lport: None,
        },
    }
}

/// Connect to a Sliver server using a config file path.
///
/// Also spawns an event listener and checkin poller, then emits a
/// `ConnectionChanged(true, ...)` event so SSE clients get notified.
pub async fn connect(
    conn: &Arc<Mutex<Option<SliverConnection>>>,
    config_path: &str,
    event_tx: broadcast::Sender<SliverEvent>,
) -> Result<SliverStatusResponse, String> {
    let path = Path::new(config_path);
    if !path.exists() {
        return Err(format!("Config file not found: {config_path}"));
    }

    let sliver_cfg =
        SliverCfg::from_file(path).map_err(|e| format!("Failed to parse config: {e}"))?;
    let connection = SliverConnection::connect(&sliver_cfg)
        .await
        .map_err(|e| format!("Failed to connect: {e}"))?;

    let profile_name = sliver_cfg.profile_name();
    let operator = sliver_cfg.operator.clone();
    let lhost = sliver_cfg.lhost.clone();
    let lport: u32 = sliver_cfg.lport.into();

    let status = SliverStatusResponse {
        connected: true,
        profile_name: Some(profile_name.clone()),
        operator: Some(operator),
        lhost: Some(lhost),
        lport: Some(lport),
    };

    // Spawn the Sliver event stream listener
    events::spawn_event_listener(connection.clone(), event_tx.clone());

    // Spawn a periodic checkin poller for sessions (heartbeat signal)
    events::spawn_checkin_poller(connection.clone(), event_tx.clone());

    // Notify SSE clients that we are now connected
    let _ = event_tx.send(SliverEvent::ConnectionChanged(ConnectionInfo {
        connected: true,
        profile_name: profile_name.clone(),
    }));

    let mut guard = conn.lock().await;
    *guard = Some(connection);

    tracing::info!(
        "Connected to Sliver server {}@{}:{} (profile: {}) — event listener started",
        status.operator.as_deref().unwrap_or("?"),
        status.lhost.as_deref().unwrap_or("?"),
        status.lport.unwrap_or(0),
        profile_name,
    );

    Ok(status)
}

/// Disconnect from the current Sliver server.
///
/// Emits a `ConnectionChanged(false, ...)` event so SSE clients get notified.
pub async fn disconnect(
    conn: &Arc<Mutex<Option<SliverConnection>>>,
    event_tx: broadcast::Sender<SliverEvent>,
) -> SliverStatusResponse {
    // Notify SSE clients that we are now disconnected
    let _ = event_tx.send(SliverEvent::ConnectionChanged(ConnectionInfo {
        connected: false,
        profile_name: String::new(),
    }));

    let mut guard = conn.lock().await;
    *guard = None;
    tracing::info!("Disconnected from Sliver server");
    SliverStatusResponse {
        connected: false,
        profile_name: None,
        operator: None,
        lhost: None,
        lport: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_status_when_disconnected() {
        let conn: Arc<Mutex<Option<SliverConnection>>> = Arc::new(Mutex::new(None));
        let s = status(&conn);
        assert!(!s.connected);
        assert!(s.profile_name.is_none());
    }

    #[tokio::test]
    async fn test_disconnect_on_empty() {
        let conn: Arc<Mutex<Option<SliverConnection>>> = Arc::new(Mutex::new(None));
        let (tx, _rx) = broadcast::channel(16);
        let s = disconnect(&conn, tx).await;
        assert!(!s.connected);
    }
}
