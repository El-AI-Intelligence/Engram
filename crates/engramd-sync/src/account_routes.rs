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
use crate::password_routes;
use crate::SyncState;
use std::collections::HashSet;
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{delete, get, post, put},
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
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
        .route("/account/vaults", get(account_vaults))
        .route("/account/vaults/{vault_id}", delete(delete_account_vault))
        .route("/account/keys", post(create_account_key))
        .route("/account/keys/{key_id}", delete(revoke_account_key))
        .route("/account/credentials", get(account_credentials))
        .route("/account/password", post(change_account_password))
        .route("/account/passkeys", get(account_passkeys))
        .route("/account/passkeys/{credential_id}", delete(delete_account_passkey))
        .route("/account/wraps", get(account_wraps))
        .route(
            "/account/wraps/password",
            put(put_password_wrap).get(get_password_wrap),
        )
        .route(
            "/account/wraps/recovery",
            put(put_recovery_wrap).get(get_recovery_wrap),
        )
        .route(
            "/account/vaults/{vault_id}/wrap",
            put(put_vault_wrap)
                .delete(delete_vault_wrap)
                .get(get_vault_wrap),
        )
        .route("/devices/pair-codes", post(mint_pairing_code))
        .route("/devices/pair", post(redeem_pairing_code))
        .route("/devices/link-intents", post(create_link_intent))
        .route(
            "/devices/link-intents/{id}/status",
            get(link_intent_status),
        )
        .route(
            "/devices/link-intents/{id}/confirm",
            post(confirm_link_intent),
        )
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
    // Rate-limit the ceremony start: challenges are cheap to mint but floods
    // would pin the auth_store. Bucketed per account when attaching,
    // otherwise per source (anonymous starts create accounts).
    {
        let bucket = attach_account
            .as_deref()
            .map(|a| format!("passkey-reg-start:{a}"))
            .unwrap_or_else(|| "passkey-reg-start:anon".to_string());
        password_routes::check_bucket(&state, bucket, 10.0 / 300.0, 10.0).await?;
    }
    // Attaching a passkey is a credential mutation: a session alone is not
    // proof (7-day TTL). Accounts with a password must verify it here.
    if let Some(ref account_id) = attach_account {
        let password = body.get("password").and_then(|v| v.as_str());
        let conn = state.conn.lock().await;
        password_routes::require_fresh_password(&conn, account_id, password)?;
    }
    let user_unique_id = attach_account
        .as_ref()
        .and_then(|a| Uuid::parse_str(a).ok())
        .unwrap_or_else(Uuid::new_v4);
    // The account id IS the WebAuthn user handle: existing account when
    // attaching, fresh uuid when this ceremony creates the account.
    let account_id = attach_account
        .clone()
        .unwrap_or_else(|| user_unique_id.to_string());

    // Credential label in password managers: a short branded slice of the
    // account id keeps every account's passkey distinct (all-accounts-
    // share-"engram-account" made managers show identical entries).
    let username = format!("engram-{}", &account_id[..8.min(account_id.len())]);
    let (mut challenge, reg_state) = state
        .webauthn
        .start_passkey_registration(user_unique_id, &username, "Engram Account", None)
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
    // Finishes do the expensive signature verification; bound them per
    // account so a captured challenge can't be replayed at flood rates.
    password_routes::check_bucket(
        &state,
        format!("passkey-reg-finish:{account_id}"),
        30.0 / 60.0,
        30.0,
    )
    .await?;

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
    // no-op thanks to OR IGNORE. The audit event needs to know which.
    let account_existed: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM accounts WHERE id = ?1",
            rusqlite::params![account_id],
            |r| r.get(0),
        )
        .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
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
                    audit_event(&conn, &a, "passkey_signin", Some("already_registered"));
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

    audit_event(
        &conn,
        &account_id,
        if account_existed > 0 {
            "passkey_attach"
        } else {
            "account_created"
        },
        None,
    );

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
    // Login starts are cheap to mint; bound them globally so floods can't
    // pin the auth_store. (login_finish does the expensive verify.)
    password_routes::check_bucket(&state, "passkey-login-start".into(), 30.0 / 60.0, 30.0)
        .await?;

    let conn = state.conn.lock().await;
    // User-less login: load the stored passkeys and let the browser offer
    // them as a picker. The credential is identified in finish (the auth
    // state carries the allowed-credential list). An optional `account_id`
    // in the body (a "handle" the SPA remembers) filters to one account.
    let account_filter = body.get("account_id").and_then(|v| v.as_str());
    let rows = login_passkey_rows(&conn, account_filter)?;
    let mut passkeys: Vec<Passkey> = Vec::new();
    for bytes in rows {
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

/// Load the serialized passkeys `login_start` offers. `account_id` filters
/// to one account's credentials (None = every account). A per-account cap
/// of 20 (oldest first) keeps one abandoned account's passkey pile from
/// turning every login into a DoS and from bloating the browser picker.
/// Row level on purpose: Passkey deserialization needs real blobs and is
/// tested separately.
fn login_passkey_rows(
    conn: &rusqlite::Connection,
    account_id: Option<&str>,
) -> Result<Vec<Vec<u8>>, ApiError> {
    let mut stmt = conn
        .prepare(
            "SELECT public_key FROM (\
               SELECT public_key, ROW_NUMBER() OVER (PARTITION BY account_id ORDER BY rowid ASC) AS rn \
               FROM passkeys WHERE ?1 IS NULL OR account_id = ?1)\
             WHERE rn <= 20",
        )
        .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    let rows = stmt
        .query_map(rusqlite::params![account_id], |r| r.get::<_, Vec<u8>>(0))
        .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    rows.collect::<Result<Vec<Vec<u8>>, _>>()
        .map_err(|e| err_json(500, "database_error", &e.to_string()))
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
    audit_event(&conn, &account_id, "passkey_signin", None);
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

    let vaults = crate::quota::account_vaults(&conn, &account_id)
        .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    let (devices_used, bytes_used) = crate::quota::usage_in_vaults(&conn, &vaults)
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
/// relay stores only its sha256. `vault_id` is REQUIRED: keys with NULL
/// scope predate per-vault scoping and are policy-denied everywhere, so
/// minting one would be dead on arrival. Minting is gated by vault
/// ownership (see `validate_key_mint`).
async fn create_account_key(
    State(state): State<SyncState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let account_id = authenticate_session(&state, &headers).await?;
    let vault_id = required_vault_id(&body)?;

    let api_key = auth::generate_api_key()
        .map_err(|e| err_json(500, "rng_error", &e.to_string()))?;
    let key_id = Uuid::new_v4().to_string();
    let now_str = chrono::Utc::now().to_rfc3339();

    let conn = state.conn.lock().await;
    validate_key_mint(&conn, &account_id, &vault_id)?;
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
    audit_event(&conn, &account_id, "api_key_mint", Some(&vault_id));

    Ok(Json(json!({
        "key_id": key_id,
        "api_key": api_key, // shown once — the server keeps only its hash
        "key_prefix": auth::API_KEY_PREFIX,
        "rate": 100.0,
        "vault_id": vault_id,
        "created_at": now_str,
    })))
}

/// Parse the REQUIRED `vault_id` string field from a JSON body. NULL /
/// missing / empty are all 400s — no NULL-scoped keys are minted anymore.
fn required_vault_id(body: &Value) -> Result<String, ApiError> {
    match body.get("vault_id") {
        Some(Value::Null) | None => Err(err_json(
            400,
            "missing_vault_id",
            "vault_id is required",
        )),
        Some(v) => {
            let s = v
                .as_str()
                .ok_or_else(|| err_json(400, "bad_vault_id", "vault_id must be a string"))?;
            if s.is_empty() {
                return Err(err_json(400, "bad_vault_id", "vault_id must not be empty"));
            }
            Ok(s.to_string())
        }
    }
}

/// Gate API-key minting on vault ownership. Allow iff the account
/// (a) holds an unrevoked key scoped to the vault, (b) has stored a vault
/// key wrap for it (the browser claims vaults it has opened), or (c) is the
/// founding member — the vault has no blobs on the relay yet, so a fresh
/// first-device pair/link must work before any key exists. Sync (callers
/// hold the conn guard).
fn validate_key_mint(
    conn: &rusqlite::Connection,
    account_id: &str,
    vault_id: &str,
) -> Result<(), ApiError> {
    let scoped: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM api_keys \
             WHERE account_id = ?1 AND vault_id = ?2 AND revoked = 0",
            rusqlite::params![account_id, vault_id],
            |r| r.get(0),
        )
        .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    if scoped > 0 {
        return Ok(());
    }
    let wrapped: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM vault_key_wraps \
             WHERE account_id = ?1 AND vault_id = ?2 AND kind = 'account'",
            rusqlite::params![account_id, vault_id],
            |r| r.get(0),
        )
        .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    if wrapped > 0 {
        return Ok(());
    }
    let blobs: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sync_blobs WHERE vault_id = ?1",
            rusqlite::params![vault_id],
            |r| r.get(0),
        )
        .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    if blobs == 0 {
        return Ok(());
    }
    Err(err_json(
        403,
        "vault_not_owned",
        "this account has no access to that vault",
    ))
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
    audit_event(&conn, &account_id, "api_key_revoke", Some(&key_id));
    Ok(Json(json!({"key_id": key_id, "revoked": true})))
}

// ── Device pairing (WARP-style onboarding) ──────────────────────────────────

/// Pairing-code lifetime: 10 minutes — enough to walk from the browser to
/// the machine, short enough to keep the guessing window small.
const PAIRING_CODE_TTL_SECS: i64 = 600;
/// Cap on live (unused, unexpired) pairing codes per account.
const PAIRING_CODES_PER_ACCOUNT: i64 = 5;

/// Mint a one-time pairing code for the signed-in account, FOR a specific
/// vault chosen in the browser (where vault names are visible) — the
/// redeeming CLI never has to guess a vault id. The plaintext code is
/// returned exactly once — the relay stores only its sha256 (same
/// discipline as API keys and sessions).
async fn mint_pairing_code(
    State(state): State<SyncState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let account_id = authenticate_session(&state, &headers).await?;
    let vault_id = required_vault_id(&body)?;
    let device_label = body
        .get("device_label")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    let code = auth::generate_pairing_code()
        .map_err(|e| err_json(500, "rng_error", &e.to_string()))?;
    let now = chrono::Utc::now();
    let expires = now + chrono::Duration::seconds(PAIRING_CODE_TTL_SECS);

    let conn = state.conn.lock().await;
    // Sweep stale codes, then cap live codes per account.
    conn.execute(
        "DELETE FROM pairing_codes WHERE expires_at < ?1",
        rusqlite::params![now.to_rfc3339()],
    )
    .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    let live: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pairing_codes WHERE account_id = ?1 AND used = 0",
            rusqlite::params![account_id],
            |r| r.get(0),
        )
        .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    if live >= PAIRING_CODES_PER_ACCOUNT {
        return Err(err_json(
            409,
            "too_many_codes",
            &format!(
                "{PAIRING_CODES_PER_ACCOUNT} live pairing codes already exist — wait for one to expire"
            ),
        ));
    }

    // Ownership is validated at mint time — a wrong-vault mint fails before
    // the code exists, instead of the redeemer discovering it later.
    validate_key_mint(&conn, &account_id, &vault_id)?;

    conn.execute(
        "INSERT INTO pairing_codes (code_hash, account_id, created_at, expires_at, used, vault_id, device_label) \
         VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6)",
        rusqlite::params![
            auth::hash_key(&code).to_vec(),
            account_id,
            now.to_rfc3339(),
            expires.to_rfc3339(),
            vault_id,
            device_label,
        ],
    )
    .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    audit_event(&conn, &account_id, "pairing_code_mint", Some(&vault_id));

    Ok(Json(json!({
        "code": code, // shown once — the server keeps only its hash
        "expires_in": PAIRING_CODE_TTL_SECS,
        "vault_id": vault_id,
    })))
}

