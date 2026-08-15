use crate::AppState;
use crate::errors;
use axum::{
    extract::State,
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/config", get(get_config).patch(patch_config))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedConfig {
    #[serde(default)]
    context: ContextConfig,
    #[serde(default)]
    schedule: ScheduleConfig,
    #[serde(default)]
    embedding: EmbeddingConfig,
    #[serde(default)]
    sync: SyncConfig,
    #[serde(default)]
    summarization: SummarizationConfig,
    #[serde(default)]
    qem: QemConfigSection,
    #[serde(default)]
    noise: NoiseConfig,
    #[serde(default)]
    digest: DigestConfig,
}

impl Default for PersistedConfig {
    fn default() -> Self {
        Self {
            context: ContextConfig::default(),
            schedule: ScheduleConfig::default(),
            embedding: EmbeddingConfig::default(),
            sync: SyncConfig::default(),
            summarization: SummarizationConfig::default(),
            qem: QemConfigSection::default(),
            noise: NoiseConfig::default(),
            digest: DigestConfig::default(),
        }
    }
}

/// Capture-side source policy. Sources listed here are dropped at the REST
/// route — no row, no QEM populate, no WebSocket event. Defaults to the two
/// sources proven to be transcript-redundant noise (Claude Code shell
/// commands); empty list restores capture-everything behavior.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct NoiseConfig {
    #[serde(default = "default_ignored_sources")]
    ignored_sources: Vec<String>,
}

fn default_ignored_sources() -> Vec<String> {
    vec!["ai-session".into(), "ai-tool".into()]
}

impl Default for NoiseConfig {
    fn default() -> Self {
        Self { ignored_sources: default_ignored_sources() }
    }
}

/// L1 holographic cache tuning. Only `warm_strength_min` is exposed for now —
/// the rest of the QemConfig fields keep their in-code defaults.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct QemConfigSection {
    /// Minimum strength for entries loaded into L1 during startup warm.
    /// Lower = warmer cache, higher = only strong memories cached.
    #[serde(default = "default_qem_warm_strength_min")]
    warm_strength_min: f64,
}

fn default_qem_warm_strength_min() -> f64 { 0.3 }

impl Default for QemConfigSection {
    fn default() -> Self {
        Self { warm_strength_min: 0.3 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ContextConfig {
    /// Default token budget for /context/assemble
    #[serde(default = "default_context_budget")]
    default_budget: usize,
    /// Fraction of budget reserved for high-priority memories (0.0–1.0)
    #[serde(default = "default_high_priority_reserve")]
    high_priority_reserve: f64,
    /// Max recent conversation turns to include
    #[serde(default = "default_max_recent_turns")]
    max_recent_turns: usize,
    /// Max engrams to retrieve per assembly
    #[serde(default = "default_max_engrams")]
    max_engrams: usize,
}

fn default_context_budget() -> usize { 8192 }
fn default_high_priority_reserve() -> f64 { 0.6 }
fn default_max_recent_turns() -> usize { 12 }
fn default_max_engrams() -> usize { 10 }

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            default_budget: 8192,
            high_priority_reserve: 0.6,
            max_recent_turns: 12,
            max_engrams: 10,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ScheduleConfig {
    #[serde(default = "default_decay_interval")]
    decay_interval_hours: u32,
    #[serde(default = "default_consolidation_interval")]
    consolidation_interval_hours: u32,
    #[serde(default = "default_true")]
    auto_decay: bool,
    #[serde(default = "default_true")]
    auto_consolidation: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EmbeddingConfig {
    #[serde(default = "default_embedding_model")]
    model: String,
    #[serde(default = "default_dimensions")]
    dimensions: u32,
    #[serde(default)]
    enabled: bool,
}

fn default_decay_interval() -> u32 { 1 }
fn default_consolidation_interval() -> u32 { 24 }
fn default_true() -> bool { true }
fn default_embedding_model() -> String { "text-embedding-3-small".into() }
fn default_dimensions() -> u32 { 1536 }

impl Default for ScheduleConfig {
    fn default() -> Self {
        Self {
            decay_interval_hours: 1,
            consolidation_interval_hours: 24,
            auto_decay: true,
            auto_consolidation: true,
        }
    }
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            model: "text-embedding-3-small".into(),
            dimensions: 1536,
            enabled: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SyncConfig {
    #[serde(default)]
    enabled: bool,
    /// Sync server URL (e.g., "https://sync.engram.ellmstack.dev")
    #[serde(default)]
    server_url: Option<String>,
    /// API key for sync server authentication
    #[serde(default)]
    api_key: Option<String>,
    /// Pull interval in seconds
    #[serde(default = "default_sync_interval")]
    interval_secs: u64,
    /// Shared-vault identity — devices pushing/pulling the same vault_id with
    /// the same passphrase form a team (Teams v0). Null = solo vault.
    #[serde(default)]
    vault_id: Option<String>,
    /// Human-readable vault/team name, shown in the Sync & Team panel.
    #[serde(default)]
    name: Option<String>,
}

fn default_sync_interval() -> u64 { 60 }

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            server_url: None,
            api_key: None,
            interval_secs: 60,
            vault_id: None,
            name: None,
        }
    }
}

/// How session summaries are generated at \`engram session stop\`.
/// Default: heuristic (structured grouping, zero AI calls, fully private).
/// Opt-in: set \`llm\` to \`"ollama:phi3:mini"\` (or any ollama model) for
/// LLM-generated narrative summaries — the call runs on the local ollama
/// instance; plaintext never leaves the machine.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SummarizationConfig {
    /// LLM model for session summarization, e.g. \`"ollama:phi3:mini"\`.
    /// When \`null\` (default), uses the heuristic summarizer.
    llm: Option<String>,
}

impl Default for SummarizationConfig {
    fn default() -> Self {
        Self { llm: None }
    }
}

/// Sync patch — every field optional so a partial PATCH only overwrites the
/// fields actually present (full `SyncConfig` replacement would null
/// `vault_id`/`name` whenever the UI saves a partial form).
#[derive(Debug, Deserialize)]
struct SyncPatch {
    /// The masked value from GET ("••••••••") round-trips unchanged; a real
    /// key here replaces the stored one.
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    server_url: Option<String>,
    #[serde(default)]
    interval_secs: Option<u64>,
    #[serde(default)]
    vault_id: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

/// Weekly digest settings. The digest core (stats + themes) is deterministic
/// and local — zero cost, works forever offline. The optional `llm` block
/// upgrades prose via a BYO-key OpenAI-compatible endpoint (a local Ollama
/// qualifies: base_url "http://localhost:11434/v1"). Prose is generated only
/// on the explicit `?prose=1` request flag — BYO-key calls bill the user's
/// own key, so they are never automatic.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct DigestConfig {
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default)]
    llm: Option<DigestLlmConfig>,
}

