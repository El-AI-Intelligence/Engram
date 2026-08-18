//! Engram Sync Server — dumb pipe for E2E encrypted memory blobs.
//!
//! Design principles:
//!   - Server NEVER sees plaintext — all encryption happens client-side
//!   - Accounts: email+password primary, passkeys optional. Email is the
//!     only PII; passwords stay as a server-side Argon2id login hash whose
//!     params/salt differ from the client key-wrap derivation (a leaked
//!     hash yields no wrap key material). Key envelopes in the wrap tables
//!     are client-produced AES-GCM ciphertext the relay cannot open.
//!   - Single responsibility: accept blobs, serve blobs, verify HMAC
//!   - Billing is handled by a separate service (Stripe webhooks)
//!
//! API:
//!   GET    /health                                       — server health
//!   POST   /v1/vaults/{vault_id}/push                    — push encrypted blobs
//!   GET    /v1/vaults/{vault_id}/pull?since=&limit=      — pull changes
//!   GET    /v1/vaults/{vault_id}/stats                   — vault statistics
//!   GET    /v1/vaults/{vault_id}/devices                 — device roster
//!   DELETE /v1/vaults/{vault_id}/devices/{device_id}     — revoke a device
//!   POST   /auth/signup|signin                           — email+password auth
//!   POST   /auth/reset/request|confirm                   — password reset
//!   POST   /auth/register/start|finish                   — create account (passkey)
//!   POST   /auth/login/start|finish                      — sign in (passkey)
//!   POST   /auth/logout                                  — end session
//!   GET    /account, POST/DELETE /account/keys           — account + API keys
//!   GET    /account/credentials, POST /account/password  — email + password
//!   GET    /account/passkeys, DELETE /account/passkeys/{id}
//!   GET    /account/wraps, PUT /account/wraps/{password,recovery}
//!   PUT|DELETE /account/vaults/{vault_id}/wrap           — vault open/locked
//!
//! Auth (two tiers):
//!   1. Static keys from SYNC_API_KEYS env — the original operator keys.
//!   2. Account keys minted via /account/keys (stored as sha256 hashes).
//!   Clients send Authorization: Bearer <key>. Auth is optional on
//!   loopback only while no account keys exist; once the first account
//!   key is minted, keyless loopback requests are rejected like any
//!   other (matches Guardrail's default-secure pattern).
//!
//!   Static key format (comma-separated entries):
//!     key                        — all vaults, rate limit 100 req/s
//!     key:rate                   — all vaults, custom rate limit
//!     key:rate:vault1;vault2     — scoped to the listed vaults only
//!     key:rate:vault1+admin      — scoped, and administers vault1
//!   Unscoped keys keep the original all-vaults behavior (and are the
//!   superuser: they may revoke devices on any vault). Scoped keys can
//!   only touch their own vaults; +admin also lets a key revoke devices
//!   on that vault — the substrate a hosted control plane mints
//!   per-member keys against.

mod account_routes;
mod auth;
mod link_crypto;
mod password_routes;
mod quota;
mod routes;

use crate::auth::{build_webauthn, WebauthnStore, Webauthn};
use clap::{Parser, Subcommand};
use rusqlite::OptionalExtension;
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing::info;

/// Max ciphertext size per blob (1 MB). Rejects larger blobs to prevent
/// storage exhaustion attacks.
const MAX_BLOB_SIZE: usize = 1_048_576;

/// Max blobs per push request. Batches larger than this are rejected.
const MAX_BLOBS_PER_PUSH: usize = 1000;

/// Tombstone retention: blobs marked deleted older than this many days
/// are physically removed on the next cleanup pass.
const TOMBSTONE_RETENTION_DAYS: i64 = 30;

#[derive(Parser, Debug)]
#[command(name = "engramd-sync", version, about = "Engram Sync Server")]
struct Cli {
    /// Path to the sync database directory
    #[arg(short, long, default_value = "./sync-data")]
    data_dir: PathBuf,

    /// Listen address
    #[arg(short, long, default_value = "0.0.0.0:8788")]
    bind: SocketAddr,

    /// WebAuthn Relying Party ID — a registrable domain suffix of the
    /// vault UI origin. Passkeys bind to it: changing it orphans them.
    #[arg(long, default_value = "localhost")]
    rp_id: String,

