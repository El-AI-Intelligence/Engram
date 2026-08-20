use crate::AppState;
use axum::{extract::State, routing::get, Json, Router};
use serde_json::json;

pub fn router() -> Router<AppState> {
    Router::new().route("/health", get(health))
}

async fn health(State(state): State<AppState>) -> Json<serde_json::Value> {
    let vault = state.vault.lock().await;
    let count = vault.count().await.unwrap_or(0);
    let uptime = (chrono::Utc::now() - state.start_time).num_seconds();
    let db_size = state.vault_path.join("engrams.db")
        .metadata()
        .map(|m| m.len())
        .ok();
    let qem_hit_rate = state.qem.hit_rate();
    let qem_entries = state.qem.cache_size();
    Json(json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "vault": state.vault_path.to_string_lossy(),
        "uptime_secs": uptime,
        "memories_total": count,
        "qem_hit_rate": qem_hit_rate,
        "qem_hits": state.qem.hits(),
        "qem_misses": state.qem.misses(),
        "qem_cache_entries": qem_entries,
        "db_size_bytes": db_size,
        "device_id": state.device_id,
        "sync": {
            "enabled": state.sync_enabled,
            "passphrase_set": state.sync_passphrase_set,
        },
    }))
}