impl Default for DigestConfig {
    fn default() -> Self {
        Self { enabled: true, llm: None }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DigestLlmConfig {
    /// OpenAI-compatible base URL ending in /v1, e.g. "http://localhost:11434/v1"
    #[serde(default)]
    base_url: String,
    /// Provider API key. Masked in /config responses; never persisted when the
    /// masked value round-trips.
    #[serde(default)]
    api_key: Option<String>,
    /// Model name, e.g. "gpt-4o-mini" or "llama3.1"
    #[serde(default)]
    model: String,
}

impl Default for DigestLlmConfig {
    fn default() -> Self {
        Self { base_url: String::new(), api_key: None, model: String::new() }
    }
}

/// Digest patch — fields merge field-wise like `SyncPatch`. `llm` is a nested
/// option: absent = untouched, `null` = clear the whole block, object =
/// field-wise merge into it.
#[derive(Debug, Deserialize)]
struct DigestPatch {
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    llm: Option<Option<DigestLlmPatch>>,
}

#[derive(Debug, Deserialize)]
struct DigestLlmPatch {
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    model: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PatchConfigBody {
    context: Option<ContextConfig>,
    schedule: Option<ScheduleConfig>,
    embedding: Option<EmbeddingConfig>,
    sync: Option<SyncPatch>,
    summarization: Option<SummarizationConfig>,
    qem: Option<QemConfigSection>,
    noise: Option<NoiseConfig>,
    digest: Option<DigestPatch>,
}

fn config_path(vault_path: &std::path::Path) -> std::path::PathBuf {
    vault_path.join("config.json")
}

fn load_config(vault_path: &std::path::Path) -> Result<PersistedConfig, String> {
    let path = config_path(vault_path);
    if path.exists() {
        let s = std::fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read config at {}: {}", path.display(), e))?;
        serde_json::from_str(&s)
            .map_err(|e| format!("Config at {} is corrupted (invalid JSON): {}", path.display(), e))
    } else {
        Ok(PersistedConfig::default())
    }
}

fn save_config(vault_path: &std::path::Path, config: &PersistedConfig) -> std::io::Result<()> {
    let path = config_path(vault_path);
    let json = serde_json::to_string_pretty(config)?;
    std::fs::write(path, json)
}

#[derive(Debug, Serialize)]
struct ConfigResponse {
    vault_path: String,
    encryption: String,
    version: String,
    context: ContextConfig,
    schedule: ScheduleConfig,
    embedding: EmbeddingConfig,
    sync: SyncConfig,
    summarization: SummarizationConfig,
    qem: QemConfigSection,
    noise: NoiseConfig,
    digest: DigestConfig,
}

impl From<PersistedConfig> for ConfigResponse {
    fn from(c: PersistedConfig) -> Self {
        ConfigResponse {
            vault_path: String::new(),
            encryption: "SQLCipher AES-256 (SHA-256 key derivation)".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            context: c.context,
            schedule: c.schedule,
            embedding: c.embedding,
            sync: c.sync,
            summarization: c.summarization,
            qem: c.qem,
            noise: c.noise,
            digest: c.digest,
        }
    }
}

/// Mask every secret in a config response — sync api_key and digest llm
/// api_key are never returned in plaintext.
fn mask_secrets(resp: &mut ConfigResponse) {
    resp.sync.api_key = resp.sync.api_key.take().map(|_| "••••••••".into());
    if let Some(ref mut llm) = resp.digest.llm {
        llm.api_key = llm.api_key.take().map(|_| "••••••••".into());
    }
}

async fn get_config(
    State(state): State<AppState>,
) -> Result<Json<ConfigResponse>, (axum::http::StatusCode, Json<serde_json::Value>)> {
    let config = load_config(&state.vault_path).map_err(|e| {
        tracing::error!("{e}");
        errors::internal(errors::code::INTERNAL, "Failed to load configuration")
    })?;
    let mut resp: ConfigResponse = config.into();
    mask_secrets(&mut resp);
    resp.vault_path = state.vault_path.to_string_lossy().to_string();
    Ok(Json(resp))
}

async fn patch_config(
    State(state): State<AppState>,
    Json(body): Json<PatchConfigBody>,
) -> Result<Json<ConfigResponse>, (axum::http::StatusCode, Json<serde_json::Value>)> {
    let mut config = load_config(&state.vault_path).map_err(|e| {
        tracing::error!("{e}");
        (axum::http::StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e})))
    })?;
    if let Some(schedule) = body.schedule {
        config.schedule = schedule;
    }
    if let Some(context) = body.context {
        config.context = context;
    }
    if let Some(embedding) = body.embedding {
        config.embedding = embedding;
    }
    if let Some(sync) = body.sync {
        // Field-wise merge: a partial PATCH (e.g. just the team name) must
        // not erase vault_id or null other untouched fields. Never persist
        // the masked API key from a GET round-trip.
        if sync.api_key.as_deref() != Some("••••••••") {
            if let Some(k) = sync.api_key {
                config.sync.api_key = Some(k);
            }
        }
        if let Some(v) = sync.enabled {
            config.sync.enabled = v;
        }
        if let Some(v) = sync.server_url {
            config.sync.server_url = Some(v);
        }
        if let Some(v) = sync.interval_secs {
            config.sync.interval_secs = v;
        }
        if let Some(v) = sync.vault_id {
            config.sync.vault_id = Some(v);
        }
        if let Some(v) = sync.name {
            config.sync.name = Some(v);
        }
    }
    if let Some(summarization) = body.summarization {
        config.summarization = summarization;
    }
    if let Some(qem) = body.qem {
        config.qem = qem;
    }
    if let Some(noise) = body.noise {
        config.noise = noise;
    }
    if let Some(digest) = body.digest {
        // Field-wise merge like sync: a partial PATCH must not erase the
        // other digest fields. `llm: null` explicitly clears the block.
        if let Some(v) = digest.enabled {
            config.digest.enabled = v;
        }
        match digest.llm {
            None => {}
            Some(None) => config.digest.llm = None,
            Some(Some(llm)) => {
                let slot = config.digest.llm.get_or_insert_with(DigestLlmConfig::default);
                // Never persist the masked API key from a GET round-trip.
                if llm.api_key.as_deref() != Some("••••••••") {
                    if let Some(k) = llm.api_key {
                        slot.api_key = Some(k);
                    }
                }
                if let Some(v) = llm.base_url {
                    slot.base_url = v;
                }
                if let Some(v) = llm.model {
                    slot.model = v;
                }
            }
        }
    }
    save_config(&state.vault_path, &config)
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to save config");
            errors::internal(errors::code::INTERNAL, "Failed to save configuration to disk")
        })?;
    let mut resp: ConfigResponse = config.into();
    mask_secrets(&mut resp);
    resp.vault_path = state.vault_path.to_string_lossy().to_string();
    Ok(Json(resp))
}
