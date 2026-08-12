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
}

impl Default for PersistedConfig {
    fn default() -> Self {
        Self {
            context: ContextConfig::default(),
            schedule: ScheduleConfig::default(),
            embedding: EmbeddingConfig::default(),
            sync: SyncConfig::default(),
            summarization: SummarizationConfig::default(),
        }
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
}

fn default_sync_interval() -> u64 { 60 }

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            server_url: None,
            api_key: None,
            interval_secs: 60,
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

#[derive(Debug, Deserialize)]
struct PatchConfigBody {
    context: Option<ContextConfig>,
    schedule: Option<ScheduleConfig>,
    embedding: Option<EmbeddingConfig>,
    sync: Option<SyncConfig>,
    summarization: Option<SummarizationConfig>,
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
        }
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
    // Mask API key — never return it in plaintext
    resp.sync.api_key = resp.sync.api_key.map(|_| "••••••••".into());
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
        // Never persist the API key from a GET response (masked value)
        let mut s = sync;
        if s.api_key.as_deref() == Some("••••••••") {
            s.api_key = config.sync.api_key.clone();
        }
        config.sync = s;
    }
    if let Some(summarization) = body.summarization {
        config.summarization = summarization;
    }
    save_config(&state.vault_path, &config)
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to save config");
            errors::internal(errors::code::INTERNAL, "Failed to save configuration to disk")
        })?;
    // Mask API key in response
    let mut resp: ConfigResponse = config.into();
    resp.sync.api_key = resp.sync.api_key.map(|_| "••••••••".into());
    resp.vault_path = state.vault_path.to_string_lossy().to_string();
    Ok(Json(resp))
}
