// Shared application state types used by both the binary (main.rs) and
// integration tests (via lib.rs).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{broadcast, watch, Mutex};

use axiom_engram::{EngramStore, EngramStoreAdapter, LinkInferenceConfig, QemCache};
use axiom_engram::embed::Embedder;
use axiom_inference::InferenceProvider;

/// A live event broadcast to connected WebSocket clients.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LiveEvent {
    Capture {
        /// The full serialized memory (an Engram as JSON), so clients can
        /// render the event without a follow-up fetch. The UI's WS handler
        /// keys on `msg.memory`.
        memory: serde_json::Value,
        timestamp: String,
    },
    Decay {
        strengthened: usize,
        decayed: usize,
        timestamp: String,
    },
    Consolidation {
        promoted: usize,
        pruned: usize,
        timestamp: String,
    },
}

/// E2E sync key material for the box vault — the enc + hmac keys the sync
/// loop uses, byte-exact copies from `SyncClient`. Held so the one-time
/// key-handoff route can give the browser the vault keys exactly once
/// during account migration (the SPA wraps them under account key A).
#[derive(Clone)]
pub struct SyncKeyMaterial {
    pub enc_key: [u8; 32],
    pub hmac_key: [u8; 32],
}

/// One-time key-handoff token store: single-use, 300s TTL, swept on access.
/// The mint/redeem logic lives in routes::key_handoff (field is pub(crate)
/// so that impl can reach it). The route itself is gated by the box
/// basic-auth in Caddy — the same wall the /config routes sit behind.
#[derive(Clone, Default)]
pub struct KeyHandoff {
    pub(crate) tokens: Arc<Mutex<HashMap<String, u64>>>,
}

/// Alias for the QEM-wrapped vault.
pub type CachedStore = QemCache<EngramStoreAdapter>;

/// The shared application state, accessible from every route handler.
#[derive(Clone)]
pub struct AppState {
    pub vault: Arc<Mutex<EngramStore>>,
    /// L1 holographic cache (associative lookup, novelty filter, hit-rate tracking)
    pub qem: Arc<CachedStore>,
    pub vault_path: PathBuf,
    pub start_time: chrono::DateTime<chrono::Utc>,
    /// Broadcast channel for live WebSocket events (capture, decay, consolidation)
    pub events_tx: broadcast::Sender<LiveEvent>,
    /// Persistent device identity (stored in vault_path/device.json).
    pub device_id: String,
    /// Optional embedding provider for vector search.
    pub embedder: Option<Arc<dyn Embedder>>,
    /// Optional local-first LLM for narrative consolidation.
    /// Only ever built from `summarization.llm = "ollama:<model>"` (localhost
    /// Ollama); never from env vars or cloud config. `None` → the narratives
    /// endpoint falls back to the deterministic heuristic summarizer.
    pub inference: Option<Arc<dyn InferenceProvider>>,
    /// Capture sources dropped at the REST route (from `noise.ignored_sources`
    /// in config.json). Loaded once at startup — NOT hot-reloaded; a PATCH to
    /// /config requires a restart to take effect. Default: ["ai-session",
    /// "ai-tool"] (transcript-redundant agent captures).
    pub noise_ignored_sources: Vec<String>,
    /// Watch channel for manual sync triggers (`POST /sync/now`). Each
    /// increment wakes the sync loop for an immediate cycle; a counter
    /// rather than a Notify so a trigger fired mid-cycle isn't lost.
    pub sync_trigger: Arc<watch::Sender<u64>>,
    /// Automatic associative link inference from embeddings (`links` section
    /// of config.json). `None` disables write-time inference; loaded once at
    /// startup — a PATCH to /config requires a restart.
    pub link_inference: Option<LinkInferenceConfig>,
    /// Sync keys for the one-time browser key handoff (None = sync disabled).
    pub sync_keys: Option<Arc<SyncKeyMaterial>>,
    /// One-time key-handoff tokens (single-use, 300s TTL).
    pub key_handoff: KeyHandoff,
}
