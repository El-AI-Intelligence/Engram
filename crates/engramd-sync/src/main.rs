//! Engram Sync Server — dumb pipe for E2E encrypted memory blobs.
//!
//! Design principles:
//!   - Server NEVER sees plaintext — all encryption happens client-side
//!   - Stateless: no sessions, no user accounts beyond API key
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
//!
//! Auth:
//!   Set SYNC_API_KEYS in env. Clients send
//!   Authorization: Bearer <key>. Auth is optional on loopback,
//!   required on non-loopback (matches Guardrail's default-secure pattern).
//!
//!   Key format (comma-separated entries):
//!     key                        — all vaults, rate limit 100 req/s
//!     key:rate                   — all vaults, custom rate limit
//!     key:rate:vault1;vault2     — scoped to the listed vaults only
//!     key:rate:vault1+admin      — scoped, and administers vault1
//!   Unscoped keys keep the original all-vaults behavior (and are the
//!   superuser: they may revoke devices on any vault). Scoped keys can
//!   only touch their own vaults; +admin also lets a key revoke devices
//!   on that vault — the substrate a hosted control plane mints
//!   per-member keys against.

mod routes;

use clap::Parser;
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
}

/// Per-key rate limiter: simple token bucket with 1-second refill.
pub struct RateLimiter {
    tokens: f64,
    max_tokens: f64,
    last_refill: std::time::Instant,
}

impl RateLimiter {
    fn new(rate: f64) -> Self {
        Self { tokens: rate, max_tokens: rate, last_refill: std::time::Instant::now() }
    }

    /// Returns true if a request is allowed (consumes 1 token).
    fn allow(&mut self) -> bool {
        let now = std::time::Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.tokens = (self.tokens + elapsed * (self.max_tokens / 1.0)).min(self.max_tokens);
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
        );",
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

    let state = SyncState {
        conn: Arc::new(Mutex::new(conn)),
        start_time: chrono::Utc::now(),
        data_dir: cli.data_dir.clone(),
        api_keys: Arc::new(api_keys),
        rate_limiters: Arc::new(Mutex::new(HashMap::new())),
        is_loopback,
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
    let app = axum::Router::new()
        .merge(routes::router())
        .with_state(state)
        .layer(
            CorsLayer::new()
                .allow_methods([
                    axum::http::Method::GET,
                    axum::http::Method::POST,
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

/// Resolves once SIGINT (Ctrl+C) or SIGTERM is received, so the server
/// can checkpoint and exit cleanly.
async fn shutdown_signal() {
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("install SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {},
        _ = sigterm.recv() => {},
    }
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