    /// Allowed browser origins for WebAuthn ceremonies, comma-separated.
    /// The vault SPA sends window.location.origin; each finish validates
    /// against this list.
    #[arg(long, default_value = "http://localhost:8787")]
    origin: String,

    /// Default per-account device quota (0 = unlimited). Per-account
    /// overrides live in the accounts table (roadmap 1.3 billing sets
    /// them).
    #[arg(long, default_value_t = 0)]
    quota_devices: i64,

    /// Default per-account stored-bytes quota (0 = unlimited).
    #[arg(long, default_value_t = 0)]
    quota_bytes: i64,

    /// SMTP host for password-reset emails. When unset, /auth/reset/request
    /// never emails (operator hands out tokens via `admin reset-token`).
    #[arg(long, env = "ENGRAM_SMTP_HOST")]
    smtp_host: Option<String>,

    /// SMTP port (587 STARTTLS is the supported path).
    #[arg(long, env = "ENGRAM_SMTP_PORT", default_value_t = 587)]
    smtp_port: u16,

    /// SMTP username (plain auth; omit for unauthenticated relay).
    #[arg(long, env = "ENGRAM_SMTP_USERNAME")]
    smtp_username: Option<String>,

    /// SMTP password.
    #[arg(long, env = "ENGRAM_SMTP_PASSWORD")]
    smtp_password: Option<String>,

    /// From address for reset emails (required when --smtp-host is set).
    #[arg(long, env = "ENGRAM_SMTP_FROM")]
    smtp_from: Option<String>,

    /// Public URL reset links point at (defaults to the first --origin).
    #[arg(long)]
    smtp_base_url: Option<String>,

    #[command(subcommand)]
    command: Option<AdminCommand>,
}

#[derive(Subcommand, Debug)]
enum AdminCommand {
    /// Mint and print a password-reset token for an account (operator
    /// delivery). The plaintext is shown once on stdout, stored only as a
    /// sha256 hash, single-use, valid 30 minutes.
    ResetToken {
        /// The account's signup email (case-insensitive)
        email: String,
    },
}

/// Per-key rate limiter: token bucket. `rate` = tokens per second of
/// refill; `burst` = bucket capacity (starts full). Rates ≥ 1/s behave
/// exactly as before (burst = rate); slower buckets (e.g. 3/hr) need an
/// explicit burst or the capacity would be below the 1-token request cost
/// and every request would be rejected.
pub struct RateLimiter {
    tokens: f64,
    rate: f64,
    max_tokens: f64,
    last_refill: std::time::Instant,
}

impl RateLimiter {
    pub fn new(rate: f64, burst: f64) -> Self {
        let max_tokens = burst.max(1.0);
        Self {
            tokens: max_tokens,
            rate,
            max_tokens,
            last_refill: std::time::Instant::now(),
        }
    }

