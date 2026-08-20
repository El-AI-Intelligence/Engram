// Engram — Memory Vault CLI + daemon.
//
// Dual-mode binary:
// - Invoked as `engramd`: runs the HTTP daemon (backward compatible).
// - Invoked as `engram`: dispatches CLI subcommands (`capture`, `search`,
//   `today`, `eco`, `demo`, `init`, `mcp`, `daemon`).

mod app_state;
mod auth;
mod cli;
mod envfile;
mod errors;
mod link;
mod routes;
mod sync_client;

use app_state::{AppState, KeyHandoff, LiveEvent, SyncKeyMaterial};

use axum::extract::DefaultBodyLimit;
use clap::Parser;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex};
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;
use tracing::info;
use tracing_subscriber::EnvFilter;

use axiom_engram::{EngramStore, EngramStoreAdapter, LinkInferenceConfig, QemCache, QemConfig};
use axiom_engram::embed::Embedder;
use axiom_inference::InferenceProvider;

// ── Top-level CLI ──────────────────────────────────────────────────────────

#[derive(Parser, Debug)]
#[command(name = "engram", version, about = "Engram Memory Vault — your AI deserves a memory.")]
struct Cli {
    #[command(subcommand)]
    command: Option<cli::Commands>,

    // Legacy daemon flags (used when invoked as `engramd` with no subcommand)
    /// Path to the vault directory (daemon mode).
    /// Environment: ENGRAM_VAULT.
    #[arg(short, long, default_value = "./engram-data", env = "ENGRAM_VAULT")]
    vault: PathBuf,
    /// Listen address (daemon mode).
    /// Environment: ENGRAM_BIND.
    #[arg(short, long, default_value = "127.0.0.1:8787", env = "ENGRAM_BIND")]
    bind: SocketAddr,
    /// Passphrase for vault encryption (daemon mode).
    /// Environment: ENGRAM_PASSPHRASE.
    #[arg(short, long, env = "ENGRAM_PASSPHRASE")]
    passphrase: Option<String>,
    /// Read KEY=VALUE lines from this file into the environment before
    /// parsing (fills gaps only — real env and CLI flags win).
    /// Environment: ENGRAM_ENV_FILE.
    #[arg(long, env = "ENGRAM_ENV_FILE")]
    env_file: Option<PathBuf>,
    /// Path to static UI files to serve (SPA fallback).
    /// When set, engramd serves the vault UI directly — no reverse proxy needed.
    /// Environment: ENGRAM_UI_DIR.
    #[arg(long, env = "ENGRAM_UI_DIR")]
    ui_dir: Option<PathBuf>,
}

// ── Entry point ────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    // Resolve --env-file before clap parses, so ENGRAM_* values from the
    // file (e.g. ENGRAM_PASSPHRASE) are visible to clap's env feature and
    // to everything after. Real env vars and CLI flags always win.
    if let Some(path) = env_file_from_args() {
        envfile::load_env_file(&path)?;
    }

    let cli = Cli::parse();

    let is_engramd = invoked_as_engramd();

    match cli.command {
        Some(cmd) => dispatch_cli(cmd).await,
        None if is_engramd => run_daemon(cli.vault, cli.bind, cli.passphrase, cli.ui_dir).await,
        None => {
            // `engram` with no subcommand — print help
            Cli::parse_from(["engram", "--help"]);
            Ok(())
        }
    }
}

/// True when this binary was invoked under the name `engramd` (any
/// extension — Windows appends `.exe`), which selects daemon mode when no
/// subcommand is given.
fn invoked_as_engramd() -> bool {
    std::env::args()
        .next()
        .map(|a| name_is_engramd(&a))
        .unwrap_or(false)
}

fn name_is_engramd(a: &str) -> bool {
    std::path::Path::new(a).file_stem() == Some(std::ffi::OsStr::new("engramd"))
}

