//! Email+password account auth — signup, signin, password reset.
//!
//! The relay stores ONLY an Argon2id login hash (own salt + params — never
//! derivable into the client-side key-wrap keys, which use different salts
//! and parameters derived in the browser). The account key envelopes live in
//! `account_key_wraps`/`recovery_key_wraps` and are wrapped client-side; the
//! relay cannot open them (zero-knowledge preserved — see account_routes.rs
//! wrap routes).
//!
//! Rate-limit buckets are keyed by sha256(email) hashed again to base64url
//! (auth::hash_b64) so plaintext addresses never sit in limiter state.
//! Anti-enumeration: unknown email and wrong password return the SAME 401,
//! and the unknown-email path still runs a dummy Argon2id verify so timing
//! does not reveal account existence.

use crate::auth::{self, hash_b64, SESSION_TTL};
use crate::SyncState;
use axum::{
    extract::State,
    http::StatusCode,
    routing::post,
    Json, Router,
};
use rand::TryRngCore;
use rusqlite::OptionalExtension;
use serde_json::{json, Value};

type ApiError = (StatusCode, Json<Value>);

// Server-side hash params (Argon2id, OWASP minimum). Deliberately DIFFERENT
// from the client wrap derivation (m=64MiB, t=3, p=4, hash-wasm, per-account
// random salt) so a leaked password_hash yields no wrap key material.
const ARGON_M_COST: u32 = 19_456; // KiB
const ARGON_T_COST: u32 = 2;
const ARGON_P_COST: u32 = 1;
pub(crate) const PASSWORD_MIN_CHARS: usize = 12;
pub(crate) const PASSWORD_MAX_CHARS: usize = 128;
const RESET_TOKEN_TTL_SECS: i64 = 30 * 60;

// Verified against when the email is unknown, to keep signin timing flat.
const DUMMY_SALT: &[u8; 16] = b"engram-dummy-000";
const DUMMY_HASH: &[u8; 32] = &[0x5a; 32];

pub fn router() -> Router<SyncState> {
    Router::new()
        .route("/auth/signup", post(signup))
        .route("/auth/signin", post(signin))
        .route("/auth/reset/request", post(reset_request))
        .route("/auth/reset/confirm", post(reset_confirm))
}

// ── Signup ───────────────────────────────────────────────────────────────────

async fn signup(
    State(state): State<SyncState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let email = normalize_email(str_field(&body, "email")?)?;
    let password = str_field(&body, "password")?;
    if password.chars().count() < PASSWORD_MIN_CHARS || password.len() > PASSWORD_MAX_CHARS {
        return Err(err_json(
            400,
            "weak_password",
            "password must be 12-128 characters",
        ));
    }
    check_bucket(
        &state,
        format!("acct-signup:{}", hash_b64(&auth::hash_bytes(email.as_bytes()))),
        3.0 / 3600.0,
        3.0,
    )
    .await?;

    let account_id = uuid::Uuid::new_v4().to_string();
    let (salt, hash) = hash_password(password)?;
    let now = chrono::Utc::now().to_rfc3339();
    let token = {
        let conn = state.conn.lock().await;
        conn.execute(
            "INSERT INTO accounts (id, created_at) VALUES (?1, ?2)",
            rusqlite::params![account_id, now],
        )
        .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
        match conn.execute(
            "INSERT INTO account_credentials \
             (account_id, email, password_hash, password_salt, email_verified, updated_at) \
             VALUES (?1, ?2, ?3, ?4, 0, ?5)",
            rusqlite::params![account_id, email, hash, salt, now],
        ) {
            Ok(_) => {}
            Err(e) if is_unique_violation(&e) => {
                // Email raced another signup — roll back the account row.
                let _ = conn.execute(
                    "DELETE FROM accounts WHERE id = ?1",
                    rusqlite::params![account_id],
                );
                return Err(err_json(409, "email_taken", "an account with this email already exists"));
            }
            Err(e) => return Err(err_json(500, "database_error", &e.to_string())),
        }
        let token = mint_and_store_session(&conn, &account_id)?;
        crate::account_routes::audit_event(&conn, &account_id, "signup", Some(email.as_str()));
        token
    };
    tracing::info!(account_id, "account created via email signup");
    Ok(Json(json!({ "account_id": account_id, "session_token": token })))
}