    /// Returns true if a request is allowed (consumes 1 token).
    pub fn allow(&mut self) -> bool {
        let now = std::time::Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.rate).min(self.max_tokens);
        self.last_refill = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// One API key entry: rate limit + optional per-vault scoping.
#[derive(Clone, Debug)]
pub struct ApiKeyEntry {
    pub rate: f64,
    /// `None` = unscoped: the key reaches every vault (original behavior,
    /// and the legacy superuser). `Some(vaults)` = restricted to those.
    pub vaults: Option<HashSet<String>>,
    /// Vaults this key administers (can revoke devices). Only meaningful
    /// for scoped keys; unscoped keys are implicitly admin everywhere.
    pub admin_vaults: HashSet<String>,
    /// Owning account for keys minted via /account/keys (None for static
    /// env keys). Quota enforcement keys on this (Phase 4).
    pub account_id: Option<String>,
}

#[derive(Clone)]
pub struct SyncState {
    pub conn: Arc<Mutex<rusqlite::Connection>>,
    pub start_time: chrono::DateTime<chrono::Utc>,
    pub data_dir: PathBuf,
    /// Known API keys: key string → entry (rate limit + scope).
    /// Empty map → auth disabled (loopback mode).
    pub api_keys: Arc<HashMap<String, Arc<ApiKeyEntry>>>,
    /// Rate limiter state per key.
    pub rate_limiters: Arc<Mutex<HashMap<String, RateLimiter>>>,
    /// Whether the server is bound to loopback (affects auth strictness).
    pub is_loopback: bool,
    /// WebAuthn Relying Party ID (see `Cli::rp_id`).
    pub rp_id: String,
    /// Allowed WebAuthn browser origins (see `Cli::origin`).
    pub allowed_origins: Arc<HashSet<String>>,
    /// Default per-account device quota (0 = unlimited).
    pub default_quota_devices: i64,
    /// Default per-account stored-bytes quota (0 = unlimited).
    pub default_quota_bytes: i64,
    /// WebAuthn instance for passkey ceremonies.
    pub webauthn: Arc<Webauthn>,
    /// In-flight ceremony state (registrations + authentications).
    pub auth_store: Arc<WebauthnStore>,
    /// SMTP settings for password-reset emails (None = operator fallback).
    pub smtp: Option<Arc<SmtpConfig>>,
}

/// SMTP delivery config for password-reset emails. Present only when
/// --smtp-host is set; everything else falls back to the operator CLI.
#[derive(Clone, Debug)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
    pub from: String,
    pub base_url: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    // ── Operator subcommand (no server, no API keys needed) ─────────────
    if let Some(AdminCommand::ResetToken { email }) = &cli.command {
        return admin_reset_token(&cli.data_dir, email);
    }

    // ── Load API keys ──────────────────────────────────────────────────
    let api_keys = load_api_keys();
    let is_loopback = cli.bind.ip().is_loopback();

    if api_keys.is_empty() && !is_loopback {
        tracing::error!(
            "SYNC_API_KEYS not set and binding to non-loopback {}. \
             Refusing to start — authentication is required on non-loopback. \
             Set SYNC_API_KEYS or bind to 127.0.0.1.",
            cli.bind
        );
        std::process::exit(1);
    } else if api_keys.is_empty() {
        tracing::info!(
            "No SYNC_API_KEYS set — auth disabled (loopback mode). \
             Only local clients can reach this server."
        );
    } else {
        tracing::info!("Loaded {} API key(s)", api_keys.len());
    }

    // ── WebAuthn (accounts) ────────────────────────────────────────────
    let allowed_origins: HashSet<String> = cli
        .origin
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let primary_origin = allowed_origins.iter().next().cloned().unwrap_or_else(|| {
        tracing::error!("--origin list is empty after parsing {:?}", cli.origin);
        std::process::exit(1);
    });
    let webauthn = match build_webauthn(&cli.rp_id, &primary_origin) {
        Ok(w) => Arc::new(w),
        Err(e) => {
            tracing::error!("{e:#}");
            std::process::exit(1);
        }
    };
    let auth_store = Arc::new(WebauthnStore::new());
    tracing::info!(
        "WebAuthn accounts enabled: rp_id={:?}, origins={:?}",
        cli.rp_id,
        allowed_origins
    );

    // ── Database ───────────────────────────────────────────────────────
    std::fs::create_dir_all(&cli.data_dir)?;
    let db_path = cli.data_dir.join("sync.db");

