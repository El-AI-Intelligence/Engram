// Shared application state types used by both the binary (main.rs) and
// integration tests (via lib.rs).

use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{broadcast, watch, Mutex};

use axiom_engram::{EngramStore, EngramStoreAdapter, QemCache};
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
}
