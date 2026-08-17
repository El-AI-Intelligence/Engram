//! One-time key handoff for account migration.
//!
//! The box vault's sync keys live only in the daemon's memory (derived from
//! ENGRAM_PASSPHRASE at startup). To make the vault "open by default" in the
//! browser, the SPA must wrap those keys under the account key A — which
//! means the keys must reach the browser exactly once. `POST
//! /sync/key-handoff/start` mints a single-use 300s token that redeems for
//! the key bytes exactly once at `POST /sync/key-handoff/{token}`.
//!
//! Trust boundary: Caddy gates /sync* behind the box basic-auth — the same
//! wall the /config routes sit behind. The keys never touch the relay.

use crate::app_state::{AppState, KeyHandoff, SyncKeyMaterial};
use crate::errors::err_json;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::post,
    Json, Router,
};
use base64::Engine;
use serde_json::{json, Value};
use std::sync::Arc;

type ApiError = (StatusCode, Json<Value>);

pub const HANDOFF_TTL_SECS: u64 = 300;

impl KeyHandoff {
    /// Mint a fresh single-use token, sweeping expired ones first. Injected
    /// clock keeps this testable without touching the wall clock.
    async fn mint_swept(&self, token: &str, now_secs: u64) {
        let mut tokens = self.tokens.lock().await;
        tokens.retain(|_, minted| now_secs.saturating_sub(*minted) <= HANDOFF_TTL_SECS);
        tokens.insert(token.to_string(), now_secs);
    }

    /// Redeem a token: true exactly once. Expired tokens are swept on access.
    async fn redeem_swept(&self, token: &str, now_secs: u64) -> bool {
        let mut tokens = self.tokens.lock().await;
        tokens.retain(|_, minted| now_secs.saturating_sub(*minted) <= HANDOFF_TTL_SECS);
        tokens.remove(token).is_some()
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The keys exist only when sync is enabled (a passphrase was present at
/// startup). Disabled sync means there is nothing to hand off.
fn keys_or_409(state: &AppState) -> Result<Arc<SyncKeyMaterial>, ApiError> {
    state
        .sync_keys
        .clone()
        .ok_or_else(|| err_json(409, "sync is not enabled on this server — no keys to hand off"))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/sync/key-handoff/start", post(start_handoff))
        .route("/sync/key-handoff/{token}", post(redeem_handoff))
}

/// Mint a one-time token. The token is the credential: whoever holds it
/// (and can reach this box through the Caddy basic-auth wall) gets the keys.
async fn start_handoff(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    keys_or_409(&state)?;
    let token = uuid::Uuid::new_v4().simple().to_string();
    state.key_handoff.mint_swept(&token, now_secs()).await;
    Ok(Json(json!({ "token": token, "expires_in": HANDOFF_TTL_SECS })))
}

/// Redeem a token for the key bytes — exactly once.
async fn redeem_handoff(
    State(state): State<AppState>,
    Path(token): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let keys = keys_or_409(&state)?;
    if !state.key_handoff.redeem_swept(&token, now_secs()).await {
        return Err(err_json(
            401,
            "unknown, expired, or already-used handoff token",
        ));
    }
    Ok(Json(json!({
        "enc_key_b64": base64::engine::general_purpose::STANDARD.encode(keys.enc_key),
        "hmac_key_b64": base64::engine::general_purpose::STANDARD.encode(keys.hmac_key),
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn token_redeems_exactly_once() {
        let kh = KeyHandoff::default();
        kh.mint_swept("tok-1", 1000).await;
        assert!(kh.redeem_swept("tok-1", 1001).await);
        assert!(!kh.redeem_swept("tok-1", 1002).await, "single-use");
    }

    #[tokio::test]
    async fn tokens_expire_and_sweep() {
        let kh = KeyHandoff::default();
        kh.mint_swept("tok-old", 1000).await;
        kh.mint_swept("tok-new", 1400).await;
        // tok-old is 600s old at 1600 — expired; the sweep drops it.
        assert!(!kh.redeem_swept("tok-old", 1600).await, "expired");
        assert!(kh.redeem_swept("tok-new", 1600).await);
        let map = kh.tokens.lock().await;
        assert!(map.is_empty(), "expired entries swept on access");
    }

    #[tokio::test]
    async fn unknown_token_rejected() {
        let kh = KeyHandoff::default();
        assert!(!kh.redeem_swept("never-minted", 1000).await);
    }

    #[tokio::test]
    async fn mint_sweeps_stale_tokens_before_inserting() {
        let kh = KeyHandoff::default();
        kh.mint_swept("stale", 0).await;
        kh.mint_swept("fresh", 10_000).await;
        let map = kh.tokens.lock().await;
        assert_eq!(map.len(), 1);
        assert!(map.contains_key("fresh"));
    }
}
