// ── Teams (shared-vault v0) endpoint ─────────────────────────────────────────
//
// GET /teams/status — aggregates this daemon's sync config and state with the
// sync server's device roster into one response for the Settings "Sync & Team"
// panel. The server stays a dumb encrypted relay: it only knows device_ids and
// blob counts, never content. The sync api_key never reaches the browser —
// this route proxies the devices call server-side, like sync_status does for
// the health check.

use crate::AppState;

use axum::{extract::State, routing::get, Json, Router};
use serde::Serialize;

pub fn router() -> Router<AppState> {
    Router::new().route("/teams/status", get(status))
}

#[derive(Debug, Serialize)]
struct TeamStatusResponse {
    /// Shared-vault identity (config.json sync.vault_id). Null = solo vault.
    vault_id: Option<String>,
    /// Human-readable team name (config.json sync.name).
    name: Option<String>,
    /// Sync server URL the team shares.
    server_url: Option<String>,
    /// Whether the devices call to the sync server succeeded.
    remote_reachable: bool,
    /// Device roster from the server, each annotated with `is_self`.
    devices: Vec<serde_json::Value>,
    /// Last push cursor (sync_state.json).
    last_push: Option<String>,
    /// Last pull cursor (sync_state.json).
    last_pull: Option<String>,
}

/// GET /teams/status — team roster + sync reachability, aggregated
/// server-side so the sync api_key never leaves the daemon.
async fn status(
    State(state): State<AppState>,
) -> Result<Json<TeamStatusResponse>, (axum::http::StatusCode, Json<serde_json::Value>)> {
    // Read sync config raw (pattern: sync_status.rs) — raw JSON so unknown
    // fields don't break parsing and the api_key is never serialized back.
    let config_path = state.vault_path.join("config.json");
    let (enabled, server_url, api_key, vault_id, name): (
        bool,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    ) = if let Ok(data) = std::fs::read_to_string(&config_path) {
        if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&data) {
            let s = cfg.get("sync");
            (
                s.and_then(|s| s.get("enabled")).and_then(|v| v.as_bool()).unwrap_or(false),
                s.and_then(|s| s.get("server_url")).and_then(|v| v.as_str()).map(String::from),
                s.and_then(|s| s.get("api_key")).and_then(|v| v.as_str()).map(String::from),
                s.and_then(|s| s.get("vault_id")).and_then(|v| v.as_str()).map(String::from),
                s.and_then(|s| s.get("name")).and_then(|v| v.as_str()).map(String::from),
            )
        } else {
            (false, None, None, None, None)
        }
    } else {
        (false, None, None, None, None)
    };

    if !enabled {
        return Err((
            axum::http::StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": {
                    "code": "sync_disabled",
                    "message": "Sync is not enabled in config.json"
                }
            })),
        ));
    }

    // Sync state from sync_state.json
    let sync_state_path = state.vault_path.join("sync_state.json");
    let (last_push, last_pull): (Option<String>, Option<String>) =
        if let Ok(data) = std::fs::read_to_string(&sync_state_path) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&data) {
                (
                    json.get("last_push").and_then(|v| v.as_str()).map(String::from),
                    json.get("updated_at").and_then(|v| v.as_str()).map(String::from),
                )
            } else {
                (None, None)
            }
        } else {
            (None, None)
        };

    // This device's identity (for the is_self annotation)
    let device_id = {
        let path = state.vault_path.join("device.json");
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|data| serde_json::from_str::<serde_json::Value>(&data).ok())
            .and_then(|json| json.get("device_id").and_then(|v| v.as_str()).map(String::from))
            .unwrap_or_else(|| "unknown".into())
    };

    // Proxy the devices call to the sync server (pattern: sync_status.rs
    // health check) — Bearer api_key, 5s timeout, never exposed to the UI.
    let mut devices: Vec<serde_json::Value> = Vec::new();
    let mut remote_reachable = false;
    if let (Some(ref url), Some(ref v)) = (&server_url, &vault_id) {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .ok();
        if let Some(client) = client {
            let mut req = client.get(format!(
                "{}/v1/vaults/{}/devices",
                url.trim_end_matches('/'),
                v
            ));
            if let Some(ref key) = api_key {
                req = req.header("Authorization", format!("Bearer {key}"));
            }
            match req.send().await {
                Ok(resp) if resp.status().is_success() => {
                    remote_reachable = true;
                    if let Ok(body) = resp.json::<serde_json::Value>().await {
                        if let Some(list) = body.get("devices").and_then(|d| d.as_array()) {
                            devices = list
                                .iter()
                                .map(|d| {
                                    let mut d = d.clone();
                                    let is_self = d
                                        .get("device_id")
                                        .and_then(|v| v.as_str())
                                        .map(|id| id == device_id)
                                        .unwrap_or(false);
                                    if let Some(obj) = d.as_object_mut() {
                                        obj.insert("is_self".into(), serde_json::json!(is_self));
                                    }
                                    d
                                })
                                .collect();
                        }
                    }
                }
                _ => {}
            }
        }
    }

    Ok(Json(TeamStatusResponse {
        vault_id,
        name,
        server_url,
        remote_reachable,
        devices,
        last_push,
        last_pull,
    }))
}