/// Redeem a pairing code for an account API key. No session required —
/// the code itself is the credential (single-use, 10-minute TTL). The
/// vault id comes from the code's row (chosen at mint time in the
/// browser); the body carries only `code` + optional `device_label`. An
/// old client's body `vault_id` is ignored for compatibility.
async fn redeem_pairing_code(
    State(state): State<SyncState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    // Global token bucket: bounds total guessing attempts regardless of
    // source. Codes are ~60-bit and single-use, so this only slows floods.
    {
        let mut limiters = state.rate_limiters.lock().await;
        let limiter = limiters
            .entry("pair-redeem".into())
            .or_insert_with(|| crate::RateLimiter::new(5.0, 5.0));
        if !limiter.allow() {
            return Err(err_json(
                429,
                "rate_limit_exceeded",
                "too many pairing attempts — wait a moment and try again",
            ));
        }
    }

    let code = str_field(&body, "code")?.to_ascii_uppercase();
    let conn = state.conn.lock().await;
    let row: Option<(String, String, i64, Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT account_id, expires_at, used, vault_id, device_label FROM pairing_codes WHERE code_hash = ?1",
            rusqlite::params![auth::hash_key(&code).to_vec()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()
        .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    let (account_id, expires_at, used, vault_id_row, device_label) = row.ok_or_else(|| {
        err_json(
            401,
            "invalid_pairing_code",
            "unknown pairing code — mint a new one from the site",
        )
    })?;
    if used != 0 {
        return Err(err_json(
            401,
            "invalid_pairing_code",
            "pairing code already used",
        ));
    }
    let expires: chrono::DateTime<chrono::Utc> =
        chrono::DateTime::parse_from_rfc3339(&expires_at)
            .map_err(|e| err_json(500, "database_error", &format!("corrupt expires_at: {e}")))?
            .with_timezone(&chrono::Utc);
    if expires <= chrono::Utc::now() {
        return Err(err_json(
            401,
            "expired_pairing_code",
            "pairing code expired — mint a new one from the site",
        ));
    }

    // Codes minted before the per-vault schema have NULL here — they're
    // dead minutes after deploy thanks to the 10-minute TTL, so refuse
    // them with a clear re-mint message rather than guessing.
    let vault_id = vault_id_row.ok_or_else(|| {
        err_json(
            410,
            "stale_pairing_code",
            "this pairing code predates per-vault codes — mint a new one from the site",
        )
    })?;

    // Consume the code first: a concurrent redeem must never mint two keys,
    // and a wrong-vault attempt burns the code (fail-closed — no retries
    // with a stolen code).
    let consumed = conn
        .execute(
            "UPDATE pairing_codes SET used = 1 WHERE code_hash = ?1 AND used = 0",
            rusqlite::params![auth::hash_key(&code).to_vec()],
        )
        .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    if consumed == 0 {
        return Err(err_json(
            401,
            "invalid_pairing_code",
            "pairing code already used",
        ));
    }

    // Mint the key — scoped, same shape as /account/keys.
    validate_key_mint(&conn, &account_id, &vault_id)?;
    let api_key = auth::generate_api_key()
        .map_err(|e| err_json(500, "rng_error", &e.to_string()))?;
    let key_id = Uuid::new_v4().to_string();
    let now_str = chrono::Utc::now().to_rfc3339();
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
    audit_event(&conn, &account_id, "pairing_code_redeem", Some(&vault_id));

    Ok(Json(json!({
        "key_id": key_id,
        "api_key": api_key, // shown once — the server keeps only its hash
        "key_prefix": auth::API_KEY_PREFIX,
        "rate": 100.0,
        "vault_id": vault_id,
        "created_at": now_str,
        "device_label": device_label,
    })))
}

// ── One-click machine linking (`engram link`, WARP-style) ──────────────────

/// Link-intent lifetime: 10 minutes — the same walk-from-browser window as
/// pairing codes.
const LINK_INTENT_TTL_SECS: i64 = 600;

/// Create a link intent (unauthenticated): the CLI posts its ephemeral
/// X25519 public key and the client-derived `vault_id` it wants a key for,
/// gets an intent id + code to put in the confirm URL. The relay derives
/// its own per-intent keypair from (id, code_hash) — nothing private is
/// ever at rest. The code is returned once; only its sha256 is stored.
async fn create_link_intent(
    State(state): State<SyncState>,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    {
        let mut limiters = state.rate_limiters.lock().await;
        let limiter = limiters
            .entry("link-create".into())
            .or_insert_with(|| crate::RateLimiter::new(5.0, 5.0));
        if !limiter.allow() {
            return Err(err_json(
                429,
                "rate_limit_exceeded",
                "too many link attempts — wait a moment and try again",
            ));
        }
    }

    let pk_b64 = str_field(&body, "public_key")?;
    let pk_bytes = URL_SAFE_NO_PAD
        .decode(pk_b64.as_bytes())
        .map_err(|_| err_json(400, "invalid_public_key", "public_key must be base64url"))?;
    if pk_bytes.len() != 32 {
        return Err(err_json(
            400,
            "invalid_public_key",
            "public_key must be 32 bytes",
        ));
    }
    let public_key: [u8; 32] = pk_bytes
        .as_slice()
        .try_into()
        .map_err(|_| err_json(400, "invalid_public_key", "public_key must be 32 bytes"))?;
    let device_label = body.get("device_label").and_then(|v| v.as_str());
    let vault_id = required_vault_id(&body)?;

    let code =
        auth::generate_pairing_code().map_err(|e| err_json(500, "rng_error", &e.to_string()))?;
    let code_hash = auth::hash_key(&code);
    let id = Uuid::new_v4().to_string();
    let sk_r = crate::link_crypto::intent_keypair(&id, &code_hash);
    let pk_r = x25519_dalek::PublicKey::from(&sk_r);
    let now = chrono::Utc::now();
    let expires = now + chrono::Duration::seconds(LINK_INTENT_TTL_SECS);

    let conn = state.conn.lock().await;
    conn.execute(
        "DELETE FROM link_intents WHERE expires_at < ?1",
        rusqlite::params![now.to_rfc3339()],
    )
    .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    conn.execute(
        "INSERT INTO link_intents (id, code_hash, public_key, device_label, status, vault_id, created_at, expires_at) \
         VALUES (?1, ?2, ?3, ?4, 'pending', ?5, ?6, ?7)",
        rusqlite::params![
            id,
            code_hash.to_vec(),
            public_key.to_vec(),
            device_label,
            vault_id,
            now.to_rfc3339(),
            expires.to_rfc3339(),
        ],
    )
    .map_err(|e| err_json(500, "database_error", &e.to_string()))?;

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "id": id,
            "code": code, // shown once — the server keeps only its hash
            "relay_public_key": URL_SAFE_NO_PAD.encode(pk_r.as_bytes()),
            "expires_in": LINK_INTENT_TTL_SECS,
            "v": 1,
        })),
    ))
}

