//! Engram Sync Server — dumb pipe for E2E encrypted memory blobs.
//!
//! Design principles:
//!   - Server NEVER sees plaintext — all encryption happens client-side
//!   - Stateless: no sessions, no user accounts beyond API key
//!   - Single responsibility: accept blobs, serve blobs, verify HMAC
//!   - Billing is handled by a separate service (Stripe webhooks)
//!
//! API:
//!   GET  /health                                    — server health
//!   POST /v1/vaults/{vault_id}/push                 — push encrypted blobs
//!   GET  /v1/vaults/{vault_id}/pull?since=&limit=   — pull changes
//!   GET  /v1/vaults/{vault_id}/stats                — vault statistics
//!
//! Auth:
//!   Set SYNC_API_KEYS=key1,key2 in env. Clients send
//!   Authorization: Bearer <key>. Auth is optional on loopback,
//!   required on non-loopback (matches Guardrail's default-secure pattern).

mod routes;

use clap::Parser;
use std::collections::HashMap;
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

#[derive(Clone)]
pub struct SyncState {
    pub conn: Arc<Mutex<rusqlite::Connection>>,
    pub start_time: chrono::DateTime<chrono::Utc>,
    pub data_dir: PathBuf,
    /// Known API keys: key string → requests-per-second rate limit.
    /// Empty map → auth disabled (loopback mode).
    pub api_keys: Arc<HashMap<String, f64>>,
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
            ON sync_blobs(vault_id, memory_id);",
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

    // ── Build router ───────────────────────────────────────────────────
    let app = axum::Router::new()
        .merge(routes::router())
        .with_state(state)
        .layer(
            CorsLayer::new()
                .allow_methods([axum::http::Method::GET, axum::http::Method::POST])
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
    axum::serve(listener, app).await?;

    Ok(())
}

/// Load API keys from the SYNC_API_KEYS environment variable.
/// Format: "key1:10,key2:50" where the number after `:` is the
/// per-second rate limit (default 100). Keys shorter than 16 chars
/// are rejected.
fn load_api_keys() -> HashMap<String, f64> {
    let raw = match std::env::var("SYNC_API_KEYS") {
        Ok(v) => v,
        Err(_) => return HashMap::new(),
    };

    let mut keys = HashMap::new();
    for entry in raw.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let (key, rate) = match entry.split_once(':') {
            Some((k, r)) => (k.to_string(), r.parse::<f64>().unwrap_or(100.0)),
            None => (entry.to_string(), 100.0),
        };
        if key.len() < 16 {
            tracing::warn!(
                "API key '{}...' is too short ({} chars). Minimum 16 chars. Skipping.",
                &key[..key.len().min(8)],
                key.len()
            );
            continue;
        }
        keys.insert(key, rate.max(1.0));
    }
    keys
}