    let conn = rusqlite::Connection::open(&db_path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;")?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sync_blobs (
            vault_id     TEXT NOT NULL,
            memory_id    TEXT NOT NULL,
            device_id    TEXT NOT NULL,
            vector_clock INTEGER NOT NULL DEFAULT 0,
            ciphertext   TEXT NOT NULL,
            hmac         TEXT NOT NULL,
            created_at   TEXT NOT NULL,
            deleted      INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (vault_id, memory_id)
        );
        CREATE INDEX IF NOT EXISTS idx_sync_vault_created
            ON sync_blobs(vault_id, created_at);
        CREATE INDEX IF NOT EXISTS idx_sync_vault_memory
            ON sync_blobs(vault_id, memory_id);
        CREATE TABLE IF NOT EXISTS revoked_devices (
            vault_id   TEXT NOT NULL,
            device_id  TEXT NOT NULL,
            revoked_at TEXT NOT NULL,
            PRIMARY KEY (vault_id, device_id)
        );
        -- Accounts: email+password primary, passkeys optional (see
        -- account_credentials). quota_* NULL = server default
        -- (--quota-devices / --quota-bytes).
        CREATE TABLE IF NOT EXISTS accounts (
            id            TEXT PRIMARY KEY,
            created_at    TEXT NOT NULL,
            last_login_at TEXT,
            quota_devices INTEGER,
            quota_bytes   INTEGER
        );
        CREATE TABLE IF NOT EXISTS passkeys (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            account_id    TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
            credential_id BLOB NOT NULL UNIQUE,
            public_key    BLOB NOT NULL,
            created_at    TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_passkeys_account ON passkeys(account_id);
        -- Sessions: token_hash is sha256 of the Bearer token; the plaintext
        -- token exists only in the browser's localStorage.
        CREATE TABLE IF NOT EXISTS sessions (
            token_hash BLOB PRIMARY KEY,
            account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
            created_at TEXT NOT NULL,
            expires_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_sessions_account ON sessions(account_id);
        -- Account API keys: key_hash is sha256 of the full key (shown once
        -- at creation). vault_id NULL = key reaches every vault the account
        -- has synced; a value scopes the key to that vault.
        CREATE TABLE IF NOT EXISTS api_keys (
            id         TEXT PRIMARY KEY,
            account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
            key_hash   BLOB NOT NULL UNIQUE,
            key_prefix TEXT NOT NULL,
            rate       REAL NOT NULL DEFAULT 100,
            vault_id   TEXT,
            created_at TEXT NOT NULL,
            revoked    INTEGER NOT NULL DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS idx_api_keys_account ON api_keys(account_id);
        CREATE INDEX IF NOT EXISTS idx_api_keys_hash ON api_keys(key_hash);
        -- Human-readable device labels for the roster. Daemons register
        -- their device.json label at sync start; the label lives here (the
        -- encrypted blob envelope is deliberately untouched).
        CREATE TABLE IF NOT EXISTS device_labels (
            vault_id   TEXT NOT NULL,
            device_id  TEXT NOT NULL,
            label      TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (vault_id, device_id)
        );
        -- One-time device pairing codes (WARP-style onboarding): code_hash
        -- is sha256 of the plaintext (shown once at mint); single-use,
        -- 10-minute TTL enforced at redemption.
        CREATE TABLE IF NOT EXISTS pairing_codes (
            code_hash  BLOB PRIMARY KEY,
            account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
            created_at TEXT NOT NULL,
            expires_at TEXT NOT NULL,
            used       INTEGER NOT NULL DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS idx_pairing_codes_account ON pairing_codes(account_id);
        -- One-click machine linking (`engram link`): the CLI creates an
        -- intent carrying its ephemeral X25519 public key; the signed-in
        -- browser confirms it with the code from the URL. Nothing secret
        -- at rest: code_hash is sha256, the sealed key is undecryptable
        -- without the CLI's private key, and the relay's own keypair is
        -- re-derived from (id, code_hash) on demand (see link_crypto.rs).
        -- Single-shot delivery via the confirmed→delivered transition,
        -- 10-minute TTL.
        CREATE TABLE IF NOT EXISTS link_intents (
            id           TEXT PRIMARY KEY,
            code_hash    BLOB NOT NULL,
            public_key   BLOB NOT NULL,
            account_id   TEXT REFERENCES accounts(id) ON DELETE CASCADE,
            sealed_key   BLOB,
            nonce        BLOB,
            device_label TEXT,
            status       TEXT NOT NULL DEFAULT 'pending',
            created_at   TEXT NOT NULL,
            expires_at   TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_link_intents_expires ON link_intents(expires_at);
        -- Email+password credentials (roadmap: standard accounts). The
        -- password_hash is Argon2id with its OWN salt and params — never
        -- derivable into the client-side wrap keys. Legacy passkey-only
        -- accounts simply have no row here.
        CREATE TABLE IF NOT EXISTS account_credentials (
            account_id      TEXT PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
            email           TEXT NOT NULL UNIQUE,
            password_hash   BLOB NOT NULL,
            password_salt   BLOB NOT NULL,
            email_verified  INTEGER NOT NULL DEFAULT 0,
            updated_at      TEXT NOT NULL
        );
        -- Password-reset tokens: token_hash is sha256 of the plaintext token
        -- (emailed or operator-printed, shown once). Single-use, 30 min TTL.
        CREATE TABLE IF NOT EXISTS password_reset_tokens (
            token_hash BLOB PRIMARY KEY,
            account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
            created_at TEXT NOT NULL,
            expires_at TEXT NOT NULL,
            used       INTEGER NOT NULL DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS idx_reset_tokens_account ON password_reset_tokens(account_id);
        -- Account key envelopes (zero-knowledge): the relay stores ONLY
        -- wrapped keys it cannot open. wrapped_a = AES-GCM(account key A,
        -- client-derived wrap key); the recovery PHRASE itself is never
        -- stored anywhere. kdf records the client's wrap-KDF params
        -- (v1: argon2id 64MiB/3/4 — same as vault unlock) so they can be
        -- strengthened later without losing old wraps; generation bumps on
        -- every rewrap so other clients know to refetch.
        CREATE TABLE IF NOT EXISTS account_key_wraps (
            account_id TEXT PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
            wrapped_a  BLOB NOT NULL,
            salt_pw    BLOB NOT NULL,
            kdf        TEXT NOT NULL DEFAULT 'argon2id-65536-3-4',
            generation INTEGER NOT NULL DEFAULT 1,
            updated_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS recovery_key_wraps (
            account_id    TEXT PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
            wrapped_a_rec BLOB NOT NULL,
            salt_rec      BLOB NOT NULL,
            kdf           TEXT NOT NULL DEFAULT 'argon2id-65536-3-4',
            generation    INTEGER NOT NULL DEFAULT 1,
            created_at    TEXT NOT NULL
        );
        -- Vault key envelopes: kind='account' row present = vault is OPEN by
        -- default for THAT account (unwrappable by its account key A).
        -- Per (account, vault): two accounts sharing a vault each hold
        -- their own envelope. Absent = LOCKED (passphrase-only).
        -- Lock/unlock toggles rows; memories never move. Lock is a
        -- client-side concept — a malicious relay could re-serve deleted
        -- rows, so it is not a relay-enforced boundary (threat model).
        CREATE TABLE IF NOT EXISTS vault_key_wraps (
            account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
            vault_id   TEXT NOT NULL,
            kind       TEXT NOT NULL DEFAULT 'account',
            wrapped_k  BLOB NOT NULL,
            generation INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (account_id, vault_id, kind)
        );
        -- Security-relevant account events (signin, credential + wrap
        -- changes). The only way a user notices activity on a hijacked
        -- session. No PII beyond the account id and event names.
        CREATE TABLE IF NOT EXISTS auth_events (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
            event      TEXT NOT NULL,
            detail     TEXT,
            created_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_auth_events_account ON auth_events(account_id);",
    )?;

    // ── Run tombstone cleanup on startup ───────────────────────────────
    let deleted: u64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sync_blobs WHERE deleted = 1",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let cutoff = chrono::Utc::now() - chrono::Duration::days(TOMBSTONE_RETENTION_DAYS);
    let cleaned: usize = conn.execute(
        "DELETE FROM sync_blobs WHERE deleted = 1 AND created_at < ?1",
        rusqlite::params![cutoff.to_rfc3339()],
    )?;
    if cleaned > 0 {
        tracing::info!(
            "Tombstone cleanup: removed {cleaned} blobs older than {TOMBSTONE_RETENTION_DAYS}d \
             ({deleted} total deleted blobs in DB)"
        );
    }

    // ── Optional SMTP for reset emails ─────────────────────────────────
    let smtp = match &cli.smtp_host {
        Some(host) => {
            let from = cli.smtp_from.clone().unwrap_or_else(|| {
                tracing::error!("--smtp-from is required when --smtp-host is set");
                std::process::exit(1);
            });
            let base_url = cli
                .smtp_base_url
                .clone()
                .unwrap_or_else(|| primary_origin.clone());
            Some(Arc::new(SmtpConfig {
                host: host.clone(),
                port: cli.smtp_port,
                username: cli.smtp_username.clone(),
                password: cli.smtp_password.clone(),
                from,
                base_url,
            }))
        }
        None => None,
    };

    let state = SyncState {
        conn: Arc::new(Mutex::new(conn)),
        start_time: chrono::Utc::now(),
        data_dir: cli.data_dir.clone(),
        api_keys: Arc::new(api_keys),
        rate_limiters: Arc::new(Mutex::new(HashMap::new())),
        is_loopback,
        rp_id: cli.rp_id.clone(),
        allowed_origins: Arc::new(allowed_origins),
        default_quota_devices: cli.quota_devices,
        default_quota_bytes: cli.quota_bytes,
        webauthn,
        auth_store,
        smtp,
    };

    // ── Background tombstone cleanup (daily) ───────────────────────────
    let cleanup_state = state.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(86400));
        loop {
            interval.tick().await;
            let conn = cleanup_state.conn.lock().await;
            let cutoff = chrono::Utc::now() - chrono::Duration::days(TOMBSTONE_RETENTION_DAYS);
            match conn.execute(
                "DELETE FROM sync_blobs WHERE deleted = 1 AND created_at < ?1",
                rusqlite::params![cutoff.to_rfc3339()],
            ) {
                Ok(n) if n > 0 => tracing::info!("Daily tombstone cleanup: removed {n} blobs"),
                Err(e) => tracing::warn!("Tombstone cleanup error: {e}"),
                _ => {}
            }
        }
    });

    // ── Background WAL checkpoint (every 5 min) ───────────────────────
    // In WAL mode all writes land in sync.db-wal first; without a
    // checkpoint the main db file stays a stale shell and the WAL grows
    // unbounded. TRUNCATE folds the WAL into the main db each pass, so a
    // hard crash loses at most ~5 minutes of blobs (WAL replay still
    // covers everything since the last checkpoint).
    let checkpoint_state = state.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(300));
        loop {
            interval.tick().await;
            let conn = checkpoint_state.conn.lock().await;
            match wal_checkpoint(&conn) {
                Ok(0) => {}
                Ok(_) => tracing::warn!("WAL checkpoint busy — truncating next pass"),
                Err(e) => tracing::warn!("WAL checkpoint error: {e}"),
            }
        }
    });

    // Keep a handle for the shutdown checkpoint (the router owns `state`).
    let shutdown_state = state.clone();

    // ── Build router ───────────────────────────────────────────────────
    account_routes::spawn_session_sweeper(state.clone());

    let app = axum::Router::new()
        .merge(routes::router())
        .merge(account_routes::router())
        .merge(password_routes::router())
        .with_state(state)
        .layer(
            CorsLayer::new()
                .allow_methods([
                    axum::http::Method::GET,
                    axum::http::Method::POST,
                    axum::http::Method::PUT,
                    axum::http::Method::DELETE,
                ])
                .allow_headers([
                    axum::http::header::CONTENT_TYPE,
                    axum::http::header::AUTHORIZATION,
                ])
                .allow_origin(tower_http::cors::Any),
        )
        .layer(TraceLayer::new_for_http())
        // Reject oversized bodies to prevent memory-exhaustion DoS
        .layer(axum::extract::DefaultBodyLimit::max(10 * 1024 * 1024));

    info!("Engram Sync Server starting on {}", cli.bind);
    let listener = tokio::net::TcpListener::bind(cli.bind).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    // ── Final WAL checkpoint on shutdown ───────────────────────────────
    let conn = shutdown_state.conn.lock().await;
    match wal_checkpoint(&conn) {
        Ok(0) => tracing::info!("Final WAL checkpoint complete"),
        Ok(_) => tracing::warn!("Final WAL checkpoint busy (readers active)"),
        Err(e) => tracing::warn!("Final WAL checkpoint error: {e}"),
    }
    tracing::info!("Engram Sync Server shut down cleanly");

    Ok(())
}

/// Checkpoint the WAL into the main database and truncate it.
/// Returns the PRAGMA busy column: 0 = complete, 1 = busy (checkpointed
/// as far as possible but not truncated).
fn wal_checkpoint(conn: &rusqlite::Connection) -> rusqlite::Result<i64> {
    conn.query_row("PRAGMA wal_checkpoint(TRUNCATE);", [], |r| r.get(0))
}

/// Operator delivery of password-reset tokens: mint a fresh single-use
/// token for an account (found by signup email), store only its sha256
/// hash, print the plaintext ONCE on stdout. Without SMTP configured this
/// is how the user receives a token — nothing sensitive goes to logs.
fn admin_reset_token(data_dir: &PathBuf, email: &str) -> anyhow::Result<()> {
    let email = email.trim().to_lowercase();
    std::fs::create_dir_all(data_dir)?;
    let conn = rusqlite::Connection::open(data_dir.join("sync.db"))?;
    let account_id: Option<String> = conn
        .query_row(
            "SELECT account_id FROM account_credentials WHERE email = ?1",
            rusqlite::params![email],
            |r| r.get(0),
        )
        .optional()?;
    let account_id = match account_id {
        Some(a) => a,
        None => anyhow::bail!("no account with email {email} in {}", data_dir.display()),
    };
    let token = auth::mint_session_token()?;
    let now = chrono::Utc::now();
    let expires = now + chrono::Duration::minutes(30);
    conn.execute(
        "INSERT INTO password_reset_tokens (token_hash, account_id, created_at, expires_at) \
         VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![
            auth::hash_token(&token).to_vec(),
            account_id,
            now.to_rfc3339(),
            expires.to_rfc3339(),
        ],
    )?;
    eprintln!("Reset token for {email} (single use, valid 30 minutes):");
    println!("{token}");
    Ok(())
}