/// Find `--env-file <path>` / `--env-file=<path>` in the raw argv
/// (position-independent, so it works for both `engramd --env-file X` and
/// `engram daemon --env-file X`), falling back to ENGRAM_ENV_FILE.
fn env_file_from_args() -> Option<PathBuf> {
    let mut args = std::env::args().skip(1).peekable();
    while let Some(arg) = args.next() {
        if let Some(rest) = arg.strip_prefix("--env-file=") {
            return Some(PathBuf::from(rest));
        }
        if arg == "--env-file" {
            if let Some(v) = args.peek() {
                return Some(PathBuf::from(v));
            }
        }
    }
    std::env::var_os("ENGRAM_ENV_FILE").map(PathBuf::from)
}

// ── CLI dispatch ───────────────────────────────────────────────────────────

async fn dispatch_cli(cmd: cli::Commands) -> anyhow::Result<()> {
    match cmd {
        cli::Commands::Daemon { vault, bind, passphrase, ui_dir, env_file: _ } => {
            let addr: SocketAddr = bind.parse()?;
            run_daemon(vault, addr, passphrase, ui_dir).await
        }
        cli::Commands::Init => {
            cli::handle_init().await
        }
        cli::Commands::Join { vault, server_url, api_key, vault_id, name } => {
            cli::handle_join(vault, server_url, api_key, vault_id, name).await
        }
        cli::Commands::Pair { code, vault, server_url, site, name } => {
            cli::handle_pair(code, vault, server_url, site, name).await
        }
        cli::Commands::Link { vault, server_url, site, name, force } => {
            cli::handle_link(vault, server_url, site, name, force).await
        }
        cli::Commands::Handoff { vault, bind, site } => {
            cli::handle_handoff(vault, bind, site).await
        }
        cli::Commands::Capture { content, tags, layer, source, valence, project, vault } => {
            cli::handle_capture(content, tags, layer, source, valence, project, vault).await
        }
        cli::Commands::Search { query, limit, layer, vault } => {
            cli::handle_search(query, limit, layer, vault).await
        }
        cli::Commands::Today { vault } => {
            cli::handle_today(vault).await
        }
        cli::Commands::Eco { vault } => {
            cli::handle_eco(vault).await
        }
        cli::Commands::Demo { vault } => {
            cli::handle_demo(vault).await
        }
        cli::Commands::BackfillLinks { vault, max_links, min_similarity } => {
            cli::handle_backfill_links(vault, max_links, min_similarity).await
        }
        cli::Commands::Mcp { command, url, dry_run, yes } => {
            cli::handle_mcp(command, url, dry_run, yes).await
        }
        cli::Commands::Digest { url, days, prose } => {
            cli::handle_digest(url, days, prose).await
        }
        cli::Commands::Onboarding { bind } => {
            cli::handle_onboarding(bind).await
        }
        cli::Commands::ShowPassphrase { env_file } => {
            cli::handle_show_passphrase(env_file)
        }
    }
}

// ── Device identity ─────────────────────────────────────────────────────────

/// Load or create a persistent device ID stored in the vault directory.
/// This survives daemon restarts so sync vector clocks are meaningful
/// across sessions. Without this, every restart looks like a new device
/// and multi-device conflict resolution breaks.
fn load_or_create_device_id(vault_path: &std::path::Path) -> String {
    let path = vault_path.join("device.json");
    if path.exists() {
        if let Ok(data) = std::fs::read_to_string(&path) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&data) {
                if let Some(id) = json.get("device_id").and_then(|v| v.as_str()) {
                    return id.to_string();
                }
            }
        }
    }
    // First run — create a new device identity
    let device_id = uuid::Uuid::new_v4().to_string();
    let json = serde_json::json!({
        "device_id": device_id,
        "created_at": chrono::Utc::now().to_rfc3339(),
        "label": hostname(),
    });
    if let Err(e) = std::fs::write(&path, serde_json::to_string_pretty(&json).unwrap_or_default()) {
        tracing::warn!(error = %e, path = %path.display(), "Failed to persist device identity");
    } else {
        tracing::info!(%device_id, "Created persistent device identity");
    }
    device_id
}

