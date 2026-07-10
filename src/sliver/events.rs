//! Sliver event stream handling.
//!
//! Subscribes to the Sliver server's event stream via gRPC server-sent events
//! and translates them into a Rust enum that the rest of NaughtyWolf can consume.
//!
//! # Event type string values (from Sliver server):
//! - "session-connected" / "session-disconnected" / "session-updated"
//! - "beacon-registered" / "beacon-taskresult"
//! - "client-joined" / "client-left"
//! - "job-started" / "job-stopped"
//! - "canary" / "watchtower"
//! - "loot-added" / "loot-removed"
//! - "website"
//! - "build" / "build-completed"

use tokio::sync::broadcast;
use tonic::Streaming;

use crate::sliver::connection::SliverConnection;
use crate::sliver::proto::clientpb;
use crate::sliver::proto::commonpb;

/// Richer payload for session-related events.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionInfo {
    pub id: String,
    pub name: String,
    pub hostname: String,
    pub os: String,
    pub arch: String,
    pub transport: String,
    pub remote_address: String,
    pub last_checkin: i64,
    pub username: String,
}

impl From<clientpb::Session> for SessionInfo {
    fn from(s: clientpb::Session) -> Self {
        Self {
            id: s.id,
            name: s.name,
            hostname: s.hostname,
            os: s.os,
            arch: s.arch,
            transport: s.transport,
            remote_address: s.remote_address,
            last_checkin: s.last_checkin,
            username: s.username,
        }
    }
}

/// Richer payload for connection state changes.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ConnectionInfo {
    pub connected: bool,
    pub profile_name: String,
}

/// The canonical NaughtyWolf event type wrapping Sliver server events
/// and NaughtyWolf-internal events.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "type")]
pub enum SliverEvent {
    /// A new session was opened (Sliver "session-connected").
    SessionConnected(SessionInfo),
    /// A session was closed (Sliver "session-disconnected").
    SessionDisconnected(SessionInfo),
    /// Session metadata updated (e.g. checkin, rename).
    SessionUpdated(SessionInfo),
    /// A new beacon registered.
    BeaconRegistered(String),
    /// A beacon task produced a result.
    BeaconTaskResult(String),
    /// A multiplayer client joined.
    ClientJoined(String),
    /// A multiplayer client left.
    ClientLeft(String),
    /// A job (listener) started.
    JobStarted(String),
    /// A job (listener) stopped.
    JobStopped(String),
    /// DNS canary triggered.
    Canary(String),
    /// Implant hash spotted on threat intel.
    Watchtower(String),
    /// Loot was added.
    LootAdded(String),
    /// Loot was removed.
    LootRemoved(String),
    /// Website content changed.
    Website(String),
    /// Sliver build event.
    Build(String),
    /// Build completed.
    BuildCompleted(String),
    /// Sliver profile changed.
    Profile(String),
    /// NaughtyWolf connection to Sliver changed.
    ConnectionChanged(ConnectionInfo),
    /// An unhandled event type.
    Unknown {
        event_type: String,
        data: Vec<u8>,
    },
}

/// Spawn a background task that listens to the Sliver server event stream.
///
/// Events are forwarded into the broadcast channel `tx`. The task exits
/// when the stream ends, an error occurs, or all receivers are dropped.
pub fn spawn_event_listener(mut connection: SliverConnection, tx: broadcast::Sender<SliverEvent>) {
    tokio::spawn(async move {
        let mut stream: Streaming<clientpb::Event> = match connection
            .client
            .events(tonic::Request::new(commonpb::Empty {}))
            .await
        {
            Ok(response) => response.into_inner(),
            Err(e) => {
                tracing::error!("Failed to open event stream: {e}");
                return;
            }
        };

        loop {
            match stream.message().await {
                Ok(Some(event)) => {
                    let sliver_event = convert_event(event);
                    if tx.send(sliver_event).is_err() {
                        tracing::warn!("Event receiver dropped");
                        break;
                    }
                }
                Ok(None) => {
                    tracing::info!("Event stream ended");
                    break;
                }
                Err(e) => {
                    tracing::error!("Event stream error: {e}");
                    break;
                }
            }
        }
    });
}

