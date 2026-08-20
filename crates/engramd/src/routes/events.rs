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
    http::{header, HeaderMap, StatusCode},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::json;

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
    headers: HeaderMap,
    Query(filter): Query<EventsFilter>,
) -> impl IntoResponse {
    // The stream carries full memory contents, so the handshake must be
    // origin-gated: browsers always send Origin on WebSocket upgrades, and
    // anything without an allowed one (missing included — non-browser
    // clients don't send it) must not get a socket.
    let origin_allowed = headers
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .map(|o| state.cors_allowed_origins.iter().any(|a| a == o))
        .unwrap_or(false);
    if !origin_allowed {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "origin not allowed", "code": "forbidden"})),
        )
            .into_response();
    }
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
                LiveEvent::Capture { memory, .. } => {
                    let layer = memory.get("layer").and_then(|v| v.as_str());
                    let source = memory.get("source").and_then(|v| v.as_str());
                    if let Some(ref fl) = filter.layer {
                        if layer != Some(fl.as_str()) { continue; }
                    }
                    if let Some(ref fs) = filter.source {
                        if source != Some(fs.as_str()) { continue; }
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
