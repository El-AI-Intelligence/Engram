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
            "/v1/vaults/{vault_id}/devices/register",
            axum::routing::post(register_device),
        )
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

    // ── Validation pass: verdict + the row each blob would replace ──────
    // `existing` (vector clock, active length, deleted, device) is fetched
    // once here so quota projection and insertion share the same pre-image.
    let mut entries: Vec<(&SyncBlob, bool, Option<(u64, usize, bool, String)>)> = Vec::new();
    for blob in &req.blobs {
        let mut reject = false;
        if blob.ciphertext.len() > crate::MAX_BLOB_SIZE {
            reject = true; // oversized
        } else if blob.vault_id != vault_id {
            reject = true; // vault_id must match the path
        } else if revoked.contains(&blob.device_id) {
            // A zero-knowledge relay can block pushes; removing the
            // passphrase needs a re-key.
            reject = true;
            revoked_seen.insert(blob.device_id.clone());
        }

        let existing: Option<(u64, usize, bool, String)> = conn
            .query_row(
                "SELECT vector_clock, length(ciphertext), deleted, device_id \
                 FROM sync_blobs WHERE vault_id = ?1 AND memory_id = ?2",
                rusqlite::params![vault_id, blob.memory_id],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get::<_, i64>(1)? as usize,
                        r.get::<_, i32>(2)? != 0,
                        r.get(3)?,
                    ))
                },
            )
            .ok();
        if let Some((clock, _, _, _)) = &existing {
            if blob.vector_clock <= *clock {
                reject = true; // stale vector clock
            }
        }
        entries.push((blob, reject, existing));
    }

    // ── Quota projection (account keys only; static keys are exempt) ────
    // Pre-insert whole-batch check: the batch's projected usage must fit
    // inside the account's limits or nothing is written.
    if let Some(account_id) = key.entry.account_id.clone() {
        let (limit_devices, limit_bytes) = crate::quota::effective_limits(
            &conn,
            &account_id,
            state.default_quota_devices,
            state.default_quota_bytes,
        )
        .map_err(|e| err_json(500, format!("Database error: {e}")))?;
        let vaults = crate::quota::account_vaults(&conn, &account_id)
            .map_err(|e| err_json(500, format!("Database error: {e}")))?;
        let (used_devices, used_bytes) = crate::quota::usage_in_vaults(&conn, &vaults)
            .map_err(|e| err_json(500, format!("Database error: {e}")))?;
        let current_devices = crate::quota::active_devices(&conn, &vault_id)
            .map_err(|e| err_json(500, format!("Database error: {e}")))?;

        // Fold the batch over a virtual per-memory state so a memory_id
        // appearing twice in one batch projects correctly (REPLACE-aware:
        // overwriting shrinks/grows by the delta, not the raw sum).
        let mut virt: std::collections::HashMap<String, (usize, bool)> =
            std::collections::HashMap::new();
        let mut proj_bytes = used_bytes;
        let mut proj_new_devices: HashSet<String> = HashSet::new();
        for (blob, reject, existing) in &entries {
            if *reject {
                continue;
            }
            let initial = match existing {
                Some((_, len, deleted, _)) => (*len, *deleted),
                None => (0, false),
            };
            let slot = virt.entry(blob.memory_id.clone()).or_insert(initial);
            let (old_len, old_deleted) = *slot;
            if !old_deleted {
                proj_bytes -= old_len as i64;
            }
            let new_len = blob.ciphertext.len();
            if !blob.deleted {
                proj_bytes += new_len as i64;
                if !current_devices.contains(&blob.device_id) {
                    proj_new_devices.insert(blob.device_id.clone());
                }
            }
            *slot = (new_len, blob.deleted);
        }
        let proj_devices = used_devices + proj_new_devices.len() as i64;
        if limit_devices > 0 && proj_devices > limit_devices {
            return Err(crate::quota::quota_error(
                "devices",
                limit_devices,
                proj_devices,
            ));
        }
        if limit_bytes > 0 && proj_bytes > limit_bytes {
            return Err(crate::quota::quota_error("bytes", limit_bytes, proj_bytes));
        }
    }

    // ── Insert pass ─────────────────────────────────────────────────────
    let mut accepted = 0usize;
    let mut rejected: Vec<String> = Vec::new();
    for (blob, reject, _) in &entries {
        if *reject {
            rejected.push(blob.memory_id.clone());
            continue;
        }
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
///
/// Auth: API key first (existing behavior). On 401 ONLY, falls back to an
/// account session token — vault visibility then derives from the account's
/// scoped keys (legacy NULL-scoped keys grant nothing). A 429 must never
/// fall through to the session path, or a rate-limited key would bypass its
/// limiter.
async fn pull(
    State(state): State<SyncState>,
    headers: HeaderMap,
    Path(vault_id): Path<String>,
    Query(req): Query<PullRequest>,
) -> Result<Json<PullResponse>, (StatusCode, Json<serde_json::Value>)> {
    match authenticate(&state, &headers).await {
        Ok(key) => authorize_vault(&key, &vault_id)?,
        Err((status, _body)) if status == StatusCode::UNAUTHORIZED => {
            let account_id = crate::account_routes::authenticate_session(&state, &headers).await?;
            let scope = crate::account_routes::account_vault_scope(&state, &account_id).await?;
            authorize_scope(&scope, &vault_id)?;
            rate_limit_session(&state, &account_id).await?;
        }
        Err(e) => return Err(e),
    }
    pull_blobs(&state, &vault_id, req).await
}

/// The SQL body of `pull` — shared by the API-key and session auth paths.
async fn pull_blobs(
    state: &SyncState,
    vault_id: &str,
    req: PullRequest,
) -> Result<Json<PullResponse>, (StatusCode, Json<serde_json::Value>)> {
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
/// shared-vault v0 UI. Auth required, same as `stats`. Labels come from the
/// `device_labels` registry (daemons register their device.json label via
/// `POST .../devices/register`); unregistered devices show `label: null`.
/// The aggregating daemon annotates `is_self` for its UI. Each entry also
/// carries `revoked`, set by the team admin via
/// `DELETE /v1/vaults/{vault_id}/devices/{device_id}`.
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
                    (r.revoked_at IS NOT NULL) AS revoked, l.label \
             FROM sync_blobs s \
             LEFT JOIN revoked_devices r \
                    ON r.vault_id = s.vault_id AND r.device_id = s.device_id \
             LEFT JOIN device_labels l \
                    ON l.vault_id = s.vault_id AND l.device_id = s.device_id \
             WHERE s.vault_id = ?1 \
             GROUP BY s.device_id, r.revoked_at, l.label \
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
                r.get::<_, Option<String>>(4)?,
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
        let (device_id, last_seen, blob_count, revoked, label) = row.map_err(|e| {
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
            "label": label,
        }));
    }

    // Devices that registered a label but have not pushed a blob yet:
    // they have no sync_blobs row, so the blob-driven query above cannot
    // see them. The daemon registers before its first push — without this
    // a freshly-joined device is invisible in the roster.
    let mut stmt = conn
        .prepare(
            "SELECT l.device_id, l.updated_at, (r.revoked_at IS NOT NULL) AS revoked, l.label \
             FROM device_labels l \
             LEFT JOIN revoked_devices r \
                    ON r.vault_id = l.vault_id AND r.device_id = l.device_id \
             WHERE l.vault_id = ?1 AND NOT EXISTS (\
                 SELECT 1 FROM sync_blobs s \
                 WHERE s.vault_id = l.vault_id AND s.device_id = l.device_id)",
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
                r.get::<_, i64>(2)? != 0,
                r.get::<_, Option<String>>(3)?,
            ))
        })
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?;
    for row in rows {
        let (device_id, last_seen, revoked, label) = row.map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?;
        device_list.push(json!({
            "device_id": device_id,
            "last_seen": last_seen,
            "blob_count": 0,
            "revoked": revoked,
            "label": label,
        }));
    }

    // Keep the roster ordered by activity: label-only rows carry their
    // registration time as last_seen, so they sort naturally.
    let mut list = device_list;
    list.sort_by(|a, b| {
        let ta = a.get("last_seen").and_then(|v| v.as_str());
        let tb = b.get("last_seen").and_then(|v| v.as_str());
        tb.cmp(&ta)
    });

    Ok(Json(json!({
        "vault_id": vault_id,
        "devices": list,
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

/// Register (or update) a device's human-readable label in a vault. The
/// daemon calls this at sync-loop start so the roster can show who is who;
/// `label` comes from the device's device.json (≤128 chars). Pure upsert —
/// the device does not need to have pushed a blob yet, because the first
/// register happens before the first push. Auth required; any key scoped
/// to the vault may label devices in it.
#[derive(serde::Deserialize)]
struct DeviceLabelRequest {
    device_id: String,
    label: String,
}

async fn register_device(
    State(state): State<SyncState>,
    headers: HeaderMap,
    Path(vault_id): Path<String>,
    Json(body): Json<DeviceLabelRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let key = authenticate(&state, &headers).await?;
    authorize_vault(&key, &vault_id)?;

    if body.device_id.trim().is_empty() {
        return Err(err_json(400, "device_id must be non-empty"));
    }
    if body.label.trim().is_empty() {
        return Err(err_json(400, "label must be non-empty"));
    }
    if body.label.chars().count() > 128 {
        return Err(err_json(400, "label must be at most 128 characters"));
    }

    let conn = state.conn.lock().await;
    conn.execute(
        "INSERT INTO device_labels (vault_id, device_id, label, updated_at) \
         VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(vault_id, device_id) DO UPDATE SET \
             label = excluded.label, updated_at = excluded.updated_at",
        rusqlite::params![
            vault_id,
            body.device_id,
            body.label,
            chrono::Utc::now().to_rfc3339()
        ],
    )
    .map_err(|e| err_json(500, format!("Database error: {e}")))?;

    Ok(Json(json!({
        "vault_id": vault_id,
        "device_id": body.device_id,
        "label": body.label,
        "registered": true,
    })))
}

// ── Auth ────────────────────────────────────────────────────────────────────

/// The key that authenticated this request (rate-limited).
#[derive(Debug)]
struct AuthenticatedKey {
    entry: Arc<ApiKeyEntry>,
}

/// Authenticate a request using the `Authorization: Bearer <key>` header.
///
/// Two tiers: the static SYNC_API_KEYS map (constant-time scan), then
/// account keys minted via /account/keys (sha256-hash-indexed DB lookup —
/// the relay never stores their plaintext). Auth is optional on loopback
/// only while NO unrevoked account keys exist; once the first account key
/// is minted, keyless loopback requests are rejected like any other
/// (matches Guardrail's default-secure pattern).
async fn authenticate(
    state: &SyncState,
    headers: &HeaderMap,
) -> Result<AuthenticatedKey, (StatusCode, Json<serde_json::Value>)> {
    // Extract Bearer token
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer ").map(str::trim))
        .filter(|t| !t.is_empty());

    // Tier 1: static env keys (constant-time scan).
    if let Some(token) = token {
        if let Some((key, entry)) = state
            .api_keys
            .iter()
            .find(|(k, _)| constant_time_eq(k.as_bytes(), token.as_bytes()))
            .map(|(k, e)| (k.clone(), e.clone()))
        {
            return rate_limit(state, key, entry).await;
        }

        // Tier 2: account keys (hash-indexed DB lookup).
        let key_hash = crate::auth::hash_key(token);
        let conn = state.conn.lock().await;
        let row: Option<(String, f64, Option<String>, String)> = conn
            .query_row(
                "SELECT id, rate, vault_id, account_id FROM api_keys \
                 WHERE key_hash = ?1 AND revoked = 0",
                rusqlite::params![key_hash.to_vec()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .ok();
        drop(conn);
        if let Some((_id, rate, vault_id, account_id)) = row {
            let (vaults, admin_vaults) = match vault_id {
                Some(v) => (
                    Some(HashSet::from([v.clone()])),
                    // A key scoped to a vault administers that vault (its
                    // owner revokes devices there).
                    HashSet::from([v]),
                ),
                None => (None, HashSet::new()),
            };
            let entry = Arc::new(ApiKeyEntry {
                rate: rate.max(1.0),
                vaults,
                admin_vaults,
                account_id: Some(account_id),
            });
            // Limiter bucket = base64url of the key hash — plaintext keys
            // never live in limiter state.
            return rate_limit(state, crate::auth::hash_b64(&key_hash), entry).await;
        }
    }

    // Wildcard superuser: loopback with no static keys AND no unrevoked
    // account keys — only while the relay is effectively unconfigured.
    // The first minted account key flips it to require Bearer auth.
    if state.api_keys.is_empty() && state.is_loopback {
        let conn = state.conn.lock().await;
        let account_key_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM api_keys WHERE revoked = 0",
                [],
                |r| r.get(0),
            )
            .ok()
            .unwrap_or(i64::MAX);
        drop(conn);
        if account_key_count == 0 {
            return Ok(AuthenticatedKey {
                entry: Arc::new(ApiKeyEntry {
                    rate: f64::MAX,
                    vaults: None,
                    admin_vaults: HashSet::new(),
                    account_id: None,
                }),
            });
        }
    }

    Err(err_json(401, "invalid API key"))
}

/// Rate-limit a resolved key and return it. `limiter_key` names the
/// limiter bucket: the key string itself for static keys, the hashed key
/// for account keys.
async fn rate_limit(
    state: &SyncState,
    limiter_key: String,
    entry: Arc<ApiKeyEntry>,
) -> Result<AuthenticatedKey, (StatusCode, Json<serde_json::Value>)> {
    let mut limiters = state.rate_limiters.lock().await;
    let limiter = limiters
        .entry(limiter_key)
        .or_insert_with(|| crate::RateLimiter::new(entry.rate, entry.rate.max(1.0)));
    if !limiter.allow() {
        return Err(err_json(429, "rate limit exceeded"));
    }
    Ok(AuthenticatedKey { entry })
}

/// Scope check: can this key touch `vault_id` at all?
///
/// `None` (unscoped) is the legacy superuser scope: it still passes for
/// static env keys and the pre-configuration loopback wildcard (account-less
/// keys), but account keys minted with a NULL `vault_id` predate per-vault
/// scoping and are policy-denied — re-running `engram pair` / `engram link`
/// mints a scoped key.
fn authorize_vault(
    key: &AuthenticatedKey,
    vault_id: &str,
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    match &key.entry.vaults {
        None if key.entry.account_id.is_none() => Ok(()),
        None => Err(err_json(
            403,
            "this API key predates per-vault scoping — re-run `engram pair` or `engram link`",
        )),
        Some(vaults) if vaults.contains(vault_id) => Ok(()),
        Some(_) => Err(err_json(
            403,
            "API key is not authorized for this vault",
        )),
    }
}

/// Scope check for session-authenticated pulls: `None` = unscoped (any vault).
pub(crate) fn authorize_scope(
    scope: &Option<HashSet<String>>,
    vault_id: &str,
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    match scope {
        None => Ok(()),
        Some(vaults) if vaults.contains(vault_id) => Ok(()),
        Some(_) => Err(err_json(403, "session is not authorized for this vault")),
    }
}

/// Rate-limit a session-authenticated pull at the account's fastest key rate
/// (fallback 100 req/s). Bucket name can't collide with key-hash buckets.
pub(crate) async fn rate_limit_session(
    state: &SyncState,
    account_id: &str,
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    let conn = state.conn.lock().await;
    let rate: Option<f64> = conn
        .query_row(
            "SELECT MAX(rate) FROM api_keys WHERE account_id = ?1 AND revoked = 0",
            rusqlite::params![account_id],
            |r| r.get(0),
        )
        .ok();
    drop(conn);
    let entry = Arc::new(ApiKeyEntry {
        rate: rate.unwrap_or(100.0).max(1.0),
        vaults: None,
        admin_vaults: HashSet::new(),
        account_id: Some(account_id.to_string()),
    });
    rate_limit(state, format!("acct-session:{account_id}"), entry)
        .await
        .map(|_| ())
}

/// Admin check: account-less unscoped keys (static env / loopback wildcard)
/// are the superuser; scoped keys must carry the `+admin` suffix for this
/// vault. Legacy NULL-scoped account keys are never admin — they are
/// policy-denied by `authorize_vault` anyway.
fn vault_admin(key: &AuthenticatedKey, vault_id: &str) -> bool {
    match &key.entry.vaults {
        None => key.entry.account_id.is_none(),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_state() -> SyncState {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "PRAGMA foreign_keys=ON;
             CREATE TABLE sync_blobs (
                 vault_id TEXT NOT NULL, memory_id TEXT NOT NULL, device_id TEXT NOT NULL,
                 vector_clock INTEGER NOT NULL DEFAULT 0, ciphertext TEXT NOT NULL,
                 hmac TEXT NOT NULL, created_at TEXT NOT NULL, deleted INTEGER NOT NULL DEFAULT 0,
                 PRIMARY KEY (vault_id, memory_id)
             );
             CREATE TABLE api_keys (
                 id TEXT PRIMARY KEY, account_id TEXT NOT NULL, key_hash BLOB NOT NULL UNIQUE,
                 key_prefix TEXT NOT NULL, rate REAL NOT NULL DEFAULT 100, vault_id TEXT,
                 created_at TEXT NOT NULL, revoked INTEGER NOT NULL DEFAULT 0
             );
             CREATE TABLE accounts (
                 id TEXT PRIMARY KEY, created_at TEXT NOT NULL, last_login_at TEXT,
                 quota_devices INTEGER, quota_bytes INTEGER
             );
             CREATE TABLE revoked_devices (
                 vault_id TEXT NOT NULL, device_id TEXT NOT NULL, revoked_at TEXT NOT NULL,
                 PRIMARY KEY (vault_id, device_id)
             );
             CREATE TABLE device_labels (
                 vault_id TEXT NOT NULL, device_id TEXT NOT NULL, label TEXT NOT NULL,
                 updated_at TEXT NOT NULL, PRIMARY KEY (vault_id, device_id)
             );
             CREATE TABLE sessions (
                 token_hash BLOB PRIMARY KEY, account_id TEXT NOT NULL,
                 created_at TEXT NOT NULL, expires_at TEXT NOT NULL
             );",
        )
        .unwrap();
        SyncState {
            conn: Arc::new(tokio::sync::Mutex::new(conn)),
            start_time: chrono::Utc::now(),
            data_dir: std::path::PathBuf::from("/tmp"),
            api_keys: Arc::new(Default::default()),
            rate_limiters: Arc::new(tokio::sync::Mutex::new(Default::default())),
            is_loopback: true,
            rp_id: "localhost".into(),
            allowed_origins: Arc::new(Default::default()),
            default_quota_devices: 0,
            default_quota_bytes: 0,
            webauthn: Arc::new(crate::auth::build_webauthn("localhost", "http://localhost:8787").unwrap()),
            auth_store: Arc::new(crate::auth::WebauthnStore::new()),
            smtp: None,
        }
    }

    fn bearer(token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", format!("Bearer {token}").parse().unwrap());
        headers
    }

    #[tokio::test]
    async fn loopback_wildcard_active_until_first_account_key() {
        let state = test_state();
        // Fresh server: no static keys, no account keys → wildcard.
        let key = authenticate(&state, &HeaderMap::new()).await.expect("wildcard superuser");
        assert!(key.entry.vaults.is_none());
        assert!(key.entry.account_id.is_none());

        // Mint one account key → keyless requests are rejected.
        {
            let conn = state.conn.lock().await;
            conn.execute(
                "INSERT INTO api_keys (id, account_id, key_hash, key_prefix, rate, vault_id, created_at, revoked) \
                 VALUES ('k1', 'acct-1', ?1, 'en_', 100, NULL, 'now', 0)",
                rusqlite::params![
                    crate::auth::hash_key("en_0123456789012345678901234567890123456789").to_vec()
                ],
            )
            .unwrap();
        }
        let err = authenticate(&state, &HeaderMap::new()).await.expect_err("keyless rejected");
        assert_eq!(err.0, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn account_key_authenticates_with_scope_and_owner() {
        let state = test_state();
        let plain = "en_0000000000000000000000000000000000000000ab";
        {
            let conn = state.conn.lock().await;
            conn.execute(
                "INSERT INTO api_keys (id, account_id, key_hash, key_prefix, rate, vault_id, created_at, revoked) \
                 VALUES ('k1', 'acct-1', ?1, 'en_', 5, 'vault-a', 'now', 0)",
                rusqlite::params![crate::auth::hash_key(plain).to_vec()],
            )
            .unwrap();
        }

        let key = authenticate(&state, &bearer(plain)).await.expect("account key accepted");
        assert_eq!(key.entry.account_id.as_deref(), Some("acct-1"));
        assert_eq!(key.entry.rate, 5.0);
        let vaults = key.entry.vaults.as_ref().unwrap();
        assert!(vaults.contains("vault-a"));
        assert!(
            key.entry.admin_vaults.contains("vault-a"),
            "scoped key administers its vault"
        );

        assert!(authorize_vault(&key, "vault-a").is_ok());
        assert!(authorize_vault(&key, "vault-b").is_err());
        assert!(vault_admin(&key, "vault-a"));
        assert!(!vault_admin(&key, "vault-b"));
    }

    #[tokio::test]
    async fn revoked_account_key_is_rejected() {
        let state = test_state();
        let plain = "en_0000000000000000000000000000000000000000cd";
        {
            let conn = state.conn.lock().await;
            // A live key from another account keeps the wildcard off so the
            // revoked key is genuinely rejected (not wildcarded).
            conn.execute(
                "INSERT INTO api_keys (id, account_id, key_hash, key_prefix, rate, vault_id, created_at, revoked) \
                 VALUES ('live', 'acct-2', ?1, 'en_', 100, NULL, 'now', 0)",
                rusqlite::params![crate::auth::hash_key("en_other").to_vec()],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO api_keys (id, account_id, key_hash, key_prefix, rate, vault_id, created_at, revoked) \
                 VALUES ('k1', 'acct-1', ?1, 'en_', 100, NULL, 'now', 1)",
                rusqlite::params![crate::auth::hash_key(plain).to_vec()],
            )
            .unwrap();
        }
        let err = authenticate(&state, &bearer(plain)).await.expect_err("revoked → 401");
        assert_eq!(err.0, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn static_keys_win_and_wildcard_stays_gated() {
        let mut state = test_state();
        let static_key = "static-key-0000001";
        Arc::get_mut(&mut state.api_keys)
            .unwrap()
            .insert(
                static_key.into(),
                Arc::new(ApiKeyEntry {
                    rate: 100.0,
                    vaults: None,
                    admin_vaults: HashSet::new(),
                    account_id: None,
                }),
            );

        let key = authenticate(&state, &bearer(static_key)).await.expect("static key works");
        assert!(key.entry.account_id.is_none());

        // With static keys configured, keyless is rejected even on loopback.
        let err = authenticate(&state, &HeaderMap::new())
            .await
            .expect_err("static keys → auth required");
        assert_eq!(err.0, StatusCode::UNAUTHORIZED);
    }

    // ── Quotas (Phase 4) ────────────────────────────────────────────────

    fn blob(vault: &str, mid: &str, dev: &str, clock: u64, text: &str) -> axiom_engram::sync::SyncBlob {
        axiom_engram::sync::SyncBlob {
            vault_id: vault.into(),
            memory_id: mid.into(),
            device_id: dev.into(),
            vector_clock: clock,
            ciphertext: text.into(),
            hmac: "h".into(),
            created_at: "now".into(),
            deleted: false,
        }
    }

    async fn seed_account(state: &SyncState, account_id: &str, quota_devices: i64, quota_bytes: i64) {
        let conn = state.conn.lock().await;
        conn.execute(
            "INSERT INTO accounts (id, created_at, quota_devices, quota_bytes) \
             VALUES (?1, 'now', ?2, ?3)",
            rusqlite::params![account_id, quota_devices, quota_bytes],
        )
        .unwrap();
    }

    async fn seed_account_key(state: &SyncState, account_id: &str, vault: Option<&str>, plain: &str) {
        let conn = state.conn.lock().await;
        conn.execute(
            "INSERT INTO api_keys (id, account_id, key_hash, key_prefix, rate, vault_id, created_at, revoked) \
             VALUES (?1, ?2, ?3, 'en_', 100, ?4, 'now', 0)",
            rusqlite::params![
                format!("id-{plain}"),
                account_id,
                crate::auth::hash_key(plain).to_vec(),
                vault,
            ],
        )
        .unwrap();
    }

    async fn do_push(
        state: &SyncState,
        token: &str,
        vault: &str,
        blobs: Vec<axiom_engram::sync::SyncBlob>,
    ) -> Result<axum::Json<PushResponse>, (StatusCode, Json<serde_json::Value>)> {
        push(
            State(state.clone()),
            bearer(token),
            Path(vault.to_string()),
            Json(PushRequest { blobs }),
        )
        .await
    }

    // ── Session pull fallback ────────────────────────────────────────────

    async fn seed_session(state: &SyncState, account_id: &str, token: &str) {
        let conn = state.conn.lock().await;
        conn.execute(
            "INSERT OR IGNORE INTO accounts (id, created_at) VALUES (?1, 'now')",
            rusqlite::params![account_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (token_hash, account_id, created_at, expires_at) \
             VALUES (?1, ?2, 'now', '2999-01-01T00:00:00Z')",
            rusqlite::params![crate::auth::hash_token(token).to_vec(), account_id],
        )
        .unwrap();
    }

    async fn do_session_pull(
        state: &SyncState,
        token: &str,
        vault: &str,
    ) -> Result<axum::Json<PullResponse>, (StatusCode, Json<serde_json::Value>)> {
        pull(
            State(state.clone()),
            bearer(token),
            Path(vault.to_string()),
            Query(axiom_engram::sync::PullRequest {
                since: None,
                limit: 1000,
            }),
        )
        .await
    }

    async fn seed_blob(state: &SyncState, vault: &str, mid: &str) {
        let conn = state.conn.lock().await;
        conn.execute(
            "INSERT INTO sync_blobs (vault_id, memory_id, device_id, vector_clock, ciphertext, hmac, created_at, deleted) \
             VALUES (?1, ?2, 'dev-1', 1, 'x', 'h', 'now', 0)",
            rusqlite::params![vault, mid],
        )
        .unwrap();
    }

    #[tokio::test]
    async fn pull_with_session_scoped_account_ok() {
        let state = test_state();
        seed_account(&state, "acct-1", 0, 0).await;
        seed_account_key(&state, "acct-1", Some("vault-a"), "en_scoped").await;
        seed_session(&state, "acct-1", "tok-1").await;
        seed_blob(&state, "vault-a", "m1").await;
        let res = do_session_pull(&state, "tok-1", "vault-a")
            .await
            .expect("session pull ok");
        assert_eq!(res.blobs.len(), 1);
        assert_eq!(res.blobs[0].memory_id, "m1");
        assert!(!res.has_more);
    }

    /// Legacy NULL-scoped account keys grant nothing: not via the API-key
    /// path, and not via sessions either (account_vault_scope only counts
    /// scoped keys). Re-linking mints a scoped key.
    #[tokio::test]
    async fn pull_with_session_legacy_null_key_denied() {
        let state = test_state();
        seed_account(&state, "acct-1", 0, 0).await;
        seed_account_key(&state, "acct-1", None, "en_unscoped").await;
        seed_session(&state, "acct-1", "tok-1").await;
        seed_blob(&state, "vault-a", "m1").await;
        let err = do_session_pull(&state, "tok-1", "vault-a")
            .await
            .expect_err("legacy NULL keys grant no vault access");
        assert_eq!(err.0, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn legacy_null_account_key_policy_denied_with_relink_message() {
        let state = test_state();
        seed_account(&state, "acct-1", 0, 0).await;
        seed_account_key(&state, "acct-1", None, "en_unscoped").await;
        seed_blob(&state, "vault-a", "m1").await;
        // Push path — legacy NULL-scoped account key hits the policy deny.
        let blob = axiom_engram::sync::SyncBlob {
            vault_id: "vault-a".into(),
            memory_id: "m9".into(),
            device_id: "dev-1".into(),
            vector_clock: 2,
            ciphertext: "x".into(),
            hmac: "h".into(),
            created_at: "2026-08-20T00:00:00Z".into(),
            deleted: false,
        };
        let err = do_push(&state, "en_unscoped", "vault-a", vec![blob])
            .await
            .expect_err("legacy NULL key must be denied");
        assert_eq!(err.0, StatusCode::FORBIDDEN);
        let body = err.1;
        let msg = body["error"].as_str().unwrap_or_default();
        assert!(msg.contains("re-run"), "403 must carry the re-link hint: {msg}");
        assert!(msg.contains("engram"), "403 must name the re-link commands: {msg}");
    }

    #[tokio::test]
    async fn unscoped_static_key_stays_superuser() {
        let mut state = test_state();
        // A static env key is account-less and unscoped — the wildcard
        // superuser path. Seed an account key so authenticate() can't take
        // the loopback branch, then register a static key.
        seed_account(&state, "acct-1", 0, 0).await;
        seed_account_key(&state, "acct-1", Some("vault-a"), "en_scoped").await;
        Arc::get_mut(&mut state.api_keys)
            .unwrap()
            .insert(
                "static-super".into(),
                Arc::new(ApiKeyEntry {
                    rate: 1000.0,
                    vaults: None,
                    admin_vaults: HashSet::new(),
                    account_id: None,
                }),
            );
        seed_blob(&state, "vault-any", "m1").await;
        let blob = axiom_engram::sync::SyncBlob {
            vault_id: "vault-any".into(),
            memory_id: "m2".into(),
            device_id: "dev-1".into(),
            vector_clock: 2,
            ciphertext: "x".into(),
            hmac: "h".into(),
            created_at: "2026-08-20T00:00:00Z".into(),
            deleted: false,
        };
        let res = do_push(&state, "static-super", "vault-any", vec![blob])
            .await
            .expect("static env keys remain superuser");
        assert_eq!(res.0.accepted, 1);
    }

    #[tokio::test]
    async fn pull_with_session_scoped_account_forbidden_for_other_vault() {
        let state = test_state();
        seed_account(&state, "acct-1", 0, 0).await;
        seed_account_key(&state, "acct-1", Some("vault-a"), "en_scoped").await;
        seed_session(&state, "acct-1", "tok-1").await;
        seed_blob(&state, "vault-b", "m1").await;
        let err = do_session_pull(&state, "tok-1", "vault-b")
            .await
            .expect_err("scoped session can't read other vaults");
        assert_eq!(err.0, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn pull_with_session_invalid_token_returns_401() {
        let state = test_state();
        // An existing account key keeps the loopback wildcard off, so the
        // bogus token is genuinely rejected (not wildcarded).
        seed_account(&state, "acct-1", 0, 0).await;
        seed_account_key(&state, "acct-1", None, "en_other").await;
        let err = do_session_pull(&state, "nope", "vault-a")
            .await
            .expect_err("bad session token → 401");
        assert_eq!(err.0, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn pull_with_api_key_still_works() {
        let state = test_state();
        seed_account(&state, "acct-1", 0, 0).await;
        seed_account_key(&state, "acct-1", Some("vault-a"), "en_0123456789012345678901234567890123456789").await;
        seed_blob(&state, "vault-a", "m1").await;
        let res = do_session_pull(&state, "en_0123456789012345678901234567890123456789", "vault-a")
            .await
            .expect("api key pull unaffected");
        assert_eq!(res.blobs.len(), 1);
    }

    #[tokio::test]
    async fn pull_rate_limited_api_key_does_not_fallback_to_session() {
        let state = test_state();
        seed_account(&state, "acct-1", 0, 0).await;
        seed_session(&state, "acct-1", "tok-1").await;
        // rate 1.0: first request passes, second is 429 — and must NOT fall
        // through to the (valid) session path. Scoped to vault-a: NULL-scoped
        // keys are policy-denied before the limiter ever runs.
        {
            let conn = state.conn.lock().await;
            conn.execute(
                "INSERT INTO api_keys (id, account_id, key_hash, key_prefix, rate, vault_id, created_at, revoked) \
                 VALUES ('k1', 'acct-1', ?1, 'en_', 1.0, 'vault-a', 'now', 0)",
                rusqlite::params![crate::auth::hash_key("en_slow").to_vec()],
            )
            .unwrap();
        }
        seed_blob(&state, "vault-a", "m1").await;
        let first = do_session_pull(&state, "en_slow", "vault-a")
            .await
            .expect("first pull passes");
        assert_eq!(first.blobs.len(), 1);
        let err = do_session_pull(&state, "en_slow", "vault-a")
            .await
            .expect_err("second pull is rate limited");
        assert_eq!(err.0, StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn pull_session_rate_limited() {
        let state = test_state();
        seed_account(&state, "acct-1", 0, 0).await;
        seed_session(&state, "acct-1", "tok-1").await;
        // The seeded key keeps the loopback wildcard off AND scopes the
        // account to vault-a — a NULL scope would be policy-denied.
        {
            let conn = state.conn.lock().await;
            conn.execute(
                "INSERT INTO api_keys (id, account_id, key_hash, key_prefix, rate, vault_id, created_at, revoked) \
                 VALUES ('k1', 'acct-1', ?1, 'en_', 1.0, 'vault-a', 'now', 0)",
                rusqlite::params![crate::auth::hash_key("en_slow").to_vec()],
            )
            .unwrap();
        }
        seed_blob(&state, "vault-a", "m1").await;
        let first = do_session_pull(&state, "tok-1", "vault-a")
            .await
            .expect("first session pull passes");
        assert_eq!(first.blobs.len(), 1);
        let err = do_session_pull(&state, "tok-1", "vault-a")
            .await
            .expect_err("second session pull is rate limited");
        assert_eq!(err.0, StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn quota_blocks_new_device_over_limit() {
        let state = test_state();
        seed_account(&state, "acct-1", 1, 0).await; // 1 device, unlimited bytes
        seed_account_key(&state, "acct-1", Some("vault-a"), "en_devquota").await;
        // dev-1 already holds an active blob in vault-a
        {
            let conn = state.conn.lock().await;
            conn.execute(
                "INSERT INTO sync_blobs (vault_id, memory_id, device_id, vector_clock, ciphertext, hmac, created_at, deleted) \
                 VALUES ('vault-a', 'm0', 'dev-1', 1, 'x', 'h', 'now', 0)",
                [],
            )
            .unwrap();
        }
        // dev-2 pushes → projected devices = 2 > 1 → 402, nothing written
        let err = do_push(
            &state,
            "en_devquota",
            "vault-a",
            vec![blob("vault-a", "m1", "dev-2", 1, "abc")],
        )
        .await
        .expect_err("device quota");
        assert_eq!(err.0, StatusCode::PAYMENT_REQUIRED);
        assert_eq!(err.1["error"]["code"], "quota_exceeded");
        assert_eq!(err.1["error"]["detail"], "devices");
        assert_eq!(err.1["error"]["limit"], 1);
        assert_eq!(err.1["error"]["used"], 2);
        let conn = state.conn.lock().await;
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sync_blobs WHERE memory_id = 'm1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0, "whole batch rejected — nothing written");
    }

    #[tokio::test]
    async fn quota_counts_same_device_once_in_batch() {
        let state = test_state();
        seed_account(&state, "acct-1", 1, 0).await;
        seed_account_key(&state, "acct-1", Some("vault-a"), "en_samedev").await;
        // Two blobs from the same new device in one batch → 1 device.
        let res = do_push(
            &state,
            "en_samedev",
            "vault-a",
            vec![
                blob("vault-a", "m1", "dev-1", 1, "aa"),
                blob("vault-a", "m2", "dev-1", 1, "bb"),
            ],
        )
        .await
        .expect("one device, two blobs");
        assert_eq!(res.accepted, 2);
        assert!(res.rejected.is_empty());
    }

    #[tokio::test]
    async fn quota_replace_shrink_fits_but_growth_blocks() {
        let state = test_state();
        seed_account(&state, "acct-1", 0, 10).await; // 10-byte quota
        seed_account_key(&state, "acct-1", Some("vault-a"), "en_bytes").await;
        {
            let conn = state.conn.lock().await;
            conn.execute(
                "INSERT INTO sync_blobs (vault_id, memory_id, device_id, vector_clock, ciphertext, hmac, created_at, deleted) \
                 VALUES ('vault-a', 'm1', 'dev-1', 1, 'xxxx', 'h', 'now', 0)",
                [],
            )
            .unwrap();
        }
        // Overwrite big→small: projected 2 ≤ 10 → accepted.
        let res = do_push(
            &state,
            "en_bytes",
            "vault-a",
            vec![blob("vault-a", "m1", "dev-1", 2, "xx")],
        )
        .await
        .expect("REPLACE-shrink fits");
        assert_eq!(res.accepted, 1);

        // New blob pushes projected bytes to 2 + 9 = 11 > 10 → 402.
        let err = do_push(
            &state,
            "en_bytes",
            "vault-a",
            vec![blob("vault-a", "m2", "dev-1", 1, "123456789")],
        )
        .await
        .expect_err("bytes quota");
        assert_eq!(err.0, StatusCode::PAYMENT_REQUIRED);
        assert_eq!(err.1["error"]["detail"], "bytes");
        assert_eq!(err.1["error"]["limit"], 10);
        assert_eq!(err.1["error"]["used"], 11);
    }

    #[tokio::test]
    async fn zero_limits_are_unlimited() {
        let state = test_state();
        seed_account(&state, "acct-1", 0, 0).await;
        seed_account_key(&state, "acct-1", Some("vault-a"), "en_unlim").await;
        let res = do_push(
            &state,
            "en_unlim",
            "vault-a",
            vec![blob("vault-a", "m1", "dev-1", 1, &"x".repeat(5000))],
        )
        .await
        .expect("0 = unlimited");
        assert_eq!(res.accepted, 1);
    }

    #[tokio::test]
    async fn static_keys_are_exempt_from_quota() {
        let mut state = test_state();
        // Account with a tiny quota — but the request uses a static key.
        seed_account(&state, "acct-1", 1, 1).await;
        let static_key = "static-key-0000002";
        Arc::get_mut(&mut state.api_keys)
            .unwrap()
            .insert(
                static_key.into(),
                Arc::new(ApiKeyEntry {
                    rate: 100.0,
                    vaults: None,
                    admin_vaults: HashSet::new(),
                    account_id: None,
                }),
            );
        let res = do_push(
            &state,
            static_key,
            "vault-a",
            vec![blob("vault-a", "m1", "dev-1", 1, "big blob way over one byte")],
        )
        .await
        .expect("static keys exempt");
        assert_eq!(res.accepted, 1);
    }

    // ── Device registry (Phase 5) ────────────────────────────────────────

    async fn do_register(
        state: &SyncState,
        token: &str,
        vault: &str,
        device_id: &str,
        label: &str,
    ) -> Result<axum::Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
        register_device(
            State(state.clone()),
            bearer(token),
            Path(vault.to_string()),
            Json(DeviceLabelRequest {
                device_id: device_id.into(),
                label: label.into(),
            }),
        )
        .await
    }

    async fn do_devices(
        state: &SyncState,
        token: &str,
        vault: &str,
    ) -> Result<axum::Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
        devices(State(state.clone()), bearer(token), Path(vault.to_string())).await
    }

    #[tokio::test]
    async fn register_device_upserts_label_and_roster_shows_it() {
        let state = test_state();
        seed_account_key(&state, "acct-1", Some("vault-a"), "en_devreg").await;

        // First register, then re-register with a new label → upsert.
        let res = do_register(&state, "en_devreg", "vault-a", "dev-1", "Laptop")
            .await
            .expect("register works");
        assert_eq!(res["registered"], true);
        let res = do_register(&state, "en_devreg", "vault-a", "dev-1", "Work laptop")
            .await
            .expect("re-register upserts");
        assert_eq!(res["label"], "Work laptop");

        // Blobs from dev-1 (labeled) and dev-2 (never registered).
        {
            let conn = state.conn.lock().await;
            for (mid, dev) in [("m1", "dev-1"), ("m2", "dev-2")] {
                conn.execute(
                    "INSERT INTO sync_blobs (vault_id, memory_id, device_id, vector_clock, ciphertext, hmac, created_at, deleted) \
                     VALUES ('vault-a', ?1, ?2, 1, 'x', 'h', 'now', 0)",
                    rusqlite::params![mid, dev],
                )
                .unwrap();
            }
        }

        let roster = do_devices(&state, "en_devreg", "vault-a")
            .await
            .expect("roster works");
        let devices = roster["devices"].as_array().unwrap();
        assert_eq!(devices.len(), 2);
        let labeled = devices.iter().find(|d| d["device_id"] == "dev-1").unwrap();
        assert_eq!(labeled["label"], "Work laptop");
        let unlabeled = devices.iter().find(|d| d["device_id"] == "dev-2").unwrap();
        assert_eq!(unlabeled["label"], serde_json::Value::Null);

        // Upsert, not insert: exactly one label row for dev-1.
        let conn = state.conn.lock().await;
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM device_labels WHERE vault_id = 'vault-a' AND device_id = 'dev-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }

    #[tokio::test]
    async fn roster_lists_labeled_device_before_its_first_push() {
        let state = test_state();
        seed_account_key(&state, "acct-1", Some("vault-a"), "en_devreg").await;

        // Label only — no sync_blobs row yet (the daemon registers at
        // sync-loop start, before its first push).
        let _ = do_register(&state, "en_devreg", "vault-a", "dev-fresh", "New phone")
            .await
            .expect("register works");

        let roster = do_devices(&state, "en_devreg", "vault-a")
            .await
            .expect("roster works");
        let devices = roster["devices"].as_array().unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0]["device_id"], "dev-fresh");
        assert_eq!(devices[0]["label"], "New phone");
        assert_eq!(devices[0]["blob_count"], 0);
        assert_eq!(devices[0]["revoked"], false);

        // After the first push the same device still appears once, now
        // blob-driven with the label attached.
        {
            let conn = state.conn.lock().await;
            conn.execute(
                "INSERT INTO sync_blobs (vault_id, memory_id, device_id, vector_clock, ciphertext, hmac, created_at, deleted) \
                 VALUES ('vault-a', 'm1', 'dev-fresh', 1, 'x', 'h', 'now', 0)",
                [],
            )
            .unwrap();
        }
        let roster = do_devices(&state, "en_devreg", "vault-a")
            .await
            .expect("roster works");
        let devices = roster["devices"].as_array().unwrap();
        assert_eq!(devices.len(), 1, "no duplicate rows after first push");
        assert_eq!(devices[0]["label"], "New phone");
        assert_eq!(devices[0]["blob_count"], 1);
    }

    #[tokio::test]
    async fn register_device_rejects_bad_labels() {
        let state = test_state();
        seed_account_key(&state, "acct-1", Some("vault-a"), "en_badlabel").await;

        let err = do_register(&state, "en_badlabel", "vault-a", "dev-1", "")
            .await
            .expect_err("empty label");
        assert_eq!(err.0, StatusCode::BAD_REQUEST);

        let err = do_register(&state, "en_badlabel", "vault-a", "dev-1", &"x".repeat(129))
            .await
            .expect_err("label too long");
        assert_eq!(err.0, StatusCode::BAD_REQUEST);

        let err = do_register(&state, "en_badlabel", "vault-a", "  ", "Label")
            .await
            .expect_err("empty device id");
        assert_eq!(err.0, StatusCode::BAD_REQUEST);

        // The 128-char boundary itself is accepted.
        let res = do_register(&state, "en_badlabel", "vault-a", "dev-1", &"x".repeat(128))
            .await
            .expect("128 chars is the limit, inclusive");
        assert_eq!(res["registered"], true);
    }

    #[tokio::test]
    async fn register_device_requires_auth_and_vault_scope() {
        let state = test_state();
        // Key scoped to vault-b: turns the loopback wildcard off and
        // authorizes only vault-b.
        seed_account_key(&state, "acct-1", Some("vault-b"), "en_scoped").await;

        let err = do_register(&state, "en_scoped", "vault-a", "dev-1", "Laptop")
            .await
            .expect_err("out of scope");
        assert_eq!(err.0, StatusCode::FORBIDDEN);

        let err = do_register(&state, "en_wrong", "vault-a", "dev-1", "Laptop")
            .await
            .expect_err("bad key");
        assert_eq!(err.0, StatusCode::UNAUTHORIZED);

        let res = do_register(&state, "en_scoped", "vault-b", "dev-1", "Laptop")
            .await
            .expect("scoped key registers in its vault");
        assert_eq!(res["vault_id"], "vault-b");
    }
}