// ── Signin ───────────────────────────────────────────────────────────────────

async fn signin(
    State(state): State<SyncState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let email = normalize_email(str_field(&body, "email")?)?;
    let password = str_field(&body, "password")?;
    check_bucket(
        &state,
        format!("acct-signin:{}", hash_b64(&auth::hash_bytes(email.as_bytes()))),
        10.0 / 300.0,
        10.0,
    )
    .await?;

    let (account_id, token) = {
        let conn = state.conn.lock().await;
        let row: Option<(String, Vec<u8>, Vec<u8>)> = conn
            .query_row(
                "SELECT account_id, password_hash, password_salt FROM account_credentials \
                 WHERE email = ?1",
                rusqlite::params![email],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
        let (account_id, stored_hash, salt) = match row {
            Some(row) => row,
            None => {
                // Dummy verify keeps timing flat for unknown emails.
                verify_password(password, DUMMY_SALT, DUMMY_HASH)?;
                return Err(err_json(401, "invalid_credentials", "invalid email or password"));
            }
        };
        if !verify_password(password, &salt, &stored_hash)? {
            return Err(err_json(401, "invalid_credentials", "invalid email or password"));
        }
        conn.execute(
            "UPDATE accounts SET last_login_at = ?1 WHERE id = ?2",
            rusqlite::params![chrono::Utc::now().to_rfc3339(), account_id],
        )
        .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
        let token = mint_and_store_session(&conn, &account_id)?;
        crate::account_routes::audit_event(&conn, &account_id, "signin", Some(email.as_str()));
        (account_id, token)
    };
    Ok(Json(json!({ "account_id": account_id, "session_token": token })))
}

// ── Password reset ───────────────────────────────────────────────────────────

/// Always 200: whether an account exists must not be disclosed. Mints a
/// single-use 30-minute token; delivery is email (when SMTP is configured,
/// `sent: true`) or the operator CLI (`engramd-sync admin reset-token`,
/// `sent: false`).
async fn reset_request(
    State(state): State<SyncState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let email = normalize_email(str_field(&body, "email")?)?;
    check_bucket(
        &state,
        format!("acct-reset:{}", hash_b64(&auth::hash_bytes(email.as_bytes()))),
        3.0 / 3600.0,
        3.0,
    )
    .await?;

    // DB work first (guard held), then SMTP outside the lock — the guard is
    // !Send and an await under it would make the future non-Send. `sent` is
    // optimistic (Kimi review #5): a configured relay with a known account
    // reports true immediately, so known-email and unknown-email responses
    // take the same shape and time. Delivery failures land in the logs —
    // the operator CLI fallback covers them.
    let (account_id, token, sent) = {
        let conn = state.conn.lock().await;
        let account_id: Option<String> = conn
            .query_row(
                "SELECT account_id FROM account_credentials WHERE email = ?1",
                rusqlite::params![email],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
        let token = match account_id {
            Some(ref account_id) => {
                let token = auth::mint_session_token()
                    .map_err(|e| err_json(500, "rng_error", &e.to_string()))?;
                let now = chrono::Utc::now();
                let expires = now + chrono::Duration::seconds(RESET_TOKEN_TTL_SECS);
                conn.execute(
                    "INSERT INTO password_reset_tokens \
                     (token_hash, account_id, created_at, expires_at) \
                     VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![
                        auth::hash_token(&token).to_vec(),
                        account_id,
                        now.to_rfc3339(),
                        expires.to_rfc3339(),
                    ],
                )
                .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
                Some(token)
            }
            None => None,
        };
        let sent = account_id.is_some() && state.smtp.is_some();
        if let Some(ref account_id) = account_id {
            crate::account_routes::audit_event(&conn, account_id, "reset_request", None);
        }
        (account_id, token, sent)
    };

    if let (Some(account_id), Some(token), Some(smtp)) = (&account_id, &token, &state.smtp) {
        tracing::info!(account_id, "password reset requested");
        let smtp = smtp.clone();
        let account_id = account_id.clone();
        let recipient = email.clone();
        let link = format!("{}/#/reset/{}", smtp.base_url, token);
        // Fire-and-forget: SMTP latency must not hold the response, and the
        // task cannot touch the DB (the guard is !Send).
        tokio::spawn(async move {
            let result = tokio::task::spawn_blocking(move || {
                crate::send_reset_email(&smtp, &recipient, &link)
            })
            .await;
            match result {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    tracing::warn!(account_id, error = %e, "reset email failed (operator fallback)");
                }
                Err(e) => {
                    tracing::warn!(account_id, error = %e, "reset email task panicked (operator fallback)");
                }
            }
        });
    }
    Ok(Json(json!({ "sent": sent })))
}