/// Persist the effective sync `vault_id` into config.json when it isn't
/// pinned there yet, so `/teams/status` and later restarts agree with the
/// id the sync loop actually uses. Best-effort: a read-only vault dir just
/// means the id is re-derived on each start (it is deterministic anyway).
fn pin_vault_id(vault_path: &std::path::Path, vault_id: &str) {
    let config_path = vault_path.join("config.json");
    let Ok(data) = std::fs::read_to_string(&config_path) else {
        return;
    };
    let Ok(mut cfg) = serde_json::from_str::<serde_json::Value>(&data) else {
        return;
    };
    let needs_pin = match cfg.get_mut("sync") {
        Some(serde_json::Value::Object(obj)) if !obj.contains_key("vault_id") => {
            obj.insert(
                "vault_id".to_string(),
                serde_json::Value::String(vault_id.to_string()),
            );
            true
        }
        _ => false,
    };
    if needs_pin {
        if let Ok(pretty) = serde_json::to_string_pretty(&cfg) {
            if let Err(e) = std::fs::write(&config_path, pretty) {
                tracing::warn!(error = %e, "Failed to pin vault_id into config");
            }
        }
    }
}

fn hostname() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "unknown".into())
}

/// Read the hygiene schedule interval from config.json, falling back to
/// 24 hours if the config is missing or unparseable.
fn load_schedule_interval(vault_path: &std::path::Path) -> u64 {
    let config_path = vault_path.join("config.json");
    if let Ok(data) = std::fs::read_to_string(&config_path) {
        if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&data) {
            if let Some(hours) = cfg
                .get("schedule")
                .and_then(|s| s.get("decay_interval_hours"))
                .and_then(|v| v.as_u64())
            {
                let secs = hours * 3600;
                // Clamp: minimum 1 hour, maximum 7 days
                return secs.clamp(3600, 604800);
            }
        }
    }
    86400 // default: 24 hours
}

/// Read `qem.warm_strength_min` from config.json, falling back to the
/// in-code default (0.3) if the config is missing or unparseable.
fn load_qem_config(vault_path: &std::path::Path) -> QemConfig {
    let mut config = QemConfig::default();
    let config_path = vault_path.join("config.json");
    if let Ok(data) = std::fs::read_to_string(&config_path) {
        if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&data) {
            if let Some(min) = cfg
                .get("qem")
                .and_then(|q| q.get("warm_strength_min"))
                .and_then(|v| v.as_f64())
            {
                config.warm_strength_min = min.clamp(0.0, 1.0);
            }
        }
    }
    config
}

/// Read `noise.ignored_sources` from config.json. Defaults to
/// ["ai-session", "ai-tool"] (transcript-redundant agent captures) when the
/// config is missing, unparseable, or has an empty list. Loaded once at
/// startup — a PATCH to /config requires a restart to take effect.
fn load_noise_ignored_sources(vault_path: &std::path::Path) -> Vec<String> {
    const DEFAULTS: &[&str] = &["ai-session", "ai-tool"];
    let config_path = vault_path.join("config.json");
    if let Ok(data) = std::fs::read_to_string(&config_path) {
        if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&data) {
            if let Some(list) = cfg
                .get("noise")
                .and_then(|n| n.get("ignored_sources"))
                .and_then(|v| v.as_array())
            {
                let sources: Vec<String> = list
                    .iter()
                    .filter_map(|v| v.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                return sources;
            }
        }
    }
    DEFAULTS.iter().map(|s| s.to_string()).collect()
}