/// Poll the intent: pending until confirmed, then the sealed key is
/// delivered exactly once (atomic confirmed→delivered claim — a
/// concurrent poll loses the race and gets 410).
async fn link_intent_status(
    State(state): State<SyncState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    {
        let mut limiters = state.rate_limiters.lock().await;
        let limiter = limiters
            .entry("link-status".into())
            .or_insert_with(|| crate::RateLimiter::new(20.0, 20.0));
        if !limiter.allow() {
            return Err(err_json(
                429,
                "rate_limit_exceeded",
                "polling too fast — wait a moment and try again",
            ));
        }
    }

    let conn = state.conn.lock().await;
    let row: Option<(String, Option<Vec<u8>>, Option<Vec<u8>>, String)> = conn
        .query_row(
            "SELECT status, sealed_key, nonce, expires_at FROM link_intents WHERE id = ?1",
            rusqlite::params![id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()
        .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    let (status, sealed, nonce, expires_at) = row.ok_or_else(|| {
        err_json(
            404,
            "link_intent_not_found",
            "unknown link intent — run `engram link` again",
        )
    })?;

    if link_intent_expired(&expires_at)? {
        return Err(err_json(
            410,
            "link_intent_expired",
            "link expired — run `engram link` again",
        ));
    }

    match status.as_str() {
        "pending" => Ok(Json(json!({"status": "pending", "v": 1}))),
        "delivered" => Err(err_json(
            410,
            "link_intent_delivered",
            "link already claimed — run `engram link` again",
        )),
        "confirmed" => {
            let claimed = conn
                .execute(
                    "UPDATE link_intents SET status = 'delivered' WHERE id = ?1 AND status = 'confirmed'",
                    rusqlite::params![id],
                )
                .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
            if claimed == 0 {
                return Err(err_json(
                    410,
                    "link_intent_delivered",
                    "link already claimed — run `engram link` again",
                ));
            }
            let sealed = sealed
                .ok_or_else(|| err_json(500, "database_error", "confirmed intent missing sealed_key"))?;
            let nonce = nonce
                .ok_or_else(|| err_json(500, "database_error", "confirmed intent missing nonce"))?;
            Ok(Json(json!({
                "status": "confirmed",
                "sealed_key": URL_SAFE_NO_PAD.encode(sealed),
                "nonce": URL_SAFE_NO_PAD.encode(nonce),
                "key_prefix": auth::API_KEY_PREFIX,
                "v": 1,
            })))
        }
        _ => Err(err_json(500, "database_error", "corrupt link intent status")),
    }
}

/// The signed-in browser approves the intent: verifies the code (the
/// capability carried in the URL), binds the intent to this account, mints
/// a key scoped to the intent's `vault_id` (ownership-validated), and seals
/// it to the CLI's public key. The plaintext key is never echoed — the CLI
/// polls for the seal.
async fn confirm_link_intent(
    State(state): State<SyncState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let account_id = authenticate_session(&state, &headers).await?;

    {
        let mut limiters = state.rate_limiters.lock().await;
        let limiter = limiters
            .entry("link-confirm".into())
            .or_insert_with(|| crate::RateLimiter::new(5.0, 5.0));
        if !limiter.allow() {
            return Err(err_json(
                429,
                "rate_limit_exceeded",
                "too many attempts — wait a moment and try again",
            ));
        }
    }

    let code = str_field(&body, "code")?.to_ascii_uppercase();
    let conn = state.conn.lock().await;
    let row: Option<(Vec<u8>, Vec<u8>, String, String, Option<String>)> = conn
        .query_row(
            "SELECT code_hash, public_key, status, expires_at, vault_id FROM link_intents WHERE id = ?1",
            rusqlite::params![id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()
        .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    let (code_hash, public_key, status, expires_at, vault_id) = row.ok_or_else(|| {
        err_json(
            404,
            "link_intent_not_found",
            "unknown link intent — run `engram link` again",
        )
    })?;

    if link_intent_expired(&expires_at)? {
        return Err(err_json(
            410,
            "link_intent_expired",
            "link expired — run `engram link` again",
        ));
    }
    if status != "pending" {
        return Err(err_json(
            410,
            "link_intent_delivered",
            "link already used or expired — run `engram link` again",
        ));
    }
    // Intents created before the vault_id column existed have nothing to
    // scope a key to — they must be re-created (same TTL walk anyway).
    let vault_id = vault_id.ok_or_else(|| {
        err_json(
            410,
            "link_intent_delivered",
            "this link predates vault scoping — run `engram link` again",
        )
    })?;

    // The code is the capability carried in the URL — compare hashes, not
    // plaintexts (nothing plaintext is at rest).
    let code_hash_in = auth::hash_key(&code);
    if code_hash_in.as_slice() != code_hash.as_slice() {
        return Err(err_json(
            403,
            "invalid_link_code",
            "this link doesn't match — run `engram link` again on the machine",
        ));
    }

    let sk_r = crate::link_crypto::intent_keypair(&id, &code_hash_in);
    let pk_bytes: [u8; 32] = public_key
        .as_slice()
        .try_into()
        .map_err(|_| err_json(500, "database_error", "corrupt public_key"))?;
    let shared =
        crate::link_crypto::link_shared_secret(&sk_r, &x25519_dalek::PublicKey::from(pk_bytes))
            .map_err(|e| err_json(400, "invalid_public_key", &e.to_string()))?;

    // Mint the key — scoped, same shape as redeem_pairing_code. Ownership
    // validated so a link URL for vault X can't mint a key for vault Y.
    validate_key_mint(&conn, &account_id, &vault_id)?;
    let api_key = auth::generate_api_key().map_err(|e| err_json(500, "rng_error", &e.to_string()))?;
    let key_id = Uuid::new_v4().to_string();
    let now_str = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO api_keys (id, account_id, key_hash, key_prefix, rate, vault_id, created_at, revoked) \
         VALUES (?1, ?2, ?3, ?4, 100, ?5, ?6, 0)",
        rusqlite::params![
            key_id,
            account_id,
            auth::hash_key(&api_key).to_vec(),
            auth::API_KEY_PREFIX,
            vault_id,
            now_str
        ],
    )
    .map_err(|e| err_json(500, "database_error", &e.to_string()))?;

    let (sealed, nonce) = crate::link_crypto::seal_api_key(&id, &shared, &api_key)
        .map_err(|e| err_json(500, "seal_error", &e.to_string()))?;

    // Atomic: a second confirm loses the race and gets 410.
    let claimed = conn
        .execute(
            "UPDATE link_intents SET account_id = ?1, sealed_key = ?2, nonce = ?3, status = 'confirmed' \
             WHERE id = ?4 AND status = 'pending'",
            rusqlite::params![account_id, sealed, nonce.to_vec(), id],
        )
        .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    if claimed == 0 {
        return Err(err_json(
            410,
            "link_intent_delivered",
            "link already used or expired — run `engram link` again",
        ));
    }
    // The confirm is the moment the account binds to the intent — the
    // creation itself is anonymous and unauditable (auth_events.account_id
    // is NOT NULL), so this is the security-relevant event.
    audit_event(&conn, &account_id, "link_intent_confirm", Some(&vault_id));

    Ok(Json(json!({
        "status": "confirmed",
        "key_id": key_id,
        "key_prefix": auth::API_KEY_PREFIX,
        "v": 1,
    })))
}

/// Parse an intent's expires_at into a "is it expired" check.
fn link_intent_expired(expires_at: &str) -> Result<bool, ApiError> {
    let expires: chrono::DateTime<chrono::Utc> = chrono::DateTime::parse_from_rfc3339(expires_at)
        .map_err(|e| err_json(500, "database_error", &format!("corrupt expires_at: {e}")))?
        .with_timezone(&chrono::Utc);
    Ok(expires <= chrono::Utc::now())
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

/// What vaults can this account read? Derived from the account's unrevoked
/// SCOPED API keys: `Some(set)` — exactly these vaults. Legacy keys with a
/// NULL `vault_id` grant nothing anywhere (they are policy-denied on the
/// key path too, so sessions must not inherit their reach). Single source
/// of truth for both the session-pull fallback and the vault list.
pub(crate) async fn account_vault_scope(
    state: &SyncState,
    account_id: &str,
) -> Result<Option<HashSet<String>>, ApiError> {
    let conn = state.conn.lock().await;
    let mut stmt = conn
        .prepare(
            "SELECT DISTINCT vault_id FROM api_keys \
             WHERE account_id = ?1 AND revoked = 0 AND vault_id IS NOT NULL",
        )
        .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    let vaults = stmt
        .query_map(rusqlite::params![account_id], |r| r.get::<_, String>(0))
        .map_err(|e| err_json(500, "database_error", &e.to_string()))?
        .collect::<Result<HashSet<String>, _>>()
        .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    Ok(Some(vaults))
}

/// Vaults this account can unlock in a browser: the distinct vault_ids in
/// `sync_blobs` that the account's keys authorize, with blob-version counts
/// and the latest sync time. `blob_count` counts versions, not memories;
/// `live_count` counts non-tombstone rows (= live memories, since
/// sync_blobs is one row per memory), and `label` is the newest device
/// label registered for the vault (ties broken by device_id) — the closest
/// thing to a friendly vault name the relay has. `is_open` reflects the
/// vault_key_wraps envelope (true = opens with the account key, no
/// passphrase prompt).
/// Session auth only — a read-only view of the account's sync footprint.
async fn account_vaults(
    State(state): State<SyncState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let account_id = authenticate_session(&state, &headers).await?;
    let scope = account_vault_scope(&state, &account_id).await?;

    const SELECT: &str = "\
        SELECT s.vault_id, \
               COUNT(*) AS blob_count, \
               COUNT(*) FILTER (WHERE s.deleted = 0) AS live_count, \
               MAX(s.created_at) AS latest_sync, \
               (SELECT dl.label FROM device_labels dl \
                 WHERE dl.vault_id = s.vault_id \
                 ORDER BY dl.updated_at DESC, dl.device_id ASC LIMIT 1) AS label, \
               EXISTS(SELECT 1 FROM vault_key_wraps kw \
                 WHERE kw.vault_id = s.vault_id AND kw.kind = 'account' \
                   AND kw.account_id = ?1) AS is_open \
        FROM sync_blobs s";
    let row_json = |r: &rusqlite::Row<'_>| -> rusqlite::Result<Value> {
        Ok(json!({
            "vault_id": r.get::<_, String>(0)?,
            "blob_count": r.get::<_, i64>(1)?,
            "live_count": r.get::<_, i64>(2)?,
            "latest_sync": r.get::<_, Option<String>>(3)?,
            "label": r.get::<_, Option<String>>(4)?,
            "is_open": r.get::<_, bool>(5)?,
        }))
    };

    let conn = state.conn.lock().await;
    let vaults: Vec<Value> = match &scope {
        None => {
            let mut stmt = conn
                .prepare(&format!(
                    "{SELECT} GROUP BY s.vault_id ORDER BY latest_sync DESC"
                ))
                .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
            let rows = stmt
                .query_map(rusqlite::params![account_id], row_json)
                .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
            rows.collect::<Result<Vec<Value>, _>>()
                .map_err(|e| err_json(500, "database_error", &e.to_string()))?
        }
        Some(ids) => {
            if ids.is_empty() {
                Vec::new()
            } else {
                let sql = format!(
                    "{SELECT} WHERE s.vault_id IN ({}) \
                     GROUP BY s.vault_id ORDER BY latest_sync DESC",
                    vec!["?"; ids.len()].join(",")
                );
                let mut stmt = conn
                    .prepare(&sql)
                    .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
                // ?1 = account_id (is_open), then the IN list.
                let params = std::iter::once(account_id.as_str())
                    .chain(ids.iter().map(String::as_str));
                let rows = stmt
                    .query_map(rusqlite::params_from_iter(params), row_json)
                    .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
                rows.collect::<Result<Vec<Value>, _>>()
                    .map_err(|e| err_json(500, "database_error", &e.to_string()))?
            }
        }
    };
    Ok(Json(json!({ "vaults": vaults })))
}

/// Forget a synced vault: removes every blob, device label and revoked-
/// device row for it. Idempotent — deleting an unknown vault is a 200 with
/// `deleted_blobs: 0` (the picker re-renders after the call, so a double
/// click must not 404). Deliberately does NOT touch `api_keys`: a key
/// scoped to a forgotten vault keeps phantom scope (pull/list simply return
/// nothing) and revoking it here would silently kill a live device's sync.
/// A device that pushes later recreates its vault — expected; this cleans
/// decommissioned vaults, it is not a write-lock.
async fn delete_account_vault(
    State(state): State<SyncState>,
    headers: HeaderMap,
    Path(vault_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let account_id = authenticate_session(&state, &headers).await?;
    let scope = account_vault_scope(&state, &account_id).await?;
    crate::routes::authorize_scope(&scope, &vault_id)?;
    crate::routes::rate_limit_session(&state, &account_id).await?;

    // No awaits while the conn guard is held (mint_and_store_session comment).
    let conn = state.conn.lock().await;
    let deleted_blobs = conn
        .execute(
            "DELETE FROM sync_blobs WHERE vault_id = ?1",
            rusqlite::params![vault_id],
        )
        .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    conn.execute(
        "DELETE FROM device_labels WHERE vault_id = ?1",
        rusqlite::params![vault_id],
    )
    .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    conn.execute(
        "DELETE FROM revoked_devices WHERE vault_id = ?1",
        rusqlite::params![vault_id],
    )
    .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    tracing::info!(account_id, vault_id, deleted_blobs, "account forgot vault");
    Ok(Json(json!({ "vault_id": vault_id, "deleted_blobs": deleted_blobs })))
}

// ── Email+password credentials + passkey management ──────────────────────────

/// What credential methods this account has. No secrets: only the email,
/// existence flags, and base64url ids of attached passkeys (ids are public
/// in WebAuthn anyway). A legacy passkey-only account has no
/// account_credentials row and reports `email: null`.
async fn account_credentials(
    State(state): State<SyncState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let account_id = authenticate_session(&state, &headers).await?;
    crate::routes::rate_limit_session(&state, &account_id).await?;

    let conn = state.conn.lock().await;
    let creds: Option<(String, i64)> = conn
        .query_row(
            "SELECT email, email_verified FROM account_credentials WHERE account_id = ?1",
            rusqlite::params![account_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    let recovery_created_at: Option<String> = conn
        .query_row(
            "SELECT created_at FROM recovery_key_wraps WHERE account_id = ?1",
            rusqlite::params![account_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    let passkeys = list_passkeys(&conn, &account_id)?;
    Ok(Json(json!({
        "email": creds.as_ref().map(|(e, _)| e.clone()),
        "email_verified": creds.as_ref().map(|(_, v)| *v != 0).unwrap_or(false),
        "has_password": creds.is_some(),
        "has_recovery_key": recovery_created_at.is_some(),
        "recovery_created_at": recovery_created_at,
        "passkeys": passkeys,
    })))
}

/// The attached passkeys, newest first. `credential_id` is base64url (no
/// padding) so the SPA can match rows against navigator-returned ids.
async fn account_passkeys(
    State(state): State<SyncState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let account_id = authenticate_session(&state, &headers).await?;
    crate::routes::rate_limit_session(&state, &account_id).await?;
    let conn = state.conn.lock().await;
    Ok(Json(json!({ "passkeys": list_passkeys(&conn, &account_id)? })))
}

fn list_passkeys(conn: &rusqlite::Connection, account_id: &str) -> Result<Vec<Value>, ApiError> {
    let mut stmt = conn
        .prepare(
            "SELECT credential_id, created_at FROM passkeys \
             WHERE account_id = ?1 ORDER BY created_at DESC",
        )
        .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    let rows = stmt
        .query_map(rusqlite::params![account_id], |r| {
            Ok(json!({
                "credential_id": URL_SAFE_NO_PAD.encode(r.get::<_, Vec<u8>>(0)?),
                "created_at": r.get::<_, String>(1)?,
            }))
        })
        .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    rows.collect::<Result<Vec<Value>, _>>()
        .map_err(|e| err_json(500, "database_error", &e.to_string()))
}

/// Detach a passkey (delete the row only — the authenticator still holds a
/// resident copy, which the user should remove there too; that copy can no
/// longer sign in). Guards against self-lockout: an account with no
/// password cannot remove its last passkey.
async fn delete_account_passkey(
    State(state): State<SyncState>,
    headers: HeaderMap,
    Path(credential_id_b64): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let account_id = authenticate_session(&state, &headers).await?;
    crate::routes::rate_limit_session(&state, &account_id).await?;
    let credential_id = URL_SAFE_NO_PAD
        .decode(credential_id_b64.trim_end_matches('='))
        .map_err(|_| err_json(400, "bad_credential_id", "credential_id is not valid base64url"))?;

    let conn = state.conn.lock().await;
    let has_password: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM account_credentials WHERE account_id = ?1",
            rusqlite::params![account_id],
            |r| r.get(0),
        )
        .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    let passkey_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM passkeys WHERE account_id = ?1",
            rusqlite::params![account_id],
            |r| r.get(0),
        )
        .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    if has_password == 0 && passkey_count <= 1 {
        return Err(err_json(
            409,
            "last_credential",
            "this account has no password; removing its only passkey would lock you out",
        ));
    }
    // Fresh-password gate: detaching a passkey is a credential mutation, and
    // a 7-day session alone is not proof.
    let password = body.get("password").and_then(|v| v.as_str());
    password_routes::require_fresh_password(&conn, &account_id, password)?;
    let removed = conn
        .execute(
            "DELETE FROM passkeys WHERE account_id = ?1 AND credential_id = ?2",
            rusqlite::params![account_id, credential_id],
        )
        .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    if removed == 0 {
        return Err(err_json(404, "unknown_passkey", "no such passkey on this account"));
    }
    audit_event(&conn, &account_id, "passkey_detach", None);
    tracing::info!(account_id, "account detached passkey");
    Ok(Json(json!({ "detached": true })))
}

/// Change the account password in-session. Rewrites only the login hash;
/// the client is responsible for rewrapping the account key A under the new
/// password (PUT /account/wraps/password). Other sessions are revoked so a
/// changed password cannot keep previously-issued sessions alive.
async fn change_account_password(
    State(state): State<SyncState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let account_id = authenticate_session(&state, &headers).await?;
    crate::routes::rate_limit_session(&state, &account_id).await?;
    let current_password = str_field(&body, "current_password")?;
    let new_password = str_field(&body, "new_password")?;
    if new_password.chars().count() < password_routes::PASSWORD_MIN_CHARS
        || new_password.len() > password_routes::PASSWORD_MAX_CHARS
    {
        return Err(err_json(400, "weak_password", "password must be 12-128 characters"));
    }
    let current_token_hash = auth::hash_token(bearer_token(&headers)?).to_vec();

    let conn = state.conn.lock().await;
    let stored: Option<(Vec<u8>, Vec<u8>)> = conn
        .query_row(
            "SELECT password_hash, password_salt FROM account_credentials \
             WHERE account_id = ?1",
            rusqlite::params![account_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    let (stored_hash, salt) = match stored {
        Some(row) => row,
        None => {
            return Err(err_json(
                409,
                "no_password",
                "this account has no password (sign in with a passkey)",
            ))
        }
    };
    if !password_routes::verify_password(current_password, &salt, &stored_hash)? {
        return Err(err_json(401, "invalid_password", "current password is incorrect"));
    }
    let (new_salt, new_hash) = password_routes::hash_password(new_password)?;
    conn.execute(
        "UPDATE account_credentials SET password_hash = ?1, password_salt = ?2, updated_at = ?3 \
         WHERE account_id = ?4",
        rusqlite::params![new_hash, new_salt, chrono::Utc::now().to_rfc3339(), account_id],
    )
    .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    let revoked = conn
        .execute(
            "DELETE FROM sessions WHERE account_id = ?1 AND token_hash != ?2",
            rusqlite::params![account_id, current_token_hash],
        )
        .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    audit_event(&conn, &account_id, "password_change", None);
    tracing::info!(account_id, revoked_sessions = revoked, "account password changed");
    Ok(Json(json!({ "changed": true })))
}

// ── Account + vault key envelopes (zero-knowledge: the relay only stores
//    AES-GCM ciphertexts it can never open) ──────────────────────────────────

/// Which envelopes exist for this account + which vaults are OPEN (an
/// `kind='account'` wrap row). `open_vaults` is scoped to vaults the
/// account's API keys authorize, like every other vault listing.
async fn account_wraps(
    State(state): State<SyncState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let account_id = authenticate_session(&state, &headers).await?;
    crate::routes::rate_limit_session(&state, &account_id).await?;
    let scope = account_vault_scope(&state, &account_id).await?;

    let conn = state.conn.lock().await;
    // Generations let the SPA detect stale wraps after a rewrap race (two
    // sessions rotating the password): a PUT that returns a lower generation
    // than the client last saw means another session won.
    let password_wrap_generation: Option<i64> = conn
        .query_row(
            "SELECT generation FROM account_key_wraps WHERE account_id = ?1",
            rusqlite::params![account_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    let recovery_wrap_generation: Option<i64> = conn
        .query_row(
            "SELECT generation FROM recovery_key_wraps WHERE account_id = ?1",
            rusqlite::params![account_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    let mut stmt = conn
        .prepare(
            "SELECT DISTINCT vault_id FROM vault_key_wraps \
             WHERE account_id = ?1 AND kind = 'account' ORDER BY vault_id",
        )
        .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    let rows = stmt
        .query_map(rusqlite::params![account_id], |r| r.get::<_, String>(0))
        .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    let open_vaults: Vec<String> = rows
        .collect::<Result<Vec<String>, _>>()
        .map_err(|e| err_json(500, "database_error", &e.to_string()))?
        .into_iter()
        .filter(|v| scope.as_ref().map(|s| s.contains(v)).unwrap_or(true))
        .collect();
    Ok(Json(json!({
        "password_wrap": password_wrap_generation.is_some(),
        "password_wrap_generation": password_wrap_generation,
        "recovery_wrap": recovery_wrap_generation.is_some(),
        "recovery_wrap_generation": recovery_wrap_generation,
        "open_vaults": open_vaults,
    })))
}

/// Fetch the password envelope blob (ciphertext only). Signin needs it to
/// unwrap A client-side. Serving it to a valid session leaks nothing the
/// session doesn't already imply — it is AES-GCM output under a key derived
/// from the password in the browser, and the relay cannot open it.
async fn get_password_wrap(
    State(state): State<SyncState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let account_id = authenticate_session(&state, &headers).await?;
    crate::routes::rate_limit_session(&state, &account_id).await?;
    let conn = state.conn.lock().await;
    let row = conn
        .query_row(
            "SELECT wrapped_a, salt_pw, generation FROM account_key_wraps WHERE account_id = ?1",
            rusqlite::params![account_id],
            |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, Vec<u8>>(1)?, r.get::<_, i64>(2)?)),
        )
        .optional()
        .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    let Some((wrapped_a, salt_pw, generation)) = row else {
        return Err(err_json(404, "no_password_wrap", "this account has no password wrap"));
    };
    let b64 = |b: Vec<u8>| base64::engine::general_purpose::STANDARD.encode(b);
    Ok(Json(json!({
        "wrapped_a": b64(wrapped_a),
        "salt_pw": b64(salt_pw),
        "generation": generation,
    })))
}

/// Fetch the recovery envelope blob (ciphertext only) — same trust story as
/// the password wrap. The recovery phrase itself is never stored anywhere.
async fn get_recovery_wrap(
    State(state): State<SyncState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let account_id = authenticate_session(&state, &headers).await?;
    crate::routes::rate_limit_session(&state, &account_id).await?;
    let conn = state.conn.lock().await;
    let row = conn
        .query_row(
            "SELECT wrapped_a_rec, salt_rec, generation FROM recovery_key_wraps WHERE account_id = ?1",
            rusqlite::params![account_id],
            |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, Vec<u8>>(1)?, r.get::<_, i64>(2)?)),
        )
        .optional()
        .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    let Some((wrapped_a_rec, salt_rec, generation)) = row else {
        return Err(err_json(404, "no_recovery_wrap", "this account has no recovery wrap"));
    };
    let b64 = |b: Vec<u8>| base64::engine::general_purpose::STANDARD.encode(b);
    Ok(Json(json!({
        "wrapped_a_rec": b64(wrapped_a_rec),
        "salt_rec": b64(salt_rec),
        "generation": generation,
    })))
}

/// Fetch a vault's A-wrapped keys (ciphertext only). Scoped like every
/// vault listing: the session must hold an API key authorizing the vault.
async fn get_vault_wrap(
    State(state): State<SyncState>,
    headers: HeaderMap,
    Path(vault_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let account_id = authenticate_session(&state, &headers).await?;
    let scope = account_vault_scope(&state, &account_id).await?;
    crate::routes::authorize_scope(&scope, &vault_id)?;
    crate::routes::rate_limit_session(&state, &account_id).await?;
    let conn = state.conn.lock().await;
    let row = conn
        .query_row(
            "SELECT wrapped_k, generation FROM vault_key_wraps \
             WHERE account_id = ?1 AND vault_id = ?2 AND kind = 'account'",
            rusqlite::params![account_id, vault_id],
            |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, i64>(1)?)),
        )
        .optional()
        .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    let Some((wrapped_k, generation)) = row else {
        return Err(err_json(404, "no_vault_wrap", "this vault is locked (no account wrap)"));
    };
    Ok(Json(json!({
        "vault_id": vault_id,
        "wrapped_k": base64::engine::general_purpose::STANDARD.encode(wrapped_k),
        "generation": generation,
    })))
}

/// Store (or replace) the password envelope of the account key A.
/// `wrapped_a` is AES-GCM output the client produced; the relay cannot open
/// it. Blobs are size-capped so the wrap tables cannot become a dump.
async fn put_password_wrap(
    State(state): State<SyncState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let account_id = authenticate_session(&state, &headers).await?;
    crate::routes::rate_limit_session(&state, &account_id).await?;
    let wrapped_a = blob_field(&body, "wrapped_a", 512)?;
    let salt_pw = blob_field(&body, "salt_pw", 64)?;
    let now = chrono::Utc::now().to_rfc3339();

    let conn = state.conn.lock().await;
    conn.execute(
        "INSERT INTO account_key_wraps (account_id, wrapped_a, salt_pw, kdf, generation, updated_at) \
         VALUES (?1, ?2, ?3, 'argon2id-65536-3-4', 1, ?4) \
         ON CONFLICT(account_id) DO UPDATE SET \
           wrapped_a = ?2, salt_pw = ?3, kdf = 'argon2id-65536-3-4', \
           generation = account_key_wraps.generation + 1, updated_at = ?4",
        rusqlite::params![account_id, wrapped_a, salt_pw, now],
    )
    .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    let generation: i64 = conn
        .query_row(
            "SELECT generation FROM account_key_wraps WHERE account_id = ?1",
            rusqlite::params![account_id],
            |r| r.get(0),
        )
        .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    audit_event(&conn, &account_id, "password_wrap_put", None);
    tracing::info!(account_id, generation, "account stored password wrap");
    Ok(Json(json!({ "stored": true, "generation": generation })))
}

/// Store (or replace) the recovery-phrase envelope of A. The phrase itself
/// is never sent here — only the ciphertext and the salt. `created_at` is
/// preserved on rotate: it records when recovery was first set up.
async fn put_recovery_wrap(
    State(state): State<SyncState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let account_id = authenticate_session(&state, &headers).await?;
    crate::routes::rate_limit_session(&state, &account_id).await?;
    let wrapped_a_rec = blob_field(&body, "wrapped_a_rec", 512)?;
    let salt_rec = blob_field(&body, "salt_rec", 64)?;
    let now = chrono::Utc::now().to_rfc3339();

    let conn = state.conn.lock().await;
    conn.execute(
        "INSERT INTO recovery_key_wraps (account_id, wrapped_a_rec, salt_rec, kdf, generation, created_at) \
         VALUES (?1, ?2, ?3, 'argon2id-65536-3-4', 1, ?4) \
         ON CONFLICT(account_id) DO UPDATE SET \
           wrapped_a_rec = ?2, salt_rec = ?3, kdf = 'argon2id-65536-3-4', \
           generation = recovery_key_wraps.generation + 1",
        rusqlite::params![account_id, wrapped_a_rec, salt_rec, now],
    )
    .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    let generation: i64 = conn
        .query_row(
            "SELECT generation FROM recovery_key_wraps WHERE account_id = ?1",
            rusqlite::params![account_id],
            |r| r.get(0),
        )
        .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    audit_event(&conn, &account_id, "recovery_wrap_put", None);
    tracing::info!(account_id, generation, "account stored recovery wrap");
    Ok(Json(json!({ "stored": true, "generation": generation })))
}

/// Mark a vault OPEN by default: store A-wrapped vault keys. Scoped — the
/// account must hold an API key for the vault, same as pull/list.
async fn put_vault_wrap(
    State(state): State<SyncState>,
    headers: HeaderMap,
    Path(vault_id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let account_id = authenticate_session(&state, &headers).await?;
    let scope = account_vault_scope(&state, &account_id).await?;
    authorize_wrap_write(&scope, &account_id, &vault_id)?;
    crate::routes::rate_limit_session(&state, &account_id).await?;
    let wrapped_k = blob_field(&body, "wrapped_k", 512)?;
    let now = chrono::Utc::now().to_rfc3339();

    let conn = state.conn.lock().await;
    // Opening a vault gates access to its keys, so like locking it is a
    // credential mutation: accounts with a password must verify it fresh,
    // in this request (passkey-only accounts pass with no password).
    let password = body.get("password").and_then(|v| v.as_str());
    password_routes::require_fresh_password(&conn, &account_id, password)?;
    conn.execute(
        "INSERT INTO vault_key_wraps (account_id, vault_id, kind, wrapped_k, generation, created_at, updated_at) \
         VALUES (?1, ?2, 'account', ?3, 1, ?4, ?4) \
         ON CONFLICT(account_id, vault_id, kind) DO UPDATE SET \
           wrapped_k = ?3, generation = vault_key_wraps.generation + 1, updated_at = ?4",
        rusqlite::params![account_id, vault_id, wrapped_k, now],
    )
    .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    let generation: i64 = conn
        .query_row(
            "SELECT generation FROM vault_key_wraps \
             WHERE account_id = ?1 AND vault_id = ?2 AND kind = 'account'",
            rusqlite::params![account_id, vault_id],
            |r| r.get(0),
        )
        .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    audit_event(&conn, &account_id, "vault_wrap_put", Some(vault_id.as_str()));
    tracing::info!(account_id, vault_id, generation, "account opened vault (wrap stored)");
    Ok(Json(json!({ "vault_id": vault_id, "open": true, "generation": generation })))
}

/// Envelope writes are self-regarding: the account stores its OWN wrapped
/// key blob, encrypted under its account key A — a wrap proves nothing
/// without the real vault keys, which only the key handoff can deliver.
/// Scoped accounts must still not claim vaults outside their scope; a
/// keyless account (empty scope — nothing paired yet) may claim any vault,
/// because it has no other vaults to cross into. Reads stay strictly scoped.
fn authorize_wrap_write(
    scope: &Option<HashSet<String>>,
    account_id: &str,
    vault_id: &str,
) -> Result<(), ApiError> {
    match scope {
        None => Ok(()),
        Some(vaults) if vaults.is_empty() => Ok(()),
        Some(vaults) if vaults.contains(vault_id) => Ok(()),
        Some(_) => {
            tracing::warn!(account_id, vault_id, "vault wrap write refused by session scope");
            Err((
                StatusCode::FORBIDDEN,
                Json(json!({ "error": "session is not authorized for this vault" })),
            ))
        }
    }
}

/// Lock a vault again: delete the envelope (idempotent — locking an already
/// locked vault is a 200 with `open: false`). Memories are never
/// re-encrypted; only this envelope row is removed.
async fn delete_vault_wrap(
    State(state): State<SyncState>,
    headers: HeaderMap,
    Path(vault_id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let account_id = authenticate_session(&state, &headers).await?;
    let scope = account_vault_scope(&state, &account_id).await?;
    authorize_wrap_write(&scope, &account_id, &vault_id)?;
    crate::routes::rate_limit_session(&state, &account_id).await?;

    let conn = state.conn.lock().await;
    // Locking gates access to vault keys, so it is a credential mutation:
    // accounts with a password must verify it fresh, in this request.
    let password = body.get("password").and_then(|v| v.as_str());
    password_routes::require_fresh_password(&conn, &account_id, password)?;
    conn.execute(
        "DELETE FROM vault_key_wraps WHERE account_id = ?1 AND vault_id = ?2 AND kind = 'account'",
        rusqlite::params![account_id, vault_id],
    )
    .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    audit_event(&conn, &account_id, "vault_wrap_delete", Some(vault_id.as_str()));
    tracing::info!(account_id, vault_id, "account locked vault (wrap deleted)");
    Ok(Json(json!({ "vault_id": vault_id, "open": false })))
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

/// Decode a required base64 field, size-capped on the DECODED bytes so the
/// wrap tables cannot become a dump.
fn blob_field(body: &Value, name: &str, max_len: usize) -> Result<Vec<u8>, ApiError> {
    let b64 = str_field(body, name)?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .map_err(|_| err_json(400, "bad_base64", &format!("{name} is not valid base64")))?;
    if bytes.len() > max_len {
        return Err(err_json(
            400,
            "too_large",
            &format!("{name} exceeds {max_len} bytes"),
        ));
    }
    Ok(bytes)
}

/// Append a security-relevant account event to the audit log (auth_events
/// table). Best-effort: a failed insert warns but never fails the request.
/// Callers already hold the conn guard. The event list is the only way a
/// user notices activity on a hijacked session (e.g. a passkey added).
pub(crate) fn audit_event(
    conn: &rusqlite::Connection,
    account_id: &str,
    event: &str,
    detail: Option<&str>,
) {
    if let Err(e) = conn.execute(
        "INSERT INTO auth_events (account_id, event, detail, created_at) \
         VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![account_id, event, detail, chrono::Utc::now().to_rfc3339()],
    ) {
        tracing::warn!("audit event insert failed: {e}");
    }
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
             CREATE TABLE pairing_codes (
                 code_hash BLOB PRIMARY KEY, account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
                 created_at TEXT NOT NULL, expires_at TEXT NOT NULL, used INTEGER NOT NULL DEFAULT 0,
                 vault_id TEXT, device_label TEXT
             );
             CREATE TABLE link_intents (
                 id TEXT PRIMARY KEY, code_hash BLOB NOT NULL, public_key BLOB NOT NULL,
                 account_id TEXT REFERENCES accounts(id) ON DELETE CASCADE,
                 sealed_key BLOB, nonce BLOB, device_label TEXT,
                 status TEXT NOT NULL DEFAULT 'pending', vault_id TEXT,
                 created_at TEXT NOT NULL, expires_at TEXT NOT NULL
             );
             CREATE TABLE sync_blobs (
                 vault_id TEXT NOT NULL, memory_id TEXT NOT NULL, device_id TEXT NOT NULL,
                 vector_clock INTEGER NOT NULL DEFAULT 0, ciphertext TEXT NOT NULL,
                 hmac TEXT NOT NULL, created_at TEXT NOT NULL, deleted INTEGER NOT NULL DEFAULT 0,
                 PRIMARY KEY (vault_id, memory_id)
             );
             CREATE TABLE revoked_devices (
                 vault_id TEXT NOT NULL, device_id TEXT NOT NULL, revoked_at TEXT NOT NULL,
                 PRIMARY KEY (vault_id, device_id)
             );
             CREATE TABLE device_labels (
                 vault_id TEXT NOT NULL, device_id TEXT NOT NULL, label TEXT NOT NULL, updated_at TEXT NOT NULL,
                 PRIMARY KEY (vault_id, device_id)
             );
             CREATE TABLE account_credentials (
                 account_id TEXT PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
                 email TEXT NOT NULL UNIQUE, password_hash BLOB NOT NULL, password_salt BLOB NOT NULL,
                 email_verified INTEGER NOT NULL DEFAULT 0, updated_at TEXT NOT NULL
             );
             CREATE TABLE password_reset_tokens (
                 token_hash BLOB PRIMARY KEY, account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
                 created_at TEXT NOT NULL, expires_at TEXT NOT NULL, used INTEGER NOT NULL DEFAULT 0
             );
             CREATE TABLE account_key_wraps (
                 account_id TEXT PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
                 wrapped_a BLOB NOT NULL, salt_pw BLOB NOT NULL,
                 kdf TEXT NOT NULL DEFAULT 'argon2id-65536-3-4',
                 generation INTEGER NOT NULL DEFAULT 1,
                 updated_at TEXT NOT NULL
             );
             CREATE TABLE recovery_key_wraps (
                 account_id TEXT PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
                 wrapped_a_rec BLOB NOT NULL, salt_rec BLOB NOT NULL,
                 kdf TEXT NOT NULL DEFAULT 'argon2id-65536-3-4',
                 generation INTEGER NOT NULL DEFAULT 1,
                 created_at TEXT NOT NULL
             );
             CREATE TABLE vault_key_wraps (
                 account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
                 vault_id TEXT NOT NULL, kind TEXT NOT NULL DEFAULT 'account',
                 wrapped_k BLOB NOT NULL,
                 generation INTEGER NOT NULL DEFAULT 1,
                 created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
                 PRIMARY KEY (account_id, vault_id, kind)
             );
             CREATE TABLE auth_events (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
                 event TEXT NOT NULL, detail TEXT, created_at TEXT NOT NULL
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
            smtp: None,
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

    async fn seed_blob(state: &SyncState, vault: &str, mid: &str, created: &str) {
        let conn = state.conn.lock().await;
        conn.execute(
            "INSERT INTO sync_blobs (vault_id, memory_id, device_id, vector_clock, ciphertext, hmac, created_at, deleted) \
             VALUES (?1, ?2, 'dev-1', 1, 'x', 'h', ?3, 0)",
            rusqlite::params![vault, mid, created],
        )
        .unwrap();
    }

    async fn seed_tombstone(state: &SyncState, vault: &str, mid: &str, created: &str) {
        let conn = state.conn.lock().await;
        // REPLACE mirrors the relay's push: one row per (vault, memory),
        // tombstone overwrites the live row in place.
        conn.execute(
            "INSERT OR REPLACE INTO sync_blobs (vault_id, memory_id, device_id, vector_clock, ciphertext, hmac, created_at, deleted) \
             VALUES (?1, ?2, 'dev-1', 2, 'x', 'h', ?3, 1)",
            rusqlite::params![vault, mid, created],
        )
        .unwrap();
    }

    async fn seed_label(state: &SyncState, vault: &str, device: &str, label: &str, updated: &str) {
        let conn = state.conn.lock().await;
        conn.execute(
            "INSERT INTO device_labels (vault_id, device_id, label, updated_at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![vault, device, label, updated],
        )
        .unwrap();
    }

    // ── Vault list (browser unlock) ──────────────────────────────────────

    #[tokio::test]
    async fn account_vaults_lists_all_vaults_for_scoped_account() {
        let state = test_state();
        seed_session(&state, "acct-1", "tok-1").await;
        {
            let conn = state.conn.lock().await;
            conn.execute(
                "INSERT INTO api_keys (id, account_id, key_hash, key_prefix, rate, vault_id, created_at, revoked) \
                 VALUES ('k1', 'acct-1', ?1, 'en_', 100, 'vault-a', 'now', 0)",
                rusqlite::params![auth::hash_key("en_scoped_a").to_vec()],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO api_keys (id, account_id, key_hash, key_prefix, rate, vault_id, created_at, revoked) \
                 VALUES ('k2', 'acct-1', ?1, 'en_', 100, 'vault-b', 'now', 0)",
                rusqlite::params![auth::hash_key("en_scoped_b").to_vec()],
            )
            .unwrap();
        }
        seed_blob(&state, "vault-a", "m1", "2026-01-01T00:00:00Z").await;
        seed_blob(&state, "vault-a", "m2", "2026-02-01T00:00:00Z").await;
        seed_tombstone(&state, "vault-a", "m2", "2026-03-01T00:00:00Z").await;
        seed_blob(&state, "vault-b", "m3", "2026-01-15T00:00:00Z").await;
        // vault-a: newest label wins; same-updated_at ties break by lowest
        // device_id ("dev-b" < "dev-c" → "Newer"); vault-b has no labels → null.
        seed_label(&state, "vault-a", "dev-a", "Older", "2026-01-01T00:00:00Z").await;
        seed_label(&state, "vault-a", "dev-b", "Newer", "2026-02-01T00:00:00Z").await;
        seed_label(&state, "vault-a", "dev-c", "AlsoNew", "2026-02-01T00:00:00Z").await;

        let res = account_vaults(State(state.clone()), bearer("tok-1"))
            .await
            .expect("valid session");
        let vaults = res.0["vaults"].as_array().expect("vaults array");
        assert_eq!(vaults.len(), 2);
        let a = vaults.iter().find(|v| v["vault_id"] == "vault-a").unwrap();
        assert_eq!(a["blob_count"], 2); // m1 live + m2 tombstone (replaced in place)
        assert_eq!(a["live_count"], 1); // tombstone excluded from live count
        assert_eq!(a["latest_sync"], "2026-03-01T00:00:00Z");
        assert_eq!(a["label"], "Newer"); // newest updated_at; tie → lowest device_id
        let b = vaults.iter().find(|v| v["vault_id"] == "vault-b").unwrap();
        assert_eq!(b["blob_count"], 1);
        assert_eq!(b["live_count"], 1);
        assert_eq!(b["label"], Value::Null);
    }

    #[tokio::test]
    async fn account_vaults_lists_only_scoped_vault() {
        let state = test_state();
        seed_session(&state, "acct-1", "tok-1").await;
        {
            let conn = state.conn.lock().await;
            conn.execute(
                "INSERT INTO api_keys (id, account_id, key_hash, key_prefix, rate, vault_id, created_at, revoked) \
                 VALUES ('k1', 'acct-1', ?1, 'en_', 100, 'vault-a', 'now', 0)",
                rusqlite::params![auth::hash_key("en_scoped").to_vec()],
            )
            .unwrap();
        }
        seed_blob(&state, "vault-a", "m1", "2026-01-01T00:00:00Z").await;
        seed_blob(&state, "vault-b", "m2", "2026-01-01T00:00:00Z").await;
        seed_label(&state, "vault-a", "dev-a", "Kitchen", "2026-01-01T00:00:00Z").await;
        seed_label(&state, "vault-b", "dev-b", "Hidden", "2026-01-01T00:00:00Z").await;

        let res = account_vaults(State(state.clone()), bearer("tok-1"))
            .await
            .expect("valid session");
        let vaults = res.0["vaults"].as_array().expect("vaults array");
        assert_eq!(vaults.len(), 1);
        assert_eq!(vaults[0]["vault_id"], "vault-a");
        assert_eq!(vaults[0]["blob_count"], 1);
        assert_eq!(vaults[0]["live_count"], 1);
        assert_eq!(vaults[0]["label"], "Kitchen");
    }

    #[tokio::test]
    async fn account_vaults_legacy_null_keys_grant_nothing() {
        let state = test_state();
        seed_session(&state, "acct-1", "tok-1").await;
        {
            let conn = state.conn.lock().await;
            conn.execute(
                "INSERT INTO api_keys (id, account_id, key_hash, key_prefix, rate, vault_id, created_at, revoked) \
                 VALUES ('k1', 'acct-1', ?1, 'en_', 100, NULL, 'now', 0)",
                rusqlite::params![auth::hash_key("en_unscoped").to_vec()],
            )
            .unwrap();
        }
        seed_blob(&state, "vault-a", "m1", "2026-01-01T00:00:00Z").await;

        let res = account_vaults(State(state.clone()), bearer("tok-1"))
            .await
            .expect("valid session");
        let vaults = res.0["vaults"].as_array().expect("vaults array");
        assert_eq!(vaults.len(), 0, "legacy NULL keys must grant no vault visibility");
    }

    #[tokio::test]
    async fn account_vaults_requires_valid_session() {
        let state = test_state();
        let err = account_vaults(State(state), HeaderMap::new())
            .await
            .expect_err("no session → 401");
        assert_eq!(err.0, StatusCode::UNAUTHORIZED);
    }

    // ── Forget vault (DELETE /account/vaults/{vault_id}) ─────────────────

    async fn seed_scoped_key(state: &SyncState, account_id: &str, id: &str, vault_id: &str) {
        let conn = state.conn.lock().await;
        conn.execute(
            "INSERT INTO api_keys (id, account_id, key_hash, key_prefix, rate, vault_id, created_at, revoked) \
             VALUES (?1, ?2, ?3, 'en_', 100, ?4, 'now', 0)",
            rusqlite::params![
                id,
                account_id,
                auth::hash_key(&format!("en_scoped-{id}")).to_vec(),
                vault_id
            ],
        )
        .unwrap();
    }

    #[tokio::test]
    async fn delete_account_vault_removes_everything_and_is_idempotent() {
        let state = test_state();
        seed_session(&state, "acct-1", "tok-1").await;
        {
            let conn = state.conn.lock().await;
            conn.execute(
                "INSERT INTO api_keys (id, account_id, key_hash, key_prefix, rate, vault_id, created_at, revoked) \
                 VALUES ('k1', 'acct-1', ?1, 'en_', 100, 'vault-a', 'now', 0)",
                rusqlite::params![auth::hash_key("en_scoped").to_vec()],
            )
            .unwrap();
        }
        seed_blob(&state, "vault-a", "m1", "2026-01-01T00:00:00Z").await;
        seed_blob(&state, "vault-a", "m2", "2026-02-01T00:00:00Z").await;
        seed_label(&state, "vault-a", "dev-a", "Old", "2026-01-01T00:00:00Z").await;
        {
            let conn = state.conn.lock().await;
            conn.execute(
                "INSERT INTO revoked_devices (vault_id, device_id, revoked_at) VALUES ('vault-a', 'dev-x', 'now')",
                [],
            )
            .unwrap();
        }

        let res = delete_account_vault(State(state.clone()), bearer("tok-1"), Path("vault-a".into()))
            .await
            .expect("valid session, scoped to vault-a");
        assert_eq!(res.0["vault_id"], "vault-a");
        assert_eq!(res.0["deleted_blobs"], 2);

        {
            let conn = state.conn.lock().await;
            for table in ["sync_blobs", "device_labels", "revoked_devices"] {
                let n: i64 = conn
                    .query_row(
                        &format!("SELECT COUNT(*) FROM {table} WHERE vault_id = 'vault-a'"),
                        [],
                        |r| r.get(0),
                    )
                    .unwrap();
                assert_eq!(n, 0, "{table} not cleared");
            }
        }

        // Idempotent: deleting again is a 200 with zero, not a 404.
        let again = delete_account_vault(State(state.clone()), bearer("tok-1"), Path("vault-a".into()))
            .await
            .expect("second delete still 200");
        assert_eq!(again.0["deleted_blobs"], 0);
    }

    #[tokio::test]
    async fn delete_account_vault_respects_scoped_keys() {
        let state = test_state();
        seed_session(&state, "acct-1", "tok-1").await;
        {
            let conn = state.conn.lock().await;
            conn.execute(
                "INSERT INTO api_keys (id, account_id, key_hash, key_prefix, rate, vault_id, created_at, revoked) \
                 VALUES ('k1', 'acct-1', ?1, 'en_', 100, 'vault-a', 'now', 0)",
                rusqlite::params![auth::hash_key("en_scoped").to_vec()],
            )
            .unwrap();
        }
        seed_blob(&state, "vault-a", "m1", "2026-01-01T00:00:00Z").await;
        seed_blob(&state, "vault-b", "m2", "2026-01-01T00:00:00Z").await;

        // In scope → 200.
        let ok = delete_account_vault(State(state.clone()), bearer("tok-1"), Path("vault-a".into()))
            .await
            .expect("scoped key covers vault-a");
        assert_eq!(ok.0["deleted_blobs"], 1);

        // Cross-vault → 403, vault-b blobs untouched.
        let err = delete_account_vault(State(state.clone()), bearer("tok-1"), Path("vault-b".into()))
            .await
            .expect_err("scoped key must not reach vault-b");
        assert_eq!(err.0, StatusCode::FORBIDDEN);
        let conn = state.conn.lock().await;
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM sync_blobs WHERE vault_id = 'vault-b'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
    }

    #[tokio::test]
    async fn delete_account_vault_requires_valid_session() {
        let state = test_state();
        let err = delete_account_vault(State(state), HeaderMap::new(), Path("vault-a".into()))
            .await
            .expect_err("no session → 401");
        assert_eq!(err.0, StatusCode::UNAUTHORIZED);
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
    async fn create_account_key_requires_vault_id() {
        let state = test_state();
        seed_session(&state, "acct-1", "test-token").await;
        for (body, code) in [
            (json!({}), "missing_vault_id"),
            (json!({"vault_id": null}), "missing_vault_id"),
            (json!({"vault_id": ""}), "bad_vault_id"),
            (json!({"vault_id": 42}), "bad_vault_id"),
        ] {
            let err = create_account_key(
                State(state.clone()),
                bearer("test-token"),
                Json(body),
            )
            .await
            .expect_err("no NULL-scoped keys are minted");
            assert_eq!(err.0, StatusCode::BAD_REQUEST);
            assert_eq!(err.1["code"], code, "body: missing/empty/non-string vault_id");
        }
    }

    #[tokio::test]
    async fn create_account_key_mint_matrix() {
        let state = test_state();
        seed_session(&state, "acct-1", "test-token").await;
        seed_session(&state, "acct-2", "token-2").await;
        seed_scoped_key(&state, "acct-1", "k1", "vault-a").await;
        seed_blob(&state, "vault-a", "m1", "2026-01-01T00:00:00Z").await;
        {
            let conn = state.conn.lock().await;
            conn.execute(
                "INSERT INTO vault_key_wraps (account_id, vault_id, kind, wrapped_k, generation, created_at, updated_at) \
                 VALUES ('acct-2', 'vault-w', 'account', 'x', 1, 'now', 'now')",
                [],
            )
            .unwrap();
        }

        // (a) scoped key for the vault → allowed even though the vault has blobs.
        let _ = create_account_key(
            State(state.clone()),
            bearer("test-token"),
            Json(json!({"vault_id": "vault-a"})),
        )
        .await
        .expect("scoped key mints another key for the same vault");

        // (b) account holds a vault key wrap → allowed.
        let _ = create_account_key(
            State(state.clone()),
            bearer("token-2"),
            Json(json!({"vault_id": "vault-w"})),
        )
        .await
        .expect("wrap-owning account mints");

        // (c) founding member: vault has no blobs on the relay → allowed.
        let _ = create_account_key(
            State(state.clone()),
            bearer("test-token"),
            Json(json!({"vault_id": "vault-new"})),
        )
        .await
        .expect("empty vault mints");

        // (d) foreign non-empty vault → denied.
        seed_blob(&state, "vault-foreign", "m1", "2026-01-01T00:00:00Z").await;
        let err = create_account_key(
            State(state.clone()),
            bearer("test-token"),
            Json(json!({"vault_id": "vault-foreign"})),
        )
        .await
        .expect_err("no key, no wrap, vault not empty");
        assert_eq!(err.0, StatusCode::FORBIDDEN);
        assert_eq!(err.1["code"], "vault_not_owned");
        // Once a blob lands in a founding vault, the founding branch closes.
        seed_blob(&state, "vault-new", "m1", "2026-01-01T00:00:00Z").await;
        let err = create_account_key(
            State(state),
            bearer("token-2"),
            Json(json!({"vault_id": "vault-new"})),
        )
        .await
        .expect_err("vault-new now has blobs and acct-2 holds no claim");
        assert_eq!(err.0, StatusCode::FORBIDDEN);
        assert_eq!(err.1["code"], "vault_not_owned");
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

    // ── Device pairing (WARP-style onboarding) ──────────────────────────

    #[tokio::test]
    async fn mint_pairing_code_requires_session() {
        let state = test_state();
        let err = mint_pairing_code(State(state), HeaderMap::new(), Json(json!({})))
            .await
            .expect_err("no session");
        assert_eq!(err.0, StatusCode::UNAUTHORIZED);
        assert_eq!(err.1["code"], "missing_authorization");
    }

    #[tokio::test]
    async fn pairing_code_round_trip_single_use_and_hashed_at_rest() {
        let state = test_state();
        seed_session(&state, "acct-1", "test-token").await;

        let res = mint_pairing_code(
            State(state.clone()),
            bearer("test-token"),
            Json(json!({"vault_id": "vault-a", "device_label": "my laptop"})),
        )
        .await
        .expect("session valid");
        let body = res.0;
        let code = body["code"].as_str().expect("plaintext code returned once");
        assert_eq!(code.len(), 18, "ENG-XXXX-XXXX-XXXX");
        assert!(code.starts_with("ENG-"));
        assert_eq!(body["expires_in"], 600);
        assert_eq!(body["vault_id"], "vault-a", "code knows its vault");

        // Stored: sha256 only — never the plaintext — plus the vault the
        // code was minted for and the device label.
        {
            let conn = state.conn.lock().await;
            let (hash, used, vid, lbl): (Vec<u8>, i64, String, String) = conn
                .query_row(
                    "SELECT code_hash, used, vault_id, device_label FROM pairing_codes",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .unwrap();
            assert_eq!(hash, auth::hash_key(code).to_vec(), "sha256 at rest");
            assert_ne!(hash, code.as_bytes());
            assert_eq!(used, 0);
            assert_eq!(vid, "vault-a", "stored code carries its vault");
            assert_eq!(lbl, "my laptop", "stored code carries the label");
        }

        // Redeem — accept lowercase input (typed codes are case-insensitive).
        // The body carries no vault_id: the vault comes from the code itself.
        let res = redeem_pairing_code(
            State(state.clone()),
            Json(json!({"code": code.to_lowercase()})),
        )
        .await
        .expect("fresh code redeems");
        let body = res.0;
        let api_key = body["api_key"].as_str().expect("full key returned once");
        assert!(api_key.starts_with(auth::API_KEY_PREFIX));
        assert_eq!(body["vault_id"], "vault-a", "minted key is scoped");
        assert_eq!(body["device_label"], "my laptop", "label echoes back");
        // The minted key authenticates through the normal api_keys path.
        {
            let conn = state.conn.lock().await;
            let (stored, scoped): (Vec<u8>, String) = conn
                .query_row(
                    "SELECT key_hash, vault_id FROM api_keys WHERE key_hash = ?1",
                    rusqlite::params![auth::hash_key(api_key).to_vec()],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_eq!(stored, auth::hash_key(api_key).to_vec());
            assert_eq!(scoped, "vault-a", "stored key carries the scope");
        }

        // Second redeem: single-use.
        let err = redeem_pairing_code(
            State(state.clone()),
            Json(json!({"code": code})),
        )
        .await
        .expect_err("already used");
        assert_eq!(err.0, StatusCode::UNAUTHORIZED);
        assert_eq!(err.1["code"], "invalid_pairing_code");

        // Wrong code: same shape as unknown.
        let err = redeem_pairing_code(
            State(state.clone()),
            Json(json!({"code": "ENG-2222-2222-2222"})),
        )
        .await
        .expect_err("unknown code");
        assert_eq!(err.0, StatusCode::UNAUTHORIZED);
        assert_eq!(err.1["code"], "invalid_pairing_code");
    }

    #[tokio::test]
    async fn pair_mint_for_foreign_nonempty_vault_is_denied() {
        let state = test_state();
        seed_session(&state, "acct-1", "test-token").await;
        seed_scoped_key(&state, "acct-1", "k1", "vault-a").await;
        seed_blob(&state, "vault-a", "m1", "2026-01-01T00:00:00Z").await;
        seed_blob(&state, "vault-b", "m1", "2026-01-01T00:00:00Z").await;

        // The account owns vault-a but not vault-b — minting a code FOR
        // vault-b must fail before the code exists (no code to burn later).
        let err = mint_pairing_code(
            State(state.clone()),
            bearer("test-token"),
            Json(json!({"vault_id": "vault-b"})),
        )
        .await
        .expect_err("foreign non-empty vault");
        assert_eq!(err.0, StatusCode::FORBIDDEN);
        assert_eq!(err.1["code"], "vault_not_owned");
        {
            let conn = state.conn.lock().await;
            let count: i64 = conn
                .query_row("SELECT COUNT(*) FROM pairing_codes", [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 0, "denied mint leaves no code behind");
        }

        // Old CLIs still send body.vault_id at redeem — it is ignored; the
        // row's vault wins. The key is scoped to vault-a, not vault-b.
        let res = mint_pairing_code(
            State(state.clone()),
            bearer("test-token"),
            Json(json!({"vault_id": "vault-a"})),
        )
        .await
        .expect("mint for owned vault");
        let code = res.0["code"].as_str().unwrap().to_string();
        let res = redeem_pairing_code(
            State(state),
            Json(json!({"code": code, "vault_id": "vault-b"})),
        )
        .await
        .expect("old-style body still redeems");
        assert_eq!(res.0["vault_id"], "vault-a", "row's vault, not body's");
    }

    #[tokio::test]
    async fn expired_pairing_code_is_rejected() {
        let state = test_state();
        seed_session(&state, "acct-1", "test-token").await;
        {
            let conn = state.conn.lock().await;
            conn.execute(
                "INSERT INTO pairing_codes (code_hash, account_id, created_at, expires_at, used) \
                 VALUES (?1, 'acct-1', 'now', '2000-01-01T00:00:00Z', 0)",
                rusqlite::params![auth::hash_key("ENG-2345-6789-ABCD").to_vec()],
            )
            .unwrap();
        }
        let err = redeem_pairing_code(
            State(state),
            Json(json!({"code": "ENG-2345-6789-ABCD"})),
        )
        .await
        .expect_err("expired");
        assert_eq!(err.0, StatusCode::UNAUTHORIZED);
        assert_eq!(err.1["code"], "expired_pairing_code");
    }

    /// The "CLI" side of a link test: a fixed keypair the relay doesn't know.
    fn link_test_cli_keypair() -> ([u8; 32], x25519_dalek::StaticSecret) {
        let sk = x25519_dalek::StaticSecret::from([7u8; 32]);
        let pk = *x25519_dalek::PublicKey::from(&sk).as_bytes();
        (pk, sk)
    }

    /// Create an intent, returning (id, code) from the relay's response.
    /// The body carries the client-derived vault_id — required since W3.
    async fn create_test_link_intent(
        state: &SyncState,
        pk_b64: &str,
    ) -> (String, String, serde_json::Value) {
        let (status, body) = create_link_intent(
            State(state.clone()),
            Json(json!({
                "public_key": pk_b64,
                "device_label": "test-laptop",
                "vault_id": "vault-a",
            })),
        )
        .await
        .expect("mint");
        assert_eq!(status, StatusCode::CREATED);
        let id = body["id"].as_str().expect("id").to_string();
        let code = body["code"].as_str().expect("code").to_string();
        assert_eq!(body["expires_in"], 600);
        (id, code, body.0)
    }

    #[tokio::test]
    async fn link_intent_round_trip_seals_and_delivers_once() {
        let state = test_state();
        let (pk, _sk_cli) = link_test_cli_keypair();
        let pk_b64 = URL_SAFE_NO_PAD.encode(pk);

        let (id, code, body) = create_test_link_intent(&state, &pk_b64).await;
        // The returned relay public key matches the deterministic derivation.
        let relay_pk = URL_SAFE_NO_PAD
            .decode(body["relay_public_key"].as_str().unwrap())
            .unwrap();
        let derived = x25519_dalek::PublicKey::from(&crate::link_crypto::intent_keypair(
            &id,
            &auth::hash_key(&code),
        ));
        assert_eq!(relay_pk, derived.as_bytes().to_vec());

        // Only the sha256 is at rest.
        {
            let conn = state.conn.lock().await;
            let (hash, status, sealed, nonce): (Vec<u8>, String, Option<Vec<u8>>, Option<Vec<u8>>) =
                conn.query_row(
                    "SELECT code_hash, status, sealed_key, nonce FROM link_intents WHERE id = ?1",
                    rusqlite::params![id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .unwrap();
            assert_eq!(hash, auth::hash_key(&code).to_vec(), "sha256 at rest");
            assert_ne!(hash, code.as_bytes());
            assert_eq!(status, "pending");
            assert!(sealed.is_none() && nonce.is_none(), "nothing sealed pre-confirm");
        }

        // Pending before the browser confirms.
        let res = link_intent_status(State(state.clone()), Path(id.clone()))
            .await
            .expect("pending poll");
        assert_eq!(res["status"], "pending");

        // Confirm without a session → 401.
        let err = confirm_link_intent(
            State(state.clone()),
            bearer("nope"),
            Path(id.clone()),
            Json(json!({"code": code})),
        )
        .await
        .expect_err("no session");
        assert_eq!(err.0, StatusCode::UNAUTHORIZED);

        seed_session(&state, "acct-1", "test-token").await;

        // Wrong code → 403, intent still pending.
        let err = confirm_link_intent(
            State(state.clone()),
            bearer("test-token"),
            Path(id.clone()),
            Json(json!({"code": "ENG-2222-2222-2222"})),
        )
        .await
        .expect_err("wrong code");
        assert_eq!(err.0, StatusCode::FORBIDDEN);
        assert_eq!(err.1["code"], "invalid_link_code");

        // Correct code (lowercase accepted) confirms — plaintext key is NOT echoed.
        let res = confirm_link_intent(
            State(state.clone()),
            bearer("test-token"),
            Path(id.clone()),
            Json(json!({"code": code.to_lowercase()})),
        )
        .await
        .expect("confirm");
        let body = res.0;
        assert_eq!(body["status"], "confirmed");
        assert!(body["key_id"].as_str().is_some());
        assert!(body.get("api_key").is_none(), "sealed, never echoed");

        // First status poll claims the seal and delivers it exactly once.
        let res = link_intent_status(State(state.clone()), Path(id.clone()))
            .await
            .expect("claim");
        let body = res.0;
        assert_eq!(body["status"], "confirmed");
        assert_eq!(body["key_prefix"], auth::API_KEY_PREFIX);
        let sealed = URL_SAFE_NO_PAD.decode(body["sealed_key"].as_str().unwrap()).unwrap();
        let nonce = URL_SAFE_NO_PAD.decode(body["nonce"].as_str().unwrap()).unwrap();
        assert_eq!(nonce.len(), 12);

        // The CLI's side of the handshake decrypts the key.
        let sk_r = crate::link_crypto::intent_keypair(&id, &auth::hash_key(&code));
        let shared =
            crate::link_crypto::link_shared_secret(&sk_r, &x25519_dalek::PublicKey::from(pk))
                .unwrap();
        let api_key = crate::link_crypto::unseal_api_key(&id, &shared, &sealed, &nonce).unwrap();
        assert!(api_key.starts_with(auth::API_KEY_PREFIX), "minted account key");
        // …and it authenticates through the normal api_keys path, scoped to
        // the intent's vault.
        {
            let conn = state.conn.lock().await;
            let (stored, scoped): (Vec<u8>, String) = conn
                .query_row(
                    "SELECT key_hash, vault_id FROM api_keys WHERE key_hash = ?1",
                    rusqlite::params![auth::hash_key(&api_key).to_vec()],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_eq!(stored, auth::hash_key(&api_key).to_vec());
            assert_eq!(scoped, "vault-a", "link-confirmed key carries the intent's vault");
            let account: String = conn
                .query_row(
                    "SELECT account_id FROM link_intents WHERE id = ?1",
                    rusqlite::params![id],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(account, "acct-1", "intent bound to confirming account");
        }

        // One-shot: second poll and second confirm both 410.
        let err = link_intent_status(State(state.clone()), Path(id.clone()))
            .await
            .expect_err("already delivered");
        assert_eq!(err.0, StatusCode::GONE);
        assert_eq!(err.1["code"], "link_intent_delivered");

        let err = confirm_link_intent(
            State(state.clone()),
            bearer("test-token"),
            Path(id.clone()),
            Json(json!({"code": code})),
        )
        .await
        .expect_err("already confirmed");
        assert_eq!(err.0, StatusCode::GONE);
        assert_eq!(err.1["code"], "link_intent_delivered");
    }

    #[tokio::test]
    async fn link_intent_status_unknown_is_404() {
        let state = test_state();
        let err = link_intent_status(State(state), Path("missing".into()))
            .await
            .expect_err("unknown");
        assert_eq!(err.0, StatusCode::NOT_FOUND);
        assert_eq!(err.1["code"], "link_intent_not_found");
    }

    #[tokio::test]
    async fn link_intent_expiry_is_410() {
        let state = test_state();
        let (pk, _sk_cli) = link_test_cli_keypair();
        let pk_b64 = URL_SAFE_NO_PAD.encode(pk);
        let (id, code, _body) = create_test_link_intent(&state, &pk_b64).await;
        {
            let conn = state.conn.lock().await;
            conn.execute(
                "UPDATE link_intents SET expires_at = '2000-01-01T00:00:00Z' WHERE id = ?1",
                rusqlite::params![id],
            )
            .unwrap();
        }
        let err = link_intent_status(State(state.clone()), Path(id.clone()))
            .await
            .expect_err("expired poll");
        assert_eq!(err.0, StatusCode::GONE);
        assert_eq!(err.1["code"], "link_intent_expired");

        seed_session(&state, "acct-1", "test-token").await;
        let err = confirm_link_intent(
            State(state.clone()),
            bearer("test-token"),
            Path(id.clone()),
            Json(json!({"code": code})),
        )
        .await
        .expect_err("expired confirm");
        assert_eq!(err.0, StatusCode::GONE);
        assert_eq!(err.1["code"], "link_intent_expired");
    }

    #[tokio::test]
    async fn confirm_link_intent_legacy_null_vault_is_410() {
        // Intents created before the vault_id column existed have no vault to
        // scope a minted key to — confirming must 410 with a re-run message
        // instead of minting a dead NULL-scoped key.
        let state = test_state();
        seed_session(&state, "acct-1", "test-token").await;
        let (pk, _sk_cli) = link_test_cli_keypair();
        let code = "ENG-AAAA-BBBB-CCCC";
        {
            let conn = state.conn.lock().await;
            conn.execute(
                "INSERT INTO link_intents (id, code_hash, public_key, account_id, sealed_key, nonce, device_label, status, vault_id, created_at, expires_at) \
                 VALUES ('intent-legacy', ?1, ?2, NULL, NULL, NULL, 'old-laptop', 'pending', NULL, 'now', '2999-01-01T00:00:00Z')",
                rusqlite::params![auth::hash_key(code).to_vec(), pk.to_vec()],
            )
            .unwrap();
        }
        let err = confirm_link_intent(
            State(state),
            bearer("test-token"),
            Path("intent-legacy".into()),
            Json(json!({"code": code})),
        )
        .await
        .expect_err("NULL vault_id → re-create the link");
        assert_eq!(err.0, StatusCode::GONE);
        assert_eq!(err.1["code"], "link_intent_delivered");
        assert!(
            err.1["error"].as_str().unwrap().contains("predates"),
            "error must tell the user to re-run `engram link`"
        );
    }

    #[tokio::test]
    async fn link_intent_rejects_bad_public_key() {
        let state = test_state();
        // 33 bytes → 400.
        let err = create_link_intent(
            State(state.clone()),
            Json(json!({"public_key": URL_SAFE_NO_PAD.encode([7u8; 33]), "vault_id": "vault-a"})),
        )
        .await
        .expect_err("33-byte key");
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert_eq!(err.1["code"], "invalid_public_key");
        // Not base64url → 400.
        let err = create_link_intent(
            State(state.clone()),
            Json(json!({"public_key": "not!!base64", "vault_id": "vault-a"})),
        )
        .await
        .expect_err("bad b64");
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert_eq!(err.1["code"], "invalid_public_key");
        // Missing public_key → 400 (str_field).
        let err = create_link_intent(
            State(state.clone()),
            Json(json!({"vault_id": "vault-a"})),
        )
        .await
        .expect_err("missing field");
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        // Missing vault_id → 400 (required since W3).
        let err = create_link_intent(
            State(state),
            Json(json!({"public_key": URL_SAFE_NO_PAD.encode([7u8; 32])})),
        )
        .await
        .expect_err("missing vault_id");
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert_eq!(err.1["code"], "missing_vault_id");
    }

    #[tokio::test]
    async fn live_pairing_codes_are_capped_per_account() {
        let state = test_state();
        seed_session(&state, "acct-1", "test-token").await;
        for _ in 0..PAIRING_CODES_PER_ACCOUNT {
            mint_pairing_code(
                State(state.clone()),
                bearer("test-token"),
                Json(json!({"vault_id": "vault-a"})),
            )
            .await
            .expect("under the cap");
        }
        let err = mint_pairing_code(
            State(state),
            bearer("test-token"),
            Json(json!({"vault_id": "vault-a"})),
        )
        .await
        .expect_err("cap reached");
        assert_eq!(err.0, StatusCode::CONFLICT);
        assert_eq!(err.1["code"], "too_many_codes");
    }

    // ── Email+password credentials + passkey management (Kimi revision) ──

    async fn insert_password_account(state: &SyncState, account_id: &str, password: &str) {
        let (salt, hash) = crate::password_routes::hash_password(password).unwrap();
        let conn = state.conn.lock().await;
        conn.execute(
            "INSERT OR IGNORE INTO accounts (id, created_at) VALUES (?1, 'now')",
            rusqlite::params![account_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO account_credentials \
             (account_id, email, password_hash, password_salt, email_verified, updated_at) \
             VALUES (?1, ?2, ?3, ?4, 0, 'now')",
            rusqlite::params![account_id, format!("{account_id}@example.com"), hash, salt],
        )
        .unwrap();
    }

    async fn insert_passkey_row(state: &SyncState, account_id: &str, cred_id: &[u8]) {
        let conn = state.conn.lock().await;
        conn.execute(
            "INSERT INTO passkeys (account_id, credential_id, public_key, created_at) \
             VALUES (?1, ?2, x'00', 'now')",
            rusqlite::params![account_id, cred_id],
        )
        .unwrap();
    }

    async fn audit_count(state: &SyncState, account_id: &str, event: &str) -> i64 {
        let conn = state.conn.lock().await;
        conn.query_row(
            "SELECT COUNT(*) FROM auth_events WHERE account_id = ?1 AND event = ?2",
            rusqlite::params![account_id, event],
            |r| r.get(0),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn account_credentials_reports_email_flags_and_passkeys() {
        let state = test_state();
        seed_session(&state, "acct-1", "tok-1").await;
        insert_password_account(&state, "acct-1", "correct-horse-battery-staple").await;
        insert_passkey_row(&state, "acct-1", b"cred-bytes-1").await;

        let res = account_credentials(State(state), bearer("tok-1"))
            .await
            .expect("valid session");
        let body = res.0;
        assert_eq!(body["email"], "acct-1@example.com");
        assert_eq!(body["email_verified"], false);
        assert_eq!(body["has_password"], true);
        assert_eq!(body["has_recovery_key"], false);
        let passkeys = body["passkeys"].as_array().unwrap();
        assert_eq!(passkeys.len(), 1);
        assert_eq!(
            passkeys[0]["credential_id"],
            URL_SAFE_NO_PAD.encode(b"cred-bytes-1".as_slice())
        );
    }

    #[tokio::test]
    async fn account_credentials_legacy_passkey_only_account_has_no_email() {
        let state = test_state();
        seed_session(&state, "acct-1", "tok-1").await;
        insert_passkey_row(&state, "acct-1", b"cred-bytes-1").await;
        let res = account_credentials(State(state), bearer("tok-1"))
            .await
            .expect("session");
        let body = res.0;
        assert_eq!(body["email"], Value::Null);
        assert_eq!(body["has_password"], false);
        assert_eq!(body["passkeys"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn delete_account_passkey_requires_fresh_password() {
        let state = test_state();
        seed_session(&state, "acct-1", "tok-1").await;
        insert_password_account(&state, "acct-1", "correct-horse-battery-staple").await;
        insert_passkey_row(&state, "acct-1", b"cred-bytes-1").await;
        let cred_b64 = URL_SAFE_NO_PAD.encode(b"cred-bytes-1".as_slice());

        // No password in the body → 401 password_required.
        let err = delete_account_passkey(
            State(state.clone()),
            bearer("tok-1"),
            Path(cred_b64.clone()),
            Json(json!({})),
        )
        .await
        .expect_err("password gate");
        assert_eq!(err.0, StatusCode::UNAUTHORIZED);
        assert_eq!(err.1["code"], "password_required");

        // Wrong password → 401 invalid_password.
        let err = delete_account_passkey(
            State(state.clone()),
            bearer("tok-1"),
            Path(cred_b64.clone()),
            Json(json!({"password": "wrong-password-1"})),
        )
        .await
        .expect_err("wrong password");
        assert_eq!(err.0, StatusCode::UNAUTHORIZED);
        assert_eq!(err.1["code"], "invalid_password");

        // Fresh password → detach succeeds, row gone, audit written.
        let res = delete_account_passkey(
            State(state.clone()),
            bearer("tok-1"),
            Path(cred_b64),
            Json(json!({"password": "correct-horse-battery-staple"})),
        )
        .await
        .expect("fresh password");
        assert_eq!(res.0["detached"], true);
        {
            let conn = state.conn.lock().await;
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM passkeys WHERE account_id = 'acct-1'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 0);
        }
        assert_eq!(audit_count(&state, "acct-1", "passkey_detach").await, 1);
    }

    #[tokio::test]
    async fn delete_account_passkey_guards_last_credential_on_passkey_only_account() {
        let state = test_state();
        seed_session(&state, "acct-1", "tok-1").await;
        insert_passkey_row(&state, "acct-1", b"cred-bytes-1").await;
        let cred_b64 = URL_SAFE_NO_PAD.encode(b"cred-bytes-1".as_slice());
        // Passkey-only accounts pass the password gate, then hit the
        // self-lockout guard (no password to fall back on).
        let err = delete_account_passkey(
            State(state),
            bearer("tok-1"),
            Path(cred_b64),
            Json(json!({"password": "irrelevant"})),
        )
        .await
        .expect_err("self-lockout guard");
        assert_eq!(err.0, StatusCode::CONFLICT);
        assert_eq!(err.1["code"], "last_credential");
    }

    #[tokio::test]
    async fn change_account_password_rotates_and_revokes_other_sessions() {
        let state = test_state();
        seed_session(&state, "acct-1", "tok-current").await;
        seed_session(&state, "acct-1", "tok-other").await;
        insert_password_account(&state, "acct-1", "correct-horse-battery-staple").await;

        // Wrong current password → 401.
        let err = change_account_password(
            State(state.clone()),
            bearer("tok-current"),
            Json(json!({"current_password": "wrong-password-1", "new_password": "new-valid-password-2"})),
        )
        .await
        .expect_err("wrong current");
        assert_eq!(err.0, StatusCode::UNAUTHORIZED);
        assert_eq!(err.1["code"], "invalid_password");

        // Correct → 200, hash rotated, other session revoked, this one alive.
        let res = change_account_password(
            State(state.clone()),
            bearer("tok-current"),
            Json(json!({"current_password": "correct-horse-battery-staple", "new_password": "new-valid-password-2"})),
        )
        .await
        .expect("correct current");
        assert_eq!(res.0["changed"], true);

        let conn = state.conn.lock().await;
        let (hash, salt): (Vec<u8>, Vec<u8>) = conn
            .query_row(
                "SELECT password_hash, password_salt FROM account_credentials WHERE account_id = 'acct-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert!(crate::password_routes::verify_password("new-valid-password-2", &salt, &hash).unwrap());
        let other: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sessions WHERE token_hash = ?1",
                rusqlite::params![auth::hash_token("tok-other").to_vec()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(other, 0, "other session revoked");
        let current: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sessions WHERE token_hash = ?1",
                rusqlite::params![auth::hash_token("tok-current").to_vec()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(current, 1, "current session survives");
        drop(conn);
        assert_eq!(audit_count(&state, "acct-1", "password_change").await, 1);
    }

    #[tokio::test]
    async fn change_account_password_rejects_passkey_only_accounts() {
        let state = test_state();
        seed_session(&state, "acct-1", "tok-1").await;
        let err = change_account_password(
            State(state),
            bearer("tok-1"),
            Json(json!({"current_password": "whatever-password", "new_password": "new-valid-password-2"})),
        )
        .await
        .expect_err("no password on account");
        assert_eq!(err.0, StatusCode::CONFLICT);
        assert_eq!(err.1["code"], "no_password");
    }

    // ── Wrap CRUD (Kimi revision: generations + per-account envelopes) ───

    #[tokio::test]
    async fn wrap_put_get_roundtrip_with_generations() {
        let state = test_state();
        seed_session(&state, "acct-1", "tok-1").await;
        let wrapped_a = base64::engine::general_purpose::STANDARD.encode(b"wrapped-a-bytes-1");
        let salt_pw = base64::engine::general_purpose::STANDARD.encode(b"salt-pw-1");
        let wrapped_rec = base64::engine::general_purpose::STANDARD.encode(b"wrapped-a-rec-1");
        let salt_rec = base64::engine::general_purpose::STANDARD.encode(b"salt-rec-1");

        let res = put_password_wrap(
            State(state.clone()),
            bearer("tok-1"),
            Json(json!({"wrapped_a": wrapped_a, "salt_pw": salt_pw})),
        )
        .await
        .expect("first put");
        assert_eq!(res.0["stored"], true);
        assert_eq!(res.0["generation"], 1);
        let res = put_password_wrap(
            State(state.clone()),
            bearer("tok-1"),
            Json(json!({"wrapped_a": wrapped_a, "salt_pw": salt_pw})),
        )
        .await
        .expect("second put bumps generation");
        assert_eq!(res.0["generation"], 2);
        let res = put_recovery_wrap(
            State(state.clone()),
            bearer("tok-1"),
            Json(json!({"wrapped_a_rec": wrapped_rec, "salt_rec": salt_rec})),
        )
        .await
        .expect("recovery put");
        assert_eq!(res.0["generation"], 1);

        let res = account_wraps(State(state.clone()), bearer("tok-1"))
            .await
            .expect("session");
        let body = res.0;
        assert_eq!(body["password_wrap"], true);
        assert_eq!(body["password_wrap_generation"], 2);
        assert_eq!(body["recovery_wrap"], true);
        assert_eq!(body["recovery_wrap_generation"], 1);
        assert_eq!(body["open_vaults"].as_array().unwrap().len(), 0);
        assert_eq!(audit_count(&state, "acct-1", "password_wrap_put").await, 2);
        assert_eq!(audit_count(&state, "acct-1", "recovery_wrap_put").await, 1);

        // The signin flow reads the blobs back through the GET endpoints.
        let res = get_password_wrap(State(state.clone()), bearer("tok-1"))
            .await
            .expect("password blob get");
        assert_eq!(res.0["wrapped_a"], wrapped_a.as_str());
        assert_eq!(res.0["salt_pw"], salt_pw.as_str());
        assert_eq!(res.0["generation"], 2);
        let res = get_recovery_wrap(State(state.clone()), bearer("tok-1"))
            .await
            .expect("recovery blob get");
        assert_eq!(res.0["wrapped_a_rec"], wrapped_rec.as_str());
        assert_eq!(res.0["salt_rec"], salt_rec.as_str());
        assert_eq!(res.0["generation"], 1);
    }

    #[tokio::test]
    async fn wrap_gets_404_without_rows() {
        let state = test_state();
        seed_session(&state, "acct-1", "tok-1").await;
        let err = get_password_wrap(State(state.clone()), bearer("tok-1"))
            .await
            .expect_err("no password wrap");
        assert_eq!(err.0, StatusCode::NOT_FOUND);
        assert_eq!(err.1["code"], "no_password_wrap");
        let err = get_recovery_wrap(State(state), bearer("tok-1"))
            .await
            .expect_err("no recovery wrap");
        assert_eq!(err.0, StatusCode::NOT_FOUND);
        assert_eq!(err.1["code"], "no_recovery_wrap");
    }

    #[tokio::test]
    async fn get_vault_wrap_scopes_and_roundtrips() {
        let state = test_state();
        seed_session(&state, "acct-1", "tok-1").await;
        seed_session(&state, "acct-2", "tok-2").await;
        // vault-b stays in scope (no wrap row) so the locked-vault case below
        // is a 404, not a 403.
        seed_scoped_key(&state, "acct-1", "k1", "vault-a").await;
        seed_scoped_key(&state, "acct-1", "k2", "vault-b").await;
        let wrapped_k = base64::engine::general_purpose::STANDARD.encode(b"wrapped-k-bytes");
        let res = put_vault_wrap(
            State(state.clone()),
            bearer("tok-1"),
            Path("vault-a".to_string()),
            Json(json!({"wrapped_k": wrapped_k})),
        )
        .await
        .expect("put wrap");
        assert_eq!(res.0["open"], true);

        // The owner reads the blob back; an unrelated session is 403 (scope).
        let res = get_vault_wrap(
            State(state.clone()),
            bearer("tok-1"),
            Path("vault-a".to_string()),
        )
        .await
        .expect("owner");
        assert_eq!(res.0["wrapped_k"], wrapped_k.as_str());
        assert_eq!(res.0["generation"], 1);
        let err = get_vault_wrap(
            State(state.clone()),
            bearer("tok-2"),
            Path("vault-a".to_string()),
        )
        .await
        .expect_err("cross-account scope");
        assert_eq!(err.0, StatusCode::FORBIDDEN);
        // In scope but no wrap row = the vault is locked → 404, not 403.
        let err = get_vault_wrap(State(state), bearer("tok-1"), Path("vault-b".to_string()))
            .await
            .expect_err("locked vault");
        assert_eq!(err.0, StatusCode::NOT_FOUND);
        assert_eq!(err.1["code"], "no_vault_wrap");
    }

    #[tokio::test]
    async fn keyless_account_can_claim_vault_wrap() {
        // A freshly signed-up account has no API keys yet. Wrapping a vault
        // is an adoption claim — the blob is encrypted under the account's
        // own key A and proves nothing without the real vault keys, which
        // only the key handoff delivers — so a keyless account may claim.
        // Reads stay strictly scoped until keys are granted.
        let state = test_state();
        seed_session(&state, "acct-fresh", "tok-f").await;
        let wrapped_k = base64::engine::general_purpose::STANDARD.encode(b"claimed-wrap");
        let res = put_vault_wrap(
            State(state.clone()),
            bearer("tok-f"),
            Path("vault-a".to_string()),
            Json(json!({"wrapped_k": wrapped_k})),
        )
        .await
        .expect("keyless account may claim a vault by wrapping it");
        assert_eq!(res.0["open"], true);
        let err = get_vault_wrap(
            State(state),
            bearer("tok-f"),
            Path("vault-a".to_string()),
        )
        .await
        .expect_err("reads stay scoped for keyless accounts");
        assert_eq!(err.0, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn put_password_wrap_caps_blob_sizes() {
        let state = test_state();
        seed_session(&state, "acct-1", "tok-1").await;
        let big_salt = base64::engine::general_purpose::STANDARD.encode(vec![7u8; 65]);
        let err = put_password_wrap(
            State(state.clone()),
            bearer("tok-1"),
            Json(json!({"wrapped_a": "aGk=", "salt_pw": big_salt})),
        )
        .await
        .expect_err("salt too big");
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert_eq!(err.1["code"], "too_large");
        let big_wrap = base64::engine::general_purpose::STANDARD.encode(vec![7u8; 513]);
        let err = put_password_wrap(
            State(state.clone()),
            bearer("tok-1"),
            Json(json!({"wrapped_a": big_wrap, "salt_pw": "aGk="})),
        )
        .await
        .expect_err("wrap too big");
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert_eq!(err.1["code"], "too_large");
        let err = put_password_wrap(
            State(state),
            bearer("tok-1"),
            Json(json!({"wrapped_a": "not base64!!", "salt_pw": "aGk="})),
        )
        .await
        .expect_err("bad base64");
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert_eq!(err.1["code"], "bad_base64");
    }

    #[tokio::test]
    async fn vault_wrap_open_lock_flow_flips_is_open() {
        let state = test_state();
        seed_session(&state, "acct-1", "tok-1").await;
        seed_scoped_key(&state, "acct-1", "k1", "vault-a").await;
        seed_blob(&state, "vault-a", "m1", "2026-01-01T00:00:00Z").await;
        let wrapped_k = base64::engine::general_purpose::STANDARD.encode(b"wrapped-k-bytes-1");

        // Locked by default.
        let res = account_vaults(State(state.clone()), bearer("tok-1"))
            .await
            .expect("session");
        assert_eq!(res.0["vaults"][0]["is_open"], false);

        // Open: PUT the envelope.
        let res = put_vault_wrap(
            State(state.clone()),
            bearer("tok-1"),
            Path("vault-a".into()),
            Json(json!({"wrapped_k": wrapped_k})),
        )
        .await
        .expect("unscoped key");
        assert_eq!(res.0["open"], true);
        assert_eq!(res.0["generation"], 1);

        // is_open flips.
        let res = account_vaults(State(state.clone()), bearer("tok-1"))
            .await
            .expect("session");
        assert_eq!(res.0["vaults"][0]["is_open"], true);

        // Lock: DELETE (account has no password → gate passes). Idempotent.
        let res = delete_vault_wrap(
            State(state.clone()),
            bearer("tok-1"),
            Path("vault-a".into()),
            Json(json!({})),
        )
        .await
        .expect("no password → gate passes");
        assert_eq!(res.0["open"], false);
        let res = delete_vault_wrap(
            State(state.clone()),
            bearer("tok-1"),
            Path("vault-a".into()),
            Json(json!({})),
        )
        .await
        .expect("idempotent");
        assert_eq!(res.0["open"], false);
        let res = account_vaults(State(state.clone()), bearer("tok-1"))
            .await
            .expect("session");
        assert_eq!(res.0["vaults"][0]["is_open"], false);
        assert_eq!(audit_count(&state, "acct-1", "vault_wrap_put").await, 1);
        assert_eq!(audit_count(&state, "acct-1", "vault_wrap_delete").await, 2);
    }

    #[tokio::test]
    async fn vault_wrap_respects_scoped_keys() {
        let state = test_state();
        seed_session(&state, "acct-1", "tok-1").await;
        {
            let conn = state.conn.lock().await;
            conn.execute(
                "INSERT INTO api_keys (id, account_id, key_hash, key_prefix, rate, vault_id, created_at, revoked) \
                 VALUES ('k1', 'acct-1', ?1, 'en_', 100, 'vault-a', 'now', 0)",
                rusqlite::params![auth::hash_key("en_scoped").to_vec()],
            )
            .unwrap();
        }
        let wrapped_k = base64::engine::general_purpose::STANDARD.encode(b"wrapped-k-bytes-1");
        let err = put_vault_wrap(
            State(state.clone()),
            bearer("tok-1"),
            Path("vault-b".into()),
            Json(json!({"wrapped_k": wrapped_k})),
        )
        .await
        .expect_err("vault-b out of scope");
        assert_eq!(err.0, StatusCode::FORBIDDEN);
        let err = delete_vault_wrap(
            State(state.clone()),
            bearer("tok-1"),
            Path("vault-b".into()),
            Json(json!({})),
        )
        .await
        .expect_err("vault-b out of scope");
        assert_eq!(err.0, StatusCode::FORBIDDEN);
        // Nothing was written for vault-b.
        let conn = state.conn.lock().await;
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM vault_key_wraps WHERE vault_id = 'vault-b'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0);
    }

    #[tokio::test]
    async fn delete_vault_wrap_requires_fresh_password_for_password_accounts() {
        let state = test_state();
        seed_session(&state, "acct-1", "tok-1").await;
        seed_scoped_key(&state, "acct-1", "k1", "vault-a").await;
        insert_password_account(&state, "acct-1", "correct-horse-battery-staple").await;
        let wrapped_k = base64::engine::general_purpose::STANDARD.encode(b"wrapped-k-bytes-1");

        // Opening is a credential mutation too: PUT needs the fresh password.
        let err = put_vault_wrap(
            State(state.clone()),
            bearer("tok-1"),
            Path("vault-a".into()),
            Json(json!({"wrapped_k": wrapped_k})),
        )
        .await
        .expect_err("open gate: password required");
        assert_eq!(err.0, StatusCode::UNAUTHORIZED);
        assert_eq!(err.1["code"], "password_required");
        let err = put_vault_wrap(
            State(state.clone()),
            bearer("tok-1"),
            Path("vault-a".into()),
            Json(json!({"wrapped_k": wrapped_k, "password": "wrong-password-1"})),
        )
        .await
        .expect_err("open gate: wrong password");
        assert_eq!(err.0, StatusCode::UNAUTHORIZED);
        assert_eq!(err.1["code"], "invalid_password");
        let _ = put_vault_wrap(
            State(state.clone()),
            bearer("tok-1"),
            Path("vault-a".into()),
            Json(json!({"wrapped_k": wrapped_k, "password": "correct-horse-battery-staple"})),
        )
        .await
        .expect("open with fresh password");
        assert_eq!(audit_count(&state, "acct-1", "vault_wrap_put").await, 1);

        let err = delete_vault_wrap(
            State(state.clone()),
            bearer("tok-1"),
            Path("vault-a".into()),
            Json(json!({})),
        )
        .await
        .expect_err("gate: password required");
        assert_eq!(err.0, StatusCode::UNAUTHORIZED);
        assert_eq!(err.1["code"], "password_required");
        let err = delete_vault_wrap(
            State(state.clone()),
            bearer("tok-1"),
            Path("vault-a".into()),
            Json(json!({"password": "wrong-password-1"})),
        )
        .await
        .expect_err("gate: wrong password");
        assert_eq!(err.0, StatusCode::UNAUTHORIZED);
        assert_eq!(err.1["code"], "invalid_password");
        // Envelope still present after failed locks.
        {
            let conn = state.conn.lock().await;
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM vault_key_wraps WHERE vault_id = 'vault-a'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1);
        }
        let res = delete_vault_wrap(
            State(state.clone()),
            bearer("tok-1"),
            Path("vault-a".into()),
            Json(json!({"password": "correct-horse-battery-staple"})),
        )
        .await
        .expect("fresh password");
        assert_eq!(res.0["open"], false);
        assert_eq!(audit_count(&state, "acct-1", "vault_wrap_delete").await, 1);
    }

    #[tokio::test]
    async fn open_vaults_are_per_account_not_per_vault() {
        let state = test_state();
        seed_session(&state, "acct-1", "tok-1").await;
        seed_session(&state, "acct-2", "tok-2").await;
        seed_scoped_key(&state, "acct-1", "k1", "vault-shared").await;
        seed_scoped_key(&state, "acct-2", "k2", "vault-shared").await;
        seed_blob(&state, "vault-shared", "m1", "2026-01-01T00:00:00Z").await;

        let wrapped_k = base64::engine::general_purpose::STANDARD.encode(b"wrapped-k-bytes-1");
        put_vault_wrap(
            State(state.clone()),
            bearer("tok-1"),
            Path("vault-shared".into()),
            Json(json!({"wrapped_k": wrapped_k})),
        )
        .await
        .expect("acct-1 opens shared vault");

        // Envelopes are per-account: acct-2's wrap row does not exist even
        // though the vault is shared (this was the shared-vault clobber bug).
        let res = account_wraps(State(state.clone()), bearer("tok-1"))
            .await
            .expect("session");
        assert_eq!(res.0["open_vaults"], json!(["vault-shared"]));
        let res = account_wraps(State(state.clone()), bearer("tok-2"))
            .await
            .expect("session");
        assert_eq!(res.0["open_vaults"].as_array().unwrap().len(), 0);

        // is_open is per-account too.
        let res = account_vaults(State(state.clone()), bearer("tok-1"))
            .await
            .expect("session");
        assert_eq!(res.0["vaults"][0]["is_open"], true);
        let res = account_vaults(State(state.clone()), bearer("tok-2"))
            .await
            .expect("session");
        assert_eq!(res.0["vaults"][0]["is_open"], false);
    }

    #[tokio::test]
    async fn register_start_requires_fresh_password_to_attach() {
        let state = test_state();
        seed_session(&state, "acct-1", "tok-1").await;
        insert_password_account(&state, "acct-1", "correct-horse-battery-staple").await;
        // With a session, this ceremony ATTACHES a passkey — a credential
        // mutation, so the password gate runs before any WebAuthn work.
        let err = register_start(
            State(state.clone()),
            bearer("tok-1"),
            Json(json!({"origin": "http://localhost:8787"})),
        )
        .await
        .expect_err("attach needs password");
        assert_eq!(err.0, StatusCode::UNAUTHORIZED);
        assert_eq!(err.1["code"], "password_required");
        // With the password the ceremony proceeds (challenge issued).
        let res = register_start(
            State(state),
            bearer("tok-1"),
            Json(json!({"origin": "http://localhost:8787", "password": "correct-horse-battery-staple"})),
        )
        .await
        .expect("password verified, challenge issued");
        assert!(res.0["challenge_id"].as_str().unwrap().len() > 0);
    }

    #[tokio::test]
    async fn register_start_is_rate_limited() {
        // Challenge minting is cheap; floods would pin the auth_store. The
        // per-account bucket allows a burst of 10 starts per 300s.
        let state = test_state();
        seed_session(&state, "acct-1", "tok-1").await;
        for _ in 0..10 {
            let _ = register_start(
                State(state.clone()),
                bearer("tok-1"),
                Json(json!({"origin": "http://localhost:8787"})),
            )
            .await
            .expect("within burst");
        }
        let err = register_start(
            State(state),
            bearer("tok-1"),
            Json(json!({"origin": "http://localhost:8787"})),
        )
        .await
        .expect_err("burst exhausted");
        assert_eq!(err.0, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(err.1["code"], "rate_limited");
    }

    #[tokio::test]
    async fn login_passkey_rows_filters_and_caps() {
        let state = test_state();
        seed_session(&state, "acct-1", "tok-1").await;
        seed_session(&state, "acct-2", "tok-2").await;
        {
            let conn = state.conn.lock().await;
            for i in 0..25u8 {
                conn.execute(
                    "INSERT INTO passkeys (account_id, credential_id, public_key, created_at) \
                     VALUES ('acct-1', ?1, ?2, 'now')",
                    rusqlite::params![format!("cred-{i}").into_bytes(), vec![i]],
                )
                .unwrap();
            }
            for i in 0..3u8 {
                conn.execute(
                    "INSERT INTO passkeys (account_id, credential_id, public_key, created_at) \
                     VALUES ('acct-2', ?1, ?2, 'now')",
                    rusqlite::params![format!("cred2-{i}").into_bytes(), vec![200 + i]],
                )
                .unwrap();
            }
            // Filtered to the requested account and capped at 20 (oldest by
            // rowid) — the browser picker and the verifier both stay bounded.
            let rows = login_passkey_rows(&conn, Some("acct-1")).unwrap();
            assert_eq!(rows.len(), 20, "cap: 25 rows → 20");
            assert!(rows.iter().all(|pk| pk.len() == 1 && pk[0] < 100), "only acct-1 rows");
            // Unfiltered: the cap applies per account, not globally.
            let rows = login_passkey_rows(&conn, None).unwrap();
            assert_eq!(rows.len(), 23, "per-account cap: 20 (acct-1) + 3 (acct-2)");
        }
    }
}