/// Email a reset link. Blocking SMTP round-trip — call via spawn_blocking,
/// never inline in a handler. Returns Err on any delivery failure so the
/// caller can fall back to `sent: false` (operator path).
pub fn send_reset_email(cfg: &SmtpConfig, recipient: &str, link: &str) -> anyhow::Result<()> {
    use lettre::transport::smtp::authentication::Credentials;
    use lettre::transport::smtp::client::{Tls, TlsParameters};
    use lettre::{Message, SmtpTransport, Transport};

    let tls = Tls::Opportunistic(TlsParameters::builder(cfg.host.clone()).build()?);
    let mut builder = SmtpTransport::relay(&cfg.host)?
        .port(cfg.port)
        .tls(tls);
    if let (Some(user), Some(pass)) = (&cfg.username, &cfg.password) {
        builder = builder.credentials(Credentials::new(user.clone(), pass.clone()));
    }
    let email = Message::builder()
        .from(cfg.from.parse()?)
        .to(recipient.parse()?)
        .subject("Your Engram password reset")
        .body(format!(
            "A password reset was requested for your Engram account.\n\n\
             If this was you, open this link within 30 minutes to set a new password:\n\n\
             \x20 {link}\n\n\
             If you didn't request this, ignore this email — your password is unchanged.\n"
        ))?;
    builder.build().send(&email)?;
    Ok(())
}

