use axum::{
    extract::State,
    response::sse::{Event, Sse},
};
use futures::stream::Stream;
use std::convert::Infallible;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;

use crate::auth::middleware::AuthenticatedUserGuard;
use crate::sliver::events::SliverEvent;
use crate::web::routes::AppState;

pub fn event_stream_routes() -> axum::Router<AppState> {
    axum::Router::new().route("/api/events", axum::routing::get(sse_handler))
}

async fn sse_handler(
    _user: AuthenticatedUserGuard,
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.event_tx.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|result| match result {
        Ok(event) => {
            // Serialize the full event payload
            let data = serde_json::to_string(&event).unwrap_or_default();

            // SSE event name — derive from the enum variant name in lowercase
            let event_name = event_name(&event);

            Some(Ok(Event::default().data(data).event(event_name)))
        }
        Err(_) => None,
    });
    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new().interval(std::time::Duration::from_secs(30)),
    )
}

/// Derive a lowercase SSE event name from a `SliverEvent` variant.
fn event_name(event: &SliverEvent) -> &'static str {
    match event {
        SliverEvent::SessionConnected(_) => "session_connected",
        SliverEvent::SessionDisconnected(_) => "session_disconnected",
        SliverEvent::SessionUpdated(_) => "session_updated",
        SliverEvent::BeaconRegistered(_) => "beacon_registered",
        SliverEvent::BeaconTaskResult(_) => "beacon_task_result",
        SliverEvent::ClientJoined(_) => "client_joined",
        SliverEvent::ClientLeft(_) => "client_left",
        SliverEvent::JobStarted(_) => "job_started",
        SliverEvent::JobStopped(_) => "job_stopped",
        SliverEvent::Canary(_) => "canary",
        SliverEvent::Watchtower(_) => "watchtower",
        SliverEvent::LootAdded(_) => "loot_added",
        SliverEvent::LootRemoved(_) => "loot_removed",
        SliverEvent::Website(_) => "website",
        SliverEvent::Build(_) => "build",
        SliverEvent::BuildCompleted(_) => "build_completed",
        SliverEvent::Profile(_) => "profile",
        SliverEvent::ConnectionChanged(_) => "connection_changed",
        SliverEvent::Unknown { .. } => "unknown",
    }
}
