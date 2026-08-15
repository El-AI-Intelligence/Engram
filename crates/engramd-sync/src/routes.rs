// ── Sync server routes ─────────────────────────────────────────────────────

use crate::{ApiKeyEntry, SyncState};
use axiom_engram::sync::{PullRequest, PullResponse, PushRequest, PushResponse, SyncBlob, SyncHealth};
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::get,
    Json, Router,
};
use serde_json::json;
use std::collections::HashSet;
use std::sync::Arc;

pub fn router() -> Router<SyncState> {
    Router::new()
        .route("/health", get(health))
        .route("/v1/vaults/{vault_id}/push", axum::routing::post(push))
        .route("/v1/vaults/{vault_id}/pull", get(pull))
        .route("/v1/vaults/{vault_id}/stats", get(stats))
        .route("/v1/vaults/{vault_id}/devices", get(devices))
        .route(
            "/v1/vaults/{vault_id}/devices/{device_id}",
            axum::routing::delete(revoke_device),
        )
}

/// Server health. No auth required (public endpoint).
async fn health(State(state): State<SyncState>) -> Json<SyncHealth> {
    let conn = state.conn.lock().await;
    let active_blobs: u64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sync_blobs WHERE deleted = 0",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let vaults: usize = conn
        .query_row(
            "SELECT COUNT(DISTINCT vault_id) FROM sync_blobs",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let db_size: u64 = state
        .data_dir
        .join("sync.db")
        .metadata()
        .map(|m| m.len())
        .unwrap_or(0);
    let uptime = (chrono::Utc::now() - state.start_time)
        .num_seconds()
        .max(0) as u64;

    Json(SyncHealth {
        status: "ok".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        uptime_secs: uptime,
        vaults,
        total_blobs: active_blobs, // report active blobs, not deleted tombstones
        db_size_bytes: db_size,
    })
}

/// Push a batch of encrypted blobs. Rejects stale vector clocks and blobs
/// from devices the team admin has revoked.
async fn push(
    State(state): State<SyncState>,
    headers: HeaderMap,
    Path(vault_id): Path<String>,
    Json(req): Json<PushRequest>,
) -> Result<Json<PushResponse>, (StatusCode, Json<serde_json::Value>)> {
    // Auth + rate limit
    let key = authenticate(&state, &headers).await?;
    authorize_vault(&key, &vault_id)?;

    if req.blobs.is_empty() {
        return Err(err_json(400, "no blobs provided"));
    }
    if req.blobs.len() > crate::MAX_BLOBS_PER_PUSH {
        return Err(err_json(400, format!(
            "too many blobs: {} (max {})",
            req.blobs.len(),
            crate::MAX_BLOBS_PER_PUSH
        )));
    }

    let conn = state.conn.lock().await;

    // Revoked devices for this vault (admin-added; blocks their pushes)
    let revoked: HashSet<String> = {
        let mut stmt = match conn.prepare(
            "SELECT device_id FROM revoked_devices WHERE vault_id = ?1",
        ) {
            Ok(s) => s,
            Err(e) => return Err(err_json(500, format!("Database error: {e}"))),
        };
        let rows = stmt.query_map(rusqlite::params![vault_id], |r| r.get::<_, String>(0));
        rows.map(|rs| rs.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    };
    let mut revoked_seen: HashSet<String> = HashSet::new();

    let mut accepted = 0usize;
    let mut rejected: Vec<String> = Vec::new();

    for blob in &req.blobs {
        // Reject oversized blobs
        if blob.ciphertext.len() > crate::MAX_BLOB_SIZE {
            rejected.push(blob.memory_id.clone());
            continue;
        }

        // Verify vault_id matches path
        if blob.vault_id != vault_id {
            rejected.push(blob.memory_id.clone());
            continue;
        }

        // Reject blobs from revoked devices (a zero-knowledge relay can
        // block pushes; removing the passphrase needs a re-key)
        if revoked.contains(&blob.device_id) {
            rejected.push(blob.memory_id.clone());
            revoked_seen.insert(blob.device_id.clone());
            continue;
        }

        // Check existing vector clock — accept only if this blob is newer
        let existing_clock: Option<u64> = conn
            .query_row(
                "SELECT vector_clock FROM sync_blobs WHERE vault_id = ?1 AND memory_id = ?2",
                rusqlite::params![vault_id, blob.memory_id],
                |r| r.get(0),
            )
            .ok();

        if let Some(clock) = existing_clock {
            if blob.vector_clock <= clock {
                rejected.push(blob.memory_id.clone());
                continue;
            }
        }

        // Insert or update
        let result = conn.execute(
            "INSERT OR REPLACE INTO sync_blobs \
             (vault_id, memory_id, device_id, vector_clock, ciphertext, hmac, created_at, deleted) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                blob.vault_id,
                blob.memory_id,
                blob.device_id,
                blob.vector_clock,
                blob.ciphertext,
                blob.hmac,
                blob.created_at,
                blob.deleted as i32,
            ],
        );

        match result {
            Ok(_) => accepted += 1,
            Err(_) => rejected.push(blob.memory_id.clone()),
        }
    }

    let mut revoked_devices: Vec<String> = revoked_seen.into_iter().collect();
    revoked_devices.sort();
    Ok(Json(PushResponse {
        accepted,
        rejected,
        revoked_devices,
    }))
}

/// Pull blobs updated since a given timestamp.
async fn pull(
    State(state): State<SyncState>,
    headers: HeaderMap,
    Path(vault_id): Path<String>,
    Query(req): Query<PullRequest>,
) -> Result<Json<PullResponse>, (StatusCode, Json<serde_json::Value>)> {
    // Auth + rate limit
    let key = authenticate(&state, &headers).await?;
    authorize_vault(&key, &vault_id)?;

    let conn = state.conn.lock().await;
    let limit = req.limit.min(1000);
    // Normalize: empty-string since → None
    let since = req.since.as_deref().filter(|s| !s.is_empty());

    let (blobs, has_more): (Vec<SyncBlob>, bool) = if let Some(since) = since {
        // Use LIMIT+1 pattern: fetch one extra row to detect whether there
        // are more blobs beyond this batch. No separate COUNT query needed —
        // avoids the COUNT(CREATED_AT > ?) vs SELECT(CREATED_AT >= ?) mismatch
        // that permanently stranded boundary-timestamp blobs.
        let fetch_limit = (limit + 1) as i64;
        let mut stmt = conn
            .prepare(
                "SELECT vault_id, memory_id, device_id, vector_clock, ciphertext, hmac, \
                 created_at, deleted FROM sync_blobs \
                 WHERE vault_id = ?1 AND created_at >= ?2 \
                 ORDER BY created_at ASC, rowid ASC LIMIT ?3",
            )
            .map_err(|e| err_json(500, format!("Database error: {e}")))?;

        let rows = stmt
            .query_map(
                rusqlite::params![vault_id, since, fetch_limit],
                |row| {
                    Ok(SyncBlob {
                        vault_id: row.get(0)?,
                        memory_id: row.get(1)?,
                        device_id: row.get(2)?,
                        vector_clock: row.get(3)?,
                        ciphertext: row.get(4)?,
                        hmac: row.get(5)?,
                        created_at: row.get(6)?,
                        deleted: row.get::<_, i32>(7).unwrap_or(0) != 0,
                    })
                },
            )
            .map_err(|e| err_json(500, format!("Database error: {e}")))?;

        let mut blobs = Vec::new();
        for row in rows {
            blobs.push(row.map_err(|e| err_json(500, format!("Database error: {e}")))?);
        }
        let has_more = blobs.len() > limit;
        if has_more {
            blobs.truncate(limit);
        }
        (blobs, has_more)
    } else {
        let fetch_limit = (limit + 1) as i64;
        let mut stmt = conn
            .prepare(
                "SELECT vault_id, memory_id, device_id, vector_clock, ciphertext, hmac, \
                 created_at, deleted FROM sync_blobs \
                 WHERE vault_id = ?1 ORDER BY created_at ASC, rowid ASC LIMIT ?2",
            )
            .map_err(|e| err_json(500, format!("Database error: {e}")))?;

        let rows = stmt
            .query_map(rusqlite::params![vault_id, fetch_limit], |row| {
                Ok(SyncBlob {
                    vault_id: row.get(0)?,
                    memory_id: row.get(1)?,
                    device_id: row.get(2)?,
                    vector_clock: row.get(3)?,
                    ciphertext: row.get(4)?,
                    hmac: row.get(5)?,
                    created_at: row.get(6)?,
                    deleted: row.get::<_, i32>(7).unwrap_or(0) != 0,
                })
            })
            .map_err(|e| err_json(500, format!("Database error: {e}")))?;

        let mut blobs = Vec::new();
        for row in rows {
            blobs.push(row.map_err(|e| err_json(500, format!("Database error: {e}")))?);
        }
        let has_more = blobs.len() > limit;
        if has_more {
            blobs.truncate(limit);
        }
        (blobs, has_more)
    };

    Ok(Json(PullResponse { blobs, has_more }))
}

/// Per-vault statistics. Auth required — returns 401/429 on failure.
async fn stats(
    State(state): State<SyncState>,
    headers: HeaderMap,
    Path(vault_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    // Auth + rate limit — hard error on failure (not soft like before)
    let key = authenticate(&state, &headers).await?;
    authorize_vault(&key, &vault_id)?;

    let conn = state.conn.lock().await;
    let count: u64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sync_blobs WHERE vault_id = ?1 AND deleted = 0",
            rusqlite::params![vault_id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let latest: Option<String> = conn
        .query_row(
            "SELECT MAX(created_at) FROM sync_blobs WHERE vault_id = ?1",
            rusqlite::params![vault_id],
            |r| r.get(0),
        )
        .ok()
        .flatten();

    Ok(Json(json!({
        "vault_id": vault_id,
        "total_blobs": count,
        "latest_sync": latest,
    })))
}

/// Devices that have pushed blobs to a vault — the team roster backing the
/// shared-vault v0 UI. Auth required, same as `stats`. The server only knows
/// device_ids (blobs carry no labels), so the aggregating daemon annotates
/// `is_self` for its UI. Each entry also carries `revoked`, set by the team
/// admin via `DELETE /v1/vaults/{vault_id}/devices/{device_id}`.
async fn devices(
    State(state): State<SyncState>,
    headers: HeaderMap,
    Path(vault_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let key = authenticate(&state, &headers).await?;
    authorize_vault(&key, &vault_id)?;

    let conn = state.conn.lock().await;
    let mut stmt = conn
        .prepare(
            "SELECT s.device_id, MAX(s.created_at) AS last_seen, COUNT(*) AS blob_count, \
                    (r.revoked_at IS NOT NULL) AS revoked \
             FROM sync_blobs s \
             LEFT JOIN revoked_devices r \
                    ON r.vault_id = s.vault_id AND r.device_id = s.device_id \
             WHERE s.vault_id = ?1 \
             GROUP BY s.device_id, r.revoked_at \
             ORDER BY last_seen DESC",
        )
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?;
    let rows = stmt
        .query_map(rusqlite::params![vault_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)? != 0,
            ))
        })
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?;

    let mut device_list = Vec::new();
    for row in rows {
        let (device_id, last_seen, blob_count, revoked) = row.map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?;
        device_list.push(json!({
            "device_id": device_id,
            "last_seen": last_seen,
            "blob_count": blob_count,
            "revoked": revoked,
        }));
    }

    Ok(Json(json!({
        "vault_id": vault_id,
        "devices": device_list,
    })))
}

/// Revoke a device: the relay stops accepting its pushes (the roster marks
/// it revoked). Requires a key that administers this vault (`+admin` scope
/// on a scoped key, or any unscoped superuser key).
///
/// Honest zero-knowledge boundary: pulls are vault-scoped, not
/// device-scoped, so the relay cannot stop the device from reading blobs
/// or decrypting what it already holds. Full removal means re-keying the
/// vault (new passphrase) or per-member key revocation — the control-plane
/// layer's job.
async fn revoke_device(
    State(state): State<SyncState>,
    headers: HeaderMap,
    Path((vault_id, device_id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let key = authenticate(&state, &headers).await?;
    authorize_vault(&key, &vault_id)?;
    if !vault_admin(&key, &vault_id) {
        return Err(err_json(
            403,
            "key is not an admin for this vault (needs `vault+admin` scope)",
        ));
    }

    let conn = state.conn.lock().await;
    conn.execute(
        "INSERT OR REPLACE INTO revoked_devices (vault_id, device_id, revoked_at) \
         VALUES (?1, ?2, ?3)",
        rusqlite::params![vault_id, device_id, chrono::Utc::now().to_rfc3339()],
    )
    .map_err(|e| err_json(500, format!("Database error: {e}")))?;

    Ok(Json(json!({
        "vault_id": vault_id,
        "device_id": device_id,
        "revoked": true,
    })))
}

// ── Auth ────────────────────────────────────────────────────────────────────

/// The key that authenticated this request (rate-limited).
struct AuthenticatedKey {
    entry: Arc<ApiKeyEntry>,
}

/// Authenticate a request using the `Authorization: Bearer <key>` header.
///
/// Pattern matches Guardrail's: auth is optional on loopback (empty api_keys),
/// required on non-loopback. Constant-time key comparison prevents timing attacks.
async fn authenticate(
    state: &SyncState,
    headers: &HeaderMap,
) -> Result<AuthenticatedKey, (StatusCode, Json<serde_json::Value>)> {
    // No keys configured + loopback → auth is optional (wildcard superuser)
    if state.api_keys.is_empty() && state.is_loopback {
        return Ok(AuthenticatedKey {
            entry: Arc::new(ApiKeyEntry {
                rate: f64::MAX,
                vaults: None,
                admin_vaults: HashSet::new(),
            }),
        });
    }

    // Extract Bearer token
    let auth_header = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| err_json(401, "missing Authorization header"))?;

    let token = auth_header
        .strip_prefix("Bearer ")
        .ok_or_else(|| err_json(401, "expected Bearer token"))?;

    if token.is_empty() {
        return Err(err_json(401, "empty token"));
    }

    // Constant-time key lookup
    let (key, entry) = state
        .api_keys
        .iter()
        .find(|(k, _)| constant_time_eq(k.as_bytes(), token.as_bytes()))
        .map(|(k, e)| (k.clone(), e.clone()))
        .ok_or_else(|| err_json(401, "invalid API key"))?;

    // Rate limit check
    let mut limiters = state.rate_limiters.lock().await;
    let limiter = limiters
        .entry(key)
        .or_insert_with(|| crate::RateLimiter::new(entry.rate));
    if !limiter.allow() {
        return Err(err_json(429, "rate limit exceeded"));
    }

    Ok(AuthenticatedKey { entry })
}

/// Scope check: can this key touch `vault_id` at all?
fn authorize_vault(
    key: &AuthenticatedKey,
    vault_id: &str,
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    match &key.entry.vaults {
        None => Ok(()),
        Some(vaults) if vaults.contains(vault_id) => Ok(()),
        Some(_) => Err(err_json(
            403,
            "API key is not authorized for this vault",
        )),
    }
}

/// Admin check: unscoped keys are the legacy superuser; scoped keys must
/// carry the `+admin` suffix for this vault.
fn vault_admin(key: &AuthenticatedKey, vault_id: &str) -> bool {
    match &key.entry.vaults {
        None => true,
        Some(_) => key.entry.admin_vaults.contains(vault_id),
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

fn err_json(
    status: u16,
    msg: impl Into<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    let code = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (code, Json(json!({"error": msg.into()})))
}

/// Constant-time byte comparison to prevent timing side-channel attacks
/// on API key validation. Pattern from Guardrail's `ct_eq`.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut acc = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        acc |= x ^ y;
    }
    // Prevent compiler from optimizing away the comparison
    std::hint::black_box(acc == 0)
}