/// Resolves once SIGINT (Ctrl+C) or SIGTERM is received, so the server
/// can checkpoint and exit cleanly.
#[cfg(unix)]
async fn shutdown_signal() {
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("install SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {},
        _ = sigterm.recv() => {},
    }
    tracing::info!("Shutdown signal received — draining connections");
}

/// Ctrl+C only on non-unix platforms. (The relay is only deployed on Linux;
/// this exists so the crate compiles for contributors on Windows/macOS.)
#[cfg(not(unix))]
async fn shutdown_signal() {
    tokio::signal::ctrl_c().await.expect("install Ctrl+C handler");
    tracing::info!("Shutdown signal received — draining connections");
}

/// Load API keys from the SYNC_API_KEYS environment variable.
fn load_api_keys() -> HashMap<String, Arc<ApiKeyEntry>> {
    match std::env::var("SYNC_API_KEYS") {
        Ok(v) => parse_api_keys(&v),
        Err(_) => HashMap::new(),
    }
}

/// Parse a SYNC_API_KEYS value. Entry formats:
///   key                          — all vaults, rate limit 100 req/s
///   key:rate                     — all vaults, custom rate limit
///   key:rate:vault1;vault2       — scoped to the listed vaults only
///   key:rate:vault1+admin        — scoped, administers vault1
/// The scope list is `;`-separated (`,` already separates entries).
/// Keys shorter than 16 chars are skipped with a warning.
fn parse_api_keys(raw: &str) -> HashMap<String, Arc<ApiKeyEntry>> {
    let mut keys = HashMap::new();
    for entry in raw.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let (key, rate, scope) = match entry.splitn(3, ':').collect::<Vec<_>>().as_slice() {
            [k] => (k.to_string(), 100.0, None),
            [k, r] => (k.to_string(), r.parse::<f64>().unwrap_or(100.0), None),
            [k, r, s] => (k.to_string(), r.parse::<f64>().unwrap_or(100.0), Some(s.to_string())),
            _ => unreachable!(),
        };
        if key.len() < 16 {
            tracing::warn!(
                "API key '{}...' is too short ({} chars). Minimum 16 chars. Skipping.",
                &key[..key.len().min(8)],
                key.len()
            );
            continue;
        }

        let scope = match scope {
            None => None,
            Some(s) if s.trim().is_empty() => {
                tracing::warn!(
                    "API key '{}...' has an empty vault scope. Skipping — use `key:rate` for unscoped.",
                    &key[..key.len().min(8)]
                );
                continue;
            }
            Some(s) => Some(s),
        };

        let entry = match scope {
            None => Arc::new(ApiKeyEntry {
                rate: rate.max(1.0),
                vaults: None,
                admin_vaults: HashSet::new(),
                account_id: None,
            }),
            Some(scope) => {
                let mut vaults = HashSet::new();
                let mut admin_vaults = HashSet::new();
                for item in scope.split(';') {
                    let item = item.trim();
                    if item.is_empty() {
                        continue;
                    }
                    if let Some(v) = item.strip_suffix("+admin") {
                        if !v.is_empty() {
                            vaults.insert(v.to_string());
                            admin_vaults.insert(v.to_string());
                        }
                    } else {
                        vaults.insert(item.to_string());
                    }
                }
                if vaults.is_empty() {
                    tracing::warn!(
                        "API key '{}...' has no valid vaults in its scope. Skipping.",
                        &key[..key.len().min(8)]
                    );
                    continue;
                }
                Arc::new(ApiKeyEntry {
                    rate: rate.max(1.0),
                    vaults: Some(vaults),
                    admin_vaults,
                    account_id: None,
                })
            }
        };
        keys.insert(key, entry);
    }
    keys
}

