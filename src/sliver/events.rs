//! Sliver event stream handling.
//!
//! Subscribes to the Sliver server's event stream via gRPC server-sent events
//! and translates them into a Rust enum that the rest of NaughtyWolf can consume.

use tokio::sync::broadcast;
use tonic::Streaming;

use crate::sliver::connection::SliverConnection;
use crate::sliver::proto::clientpb;
use crate::sliver::proto::commonpb;

/// The canonical NaughtyWolf event type wrapping Sliver server events.
#[derive(Debug, Clone, serde::Serialize)]
pub enum SliverEvent {
    SessionOpened(String),
    SessionClosed(String),
    SessionUpdated(String),
    BeaconRegistered(String),
    BeaconTaskResult(String),
    ClientJoined(String),
    ClientLeft(String),
    JobStarted(String),
    JobStopped(String),
    Canary(String),
    Watchtower(String),
    LootAdded(String),
    LootRemoved(String),
    Website(String),
    Unknown(String, Vec<u8>),
}

/// Spawn a background task that listens to the Sliver server event stream.
///
/// Events are forwarded into the broadcast channel `tx`. The task exits
/// when the stream ends, an error occurs, or all receivers are dropped.
pub fn spawn_event_listener(
    mut connection: SliverConnection,
    tx: broadcast::Sender<SliverEvent>,
) {
    tokio::spawn(async move {
        let mut stream: Streaming<clientpb::Event> =
            match connection
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

fn convert_event(event: clientpb::Event) -> SliverEvent {
    let event_type = event.event_type.as_str();

    match event_type {
        "session-opened" => {
            let id = event.session.as_ref().map(|s| s.id.clone()).unwrap_or_default();
            SliverEvent::SessionOpened(id)
        }
        "session-closed" => {
            let id = event.session.as_ref().map(|s| s.id.clone()).unwrap_or_default();
            SliverEvent::SessionClosed(id)
        }
        "session-updated" => {
            let id = event.session.as_ref().map(|s| s.id.clone()).unwrap_or_default();
            SliverEvent::SessionUpdated(id)
        }
        "beacon-registered" => {
            let id = event.data.iter().map(|b| *b as char).collect::<String>();
            SliverEvent::BeaconRegistered(id)
        }
        "beacon-taskresult" => SliverEvent::BeaconTaskResult(String::new()),
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
        "watchtower" => SliverEvent::Watchtower(String::new()),
        "loot-added" => SliverEvent::LootAdded(String::new()),
        "loot-removed" => SliverEvent::LootRemoved(String::new()),
        "website" => SliverEvent::Website(String::new()),
        _ => SliverEvent::Unknown(event_type.to_string(), event.data),
    }
}