async fn reset_confirm(
    State(state): State<SyncState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let token = str_field(&body, "token")?;
    let new_password = str_field(&body, "new_password")?;
    if new_password.chars().count() < PASSWORD_MIN_CHARS || new_password.len() > PASSWORD_MAX_CHARS {
        return Err(err_json(
            400,
            "weak_password",
            "password must be 12-128 characters",
        ));
    }
    check_bucket(&state, format!("acct-reset-confirm:{}", token), 5.0 / 300.0, 5.0)
        .await?;

    let (salt, hash) = hash_password(new_password)?;
    let token_hash = auth::hash_token(token).to_vec();
    let conn = state.conn.lock().await;
    // Single-use, race-safe: the conditional UPDATE claims the token.
    let claimed = conn
        .execute(
            "UPDATE password_reset_tokens SET used = 1 \
             WHERE token_hash = ?1 AND used = 0 AND expires_at > ?2",
            rusqlite::params![token_hash, chrono::Utc::now().to_rfc3339()],
        )
        .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    let account_id = if claimed > 0 {
        conn.query_row(
            "SELECT account_id FROM password_reset_tokens WHERE token_hash = ?1",
            rusqlite::params![token_hash],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| err_json(500, "database_error", &e.to_string()))?
    } else {
        None
    };
    let account_id = match account_id {
        Some(a) => a,
        None => return Err(err_json(401, "invalid_token", "reset token is invalid or expired")),
    };
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE account_credentials SET password_hash = ?1, password_salt = ?2, updated_at = ?3 \
         WHERE account_id = ?4",
        rusqlite::params![hash, salt, now, account_id],
    )
    .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    // A reset means the password may be compromised: drop every session.
    let revoked = conn
        .execute("DELETE FROM sessions WHERE account_id = ?1", rusqlite::params![account_id])
        .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    crate::account_routes::audit_event(&conn, &account_id, "password_reset", None);
    tracing::info!(account_id, revoked_sessions = revoked, "password reset confirmed");
    Ok(Json(json!({ "reset": true })))
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// The email is the login identity: trimmed and lowercased. Validation is a
/// loose shape check (single @, non-empty local/domain, dotted domain) —
/// deliverability is SMTP's job, not signup's.
fn normalize_email<'a>(raw: &'a str) -> Result<String, ApiError> {
    let email = raw.trim().to_lowercase();
    let valid = email.len() <= 254
        && !email.contains(char::is_whitespace)
        && email.matches('@').count() == 1
        && email
            .split_once('@')
            .map(|(local, domain)| {
                !local.is_empty() && domain.contains('.') && !domain.starts_with('.')
            })
            .unwrap_or(false);
    if valid {
        Ok(email)
    } else {
        Err(err_json(400, "invalid_email", "enter a valid email address"))
    }
}

