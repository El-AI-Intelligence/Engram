//! Account routes — passkey registration/login, sessions.
//!
//! The account model is deliberately pseudonymous: an account is an opaque
//! UUID plus its passkeys. No email, name, or other PII ever reaches the
//! relay (billing, which needs an email, is a separate private service —
//! roadmap 1.3).
//!
//! Ceremony flow (webauthn-rs two-step, in-memory challenge store):
//!   register/start → browser create() → register/finish  (no session = new
//!   account; valid session = add a passkey to that account)
//!   login/start    → browser get()    → login/finish
//! Sessions are opaque Bearer tokens (sha256 at rest, 7-day TTL) kept in
//! the browser's localStorage — cross-site cookies won't stick between the
//! vault UI origin and the relay origin.

use crate::auth::{self, SESSION_TTL};
use crate::SyncState;
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{delete, get, post},
    Json, Router,
};
use rusqlite::OptionalExtension;
use serde_json::{json, Value};
use webauthn_rs::prelude::*;
use webauthn_rs_proto::{AuthenticatorSelectionCriteria, ResidentKeyRequirement, UserVerificationPolicy};

type ApiError = (StatusCode, Json<Value>);

pub fn router() -> Router<SyncState> {
    Router::new()
        .route("/auth/register/start", post(register_start))
        .route("/auth/register/finish", post(register_finish))
        .route("/auth/login/start", post(login_start))
        .route("/auth/login/finish", post(login_finish))
        .route("/auth/logout", post(logout))
        .route("/account", get(get_account))
        .route("/account/keys", post(create_account_key))
        .route("/account/keys/{key_id}", delete(revoke_account_key))
}

// ── Register ────────────────────────────────────────────────────────────────

async fn register_start(
    State(state): State<SyncState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let origin = str_field(&body, "origin")?;
    validate_origin(&state, origin)?;

    // A valid session means "add another passkey to my account"; otherwise
    // this ceremony creates a brand-new account.
    let attach_account = match authenticate_session(&state, &headers).await {
        Ok(account_id) => Some(account_id),
        Err(e) if e.0 == StatusCode::UNAUTHORIZED => None,
        Err(e) => return Err(e),
    };
    let user_unique_id = attach_account
        .as_ref()
        .and_then(|a| Uuid::parse_str(a).ok())
        .unwrap_or_else(Uuid::new_v4);
    // The account id IS the WebAuthn user handle: existing account when
    // attaching, fresh uuid when this ceremony creates the account.
    let account_id = attach_account
        .clone()
        .unwrap_or_else(|| user_unique_id.to_string());

    let (mut challenge, reg_state) = state
        .webauthn
        .start_passkey_registration(user_unique_id, "engram-account", "Engram Account", None)
        .map_err(|e| err_json(500, "webauthn_error", &e.to_string()))?;

    // Belt-and-braces: 0.5's passkey registration already requests resident
    // keys, but some authenticators only honor an explicit request. This is
    // the guardrail-hardened pattern (resident keys + no reliance on hints).
    let selection = challenge
        .public_key
        .authenticator_selection
        .get_or_insert_with(|| AuthenticatorSelectionCriteria {
            authenticator_attachment: None,
            resident_key: Some(ResidentKeyRequirement::Required),
            require_resident_key: true,
            user_verification: UserVerificationPolicy::Required,
        });
    selection.resident_key = Some(ResidentKeyRequirement::Required);
    selection.require_resident_key = true;

    let challenge_id = Uuid::new_v4().to_string();
    state
        .auth_store
        .put_registration(challenge_id.clone(), challenge.clone(), reg_state, Some(account_id));

    Ok(Json(json!({
        "challenge_id": challenge_id,
        "challenge": serde_json::to_value(&challenge)
            .map_err(|e| err_json(500, "serialize_error", &e.to_string()))?,
    })))
}