/// Read `links` inference settings from config.json. Defaults to enabled
/// (max 5 neighbors, min cosine similarity 0.35). `links.auto_infer: false`
/// disables write-time inference entirely. Loaded once at startup — a PATCH
/// to /config requires a restart.
fn load_link_inference(vault_path: &std::path::Path) -> Option<LinkInferenceConfig> {
    let config_path = vault_path.join("config.json");
    if let Ok(data) = std::fs::read_to_string(&config_path) {
        if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&data) {
            let links = cfg.get("links");
            if links.and_then(|l| l.get("auto_infer")).and_then(|v| v.as_bool()) == Some(false) {
                return None;
            }
            let max_links = links
                .and_then(|l| l.get("max_links"))
                .and_then(|v| v.as_u64())
                .unwrap_or(5) as usize;
            let min_similarity = links
                .and_then(|l| l.get("min_similarity"))
                .and_then(|v| v.as_f64())
                .unwrap_or(0.35);
            return Some(LinkInferenceConfig { max_links, min_similarity });
        }
    }
    Some(LinkInferenceConfig::default())
}

/// Build the optional local LLM provider for narrative consolidation.
///
/// Only the `ollama:<model>` form of `summarization.llm` is accepted, and the
/// base URL is always hardcoded to localhost — local-first is a hard
/// constraint, so cloud URLs and env-var configs are rejected with a warning.
/// `None` → the narratives endpoint uses the deterministic heuristic
/// summarizer instead.
fn load_inference_provider(vault_path: &std::path::Path) -> Option<Arc<dyn InferenceProvider>> {
    let config_path = vault_path.join("config.json");
    let data = std::fs::read_to_string(&config_path).ok()?;
    let cfg: serde_json::Value = serde_json::from_str(&data).ok()?;
    let llm = cfg.get("summarization")?.get("llm")?.as_str()?;

    let Some(model) = llm.strip_prefix("ollama:") else {
        tracing::warn!(%llm, "summarization.llm rejected — only local `ollama:<model>` is supported; narratives will use the heuristic summarizer");
        return None;
    };
    if model.trim().is_empty() {
        tracing::warn!("summarization.llm has an empty model name — narratives will use the heuristic summarizer");
        return None;
    }

    let config = axiom_inference::InferenceConfig {
        base_url: "http://localhost:11434/v1".into(),
        api_key: String::new(),
        model: model.to_string(),
        ..Default::default()
    };
    info!(model, "Local Ollama inference enabled for narrative consolidation");
    Some(config.build())
}

/// Read the consolidation schedule + auto-consolidation gate from config.json.
/// Returns (interval_secs, auto_consolidation). Interval is clamped to
/// 1 hour – 7 days; defaults to 24h / enabled.
fn load_consolidation_schedule(vault_path: &std::path::Path) -> (u64, bool) {
    let config_path = vault_path.join("config.json");
    if let Ok(data) = std::fs::read_to_string(&config_path) {
        if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&data) {
            let auto = cfg
                .get("schedule")
                .and_then(|s| s.get("auto_consolidation"))
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let hours = cfg
                .get("schedule")
                .and_then(|s| s.get("consolidation_interval_hours"))
                .and_then(|v| v.as_u64())
                .unwrap_or(24);
            let secs = (hours * 3600).clamp(3600, 604_800);
            return (secs, auto);
        }
    }
    (86400, true)
}

// ── CORS ────────────────────────────────────────────────────────────────────

/// Build a CORS layer that allows localhost origins (the daemon binds loopback,
/// so only local processes can reach it) and the production UI domain.
/// This replaces the previous `CorsLayer::permissive()` which allowed ANY origin
/// to exfiltrate memories via cross-origin fetch from malicious websites.
///
/// Additional allowed origins can be configured via the `ENGRAM_CORS_ORIGINS`
/// environment variable (comma-separated URLs).
fn cors_layer() -> CorsLayer {
    use tower_http::cors::AllowOrigin;

    let extra_origins = cors_extra_origins();

    CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(
            move |origin: &axum::http::HeaderValue, _req| origin_is_allowed(origin, &extra_origins),
        ))
        .allow_methods(Any)
        .allow_headers(Any)
}

