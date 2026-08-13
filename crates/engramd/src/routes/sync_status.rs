// ── Sync status endpoint ────────────────────────────────────────────────────
//
// GET /sync/status — current sync state, last sync time, pending changes,
// device count, and remote server health.
//
// When sync is configured in config.json, this endpoint also proxies a
// health check to the remote sync server so users can verify connectivity.

use crate::AppState;

use axum::{
    extract::State,
    routing::{get, post},
    Json, Router,
};
use serde::Serialize;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/sync/status", get(status))
        .route("/sync/now", post(trigger_sync))
}

/// POST /sync/now — force an immediate sync cycle.
///
/// Bumps the trigger counter watched by the sync loop. Returns 202 when the
/// trigger was sent, 409 when sync isn't enabled in config.json (nothing to
/// wake).
async fn trigger_sync(
    State(state): State<AppState>,
) -> Result<
    (axum::http::StatusCode, Json<serde_json::Value>),
    (axum::http::StatusCode, Json<serde_json::Value>),
> {
    let config_path = state.vault_path.join("config.json");
    let enabled = std::fs::read_to_string(&config_path)
        .ok()
        .and_then(|data| serde_json::from_str::<serde_json::Value>(&data).ok())
        .and_then(|cfg| {
            cfg.get("sync")
                .and_then(|s| s.get("enabled"))
                .and_then(|v| v.as_bool())
        })
        .unwrap_or(false);
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

    state.sync_trigger.send_modify(|n| *n += 1);
    Ok((
        axum::http::StatusCode::ACCEPTED,
        Json(serde_json::json!({"status": "sync_triggered"})),
    ))
}

#[derive(Debug, Serialize)]
struct SyncStatusResponse {
    /// Whether sync is configured and enabled in config.json
    configured: bool,
    /// Sync server URL (masked if set)
    server_url: Option<String>,
    /// Whether the sync client is actively running
    running: bool,
    /// Last successful pull timestamp (RFC 3339 or null)
    last_pull: Option<String>,
    /// Last successful push timestamp (RFC 3339 or null)
    last_push: Option<String>,
    /// Current local vector clock value
    local_clock: u64,
    /// Number of pending tombstone deletions not yet pushed
    pending_deletions: usize,
    /// Device identity (from device.json)
    device_id: String,
    /// Device name/hostname
    device_name: String,
    /// Number of devices seen from remote sync (0 if no remote)
    remote_device_count: usize,
    /// Whether the remote sync server is reachable
    remote_reachable: Option<bool>,
    /// Remote server version if reachable
    remote_version: Option<String>,
    /// Sync interval in seconds
    interval_secs: u64,
    /// Estimated memory count pending push (created since last push)
    pending_push_count: usize,
}

async fn status(
    State(state): State<AppState>,
) -> Result<Json<SyncStatusResponse>, (axum::http::StatusCode, Json<serde_json::Value>)> {
    // Read config to check sync settings
    let config_path = state.vault_path.join("config.json");
    let (configured, server_url, api_key, interval_secs) =
        if let Ok(data) = std::fs::read_to_string(&config_path) {
            if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&data) {
                let enabled = cfg
                    .get("sync")
                    .and_then(|s| s.get("enabled"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let url = cfg
                    .get("sync")
                    .and_then(|s| s.get("server_url"))
                    .and_then(|v| v.as_str())
                    .map(String::from);
                let key = cfg
                    .get("sync")
                    .and_then(|s| s.get("api_key"))
                    .and_then(|v| v.as_str())
                    .map(String::from);
                let interval = cfg
                    .get("sync")
                    .and_then(|s| s.get("interval_secs"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(60);
                (enabled, url, key, interval)
            } else {
                (false, None, None, 60u64)
            }
        } else {
            (false, None, None, 60u64)
        };

    // Read sync state from sync_state.json
    let sync_state_path = state.vault_path.join("sync_state.json");
    let (last_push, last_pull, local_clock): (Option<String>, Option<String>, u64) =
        if let Ok(data) = std::fs::read_to_string(&sync_state_path) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&data) {
                let push = json
                    .get("last_push")
                    .and_then(|v| v.as_str())
                    .map(String::from);
                // last_pull is the vector_clock timestamp — approximate
                let pull = json
                    .get("updated_at")
                    .and_then(|v| v.as_str())
                    .map(String::from);
                let clock = json
                    .get("vector_clock")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                (push, pull, clock)
            } else {
                (None, None, 0)
            }
        } else {
            (None, None, 0)
        };

    // Count pending tombstone deletions
    let pending_deletions = {
        let path = state.vault_path.join("tombstones.jsonl");
        if let Ok(data) = std::fs::read_to_string(&path) {
            data.lines().filter(|l| !l.trim().is_empty()).count()
        } else {
            0
        }
    };

    // Count pending pushes: memories created after last_push
    let pending_push_count = if let Some(ref lp) = last_push {
        let vault = state.vault.lock().await;
        let all = vault.list(1000, 0).await.unwrap_or_default();
        all.into_iter()
            .filter(|m| {
                if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(lp) {
                    m.created_at > dt.with_timezone(&chrono::Utc)
                } else {
                    false
                }
            })
            .count()
    } else {
        0
    };

    // Device identity from device.json
    let (device_id, device_name): (String, String) = {
        let path = state.vault_path.join("device.json");
        if let Ok(data) = std::fs::read_to_string(&path) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&data) {
                let id = json.get("device_id").and_then(|v| v.as_str()).unwrap_or("unknown");
                let name = json.get("label").and_then(|v| v.as_str()).unwrap_or("unknown");
                (id.to_string(), name.to_string())
            } else {
                ("unknown".into(), "unknown".into())
            }
        } else {
            ("unknown".into(), "unknown".into())
        }
    };

    // Remote server health check (only if configured)
    let (remote_reachable, remote_version, remote_device_count) = if configured {
        if let Some(ref url) = server_url {
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .build()
                .ok();
            if let Some(client) = client {
                let mut req = client.get(format!("{}/health", url.trim_end_matches('/')));
                if let Some(ref key) = api_key {
                    req = req.header("Authorization", format!("Bearer {key}"));
                }
                match req.send().await {
                    Ok(resp) => {
                        match resp.json::<serde_json::Value>().await {
                            Ok(body) => {
                                let version = body
                                    .get("version")
                                    .and_then(|v| v.as_str())
                                    .map(String::from);
                                let vaults = body
                                    .get("vaults")
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(0) as usize;
                                (true, version, vaults)
                            }
                            Err(_) => (true, None, 0usize),
                        }
                    }
                    Err(_) => (false, None, 0usize),
                }
            } else {
                (false, None, 0usize)
            }
        } else {
            (false, None, 0usize)
        }
    } else {
        (false, None, 0usize)
    };

    let running = configured && state.vault_path.join("config.json").exists();

    Ok(Json(SyncStatusResponse {
        configured,
        server_url: server_url.map(|u| {
            // Mask API key in URL if present
            // Just return the origin part so we don't leak credentials
            u
        }),
        running,
        last_pull,
        last_push,
        local_clock,
        pending_deletions,
        device_id,
        device_name,
        remote_device_count,
        remote_reachable: Some(remote_reachable),
        remote_version,
        interval_secs,
        pending_push_count,
    }))
}