async fn register_finish(
    State(state): State<SyncState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let origin = str_field(&body, "origin")?;
    validate_origin(&state, origin)?;

    let challenge_id = str_field(&body, "challenge_id")?;
    let (_, reg_state, account) = state
        .auth_store
        .take_registration(challenge_id, std::time::Instant::now())
        .ok_or_else(|| {
            err_json(
                400,
                "invalid_challenge",
                "unknown or expired registration challenge",
            )
        })?;
    // `account` is always Some (set in start); the fallback is unreachable
    // but keeps the tuple shape honest.
    let account_id = account.unwrap_or_else(|| Uuid::new_v4().to_string());

    let registration: RegisterPublicKeyCredential =
        serde_json::from_value(body.get("registration").cloned().unwrap_or(Value::Null))
            .map_err(|e| err_json(400, "bad_registration", &e.to_string()))?;

    let passkey = state
        .webauthn
        .finish_passkey_registration(&registration, &reg_state)
        .map_err(|e| err_json(400, "registration_failed", &e.to_string()))?;

    let credential_id = passkey.cred_id().to_vec();
    let public_key = serde_json::to_vec(&passkey)
        .map_err(|e| err_json(500, "serialize_error", &e.to_string()))?;

    let now = chrono::Utc::now();
    let now_str = now.to_rfc3339();
    let conn = state.conn.lock().await;

    // New accounts get created here; attaching to an existing account is a
    // no-op thanks to OR IGNORE.
    conn.execute(
        "INSERT OR IGNORE INTO accounts (id, created_at, last_login_at) VALUES (?1, ?2, ?2)",
        rusqlite::params![account_id, now_str],
    )
    .map_err(|e| err_json(500, "database_error", &e.to_string()))?;

    let insert = conn.execute(
        "INSERT INTO passkeys (account_id, credential_id, public_key, created_at) \
         VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![account_id, credential_id, public_key, now_str],
    );
    match insert {
        Ok(_) => {}
        // Same passkey submitted twice (double click, retry): the credential
        // already exists — behave like a login instead of erroring.
        Err(e) if is_unique_violation(&e) => {
            let existing: Option<String> = conn
                .query_row(
                    "SELECT account_id FROM passkeys WHERE credential_id = ?1",
                    rusqlite::params![credential_id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
            match existing {
                Some(a) => {
                    let session_token = mint_and_store_session(&conn, &a)?;
                    return Ok(Json(json!({
                        "account_id": a,
                        "session_token": session_token,
                        "already_registered": true,
                    })));
                }
                None => {
                    return Err(err_json(
                        409,
                        "passkey_conflict",
                        "passkey already registered to another account",
                    ))
                }
            }
        }
        Err(e) => return Err(err_json(500, "database_error", &e.to_string())),
    }

    conn.execute(
        "UPDATE accounts SET last_login_at = ?1 WHERE id = ?2",
        rusqlite::params![now_str, account_id],
    )
    .map_err(|e| err_json(500, "database_error", &e.to_string()))?;

    let session_token = mint_and_store_session(&conn, &account_id)?;
    Ok(Json(json!({
        "account_id": account_id,
        "session_token": session_token,
    })))
}

// ── Login ───────────────────────────────────────────────────────────────────

async fn login_start(
    State(state): State<SyncState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let origin = str_field(&body, "origin")?;
    validate_origin(&state, origin)?;

    let conn = state.conn.lock().await;
    // User-less login: load every stored passkey and let the browser offer
    // them as a picker. The credential is identified in finish (the auth
    // state carries the allowed-credential list).
    let mut stmt = conn
        .prepare("SELECT public_key FROM passkeys")
        .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    let rows = stmt
        .query_map([], |r| r.get::<_, Vec<u8>>(0))
        .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    let mut passkeys: Vec<Passkey> = Vec::new();
    for row in rows {
        let bytes = row.map_err(|e| err_json(500, "database_error", &e.to_string()))?;
        passkeys.push(
            serde_json::from_slice(&bytes)
                .map_err(|e| err_json(500, "corrupt_passkey", &e.to_string()))?,
        );
    }
    if passkeys.is_empty() {
        return Err(err_json(
            409,
            "no_passkeys",
            "no passkeys registered on this server yet — register first",
        ));
    }

    let (challenge, auth_state) = state
        .webauthn
        .start_passkey_authentication(&passkeys)
        .map_err(|e| err_json(500, "webauthn_error", &e.to_string()))?;

    let challenge_id = Uuid::new_v4().to_string();
    state
        .auth_store
        .put_authentication(challenge_id.clone(), challenge.clone(), auth_state);

    Ok(Json(json!({
        "challenge_id": challenge_id,
        "challenge": serde_json::to_value(&challenge)
            .map_err(|e| err_json(500, "serialize_error", &e.to_string()))?,
    })))
}

async fn login_finish(
    State(state): State<SyncState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let origin = str_field(&body, "origin")?;
    validate_origin(&state, origin)?;

    let challenge_id = str_field(&body, "challenge_id")?;
    let (_, auth_state) = state
        .auth_store
        .take_authentication(challenge_id, std::time::Instant::now())
        .ok_or_else(|| {
            err_json(
                400,
                "invalid_challenge",
                "unknown or expired login challenge",
            )
        })?;

    let credential: PublicKeyCredential =
        serde_json::from_value(body.get("credential").cloned().unwrap_or(Value::Null))
            .map_err(|e| err_json(400, "bad_credential", &e.to_string()))?;
    let credential_id = credential.get_credential_id().to_vec();

    let conn = state.conn.lock().await;
    let row: Option<(String, Vec<u8>)> = conn
        .query_row(
            "SELECT account_id, public_key FROM passkeys WHERE credential_id = ?1",
            rusqlite::params![credential_id.clone()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    let (account_id, public_key_bytes) = row.ok_or_else(|| {
        err_json(
            400,
            "unknown_credential",
            "no passkey registered with this credential id",
        )
    })?;

    let passkey: Passkey = serde_json::from_slice(&public_key_bytes)
        .map_err(|e| err_json(500, "corrupt_passkey", &e.to_string()))?;

    let result = state
        .webauthn
        .finish_passkey_authentication(&credential, &auth_state)
        .map_err(|e| err_json(401, "auth_failed", &e.to_string()))?;

    // Persist counter/backup-state changes when the authenticator advanced
    // them (rare, but the stored copy must not regress).
    if result.needs_update() {
        let mut updated = passkey;
        if updated.update_credential(&result).unwrap_or(false) {
            let public_key = serde_json::to_vec(&updated)
                .map_err(|e| err_json(500, "serialize_error", &e.to_string()))?;
            conn.execute(
                "UPDATE passkeys SET public_key = ?1 WHERE credential_id = ?2",
                rusqlite::params![public_key, credential_id],
            )
            .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
        }
    }

    let now_str = chrono::Utc::now().to_rfc3339();
    sweep_expired_sessions(&conn).map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    conn.execute(
        "UPDATE accounts SET last_login_at = ?1 WHERE id = ?2",
        rusqlite::params![now_str, account_id],
    )
    .map_err(|e| err_json(500, "database_error", &e.to_string()))?;

    let session_token = mint_and_store_session(&conn, &account_id)?;
    Ok(Json(json!({
        "account_id": account_id,
        "session_token": session_token,
    })))
}

// ── Logout ──────────────────────────────────────────────────────────────────

async fn logout(
    State(state): State<SyncState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let Ok(token) = bearer_token(&headers) else {
        return Ok(Json(json!({"logged_out": false})));
    };
    let conn = state.conn.lock().await;
    conn.execute(
        "DELETE FROM sessions WHERE token_hash = ?1",
        rusqlite::params![auth::hash_token(token).to_vec()],
    )
    .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    Ok(Json(json!({"logged_out": true})))
}

// ── Account + API keys ──────────────────────────────────────────────────────

/// Account overview: effective quotas (server defaults when the account
/// row has none), usage across the account's vaults, key metadata (never
/// hashes or plaintexts), and the vault list.
async fn get_account(
    State(state): State<SyncState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let account_id = authenticate_session(&state, &headers).await?;

    let conn = state.conn.lock().await;
    let account: Option<(Option<i64>, Option<i64>)> = conn
        .query_row(
            "SELECT quota_devices, quota_bytes FROM accounts WHERE id = ?1",
            rusqlite::params![account_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    let (quota_devices, quota_bytes) = match account {
        Some((d, b)) => (
            d.unwrap_or(state.default_quota_devices),
            b.unwrap_or(state.default_quota_bytes),
        ),
        // Row missing is a corrupt state (sessions cascade from accounts);
        // fall back to server defaults rather than hard-erroring the UI.
        None => (state.default_quota_devices, state.default_quota_bytes),
    };

    let vaults = account_vaults(&conn, &account_id)
        .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    let (devices_used, bytes_used) = usage_in_vaults(&conn, &vaults)
        .map_err(|e| err_json(500, "database_error", &e.to_string()))?;

    let mut stmt = conn
        .prepare(
            "SELECT id, key_prefix, rate, vault_id, created_at, revoked FROM api_keys \
             WHERE account_id = ?1 ORDER BY created_at ASC",
        )
        .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    let rows = stmt
        .query_map(rusqlite::params![account_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, f64>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, i64>(5)? != 0,
            ))
        })
        .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    let mut keys = Vec::new();
    for row in rows {
        let (id, key_prefix, rate, vault_id, created_at, revoked) =
            row.map_err(|e| err_json(500, "database_error", &e.to_string()))?;
        keys.push(json!({
            "id": id,
            "key_prefix": key_prefix,
            "rate": rate,
            "vault_id": vault_id,
            "created_at": created_at,
            "revoked": revoked,
        }));
    }

    Ok(Json(json!({
        "account_id": account_id,
        "quota": {
            "devices": quota_devices,
            "bytes": quota_bytes,
            "devices_used": devices_used,
            "bytes_used": bytes_used,
        },
        "keys": keys,
        "vaults": vaults,
    })))
}

/// Mint an account API key. The full key is returned exactly once; the
/// relay stores only its sha256. `vault_id` (optional) scopes the key to
/// one vault; omitted = every vault the account reaches.
async fn create_account_key(
    State(state): State<SyncState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let account_id = authenticate_session(&state, &headers).await?;
    let vault_id = match body.get("vault_id") {
        Some(Value::Null) | None => None,
        Some(v) => {
            let s = v
                .as_str()
                .ok_or_else(|| err_json(400, "bad_vault_id", "vault_id must be a string"))?;
            if s.is_empty() {
                return Err(err_json(400, "bad_vault_id", "vault_id must not be empty"));
            }
            Some(s.to_string())
        }
    };

    let api_key = auth::generate_api_key()
        .map_err(|e| err_json(500, "rng_error", &e.to_string()))?;
    let key_id = Uuid::new_v4().to_string();
    let now_str = chrono::Utc::now().to_rfc3339();

    let conn = state.conn.lock().await;
    conn.execute(
        "INSERT INTO api_keys (id, account_id, key_hash, key_prefix, rate, vault_id, created_at, revoked) \
         VALUES (?1, ?2, ?3, ?4, 100, ?5, ?6, 0)",
        rusqlite::params![
            key_id,
            account_id,
            auth::hash_key(&api_key).to_vec(),
            auth::API_KEY_PREFIX,
            vault_id,
            now_str,
        ],
    )
    .map_err(|e| err_json(500, "database_error", &e.to_string()))?;

    Ok(Json(json!({
        "key_id": key_id,
        "api_key": api_key, // shown once — the server keeps only its hash
        "key_prefix": auth::API_KEY_PREFIX,
        "rate": 100.0,
        "vault_id": vault_id,
        "created_at": now_str,
    })))
}

/// Revoke an account API key (soft: the row stays as an audit trail, the
/// hash stops authenticating).
async fn revoke_account_key(
    State(state): State<SyncState>,
    headers: HeaderMap,
    Path(key_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let account_id = authenticate_session(&state, &headers).await?;
    let conn = state.conn.lock().await;
    let changed = conn
        .execute(
            "UPDATE api_keys SET revoked = 1 WHERE id = ?1 AND account_id = ?2 AND revoked = 0",
            rusqlite::params![key_id, account_id],
        )
        .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    if changed == 0 {
        // Not found AND not-yours both land here: identical responses so
        // key ids can't be enumerated across accounts.
        return Err(err_json(
            404,
            "key_not_found",
            "no such unrevoked key for this account",
        ));
    }
    Ok(Json(json!({"key_id": key_id, "revoked": true})))
}

/// Vaults an account reaches: its unrevoked scoped keys, or every vault
/// with blobs when it holds an unrevoked unscoped key.
fn account_vaults(conn: &rusqlite::Connection, account_id: &str) -> rusqlite::Result<Vec<String>> {
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
/// `length(ciphertext)` counts base64 characters — the ciphertext column
/// is what eats storage, so chars ≈ bytes for quota purposes.
fn usage_in_vaults(
    conn: &rusqlite::Connection,
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

// ── Session helpers (shared with account-key routes) ────────────────────────

/// Resolve a Bearer session token to an account id (401 when absent/expired).
pub async fn authenticate_session(
    state: &SyncState,
    headers: &HeaderMap,
) -> Result<String, ApiError> {
    let token = bearer_token(headers)?;
    let conn = state.conn.lock().await;
    let account_id: Option<String> = conn
        .query_row(
            "SELECT account_id FROM sessions WHERE token_hash = ?1 AND expires_at > ?2",
            rusqlite::params![
                auth::hash_token(token).to_vec(),
                chrono::Utc::now().to_rfc3339()
            ],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    account_id.ok_or_else(|| err_json(401, "invalid_session", "not signed in"))
}

// Sync on purpose: callers hold the conn mutex guard (which is !Send) and
// an await point here would make every handler future non-Send.
fn mint_and_store_session(
    conn: &rusqlite::Connection,
    account_id: &str,
) -> Result<String, ApiError> {
    let token = auth::mint_session_token()
        .map_err(|e| err_json(500, "rng_error", &e.to_string()))?;
    let now = chrono::Utc::now();
    let expires = now + chrono::Duration::from_std(SESSION_TTL).unwrap();
    conn.execute(
        "INSERT INTO sessions (token_hash, account_id, created_at, expires_at) \
         VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![
            auth::hash_token(&token).to_vec(),
            account_id,
            now.to_rfc3339(),
            expires.to_rfc3339(),
        ],
    )
    .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    Ok(token)
}

fn sweep_expired_sessions(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM sessions WHERE expires_at < ?1",
        rusqlite::params![chrono::Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

/// Background sweep of expired sessions (login/finish also sweeps inline).
pub fn spawn_session_sweeper(state: SyncState) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(3600));
        loop {
            interval.tick().await;
            let conn = state.conn.lock().await;
            if let Err(e) = sweep_expired_sessions(&conn) {
                tracing::warn!("Session sweep error: {e}");
            }
        }
    });
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn str_field<'a>(body: &'a Value, name: &str) -> Result<&'a str, ApiError> {
    body.get(name)
        .and_then(|v| v.as_str())
        .ok_or_else(|| err_json(400, "missing_field", &format!("missing {name}")))
}

fn validate_origin(state: &SyncState, origin: &str) -> Result<(), ApiError> {
    if state.allowed_origins.contains(origin) {
        Ok(())
    } else {
        Err(err_json(
            401,
            "origin_not_allowed",
            "origin is not in the server's --origin allow-list",
        ))
    }
}

fn bearer_token<'a>(headers: &'a HeaderMap) -> Result<&'a str, ApiError> {
    let value = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| err_json(401, "missing_authorization", "missing Authorization header"))?;
    let token = value
        .strip_prefix("Bearer ")
        .map(str::trim)
        .ok_or_else(|| err_json(401, "bad_authorization", "expected Bearer token"))?;
    if token.is_empty() {
        return Err(err_json(401, "bad_authorization", "empty token"));
    }
    Ok(token)
}

fn err_json(status: u16, code: &str, msg: &str) -> ApiError {
    (
        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        Json(json!({"code": code, "error": msg})),
    )
}

fn is_unique_violation(e: &rusqlite::Error) -> bool {
    matches!(
        e,
        rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::ConstraintViolation
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    fn test_state() -> SyncState {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "PRAGMA foreign_keys=ON;
             CREATE TABLE accounts (
                 id TEXT PRIMARY KEY, created_at TEXT NOT NULL, last_login_at TEXT,
                 quota_devices INTEGER, quota_bytes INTEGER
             );
             CREATE TABLE passkeys (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
                 credential_id BLOB NOT NULL UNIQUE, public_key BLOB NOT NULL, created_at TEXT NOT NULL
             );
             CREATE TABLE sessions (
                 token_hash BLOB PRIMARY KEY, account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
                 created_at TEXT NOT NULL, expires_at TEXT NOT NULL
             );
             CREATE TABLE api_keys (
                 id TEXT PRIMARY KEY, account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
                 key_hash BLOB NOT NULL UNIQUE, key_prefix TEXT NOT NULL, rate REAL NOT NULL DEFAULT 100,
                 vault_id TEXT, created_at TEXT NOT NULL, revoked INTEGER NOT NULL DEFAULT 0
             );
             CREATE TABLE sync_blobs (
                 vault_id TEXT NOT NULL, memory_id TEXT NOT NULL, device_id TEXT NOT NULL,
                 vector_clock INTEGER NOT NULL DEFAULT 0, ciphertext TEXT NOT NULL,
                 hmac TEXT NOT NULL, created_at TEXT NOT NULL, deleted INTEGER NOT NULL DEFAULT 0,
                 PRIMARY KEY (vault_id, memory_id)
             );",
        )
        .unwrap();
        SyncState {
            conn: Arc::new(Mutex::new(conn)),
            start_time: chrono::Utc::now(),
            data_dir: std::path::PathBuf::from("/tmp"),
            api_keys: Arc::new(Default::default()),
            rate_limiters: Arc::new(Mutex::new(Default::default())),
            is_loopback: true,
            rp_id: "localhost".into(),
            allowed_origins: Arc::new(
                ["http://localhost:8787".to_string()]
                    .into_iter()
                    .collect(),
            ),
            default_quota_devices: 0,
            default_quota_bytes: 0,
            webauthn: Arc::new(crate::auth::build_webauthn("localhost", "http://localhost:8787").unwrap()),
            auth_store: Arc::new(crate::auth::WebauthnStore::new()),
        }
    }

    #[tokio::test]
    async fn origin_allow_list_rejects_unknown_origins() {
        let state = test_state();
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer whatever".parse().unwrap());
        let res = register_start(
            State(state.clone()),
            headers,
            Json(json!({"origin": "https://evil.example"})),
        )
        .await;
        let (status, body) = res.expect_err("must be rejected");
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["code"], "origin_not_allowed");
    }

    #[tokio::test]
    async fn login_start_with_no_passkeys_returns_409() {
        let state = test_state();
        let res = login_start(State(state), Json(json!({"origin": "http://localhost:8787"}))).await;
        let (status, body) = res.expect_err("no passkeys yet");
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["code"], "no_passkeys");
    }

    #[test]
    fn bearer_token_parses_and_rejects() {
        let mut headers = HeaderMap::new();
        assert!(bearer_token(&headers).is_err());
        headers.insert("authorization", "Basic abc".parse().unwrap());
        assert!(bearer_token(&headers).is_err());
        headers.insert("authorization", "Bearer tok".parse().unwrap());
        assert_eq!(bearer_token(&headers).unwrap(), "tok");
        headers.insert("authorization", "Bearer ".parse().unwrap());
        assert!(bearer_token(&headers).is_err());
    }

    // ── Account keys (Phase 3) ──────────────────────────────────────────

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

    fn bearer(token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", format!("Bearer {token}").parse().unwrap());
        headers
    }

    #[tokio::test]
    async fn create_key_returns_plaintext_once_and_stores_hash() {
        let state = test_state();
        seed_session(&state, "acct-1", "test-token").await;
        let res = create_account_key(
            State(state.clone()),
            bearer("test-token"),
            Json(json!({"vault_id": "vault-a"})),
        )
        .await
        .expect("session valid");
        let body = res.0;
        let api_key = body["api_key"].as_str().expect("full key returned once");
        assert!(api_key.starts_with(auth::API_KEY_PREFIX));
        let key_id = body["key_id"].as_str().unwrap();
        assert_eq!(body["vault_id"], "vault-a");

        // Stored: sha256 only — never the plaintext.
        let conn = state.conn.lock().await;
        let (stored_hash, stored_prefix): (Vec<u8>, String) = conn
            .query_row(
                "SELECT key_hash, key_prefix FROM api_keys WHERE id = ?1",
                rusqlite::params![key_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(stored_hash, auth::hash_key(api_key).to_vec());
        assert_eq!(stored_prefix, auth::API_KEY_PREFIX);
        assert_ne!(stored_hash, api_key.as_bytes());
    }

    #[tokio::test]
    async fn revoke_key_requires_ownership() {
        let state = test_state();
        seed_session(&state, "acct-1", "token-1").await;
        seed_session(&state, "acct-2", "token-2").await;
        {
            let conn = state.conn.lock().await;
            conn.execute(
                "INSERT INTO api_keys (id, account_id, key_hash, key_prefix, rate, vault_id, created_at, revoked) \
                 VALUES ('k1', 'acct-1', ?1, 'en_', 100, NULL, 'now', 0)",
                rusqlite::params![auth::hash_key("en_secret").to_vec()],
            )
            .unwrap();
        }

        // acct-2 cannot revoke acct-1's key
        let err = revoke_account_key(State(state.clone()), bearer("token-2"), Path("k1".into()))
            .await
            .expect_err("not theirs");
        assert_eq!(err.0, StatusCode::NOT_FOUND);

        // acct-1 can
        let res = revoke_account_key(State(state.clone()), bearer("token-1"), Path("k1".into()))
            .await
            .expect("owner revokes");
        assert_eq!(res.0["revoked"], true);

        // Second revoke is a 404 (already revoked)
        let err = revoke_account_key(State(state.clone()), bearer("token-1"), Path("k1".into()))
            .await
            .expect_err("already revoked");
        assert_eq!(err.0, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn get_account_reports_usage_keys_and_vaults() {
        let state = test_state();
        seed_session(&state, "acct-1", "test-token").await;
        {
            let conn = state.conn.lock().await;
            conn.execute(
                "INSERT INTO api_keys (id, account_id, key_hash, key_prefix, rate, vault_id, created_at, revoked) \
                 VALUES ('k1', 'acct-1', ?1, 'en_', 100, 'vault-a', 'now', 0)",
                rusqlite::params![auth::hash_key("en_key1").to_vec()],
            )
            .unwrap();
            for (mid, did, text) in [("m1", "dev-1", "xxxx"), ("m2", "dev-2", "yyyyyy"), ("m3", "dev-3", "zzzzzzzz")] {
                conn.execute(
                    "INSERT INTO sync_blobs (vault_id, memory_id, device_id, vector_clock, ciphertext, hmac, created_at, deleted) \
                     VALUES (?1, ?2, ?3, 1, ?4, 'h', 'now', 0)",
                    rusqlite::params![
                        if mid == "m3" { "other-vault" } else { "vault-a" },
                        mid,
                        did,
                        text,
                    ],
                )
                .unwrap();
            }
        }

        let res = get_account(State(state), bearer("test-token")).await.expect("session valid");
        let body = res.0;
        assert_eq!(body["account_id"], "acct-1");
        // Usage counts only the account's vaults (vault-a): dev-1 + dev-2,
        // 4 + 6 chars. other-vault/dev-3 is excluded.
        assert_eq!(body["quota"]["devices_used"], 2);
        assert_eq!(body["quota"]["bytes_used"], 10);
        assert_eq!(body["quota"]["devices"], 0, "server default quota");
        assert_eq!(body["vaults"].as_array().unwrap().len(), 1);
        assert_eq!(body["vaults"][0], "vault-a");
        let keys = body["keys"].as_array().unwrap();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0]["id"], "k1");
        assert!(keys[0].get("api_key").is_none(), "no plaintext in metadata");
        assert!(keys[0].get("key_hash").is_none(), "no hashes in metadata");
    }
}