/// Extra allowed origins from the ENGRAM_CORS_ORIGINS env var (comma-separated).
fn cors_extra_origins() -> Vec<String> {
    std::env::var("ENGRAM_CORS_ORIGINS")
        .ok()
        .map(|s| s.split(',').map(|o| o.trim().to_string()).collect())
        .unwrap_or_default()
}

/// Single source of truth for which browser origins may call the daemon.
fn origin_is_allowed(origin: &axum::http::HeaderValue, extra_origins: &[String]) -> bool {
    let Ok(s) = origin.to_str() else { return false };
    // Allow localhost on any port (dev UI, Python server, etc.)
    if s.starts_with("http://localhost:")
        || s.starts_with("http://127.0.0.1:")
        || s.starts_with("http://[::1]:")
    {
        return true;
    }
    // Allow the production domain
    if s == "https://engram.ellmstack.dev" {
        return true;
    }
    // Allow any domains configured via ENGRAM_CORS_ORIGINS
    extra_origins.iter().any(|o| o == s)
}

/// Chrome 142+ enforces Local Network Access (successor to Private Network
/// Access): a fetch from a public site (https://engram.ellmstack.dev) to this
/// loopback daemon requires the preflight to answer with
/// `Access-Control-Allow-Private-Network: true`, or the browser fails the
/// fetch with a bare network error that never reaches the daemon — invisible
/// in every server log. The header is gated on the same origin predicate as
/// the CORS layer, never `*` (a wildcard would let any public page probe
/// loopback daemons through this opt-in).
async fn pna_opt_in(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let extras = cors_extra_origins();
    let origin_allowed = req
        .headers()
        .get(axum::http::header::ORIGIN)
        .map(|o| origin_is_allowed(o, &extras))
        .unwrap_or(false);
    let mut res = next.run(req).await;
    if origin_allowed {
        res.headers_mut().insert(
            axum::http::HeaderName::from_static("access-control-allow-private-network"),
            axum::http::HeaderValue::from_static("true"),
        );
    }
    res
}

// ── Daemon ─────────────────────────────────────────────────────────────────

