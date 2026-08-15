//! Quota accounting for account-scoped keys.
//!
//! Quotas are enforced pre-insert on push: the batch is projected against
//! the account's usage (REPLACE-aware — overwriting a blob shrinks its
//! bytes) and the WHOLE batch is rejected with 402 before anything is
//! written. Static env keys (no account) are exempt.
//!
//! Device accounting is conservative by design: a REPLACE that moves a
//! memory from device A to device B never frees A's slot, so rewriting
//! history can't dodge the device quota. Only active (deleted = 0) blobs
//! count, and `length(ciphertext)` counts base64 characters — the
//! ciphertext column is what eats storage, so chars ≈ bytes here.

use axum::http::StatusCode;
use axum::Json;
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};
use std::collections::HashSet;

/// Effective per-account limits: the account row's values, falling back
/// to the server defaults (0 = unlimited) per column.
pub fn effective_limits(
    conn: &Connection,
    account_id: &str,
    default_devices: i64,
    default_bytes: i64,
) -> rusqlite::Result<(i64, i64)> {
    let row: Option<(Option<i64>, Option<i64>)> = conn
        .query_row(
            "SELECT quota_devices, quota_bytes FROM accounts WHERE id = ?1",
            rusqlite::params![account_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(match row {
        Some((d, b)) => (d.unwrap_or(default_devices), b.unwrap_or(default_bytes)),
        // Row missing is a corrupt state (sessions cascade from accounts);
        // fall back to server defaults rather than hard-erroring pushes.
        None => (default_devices, default_bytes),
    })
}

/// Vaults an account reaches: its unrevoked scoped keys, or every vault
/// with blobs when it holds an unrevoked unscoped key.
pub fn account_vaults(conn: &Connection, account_id: &str) -> rusqlite::Result<Vec<String>> {
    let has_unscoped: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM api_keys \
         WHERE account_id = ?1 AND revoked = 0 AND vault_id IS NULL)",
        rusqlite::params![account_id],
        |r| r.get(0),
    )?;
    if has_unscoped {
        let mut stmt = conn.prepare("SELECT DISTINCT vault_id FROM sync_blobs WHERE deleted = 0")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        return Ok(rows.filter_map(|r| r.ok()).collect());
    }
    let mut stmt = conn.prepare(
        "SELECT DISTINCT vault_id FROM api_keys \
         WHERE account_id = ?1 AND revoked = 0 AND vault_id IS NOT NULL",
    )?;
    let rows = stmt.query_map(rusqlite::params![account_id], |r| r.get::<_, String>(0))?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// Devices and stored bytes used across a vault set (active blobs only).
pub fn usage_in_vaults(
    conn: &Connection,
    vaults: &[String],
) -> rusqlite::Result<(i64, i64)> {
    if vaults.is_empty() {
        return Ok((0, 0));
    }
    let placeholders = vec!["?"; vaults.len()].join(",");
    let devices: i64 = conn.query_row(
        &format!(
            "SELECT COUNT(DISTINCT device_id) FROM sync_blobs \
             WHERE deleted = 0 AND vault_id IN ({placeholders})"
        ),
        rusqlite::params_from_iter(vaults.iter()),
        |r| r.get(0),
    )?;
    let bytes: i64 = conn.query_row(
        &format!(
            "SELECT COALESCE(SUM(length(ciphertext)), 0) FROM sync_blobs \
             WHERE deleted = 0 AND vault_id IN ({placeholders})"
        ),
        rusqlite::params_from_iter(vaults.iter()),
        |r| r.get(0),
    )?;
    Ok((devices, bytes))
}

/// Active device ids for one vault.
pub fn active_devices(conn: &Connection, vault_id: &str) -> rusqlite::Result<HashSet<String>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT device_id FROM sync_blobs WHERE vault_id = ?1 AND deleted = 0",
    )?;
    let rows = stmt.query_map(rusqlite::params![vault_id], |r| r.get::<_, String>(0))?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// 402 body for quota rejections: `{"error":{"code":"quota_exceeded",
/// "detail":"devices"|"bytes","limit":N,"used":N}}`. Whole-batch reject —
/// nothing was written.
pub fn quota_error(detail: &str, limit: i64, used: i64) -> (StatusCode, Json<Value>) {
    (
        StatusCode::PAYMENT_REQUIRED,
        Json(json!({
            "error": {
                "code": "quota_exceeded",
                "detail": detail,
                "limit": limit,
                "used": used,
            }
        })),
    )
}