pub(crate) fn hash_password(password: &str) -> Result<(Vec<u8>, Vec<u8>), ApiError> {
    let mut salt = [0u8; 16];
    rand::rngs::OsRng
        .try_fill_bytes(&mut salt)
        .map_err(|e| err_json(500, "rng_error", &e.to_string()))?;
    let hash = compute_argon2id(password, &salt)?;
    Ok((salt.to_vec(), hash.to_vec()))
}

pub(crate) fn verify_password(password: &str, salt: &[u8], expected: &[u8]) -> Result<bool, ApiError> {
    let computed = compute_argon2id(password, salt)?;
    Ok(constant_time_eq(&computed, expected))
}

fn compute_argon2id(password: &str, salt: &[u8]) -> Result<[u8; 32], ApiError> {
    let params = argon2::Params::new(ARGON_M_COST, ARGON_T_COST, ARGON_P_COST, Some(32))
        .map_err(|e| err_json(500, "hash_error", &e.to_string()))?;
    let argon = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut out = [0u8; 32];
    argon
        .hash_password_into(password.as_bytes(), salt, &mut out)
        .map_err(|e| err_json(500, "hash_error", &e.to_string()))?;
    Ok(out)
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut acc = 0u8;
    for (x, y) in a.iter().zip(b) {
        acc |= x ^ y;
    }
    acc == 0
}

