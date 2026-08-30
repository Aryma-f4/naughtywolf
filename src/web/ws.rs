use axum::{
    extract::State,
    response::IntoResponse,
};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use futures::{SinkExt, StreamExt, future::ready};
use tokio_stream::wrappers::BroadcastStream;

use crate::auth::middleware::AuthenticatedUserGuard;
use crate::web::routes::AppState;

pub fn ws_stream_routes() -> axum::Router<AppState> {
    axum::Router::new().route("/api/ws", axum::routing::get(ws_handler))
}

/// Axum 0.8 WebSocket upgrade handler that bridges the in-process
/// `SliverEvent` broadcast to a per-connection WebSocket stream.
async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    _user: AuthenticatedUserGuard,
) -> impl IntoResponse {
    let rx = state.event_tx.subscribe();
    ws.on_upgrade(move |socket| async move {
        // BroadcastStream wraps the broadcast receiver as a Stream<Item = Result<T, RecvError>>.
        // We map T to a text WebSocket frame and drop errors silently (slow client).
        let (mut sender, mut receiver) = socket.split();
        let mut stream = BroadcastStream::new(rx).filter_map(|result| {
            ready(match result {
                Ok(event) => {
                    let json = serde_json::to_string(&event).unwrap_or_else(|_| "{}".to_string());
                    Some(Ok::<_, axum::Error>(Message::Text(json.into())))
                }
                Err(_) => None,
            })
        });

        // Forward server → client
        let send_task = tokio::spawn(async move {
            while let Some(msg_res) = stream.next().await {
                match msg_res {
                    Ok(msg) => {
                        if sender.send(msg).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => continue,
                }
            }
        });

        // Forward client → server (currently just consume / disconnect-detect)
        while let Some(msg_res) = receiver.next().await {
            match msg_res {
                Ok(_) => continue, // ignore client messages
                Err(_) => break,
            }
        }

        send_task.abort();
    })
}