#[cfg(test)]
mod tests {
    use super::*;

    const K1: &str = "key-one-000000001";
    const K2: &str = "key-two-000000002";

    #[test]
    fn unscoped_key_keeps_original_behavior() {
        let keys = parse_api_keys(K1);
        let entry = keys.get(K1).expect("key present");
        assert!(entry.vaults.is_none(), "no scope → all vaults");
        assert_eq!(entry.rate, 100.0);
        assert!(entry.admin_vaults.is_empty());
    }

    #[test]
    fn rate_limit_parses_and_defaults() {
        let keys = parse_api_keys(&format!("{K1}:50"));
        assert_eq!(keys.get(K1).unwrap().rate, 50.0);
        assert_eq!(parse_api_keys(K2).get(K2).unwrap().rate, 100.0);
        // Garbage rate falls back to default
        assert_eq!(parse_api_keys(&format!("{K1}:fast")).get(K1).unwrap().rate, 100.0);
    }

    #[test]
    fn scoped_key_lists_vaults_and_admins() {
        let keys = parse_api_keys(&format!("{K1}:10:vault-a;vault-b+admin"));
        let entry = keys.get(K1).unwrap();
        let vaults = entry.vaults.as_ref().expect("scoped");
        assert!(vaults.contains("vault-a"));
        assert!(vaults.contains("vault-b"), "admin implies access");
        assert!(entry.admin_vaults.contains("vault-b"));
        assert!(!entry.admin_vaults.contains("vault-a"));
    }

    #[test]
    fn invalid_entries_are_skipped() {
        // Too short
        assert!(parse_api_keys("short").is_empty());
        // Empty scope
        assert!(parse_api_keys(&format!("{K1}:10:")).is_empty());
        // Scope with only empty items
        assert!(parse_api_keys(&format!("{K1}:10:,")).is_empty());
        // Empty whole value
        assert!(parse_api_keys("").is_empty());
        assert!(parse_api_keys(" , ").is_empty());
    }
}