/// Fresh-password gate for credential mutations (Kimi review #4): a 7-day
/// session alone is not proof when mutating credentials — an email-hijacker
/// who reset the password holds a valid session but never knew the
/// password. When the account HAS a password, the caller must verify it in
/// THIS request. Passkey-only accounts have nothing to verify: their
/// sessions came from passkey ceremonies, and they have no email to hijack
/// a reset through.
pub(crate) fn require_fresh_password(
    conn: &rusqlite::Connection,
    account_id: &str,
    password: Option<&str>,
) -> Result<(), ApiError> {
    let stored: Option<(Vec<u8>, Vec<u8>)> = conn
        .query_row(
            "SELECT password_hash, password_salt FROM account_credentials \
             WHERE account_id = ?1",
            rusqlite::params![account_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| err_json(500, "database_error", &e.to_string()))?;
    match (stored, password) {
        (None, _) => Ok(()),
        (Some(_), None) => Err(err_json(
            401,
            "password_required",
            "verify your password to change credentials",
        )),
        (Some((hash, salt)), Some(pw)) if verify_password(pw, &salt, &hash)? => Ok(()),
        (Some(_), Some(_)) => Err(err_json(
            401,
            "invalid_password",
            "current password is incorrect",
        )),
    }
}

async fn check_bucket(
    state: &SyncState,
    key: String,
    rate: f64,
    burst: f64,
) -> Result<(), ApiError> {
    let mut limiters = state.rate_limiters.lock().await;
    let limiter = limiters
        .entry(key)
        .or_insert_with(|| crate::RateLimiter::new(rate, burst));
    if limiter.allow() {
        Ok(())
    } else {
        Err(err_json(429, "rate_limited", "rate limit exceeded"))
    }
}

fn mint_and_store_session(conn: &rusqlite::Connection, account_id: &str) -> Result<String, ApiError> {
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

fn str_field<'a>(body: &'a Value, name: &str) -> Result<&'a str, ApiError> {
    body.get(name)
        .and_then(|v| v.as_str())
        .ok_or_else(|| err_json(400, "missing_field", &format!("missing {name}")))
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

    // Only the tables the password handlers touch (accounts, credentials,
    // reset tokens, sessions) plus auth_events for the audit inserts.
    fn test_state() -> SyncState {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "PRAGMA foreign_keys=ON;
             CREATE TABLE accounts (
                 id TEXT PRIMARY KEY, created_at TEXT NOT NULL, last_login_at TEXT
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
             CREATE TABLE sessions (
                 token_hash BLOB PRIMARY KEY, account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
                 created_at TEXT NOT NULL, expires_at TEXT NOT NULL
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

    async fn call_signup(state: &SyncState, email: &str, password: &str) -> (StatusCode, Value) {
        match signup(
            State(state.clone()),
            Json(json!({"email": email, "password": password})),
        )
        .await
        {
            Ok(res) => (StatusCode::OK, res.0),
            Err(e) => (e.0, e.1 .0),
        }
    }

    async fn call_signin(state: &SyncState, email: &str, password: &str) -> (StatusCode, Value) {
        match signin(
            State(state.clone()),
            Json(json!({"email": email, "password": password})),
        )
        .await
        {
            Ok(res) => (StatusCode::OK, res.0),
            Err(e) => (e.0, e.1 .0),
        }
    }

    async fn call_reset_request(state: &SyncState, email: &str) -> (StatusCode, Value) {
        match reset_request(State(state.clone()), Json(json!({"email": email}))).await {
            Ok(res) => (StatusCode::OK, res.0),
            Err(e) => (e.0, e.1 .0),
        }
    }

    async fn call_reset_confirm(
        state: &SyncState,
        token: &str,
        new_password: &str,
    ) -> (StatusCode, Value) {
        match reset_confirm(
            State(state.clone()),
            Json(json!({"token": token, "new_password": new_password})),
        )
        .await
        {
            Ok(res) => (StatusCode::OK, res.0),
            Err(e) => (e.0, e.1 .0),
        }
    }

    async fn insert_reset_token(state: &SyncState, account_id: &str, token: &str, expires_at: &str) {
        let conn = state.conn.lock().await;
        conn.execute(
            "INSERT INTO password_reset_tokens (token_hash, account_id, created_at, expires_at, used) \
             VALUES (?1, ?2, 'now', ?3, 0)",
            rusqlite::params![auth::hash_token(token).to_vec(), account_id, expires_at],
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

    // ── Signup ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn signup_creates_account_and_mints_working_session() {
        let state = test_state();
        let (status, body) =
            call_signup(&state, "  New.User@Example.COM ", "correct-horse-battery-staple").await;
        assert_eq!(status, StatusCode::OK);
        let account_id = body["account_id"].as_str().unwrap();
        let token = body["session_token"].as_str().unwrap();
        assert!(!token.is_empty());

        let conn = state.conn.lock().await;
        let (email, stored): (String, Vec<u8>) = conn
            .query_row(
                "SELECT email, password_hash FROM account_credentials WHERE account_id = ?1",
                rusqlite::params![account_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(email, "new.user@example.com", "email normalized");
        assert_ne!(stored, b"correct-horse-battery-staple".to_vec(), "never plaintext");
        // The minted session authenticates through the same hash path.
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sessions WHERE token_hash = ?1",
                rusqlite::params![auth::hash_token(token).to_vec()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
        drop(conn);
        assert_eq!(audit_count(&state, account_id, "signup").await, 1);
    }

    #[tokio::test]
    async fn signup_rejects_weak_passwords_and_bad_emails() {
        let state = test_state();
        let (status, body) = call_signup(&state, "a@b.co", "short").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["code"], "weak_password");
        let (status, body) = call_signup(&state, "not-an-email", "long-enough-password").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["code"], "invalid_email");
        let (status, _) = call_signup(&state, "a@b.co", &"x".repeat(129)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        // Nothing was created.
        let conn = state.conn.lock().await;
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM accounts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
    }

    #[tokio::test]
    async fn signup_duplicate_email_rolls_back_account_row() {
        let state = test_state();
        let (status, _) =
            call_signup(&state, "dup@example.com", "correct-horse-battery-staple").await;
        assert_eq!(status, StatusCode::OK);
        let (status, body) =
            call_signup(&state, "DUP@example.com ", "another-valid-password").await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["code"], "email_taken");
        let conn = state.conn.lock().await;
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM accounts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1, "failed signup must not leave an orphan account row");
    }

    #[tokio::test]
    async fn signup_rate_limits_per_email_bucket() {
        let state = test_state();
        let (status, _) =
            call_signup(&state, "rl@example.com", "correct-horse-battery-staple").await;
        assert_eq!(status, StatusCode::OK);
        // Burst of 3: the next two pass the limiter and hit the duplicate path.
        let (status, _) =
            call_signup(&state, "rl@example.com", "correct-horse-battery-staple").await;
        assert_eq!(status, StatusCode::CONFLICT);
        let (status, _) =
            call_signup(&state, "rl@example.com", "correct-horse-battery-staple").await;
        assert_eq!(status, StatusCode::CONFLICT);
        let (status, body) =
            call_signup(&state, "rl@example.com", "correct-horse-battery-staple").await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(body["code"], "rate_limited");
    }

    // ── Signin ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn signin_roundtrip_sets_last_login_and_audits() {
        let state = test_state();
        let (_, body) =
            call_signup(&state, "s@example.com", "correct-horse-battery-staple").await;
        let account_id = body["account_id"].as_str().unwrap().to_string();
        let (status, body) = call_signin(&state, "S@example.com", "correct-horse-battery-staple").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["account_id"], account_id);
        assert!(body["session_token"].as_str().unwrap().len() > 0);
        let conn = state.conn.lock().await;
        let last: Option<String> = conn
            .query_row(
                "SELECT last_login_at FROM accounts WHERE id = ?1",
                rusqlite::params![account_id],
                |r| r.get(0),
            )
            .unwrap();
        assert!(last.is_some(), "signin stamps last_login_at");
        drop(conn);
        assert_eq!(audit_count(&state, &account_id, "signin").await, 1);
    }

    #[tokio::test]
    async fn signin_returns_identical_401_for_unknown_email_and_wrong_password() {
        let state = test_state();
        let (_, body) =
            call_signup(&state, "known@example.com", "correct-horse-battery-staple").await;
        let account_id = body["account_id"].as_str().unwrap().to_string();
        let (status_a, body_a) = call_signin(&state, "unknown@example.com", "whatever-password").await;
        let (status_b, body_b) = call_signin(&state, "known@example.com", "wrong-password-1").await;
        assert_eq!(
            (status_a, status_b),
            (StatusCode::UNAUTHORIZED, StatusCode::UNAUTHORIZED)
        );
        assert_eq!(body_a, body_b, "unknown email and wrong password must be indistinguishable");
        assert_eq!(body_a["code"], "invalid_credentials");
        // Failed attempts mint nothing and audit nothing.
        let conn = state.conn.lock().await;
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1, "only the signup session");
        drop(conn);
        assert_eq!(audit_count(&state, &account_id, "signin").await, 0);
    }

    #[tokio::test]
    async fn signin_rate_limits_after_burst() {
        let state = test_state();
        for _ in 0..10 {
            let (status, _) = call_signin(&state, "gone@example.com", "whatever-password").await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
        }
        let (status, body) = call_signin(&state, "gone@example.com", "whatever-password").await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(body["code"], "rate_limited");
    }

    // ── Password reset ──────────────────────────────────────────────────

    #[tokio::test]
    async fn reset_request_is_non_disclosing_and_hashes_tokens_at_rest() {
        let state = test_state();
        let (_, body) =
            call_signup(&state, "r@example.com", "correct-horse-battery-staple").await;
        let account_id = body["account_id"].as_str().unwrap().to_string();
        let (status_a, body_a) = call_reset_request(&state, "r@example.com").await;
        let (status_b, body_b) = call_reset_request(&state, "nobody@example.com").await;
        assert_eq!((status_a, status_b), (StatusCode::OK, StatusCode::OK));
        assert_eq!(body_a, body_b, "response must not disclose account existence");
        assert_eq!(body_a["sent"], false, "no SMTP configured → operator fallback");
        // The known email minted exactly one hashed token row; the plaintext
        // is returned nowhere (operator CLI mints its own token).
        let conn = state.conn.lock().await;
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM password_reset_tokens", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
        let (hash, used): (Vec<u8>, i64) = conn
            .query_row(
                "SELECT token_hash, used FROM password_reset_tokens",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(hash.len(), 32, "sha256 at rest");
        assert_eq!(used, 0);
        drop(conn);
        assert_eq!(audit_count(&state, &account_id, "reset_request").await, 1);
    }

    #[tokio::test]
    async fn reset_request_reports_sent_optimistically_with_smtp() {
        let mut state = test_state();
        let (_, _) =
            call_signup(&state, "r@example.com", "correct-horse-battery-staple").await;
        state.smtp = Some(Arc::new(crate::SmtpConfig {
            host: "127.0.0.1".into(),
            port: 1, // unreachable on purpose: a delivery failure must not fail the request
            username: None,
            password: None,
            from: "engram@example.com".into(),
            base_url: "https://vault.example".into(),
        }));
        let (status, body) = call_reset_request(&state, "r@example.com").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["sent"], true, "known account + configured SMTP reports sent immediately");
        let (status, body) = call_reset_request(&state, "nobody@example.com").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["sent"], false, "unknown account never reports sent");
    }

    #[tokio::test]
    async fn reset_confirm_rotates_password_and_revokes_all_sessions() {
        let state = test_state();
        let (_, body) =
            call_signup(&state, "r@example.com", "correct-horse-battery-staple").await;
        let account_id = body["account_id"].as_str().unwrap().to_string();
        // A second sign-in session that the reset must kill.
        let (status, _) = call_signin(&state, "r@example.com", "correct-horse-battery-staple").await;
        assert_eq!(status, StatusCode::OK);
        insert_reset_token(&state, &account_id, "tok-plain-1", "2999-01-01T00:00:00Z").await;

        let (status, body) = call_reset_confirm(&state, "tok-plain-1", "new-valid-password-2").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["reset"], true);

        // Old password dead, new one works.
        let (status, _) = call_signin(&state, "r@example.com", "correct-horse-battery-staple").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _) = call_signin(&state, "r@example.com", "new-valid-password-2").await;
        assert_eq!(status, StatusCode::OK);

        // Every pre-reset session is gone; the token is single-use.
        let conn = state.conn.lock().await;
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sessions WHERE account_id = ?1",
                rusqlite::params![account_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "only the post-reset signin session survives");
        let used: i64 = conn
            .query_row("SELECT used FROM password_reset_tokens", [], |r| r.get(0))
            .unwrap();
        assert_eq!(used, 1);
        drop(conn);
        assert_eq!(audit_count(&state, &account_id, "password_reset").await, 1);
    }

    #[tokio::test]
    async fn reset_confirm_rejects_invalid_expired_and_reused_tokens() {
        let state = test_state();
        let (_, body) =
            call_signup(&state, "r@example.com", "correct-horse-battery-staple").await;
        let account_id = body["account_id"].as_str().unwrap().to_string();
        insert_reset_token(&state, &account_id, "tok-plain-1", "2999-01-01T00:00:00Z").await;
        insert_reset_token(&state, &account_id, "tok-expired", "2000-01-01T00:00:00Z").await;

        // Unknown token.
        let (status, body) =
            call_reset_confirm(&state, "tok-never-minted", "new-valid-password-2").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["code"], "invalid_token");
        // Expired token.
        let (status, _) = call_reset_confirm(&state, "tok-expired", "new-valid-password-2").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        // Weak new password fails BEFORE the token is consumed.
        let (status, body) = call_reset_confirm(&state, "tok-plain-1", "short").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["code"], "weak_password");
        // First real use succeeds; reuse is rejected.
        let (status, _) = call_reset_confirm(&state, "tok-plain-1", "new-valid-password-2").await;
        assert_eq!(status, StatusCode::OK);
        let (status, body) =
            call_reset_confirm(&state, "tok-plain-1", "another-valid-password-3").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["code"], "invalid_token");
    }
}