async fn run_daemon(
    vault_path: PathBuf,
    bind: SocketAddr,
    passphrase: Option<String>,
    ui_dir: Option<PathBuf>,
) -> anyhow::Result<()> {
    use axum::Router;

    std::fs::create_dir_all(&vault_path)?;
    let store = match &passphrase {
        Some(pw) => EngramStore::open_with_passphrase(&vault_path, pw).await?,
        None => EngramStore::open(&vault_path).await?,
    };
    let vault: Arc<Mutex<EngramStore>> = Arc::new(Mutex::new(store));
    let start_time = chrono::Utc::now();

    // ── Warm QEM L1 cache from L2 ──────────────────────────────────────────
    let adapter = EngramStoreAdapter::new(vault.clone());
    let qem = QemCache::new(adapter, load_qem_config(&vault_path));
    info!("Warming QEM L1 cache from vault...");
    if let Err(e) = qem.warm().await {
        tracing::warn!(error = %e, "QEM warm failed (non-fatal, cache starts cold)");
    } else {
        info!(entries = qem.cache_size(), "QEM L1 cache warmed");
    }
    let qem = Arc::new(qem);

    // ── Device identity (persisted across restarts) ────────────────────
    let device_id = load_or_create_device_id(&vault_path);

    // ── Event broadcast channel (WebSocket) ─────────────────────────────
    let (events_tx, _) = broadcast::channel::<LiveEvent>(256);

    // ── Embedding provider (zero-config ONNX, auto-downloads model) ──────
    // dimensions() reports the configured size (384) even before the model
    // loads, so capture/search attempt embed() immediately; embed() lazy-loads
    // the model on first use and applies retry backoff on failure. The model
    // cache lives next to the vault (not $HOME) — HOME is often unset under
    // systemd.
    let embedder: Option<Arc<dyn Embedder>> = Some(Arc::new(
        axiom_engram::OnnxEmbedder::with_cache_dir(vault_path.clone()),
    ));
    info!("ONNX embedder enabled (MiniLM model loads lazily on first embed)");

    // Manual sync trigger channel — /sync/now bumps the counter, the sync
    // loop's select! wakes for an immediate cycle.
    let (sync_trigger_tx, sync_trigger_rx) = tokio::sync::watch::channel(0u64);
    let sync_trigger_tx = Arc::new(sync_trigger_tx);

    let mut state = AppState {
        vault: vault.clone(),
        qem,
        vault_path: vault_path.clone(),
        start_time,
        events_tx,
        device_id: device_id.clone(),
        embedder,
        inference: load_inference_provider(&vault_path),
        noise_ignored_sources: load_noise_ignored_sources(&vault_path),
        link_inference: load_link_inference(&vault_path),
        sync_trigger: sync_trigger_tx.clone(),
        sync_keys: None,
        key_handoff: KeyHandoff::default(),
    };

    // ── Background scheduler ──────────────────────────────────────────────
    let bg_state = state.clone();
    tokio::spawn(async move { background_scheduler(bg_state).await });

    // ── Background consolidator ───────────────────────────────────────────
    // Weekly consolidation on its own schedule. LLM narratives are NEVER
    // triggered from here — narrative synthesis stays manual-only.
    let bg_state = state.clone();
    tokio::spawn(async move { background_consolidator(bg_state).await });

    // ── Sync loop (if enabled via config) ──────────────────────────────────
    let sync_config_path = vault_path.join("config.json");
    if let Ok(data) = std::fs::read_to_string(&sync_config_path) {
        if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&data) {
            let sync_enabled = cfg
                .get("sync")
                .and_then(|s| s.get("enabled"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if sync_enabled {
                let server_url = cfg
                    .get("sync")
                    .and_then(|s| s.get("server_url"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("http://localhost:8788");
                let api_key = cfg
                    .get("sync")
                    .and_then(|s| s.get("api_key"))
                    .and_then(|v| v.as_str())
                    .map(String::from);
                let interval_secs = cfg
                    .get("sync")
                    .and_then(|s| s.get("interval_secs"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(60)
                    .max(5); // minimum 5s to avoid busy-loop on misconfiguration

                // Passphrase: for sync, we use the same passphrase used to open the vault.
                // If no passphrase was provided, sync won't work (encryption is required).
                if let Some(ref pw) = passphrase {
                    // The vault_id is pinned in config, or derived from the sync
                    // passphrase — same passphrase ⇒ same id, so teammates land
                    // in the same vault with no configuration. (It must NOT come
                    // from the vault directory name: directory names differ
                    // across devices and binary versions, which silently split
                    // a shared vault across devices on the server.)
                    let vault_id = match cfg
                        .get("sync")
                        .and_then(|s| s.get("vault_id"))
                        .and_then(|v| v.as_str())
                    {
                        Some(v) => v.to_string(),
                        None => {
                            let derived = sync_client::derive_vault_id(pw);
                            pin_vault_id(&vault_path, &derived);
                            info!(
                                vault_id = %derived,
                                "vault_id unset — derived from passphrase and pinned to config"
                            );
                            derived
                        }
                    };

                    let initial_clock = sync_client::SyncClient::load_clock(&vault_path);
                    let sync_client = Arc::new(sync_client::SyncClient::new(
                        server_url.to_string(),
                        vault_id,
                        pw,
                        device_id.clone(),
                        api_key,
                        initial_clock,
                    ));
                    // Hold the SAME key bytes the sync loop uses, for the
                    // one-time browser key handoff (account migration).
                    state.sync_keys = Some(Arc::new(SyncKeyMaterial {
                        enc_key: sync_client.encryption_key(),
                        hmac_key: sync_client.hmac_key(),
                        vault_id: sync_client.vault_id().to_string(),
                    }));
                    info!(
                        server_url = %server_url,
                        interval_secs,
                        %initial_clock,
                        "Starting sync loop"
                    );
                    sync_client::spawn_sync_loop(
                        sync_client,
                        vault.clone(),
                        vault_path.clone(),
                        std::time::Duration::from_secs(interval_secs),
                        sync_trigger_rx,
                        state.events_tx.clone(),
                    );
                } else {
                    tracing::error!(
                        "Sync is enabled but no passphrase is set. \
                         Sync requires a passphrase for E2E encryption. \
                         Restart with --passphrase or set a passphrase during init."
                    );
                    eprintln!(
                        "ERROR: Sync is enabled in config but no passphrase provided. \
                         Sync requires a passphrase for E2E encryption. \
                         Restart engramd with --passphrase."
                    );
                }
            }
        }
    }

    // ── Build router ──────────────────────────────────────────────────────
    // Auth from environment (ENGRAMD_API_KEY). Required on non-loopback.
    let auth_state = auth::AuthState::from_env(bind.ip().is_loopback());
    if let Err(e) = auth_state.check_startup_safe(bind) {
        tracing::error!("{e}");
        anyhow::bail!(e);
    }

    let app = Router::new()
        .merge(routes::health::router())
        .merge(routes::memories::router())
        .merge(routes::context::router())
        .merge(routes::consolidation::router())
        .merge(routes::analytics::router())
        .merge(routes::config::router())
        .merge(routes::export_import::router())
        .merge(routes::events::router())
        .merge(routes::annotations::router())
        .merge(routes::saved_searches::router())
        .merge(routes::privacy::router())
        .merge(routes::sync_status::router())
        .merge(routes::teams::router())
        .merge(routes::digest::router())
        .merge(routes::key_handoff::router())
        .with_state(state)
        // CORS must be outermost so OPTIONS preflight is handled before auth
        .layer(axum::middleware::from_fn_with_state(
            auth_state,
            auth::auth_middleware,
        ))
        .layer(cors_layer())
        .layer(axum::middleware::from_fn(pna_opt_in))
        .layer(TraceLayer::new_for_http())
        // Reject oversized bodies (10 MiB) with structured JSON errors
        .layer(DefaultBodyLimit::max(10 * 1024 * 1024))
        // Convert extractor rejections (422, 413, 400) to structured JSON errors.
        // Applied as the outermost layer so it catches responses from all inner
        // middleware (including DefaultBodyLimit and extractor errors).
        .layer(
            tower::ServiceBuilder::new()
                .map_response(errors::handle_extractor_rejection)
                .into_inner(),
        );

    // ── Static UI serving (optional) ──────────────────────────────────────
    let app = if let Some(ref ui) = ui_dir {
        info!("Serving UI from {}", ui.display());
        app.fallback_service(
            ServeDir::new(ui)
                .fallback(ServeFile::new(ui.join("index.html"))),
        )
    } else {
        app
    };

    info!(
        "Engramd starting on {} (vault: {})",
        bind,
        vault_path.display()
    );
    let listener = tokio::net::TcpListener::bind(bind).await?;

    // ── Graceful shutdown ──────────────────────────────────────────────────
    let shutdown_signal = async {
        #[cfg(unix)]
        {
            let mut sigterm = tokio::signal::unix::signal(
                tokio::signal::unix::SignalKind::terminate(),
            )
            .expect("failed to install SIGTERM handler");
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {
                    info!("SIGINT received, draining connections...");
                }
                _ = sigterm.recv() => {
                    info!("SIGTERM received, draining connections...");
                }
            }
        }
        #[cfg(not(unix))]
        {
            tokio::signal::ctrl_c()
                .await
                .expect("failed to install Ctrl+C handler");
            info!("Shutdown signal received, draining connections...");
        }
    };

    // axum 0.8 serve returns a future that we can wrap with graceful shutdown
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal)
        .await?;

    info!("Shutdown complete. Goodbye.");
    Ok(())
}

/// Background task: runs daily hygiene on a configurable schedule.
///
/// Reads the `schedule.decay_interval_hours` field from config.json.
/// Defaults to 24 hours. Respects config so users can tune decay frequency.
async fn background_scheduler(state: AppState) {
    let interval_secs = load_schedule_interval(&state.vault_path);
    let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(interval_secs));
    // Suppress the immediate first tick — hygiene on startup is aggressive
    // and can conflict with vault warm-up.
    interval.tick().await;
    loop {
        interval.tick().await;
        let vault = state.vault.lock().await;
        let (daily_strengthened, daily_decayed) = match vault.apply_daily_hygiene().await {
            Ok(v) => v,
            Err(e) => {
                tracing::error!(error = %e, "Background hygiene failed — memory decay not applied");
                continue;
            }
        };
        if daily_strengthened + daily_decayed > 0 {
            info!(daily_strengthened, daily_decayed, "Daily hygiene");
            let _ = state.events_tx.send(LiveEvent::Decay {
                strengthened: daily_strengthened as usize,
                decayed: daily_decayed as usize,
                timestamp: chrono::Utc::now().to_rfc3339(),
            });
        }
    }
}

/// Background task: runs weekly consolidation on a configurable schedule.
///
/// Reads `schedule.consolidation_interval_hours` and the
/// `schedule.auto_consolidation` gate from config.json. When the gate is off
/// the task exits without doing anything. LLM narratives are never triggered
/// from here — they stay manual (POST /consolidate/narratives).
async fn background_consolidator(state: AppState) {
    let (interval_secs, auto) = load_consolidation_schedule(&state.vault_path);
    if !auto {
        info!("Auto-consolidation disabled by config — background consolidator not running");
        return;
    }

    let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(interval_secs));
    // Suppress the immediate first tick — consolidating on startup is
    // aggressive and can conflict with vault warm-up.
    interval.tick().await;
    loop {
        interval.tick().await;
        let vault = state.vault.lock().await;
        let (promoted, pruned) = match vault.apply_weekly_consolidation().await {
            Ok(v) => v,
            Err(e) => {
                tracing::error!(error = %e, "Background consolidation failed — engrams not consolidated");
                continue;
            }
        };
        // Record the run so /analytics/stats and /consolidate/history reflect it
        let run = axiom_engram::ConsolidationRun {
            id: format!("weekly_{}", chrono::Utc::now().timestamp_millis()),
            run_at: chrono::Utc::now(),
            episodes_processed: Some((promoted + pruned) as i32),
            semantics_created: Some(promoted),
            engrams_decayed: None,
            notes: Some(format!("Weekly consolidation (scheduled): promoted {}, pruned {}", promoted, pruned)),
        };
        let _ = vault.record_consolidation_run(&run).await;
        if promoted + pruned > 0 {
            info!(promoted, pruned, "Weekly consolidation (scheduled)");
            let _ = state.events_tx.send(LiveEvent::Consolidation {
                promoted: promoted as usize,
                pruned: pruned as usize,
                timestamp: chrono::Utc::now().to_rfc3339(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::name_is_engramd;

    #[test]
    fn daemon_name_detection() {
        assert!(name_is_engramd("engramd"));
        assert!(name_is_engramd("engramd.exe"));
        assert!(name_is_engramd("/usr/local/bin/engramd"));
        // Backslash is only a separator on Windows itself.
        #[cfg(windows)]
        assert!(name_is_engramd(r"C:\Program Files\Engram\engramd.exe"));
        assert!(name_is_engramd("../bin/engramd"));
        assert!(!name_is_engramd("engram"));
        assert!(!name_is_engramd("engram.exe"));
        assert!(!name_is_engramd("engramd-mcp"));
        assert!(!name_is_engramd("engramd-mcp.exe"));
        assert!(!name_is_engramd(""));
    }
}
