// ── WebSocket live events ─────────────────────────────────────────────────
// Push capture/decay/consolidation events to connected clients via
// WebSocket. Each message is a JSON-encoded LiveEvent.
//
// Endpoint: GET /ws/events
// Query params (optional): ?layer=episodic&source=observation

use crate::{AppState, LiveEvent};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    response::IntoResponse,
    routing::get,
    Router,
};
use futures::{SinkExt, StreamExt};
use serde::Deserialize;

#[derive(Deserialize, Default)]
struct EventsFilter {
    layer: Option<String>,
    source: Option<String>,
}

pub fn router() -> Router<AppState> {
    Router::new().route("/ws/events", get(handler))
}

async fn handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Query(filter): Query<EventsFilter>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state, filter))
}

async fn handle_socket(socket: WebSocket, state: AppState, filter: EventsFilter) {
    let (mut sender, mut receiver) = socket.split();

    // Subscribe to the broadcast channel
    let mut rx = state.events_tx.subscribe();

    // Send any backlog (up to 50 most recent events won't be in backlog since
    // broadcast doesn't retain history — but we can send a connected message)
    let _ = sender
        .send(Message::Text(
            serde_json::json!({"type": "connected", "message": "Live event stream started"})
                .to_string()
                .into(),
        ))
        .await;

    // Split into two tasks: one sends events, one watches for close
    let send_task = async {
        while let Ok(event) = rx.recv().await {
            // Apply filters
            match &event {
                LiveEvent::Capture { layer, source, .. } => {
                    if let Some(ref fl) = filter.layer {
                        if layer != fl { continue; }
                    }
                    if let Some(ref fs) = filter.source {
                        if source != fs { continue; }
                    }
                }
                // Decay/consolidation always pass through (no layer/source to filter)
                _ => {}
            }

            let json: axum::extract::ws::Utf8Bytes =
                serde_json::to_string(&event).unwrap_or_default().into();
            if sender.send(Message::Text(json)).await.is_err() {
                break; // client disconnected
            }
        }
    };

    let recv_task = async {
        while let Some(Ok(msg)) = receiver.next().await {
            match msg {
                Message::Close(_) | Message::Ping(_) | Message::Pong(_) => {}
                _ => {} // ignore text messages from client
            }
        }
    };

    tokio::select! {
        _ = send_task => {},
        _ = recv_task => {},
    }
}