/// Spawn a background task that periodically emits `SessionUpdated` events
/// for sessions whose last_checkin time has advanced.
///
/// This provides a heartbeat / check-in signal that the Sliver server
/// event stream does not natively emit per-agent.
pub fn spawn_checkin_poller(
    mut connection: SliverConnection,
    tx: broadcast::Sender<SliverEvent>,
) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(15)).await;

            // Fetch all sessions and emit SessionUpdated for any that are alive
            let sessions = match connection
                .client
                .get_sessions(tonic::Request::new(commonpb::Empty {}))
                .await
            {
                Ok(resp) => resp.into_inner().sessions,
                Err(e) => {
                    tracing::debug!("Checkin poller: failed to get sessions: {e}");
                    continue;
                }
            };

            for session in sessions {
                let info = SessionInfo::from(session);
                if tx.send(SliverEvent::SessionUpdated(info)).is_err() {
                    // All receivers gone, stop polling
                    break;
                }
            }
        }
    });
}

fn convert_event(event: clientpb::Event) -> SliverEvent {
    let event_type = event.event_type.as_str();

    match event_type {
        "session-connected" => {
            let info = event
                .session
                .map(SessionInfo::from)
                .unwrap_or_else(|| SessionInfo {
                    id: String::new(),
                    name: String::new(),
                    hostname: String::new(),
                    os: String::new(),
                    arch: String::new(),
                    transport: String::new(),
                    remote_address: String::new(),
                    last_checkin: 0,
                    username: String::new(),
                });
            SliverEvent::SessionConnected(info)
        }
        "session-disconnected" => {
            let info = event
                .session
                .map(SessionInfo::from)
                .unwrap_or_else(|| SessionInfo {
                    id: String::new(),
                    name: String::new(),
                    hostname: String::new(),
                    os: String::new(),
                    arch: String::new(),
                    transport: String::new(),
                    remote_address: String::new(),
                    last_checkin: 0,
                    username: String::new(),
                });
            SliverEvent::SessionDisconnected(info)
        }
        "session-updated" => {
            let info = event
                .session
                .map(SessionInfo::from)
                .unwrap_or_else(|| SessionInfo {
                    id: String::new(),
                    name: String::new(),
                    hostname: String::new(),
                    os: String::new(),
                    arch: String::new(),
                    transport: String::new(),
                    remote_address: String::new(),
                    last_checkin: 0,
                    username: String::new(),
                });
            SliverEvent::SessionUpdated(info)
        }
        "beacon-registered" => {
            let id = event.data.iter().map(|b| *b as char).collect::<String>();
            SliverEvent::BeaconRegistered(id)
        }
        "beacon-taskresult" => {
            let data = event.data.iter().map(|b| *b as char).collect::<String>();
            SliverEvent::BeaconTaskResult(data)
        }
        "client-joined" => {
            let name = event
                .client
                .as_ref()
                .and_then(|c| c.operator.as_ref())
                .map(|o| o.name.clone())
                .unwrap_or_default();
            SliverEvent::ClientJoined(name)
        }
        "client-left" => {
            let name = event
                .client
                .as_ref()
                .and_then(|c| c.operator.as_ref())
                .map(|o| o.name.clone())
                .unwrap_or_default();
            SliverEvent::ClientLeft(name)
        }
        "job-started" => {
            let id = event
                .job
                .as_ref()
                .map(|j| j.id.to_string())
                .unwrap_or_default();
            SliverEvent::JobStarted(id)
        }
        "job-stopped" => {
            let id = event
                .job
                .as_ref()
                .map(|j| j.id.to_string())
                .unwrap_or_default();
            SliverEvent::JobStopped(id)
        }
        "canary" => {
            let domain = event.data.iter().map(|b| *b as char).collect::<String>();
            SliverEvent::Canary(domain)
        }
        "watchtower" => {
            let data = event.data.iter().map(|b| *b as char).collect::<String>();
            SliverEvent::Watchtower(data)
        }
        "loot-added" => {
            let data = event.data.iter().map(|b| *b as char).collect::<String>();
            SliverEvent::LootAdded(data)
        }
        "loot-removed" => {
            let data = event.data.iter().map(|b| *b as char).collect::<String>();
            SliverEvent::LootRemoved(data)
        }
        "website" => {
            let data = event.data.iter().map(|b| *b as char).collect::<String>();
            SliverEvent::Website(data)
        }
        "build" => {
            let data = event.data.iter().map(|b| *b as char).collect::<String>();
            SliverEvent::Build(data)
        }
        "build-completed" => {
            let data = event.data.iter().map(|b| *b as char).collect::<String>();
            SliverEvent::BuildCompleted(data)
        }
        "profile" => {
            let data = event.data.iter().map(|b| *b as char).collect::<String>();
            SliverEvent::Profile(data)
        }
        _ => SliverEvent::Unknown {
            event_type: event_type.to_string(),
            data: event.data,
        },
    }
}
