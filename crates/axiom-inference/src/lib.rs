//! Axiom Inference - inference abstraction layer

#[cfg(feature = "onnx")]
pub mod onnx;

#[cfg(feature = "llama-cpp")]
pub mod llama_cpp;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::pin::Pin;
use std::sync::Arc;
use thiserror::Error;

/// A type-erased byte stream returned by `complete_chat_stream`.
/// Each item is a `Result<bytes::Bytes, InferenceError>`.
pub type InferenceStream = Pin<
    Box<dyn futures_util::Stream<Item = std::result::Result<bytes::Bytes, InferenceError>> + Send>,
>;

#[derive(Error, Debug)]
pub enum InferenceError {
    #[error("Provider error: {0}")]
    Provider(String),
    #[error("Invalid request: {0}")]
    InvalidRequest(String),
    #[error("Timeout")]
    Timeout,
}

pub type Result<T> = std::result::Result<T, InferenceError>;

/// Inference request
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct InferenceRequest {
    pub prompt: String,
    pub max_tokens: Option<usize>,
    pub temperature: Option<f32>,
    /// Override the provider's default model for this single request.
    pub model: Option<String>,
    /// Privacy level — determines whether the request may leave the device.
    #[serde(default)]
    pub privacy_level: Option<String>,
    /// Structured output JSON schema (Ollama 0.18+ structured outputs).
    #[serde(default)]
    pub json_schema: Option<serde_json::Value>,
    /// Thinking mode control: "none", "brief", "medium", "full".
    #[serde(default)]
    pub thinking: Option<String>,
    /// Challenge gradient tier (1-3) — controls response creativity/safety.
    /// Level 1: Conservative (low temp), Level 2: Balanced, Level 3: Creative (high temp)
    #[serde(default)]
    pub challenge_level: Option<u8>,
}

// ─────────────────────────────────────────────────── InferenceConfig ─────────

/// The kind of inference provider to use.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// OpenAI-compatible API (Ollama, vLLM, etc.)
    OpenAI,
    /// ONNX Runtime (local, CPU/NPU) — requires `onnx` feature
    #[cfg(feature = "onnx")]
    OnnxRuntime { model_path: String },
    /// LlamaCpp in-process GGUF inference — requires `llama-cpp` feature
    #[cfg(feature = "llama-cpp")]
    LlamaCpp {
        model_path: String,
        n_ctx: u32,
        n_gpu_layers: u32,
        n_threads: usize,
    },
}

impl Default for ProviderKind {
    fn default() -> Self {
        ProviderKind::OpenAI
    }
}

/// Cloud fallback configuration for when local inference is unavailable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudConfig {
    /// Cloud API endpoint (e.g. https://api.anthropic.com/v1)
    pub url: String,
    /// Cloud API key
    pub api_key: String,
    /// Cloud model name (e.g. claude-sonnet-4-20250514)
    pub model: String,
    /// Whether to use cloud as fallback when local is unavailable
    pub enabled: bool,
}

impl Default for CloudConfig {
    fn default() -> Self {
        Self {
            url: "https://api.anthropic.com/v1".to_string(),
            api_key: String::new(),
            model: "claude-sonnet-4-20250514".to_string(),
            enabled: false,
        }
    }
}

impl CloudConfig {
    pub fn from_env() -> Self {
        let api_key = std::env::var("AXIOM_CLOUD_API_KEY").unwrap_or_default();
        Self {
            url: std::env::var("AXIOM_CLOUD_URL")
                .unwrap_or_else(|_| "https://api.anthropic.com/v1".to_string()),
            api_key: api_key.clone(),
            model: std::env::var("AXIOM_CLOUD_MODEL")
                .unwrap_or_else(|_| "claude-sonnet-4-20250514".to_string()),
            enabled: !api_key.is_empty()
                || std::env::var("AXIOM_USE_CLOUD_FALLBACK")
                    .map(|v| v == "1" || v.to_lowercase() == "true")
                    .unwrap_or(false),
        }
    }

    /// Returns true if cloud fallback is configured and available.
    pub fn is_available(&self) -> bool {
        self.enabled && !self.api_key.is_empty()
    }

    /// Build a cloud provider from this config.
    pub fn build_provider(&self) -> Option<Arc<dyn InferenceProvider>> {
        if self.is_available() {
            Some(Arc::new(OpenAIProvider::new(
                self.url.clone(),
                self.api_key.clone(),
                self.model.clone(),
            )))
        } else {
            None
        }
    }
}

/// Snapshot of provider configuration — can be loaded from env, persisted, and
/// applied to a live `InferenceHandle` via `reconfigure()`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceConfig {
    pub base_url: String,
    pub api_key:  String,
    pub model:    String,
    #[serde(default)]
    pub provider: ProviderKind,
    /// Cloud fallback configuration (Layer 3.3)
    #[serde(default)]
    pub cloud: CloudConfig,
}

impl Default for InferenceConfig {
    fn default() -> Self {
        Self {
            base_url: "http://localhost:11434/v1".to_string(),
            api_key:  String::new(),
            model:    "qwen2.5:14b".to_string(),
            provider: ProviderKind::default(),
            cloud: CloudConfig::default(),
        }
    }
}

impl InferenceConfig {
    /// Read from `AXIOM_INFERENCE_URL / AXIOM_INFERENCE_KEY / AXIOM_INFERENCE_MODEL`;
    /// falls back to Ollama at localhost:11434.
    pub fn from_env() -> Self {
        Self {
            base_url: std::env::var("AXIOM_INFERENCE_URL")
                .unwrap_or_else(|_| "http://localhost:11434/v1".to_string()),
            api_key: std::env::var("AXIOM_INFERENCE_KEY").unwrap_or_default(),
            model: std::env::var("AXIOM_INFERENCE_MODEL")
                .unwrap_or_else(|_| "qwen2.5:14b".to_string()),
            provider: ProviderKind::default(),
            cloud: CloudConfig::from_env(),
        }
    }

    /// Instantiate a concrete provider from this config.
    pub fn build(&self) -> Arc<dyn InferenceProvider> {
        match &self.provider {
            ProviderKind::OpenAI => Arc::new(OpenAIProvider::new(
                self.base_url.clone(),
                self.api_key.clone(),
                self.model.clone(),
            )),
            #[cfg(feature = "onnx")]
            ProviderKind::OnnxRuntime { model_path } => {
                Arc::new(onnx::OnnxRuntimeProvider::new(model_path.clone()))
            }
            #[cfg(feature = "llama-cpp")]
            ProviderKind::LlamaCpp {
                model_path,
                n_ctx,
                n_gpu_layers,
                n_threads,
            } => Arc::new(
                llama_cpp::LlamaCppProvider::new(
                    model_path,
                    *n_ctx,
                    *n_gpu_layers,
                    *n_threads,
                )
                .expect("Failed to initialise LlamaCpp provider"),
            ),
        }
    }

    /// Build a provider with cloud fallback if configured (Layer 3.3).
    ///
    /// Returns a `FallbackProvider` wrapping local → cloud when cloud is
    /// available, or just the local provider when it's not.
    pub fn build_with_fallback(&self) -> Arc<dyn InferenceProvider> {
        let primary = self.build();
        match self.cloud.build_provider() {
            Some(cloud) => Arc::new(FallbackProvider::new(primary, cloud)),
            None => primary,
        }
    }
}

// ─────────────────────────────────────────────────── InferenceHandle ─────────

/// A live, hot-swappable provider wrapper.  `Arc<InferenceHandle>` can be
/// coerced to `Arc<dyn InferenceProvider>` and passed anywhere that expects an
/// inference backend — while still allowing runtime reconfiguration via
/// `reconfigure()` without restarting the daemon.
pub struct InferenceHandle {
    inner:  tokio::sync::RwLock<Arc<dyn InferenceProvider>>,
    /// Use std::sync so `default_model()` (sync trait method) can read it.
    config: std::sync::RwLock<InferenceConfig>,
}

impl InferenceHandle {
    pub fn new(config: InferenceConfig) -> Arc<Self> {
        let provider = config.build();
        Arc::new(Self {
            inner:  tokio::sync::RwLock::new(provider),
            config: std::sync::RwLock::new(config),
        })
    }

    /// Create an `InferenceHandle` with cloud fallback enabled (Layer 3.3).
    ///
    /// Builds a `FallbackProvider` chain (local → cloud) when cloud is
    /// configured, otherwise falls back to local-only. This is the preferred
    /// constructor for daemon startup.
    pub fn new_with_fallback(config: InferenceConfig) -> Arc<Self> {
        let provider = config.build_with_fallback();
        Arc::new(Self {
            inner:  tokio::sync::RwLock::new(provider),
            config: std::sync::RwLock::new(config),
        })
    }

    /// Snapshot of the current configuration (api_key is the live value).
    pub fn get_config(&self) -> InferenceConfig {
        self.config.read().unwrap().clone()
    }

    /// Atomically swap to a new provider built from `new_cfg`.
    /// All in-flight requests on the old provider complete normally.
    pub async fn reconfigure(&self, new_cfg: InferenceConfig) {
        let new_provider = new_cfg.build();
        *self.inner.write().await = new_provider;
        *self.config.write().unwrap() = new_cfg;
    }

    /// Replace the inner provider with a pre-built one, updating config.
    /// Useful for wrapping the provider (e.g. FallbackProvider) outside of
    /// the normal build path.
    pub async fn set_provider(&self, provider: Arc<dyn InferenceProvider>, cfg: InferenceConfig) {
        *self.inner.write().await = provider;
        *self.config.write().unwrap() = cfg;
    }

    /// Get a clone of the current inner provider, e.g. to wrap as a fallback.
    pub async fn get_provider(&self) -> Arc<dyn InferenceProvider> {
        Arc::clone(&*self.inner.read().await)
    }
}

#[async_trait]
impl InferenceProvider for InferenceHandle {
    async fn complete(&self, request: InferenceRequest) -> Result<InferenceResponse> {
        self.inner.read().await.complete(request).await
    }
    async fn embed(&self, text: &str) -> Result<Vec<f64>> {
        self.inner.read().await.embed(text).await
    }
    async fn list_models(&self) -> Result<Vec<String>> {
        self.inner.read().await.list_models().await
    }
    fn default_model(&self) -> String {
        self.config.read().unwrap().model.clone()
    }
    async fn complete_chat_stream(&self, request: ChatRequest) -> Result<InferenceStream> {
        self.inner.read().await.complete_chat_stream(request).await
    }
}

// ──────────────────────────────────────────── FallbackProvider ────────────────

/// Wraps a primary and fallback provider. If the primary fails, the fallback is
/// tried transparently. Used to make the bundled local model the primary while
/// keeping Ollama as a fallback for graceful degradation.
pub struct FallbackProvider {
    primary: Arc<dyn InferenceProvider>,
    fallback: Arc<dyn InferenceProvider>,
}

impl FallbackProvider {
    pub fn new(
        primary: Arc<dyn InferenceProvider>,
        fallback: Arc<dyn InferenceProvider>,
    ) -> Self {
        Self { primary, fallback }
    }
}

#[async_trait]
impl InferenceProvider for FallbackProvider {
    async fn complete(&self, request: InferenceRequest) -> Result<InferenceResponse> {
        match self.primary.complete(request.clone()).await {
            Ok(resp) => Ok(resp),
            Err(_) => self.fallback.complete(request).await,
        }
    }

    async fn complete_chat_stream(&self, request: ChatRequest) -> Result<InferenceStream> {
        match self.primary.complete_chat_stream(request.clone()).await {
            Ok(stream) => Ok(stream),
            Err(_) => self.fallback.complete_chat_stream(request).await,
        }
    }

    async fn embed(&self, text: &str) -> Result<Vec<f64>> {
        self.primary.embed(text).await
    }

    async fn list_models(&self) -> Result<Vec<String>> {
        self.primary.list_models().await
    }

    fn default_model(&self) -> String {
        self.primary.default_model()
    }
}

// ──────────────────────────────────────────── Local model probe helper ────────

/// Probe any OpenAI-compatible endpoint for available model IDs.
/// Returns an empty vec on any error (server down, timeout, etc.).
/// Useful for auto-discovering Ollama models.
pub async fn probe_local_models(base_url: &str) -> Vec<String> {
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
    {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let url = format!("{}/models", base_url.trim_end_matches('/'));
    match client.get(&url).send().await {
        Ok(resp) if resp.status().is_success() => {
            let payload: serde_json::Value =
                resp.json().await.unwrap_or(serde_json::Value::Null);
            OpenAIProvider::parse_model_ids_pub(&payload)
        }
        _ => Vec::new(),
    }
}

/// Inference response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceResponse {
    pub text: String,
    pub tokens: usize,
    pub finish_reason: String,
}

/// A single message in a chat conversation (OpenAI-compatible)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,       // "system" | "user" | "assistant" | "tool"
    pub content: String,
}

/// OpenAI-compatible tool definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    #[serde(rename = "type")]
    pub tool_type: String,  // always "function"
    pub function: serde_json::Value,  // { name, description, parameters }
}

/// Chat inference request (messages + optional tools)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRequest {
    pub messages: Vec<ChatMessage>,
    pub tools: Option<Vec<Tool>>,
    pub max_tokens: Option<usize>,
    pub temperature: Option<f32>,
    pub stream: Option<bool>,
    pub model: Option<String>,
    /// Privacy level — determines routing.
    #[serde(default)]
    pub privacy_level: Option<String>,
    /// Structured output JSON schema.
    #[serde(default)]
    pub json_schema: Option<serde_json::Value>,
    /// Thinking mode control for reasoning models.
    #[serde(default)]
    pub thinking: Option<String>,
    /// Enable web search (Ollama 0.18+).
    #[serde(default)]
    pub web_search: Option<bool>,
    /// Challenge gradient tier (1-3) — controls response creativity/safety.
    #[serde(default)]
    pub challenge_level: Option<u8>,
}

/// Inference provider trait
#[async_trait]
pub trait InferenceProvider: Send + Sync {
    async fn complete(&self, request: InferenceRequest) -> Result<InferenceResponse>;
    async fn embed(&self, text: &str) -> Result<Vec<f64>>;
    async fn list_models(&self) -> Result<Vec<String>>;
    fn default_model(&self) -> String;
    /// Chat completions with optional tool definitions. Returns a byte stream of SSE events.
    async fn complete_chat_stream(
        &self,
        request: ChatRequest,
    ) -> Result<InferenceStream>;
}

/// OpenAI-compatible provider
pub struct OpenAIProvider {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
    /// Configurable embedding model name (e.g., "nomic-embed-text" for local, "text-embedding-3-small" for cloud)
    embedding_model: String,
}

impl OpenAIProvider {
    pub fn new(base_url: String, api_key: String, model: String) -> Self {
        let embedding_model = std::env::var("AXIOM_EMBEDDING_MODEL")
            .unwrap_or_else(|_| "nomic-embed-text".to_string());
        Self {
            client: reqwest::Client::new(),
            base_url,
            api_key,
            model,
            embedding_model,
        }
    }

    /// Create with explicit embedding model override.
    pub fn with_embedding_model(mut self, embedding_model: String) -> Self {
        self.embedding_model = embedding_model;
        self
    }

    /// Public alias used by `probe_local_models`.
    pub fn parse_model_ids_pub(payload: &serde_json::Value) -> Vec<String> {
        Self::parse_model_ids(payload)
    }

    fn parse_model_ids(payload: &serde_json::Value) -> Vec<String> {
        let mut model_ids: Vec<String> = Vec::new();

        if let Some(items) = payload.get("data").and_then(|v| v.as_array()) {
            for item in items {
                if let Some(id) = item.get("id").and_then(|v| v.as_str()) {
                    model_ids.push(id.to_string());
                }
            }
        }

        if model_ids.is_empty() {
            if let Some(items) = payload.get("models").and_then(|v| v.as_array()) {
                for item in items {
                    if let Some(id) = item.as_str() {
                        model_ids.push(id.to_string());
                        continue;
                    }
                    if let Some(id) = item.get("id").and_then(|v| v.as_str()) {
                        model_ids.push(id.to_string());
                        continue;
                    }
                    if let Some(name) = item.get("name").and_then(|v| v.as_str()) {
                        model_ids.push(name.to_string());
                    }
                }
            }
        }

        model_ids.sort();
        model_ids.dedup();
        model_ids
    }

    /// Embed text with a specific model override.
    pub async fn embed_with_model(&self, text: &str, model: &str) -> Result<Vec<f64>> {
        let response = self.client
            .post(format!("{}/embeddings", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({
                "model": model,
                "input": text
            }))
            .send()
            .await
            .map_err(|e| InferenceError::Provider(e.to_string()))?;

        let data: serde_json::Value = response.json().await
            .map_err(|e| InferenceError::Provider(e.to_string()))?;

        let embedding = data["data"][0]["embedding"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .map(|v| v.as_f64().unwrap_or(0.0))
            .collect();

        Ok(embedding)
    }

    /// Use Ollama's native `/api/chat` endpoint for streaming.
    /// This endpoint correctly respects `"think": false` (unlike `/v1/chat/completions`).
    /// Returns an `InferenceStream` that emits SSE-formatted bytes so the caller
    /// (daemon → frontend) doesn't need to know about the format difference.
    async fn complete_chat_stream_ollama_native(
        &self,
        model: &str,
        request: &ChatRequest,
        temperature: f32,
        think: bool,
    ) -> Result<InferenceStream> {
        // Ollama native /api/chat uses the base host, not /v1.
        let ollama_base = self
            .base_url
            .replace("/v1", "")
            .trim_end_matches('/')
            .to_string();

        let mut body = serde_json::json!({
            "model": model,
            "messages": request.messages,
            "stream": true,
            "think": think,
            "options": {
                "temperature": temperature,
                "num_predict": request.max_tokens.unwrap_or(1024)
            }
        });

        // Tools passthrough
        if let Some(tools) = &request.tools {
            if !tools.is_empty() {
                body["tools"] = serde_json::to_value(tools)
                    .map_err(|e| InferenceError::Provider(e.to_string()))?;
            }
        }

        // Structured output
        if let Some(schema) = &request.json_schema {
            body["format"] = schema.clone();
        }

        let resp = self
            .client
            .post(format!("{}/api/chat", ollama_base))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| InferenceError::Provider(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(InferenceError::Provider(format!(
                "Ollama /api/chat failed (HTTP {}): {}",
                status, text
            )));
        }

        // Ollama native streaming returns newline-delimited JSON:
        //   {"message":{"role":"assistant","content":"Hello"},"done":false}\n
        //   {"message":{"role":"assistant","content":"!"},"done":false}\n
        //   {"done":true,"total_duration":...}\n
        //
        // Convert to SSE format that the daemon/frontend expects:
        //   data: {"choices":[{"delta":{"content":"Hello"}}]}\n\n
        //   data: {"choices":[{"delta":{"content":"!"}}]}\n\n
        //   data: [DONE]\n\n

        use futures_util::StreamExt;

        let stream = resp.bytes_stream();
        let sse_stream = {
            let mut line_buf = String::new();
            stream.flat_map(move |chunk_result| {
                let mut sse_events: Vec<std::result::Result<bytes::Bytes, InferenceError>> =
                    Vec::new();
                match chunk_result {
                    Err(e) => {
                        sse_events
                            .push(Err(InferenceError::Provider(e.to_string())));
                    }
                    Ok(chunk) => {
                        let text = String::from_utf8_lossy(&chunk);
                        line_buf.push_str(&text);

                        while let Some(newline_pos) = line_buf.find('\n') {
                            let line: String = line_buf.drain(..=newline_pos).collect();
                            let line = line.trim();
                            if line.is_empty() {
                                continue;
                            }

                            if let Ok(obj) =
                                serde_json::from_str::<serde_json::Value>(line)
                            {
                                if obj.get("done") == Some(&serde_json::json!(true)) {
                                    let done_bytes =
                                        bytes::Bytes::from("data: [DONE]\n\n");
                                    sse_events.push(Ok(done_bytes));
                                } else if let Some(msg) = obj.get("message") {
                                    let content = msg
                                        .get("content")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("");
                                    let sse_payload = serde_json::json!({
                                        "choices": [{
                                            "delta": { "content": content }
                                        }]
                                    });
                                    let sse_line = format!(
                                        "data: {}\n\n",
                                        serde_json::to_string(&sse_payload)
                                            .unwrap_or_default()
                                    );
                                    sse_events
                                        .push(Ok(bytes::Bytes::from(sse_line)));
                                }
                            }
                        }
                    }
                }
                futures_util::stream::iter(sse_events)
            })
        };

        Ok(Box::pin(sse_stream))
    }
}

#[async_trait]
impl InferenceProvider for OpenAIProvider {
    async fn complete(&self, request: InferenceRequest) -> Result<InferenceResponse> {
        let model = request.model.as_deref().unwrap_or(&self.model);
        let think_enabled = request.thinking.as_ref()
            .map(|t| !matches!(t.to_lowercase().as_str(), "off" | "false" | "no" | "0" | "disabled"))
            .unwrap_or(false);

        let is_local_ollama = self.base_url.contains("localhost:11434")
            || self.base_url.contains("127.0.0.1:11434");

        let data: serde_json::Value = if is_local_ollama {
            // Native Ollama /api/chat — respects think: false
            let ollama_base = self.base_url.replace("/v1", "").trim_end_matches('/').to_string();
            let response = self.client
                .post(format!("{}/api/chat", ollama_base))
                .header("Content-Type", "application/json")
                .json(&serde_json::json!({
                    "model": model,
                    "messages": [{"role": "user", "content": request.prompt}],
                    "stream": false,
                    "think": think_enabled,
                    "options": {
                        "temperature": request.temperature.unwrap_or(0.7),
                        "num_predict": request.max_tokens.unwrap_or(1024)
                    }
                }))
                .send()
                .await
                .map_err(|e| InferenceError::Provider(e.to_string()))?;
            if !response.status().is_success() {
                let status = response.status();
                let text = response.text().await.unwrap_or_default();
                return Err(InferenceError::Provider(format!(
                    "Ollama /api/chat failed (HTTP {}): {}",
                    status, text
                )));
            }
            let native: serde_json::Value = response.json().await
                .map_err(|e| InferenceError::Provider(e.to_string()))?;
            // Normalize native response to OpenAI shape for uniform extraction below
            serde_json::json!({
                "choices": [{
                    "message": { "content": native["message"]["content"].as_str().unwrap_or("") },
                    "finish_reason": if native["done"].as_bool() == Some(true) { "stop" } else { "length" }
                }],
                "usage": {
                    "total_tokens": native["eval_count"].as_u64().unwrap_or(0)
                        + native["prompt_eval_count"].as_u64().unwrap_or(0)
                }
            })
        } else {
            // Standard OpenAI-compatible path
            let response = self.client
                .post(format!("{}/chat/completions", self.base_url))
                .header("Authorization", format!("Bearer {}", self.api_key))
                .header("Content-Type", "application/json")
                .json(&serde_json::json!({
                    "model": model,
                    "messages": [{"role": "user", "content": request.prompt}],
                    "max_tokens": request.max_tokens.unwrap_or(1024),
                    "temperature": request.temperature.unwrap_or(0.7),
                    "think": think_enabled
                }))
                .send()
                .await
                .map_err(|e| InferenceError::Provider(e.to_string()))?;
            if !response.status().is_success() {
                let status = response.status();
                let text = response.text().await.unwrap_or_default();
                return Err(InferenceError::Provider(format!(
                    "chat/completions failed (HTTP {}): {}",
                    status, text
                )));
            }
            response.json().await
                .map_err(|e| InferenceError::Provider(e.to_string()))?
        };

        let text = data["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string();

        let tokens = data["usage"]["total_tokens"].as_u64().unwrap_or(0) as usize;
        let finish_reason = data["choices"][0]["finish_reason"]
            .as_str()
            .unwrap_or("stop")
            .to_string();

        Ok(InferenceResponse {
            text,
            tokens,
            finish_reason,
        })
    }

    async fn complete_chat_stream(
        &self,
        request: ChatRequest,
    ) -> Result<InferenceStream> {
        let model = request.model.as_deref().unwrap_or(&self.model);
        let base_temp = request.temperature.unwrap_or(0.7);
        let effective_temp = temperature_from_challenge(request.challenge_level, base_temp);

        // Thinking mode for reasoning models — default OFF for CPU performance.
        let think_enabled = request.thinking.as_ref()
            .map(|t| !matches!(t.to_lowercase().as_str(), "off" | "false" | "no" | "0" | "disabled"))
            .unwrap_or(false);

        // ── Detect local Ollama and use native /api/chat ────────────────
        // Ollama's OpenAI-compatible /v1/chat/completions endpoint ignores
        // the `think` parameter.  Only the native /api/chat respects it.
        // We detect Ollama by checking if the base_url points to port 11434.
        let is_local_ollama = self.base_url.contains("localhost:11434")
            || self.base_url.contains("127.0.0.1:11434");

        if is_local_ollama {
            return self
                .complete_chat_stream_ollama_native(
                    model,
                    &request,
                    effective_temp,
                    think_enabled,
                )
                .await;
        }

        // ── Standard OpenAI-compatible path ─────────────────────────────
        let mut body = serde_json::json!({
            "model": model,
            "messages": request.messages,
            "stream": true,
            "max_tokens": request.max_tokens.unwrap_or(1024),
            "temperature": effective_temp
        });

        if let Some(tools) = &request.tools {
            if !tools.is_empty() {
                body["tools"] = serde_json::to_value(tools)
                    .map_err(|e| InferenceError::Provider(e.to_string()))?;
            }
        }

        if let Some(schema) = &request.json_schema {
            body["format"] = schema.clone();
        }

        body["think"] = serde_json::json!(think_enabled);

        if request.web_search == Some(true) {
            body["web_search"] = serde_json::json!(true);
        }

        let resp = self.client
            .post(format!("{}/chat/completions", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| InferenceError::Provider(e.to_string()))?;

        // Wrap reqwest byte stream into our InferenceStream type.
        use futures_util::StreamExt;
        let stream = resp.bytes_stream().map(|result| {
            result.map_err(|e| InferenceError::Provider(e.to_string()))
        });
        Ok(Box::pin(stream))
    }

    async fn list_models(&self) -> Result<Vec<String>> {
        let url = format!("{}/models", self.base_url.trim_end_matches('/'));
        let mut req = self
            .client
            .get(url)
            .header("Content-Type", "application/json");

        if !self.api_key.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", self.api_key));
        }

        let response = req
            .send()
            .await
            .map_err(|e| InferenceError::Provider(e.to_string()))?;

        if !response.status().is_success() {
            return Err(InferenceError::Provider(format!(
                "model list request failed: HTTP {}",
                response.status()
            )));
        }

        let payload: serde_json::Value = response
            .json()
            .await
            .map_err(|e| InferenceError::Provider(e.to_string()))?;

        let mut model_ids = Self::parse_model_ids(&payload);
        if model_ids.is_empty() {
            model_ids.push(self.model.clone());
        }
        Ok(model_ids)
    }

    fn default_model(&self) -> String {
        self.model.clone()
    }

    async fn embed(&self, text: &str) -> Result<Vec<f64>> {
        let response = self.client
            .post(format!("{}/embeddings", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({
                "model": self.embedding_model,
                "input": text
            }))
            .send()
            .await
            .map_err(|e| InferenceError::Provider(e.to_string()))?;
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(InferenceError::Provider(format!(
                "embeddings failed (HTTP {}): {}",
                status, text
            )));
        }

        let data: serde_json::Value = response.json().await
            .map_err(|e| InferenceError::Provider(e.to_string()))?;

        let embedding = data["data"][0]["embedding"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .map(|v| v.as_f64().unwrap_or(0.0))
            .collect();

        Ok(embedding)
    }
}

// ───────────────────────────────── Cloud model helpers ────────────────────────

/// Returns `true` if the model ID uses Ollama's `:cloud` tag,
/// meaning inference runs on a remote provider rather than locally.
pub fn is_cloud_model(model: &str) -> bool {
    model.ends_with(":cloud") || model.contains(":cloud-")
}

/// Returns true if the model is an embedding-only model that cannot handle chat.
/// These models must never be selected for chat/completion requests.
pub fn is_embedding_model(model: &str) -> bool {
    let lower = model.to_lowercase();
    lower.contains("embed") || lower.contains("nomic") || lower.starts_with("e5-")
}

/// Recommend a model based on task complexity and privacy level.
/// Returns (model_id, is_cloud).
pub fn recommend_model(
    task_hint: &str,
    privacy_level: Option<&str>,
    available_models: &[String],
) -> (String, bool) {
    let is_strict_local = privacy_level == Some("strict_local");

    // Filter out cloud models if strict_local, and always exclude embedding-only models.
    let candidates: Vec<&String> = if is_strict_local {
        available_models
            .iter()
            .filter(|m| !is_cloud_model(m) && !is_embedding_model(m))
            .collect()
    } else {
        available_models
            .iter()
            .filter(|m| !is_embedding_model(m))
            .collect()
    };

    // Simple heuristic: prefer larger models for complex tasks
    let complex = task_hint.contains("research")
        || task_hint.contains("analysis")
        || task_hint.contains("code")
        || task_hint.contains("reason");

    if candidates.is_empty() {
        return ("llama3".to_string(), false);
    }

    // Prefer cloud for complex tasks when allowed
    if complex && !is_strict_local {
        if let Some(cloud) = candidates.iter().find(|m| is_cloud_model(m)) {
            return (cloud.to_string(), true);
        }
    }

    (candidates[0].to_string(), is_cloud_model(candidates[0]))
}

/// Calculate temperature from challenge gradient level.
/// Level 1 (Conservative): Low temperature = focused, safe responses
/// Level 2 (Balanced): Medium temperature = default behavior  
/// Level 3 (Creative): High temperature = more creative, varied responses
pub fn temperature_from_challenge(level: Option<u8>, default: f32) -> f32 {
    match level {
        Some(1) => 0.3,  // Conservative - focused, deterministic
        Some(2) => 0.6,  // Balanced
        Some(3) => 0.9,  // Creative - varied, exploratory
        Some(l) if l > 3 => 0.9,  // Cap at creative
        _ => default,    // Use provided default
    }
}

// ──────────────────────────────────── Task Planner ───────────────────────────

/// System prompt used for the planning decomposition turn.
const PLANNER_SYSTEM: &str = "\
You are a task planner. Break the user's request into a numbered list of \
concrete, independent steps. Be concise — one line per step. \
Do NOT execute anything. Output ONLY the numbered list, nothing else.";

/// For tasks above this complexity, prepend a planning decomposition turn.
pub const PLAN_COMPLEXITY_THRESHOLD: f64 = 0.5;

/// Build the prompt text for a planning turn.
/// Returns `None` if the task is simple enough to skip planning.
pub fn planning_prompt(task: &str) -> Option<String> {
    if analyze_task_complexity(task) >= PLAN_COMPLEXITY_THRESHOLD {
        Some(format!(
            "[system]\n{}\n\n[user]\n{}",
            PLANNER_SYSTEM, task
        ))
    } else {
        None
    }
}

// ──────────────────────────────────── Model Router ────────────────────────────
/// Routes requests to the best available model based on task complexity,
/// privacy requirements, and available models. Supports local-first routing
/// with cloud fallback for complex tasks.

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRoute {
    pub model: String,
    pub is_cloud: bool,
    pub reasoning: String,
    pub complexity_score: f64,
}

/// Analyze task complexity from the prompt text.
/// Returns a score from 0.0 (trivial) to 1.0 (highly complex).
pub fn analyze_task_complexity(prompt: &str) -> f64 {
    analyze_task_complexity_with_coherence(prompt, None)
}

/// Coherence signal for model routing decisions.
/// When available, adjusts complexity scoring based on the companion's emotional state.
#[derive(Debug, Clone)]
pub struct CoherenceSignal {
    /// Current valence (-1.0 to 1.0). Negative = user seems frustrated/stressed.
    pub current_valence: f64,
    /// Drift from baseline (0.0 = stable, higher = more drift).
    pub drift_score: f64,
}

/// Analyze task complexity with optional coherence awareness (Improvement #6).
///
/// When the user is experiencing high drift or negative valence, the system
/// should prefer more capable (often cloud) models to provide better support.
pub fn analyze_task_complexity_with_coherence(prompt: &str, coherence: Option<&CoherenceSignal>) -> f64 {
    let mut score: f64 = 0.0;
    let lower = prompt.to_lowercase();

    // Length heuristic
    if prompt.len() > 2000 { score += 0.2; }
    if prompt.len() > 5000 { score += 0.1; }

    // Keyword indicators
    let complexity_keywords = [
        ("reason", 0.15), ("analyze", 0.15), ("research", 0.15),
        ("architecture", 0.1), ("design", 0.1), ("debug", 0.1),
        ("refactor", 0.1), ("explain", 0.05), ("compare", 0.1),
        ("implement", 0.05), ("algorithm", 0.15), ("optimize", 0.15),
        ("security", 0.1), ("test", 0.05), ("review", 0.05),
        ("plan", 0.1), ("strategy", 0.15), ("system", 0.05),
    ];
    for (keyword, weight) in complexity_keywords {
        if lower.contains(keyword) { score += weight; }
    }

    // Code indicators
    if lower.contains("```") || lower.contains("function") || lower.contains("class ") {
        score += 0.1;
    }

    // Multi-step indicators
    if lower.contains("step") || lower.contains("first") || lower.contains("then") {
        score += 0.05;
    }

    // ── Coherence-aware adjustment (Improvement #6) ──────────────────────
    // When the user is in a stressed/frustrated state (negative valence or high drift),
    // boost complexity to route to more capable models for better support.
    if let Some(cs) = coherence {
        // High drift → boost complexity (more capable model for stability)
        if cs.drift_score > 0.3 {
            score += cs.drift_score * 0.15;
        }
        // Negative valence → the user may be struggling, use stronger model
        if cs.current_valence < -0.1 {
            score += (-cs.current_valence) * 0.1;
        }
    }

    score.min(1.0)
}

/// Route a request to the best model.
pub fn route_request(
    prompt: &str,
    privacy_level: Option<&str>,
    available_models: &[String],
    prefer_local: bool,
) -> ModelRoute {
    route_request_with_coherence(prompt, privacy_level, available_models, prefer_local, None)
}

/// Route a request to the best model, optionally factoring in coherence state.
pub fn route_request_with_coherence(
    prompt: &str,
    privacy_level: Option<&str>,
    available_models: &[String],
    prefer_local: bool,
    coherence: Option<&CoherenceSignal>,
) -> ModelRoute {
    let complexity = analyze_task_complexity_with_coherence(prompt, coherence);
    let is_strict_local = privacy_level == Some("strict_local") || prefer_local;

    // Filter candidates: always exclude embedding-only models from chat routing,
    // and apply the local/cloud preference on top.
    let candidates: Vec<&String> = if is_strict_local {
        available_models
            .iter()
            .filter(|m| !is_cloud_model(m) && !is_embedding_model(m))
            .collect()
    } else {
        available_models
            .iter()
            .filter(|m| !is_embedding_model(m))
            .collect()
    };

    if candidates.is_empty() {
        // Fall back to the configured inference model rather than a hard-coded default.
        let fallback = std::env::var("AXIOM_INFERENCE_MODEL")
            .unwrap_or_else(|_| "llama3.1:8b".to_string());
        return ModelRoute {
            model: fallback,
            is_cloud: false,
            reasoning: "No chat-capable models available, using configured fallback".to_string(),
            complexity_score: complexity,
        };
    }

    // Always prefer the configured inference model when available.
    let preferred = std::env::var("AXIOM_INFERENCE_MODEL")
        .unwrap_or_else(|_| "llama3.1:8b".to_string());
    if let Some(pref) = candidates.iter().find(|m| m.as_str() == preferred.as_str()) {
        return ModelRoute {
            model: pref.to_string(),
            is_cloud: false,
            reasoning: format!("Preferred model '{}' selected", preferred),
            complexity_score: complexity,
        };
    }

    // For complex tasks, prefer cloud models when allowed
    if complexity > 0.6 && !is_strict_local {
        if let Some(cloud) = candidates.iter().find(|m| is_cloud_model(m)) {
            return ModelRoute {
                model: cloud.to_string(),
                is_cloud: true,
                reasoning: format!("Complex task (score {:.1}) routed to cloud model", complexity),
                complexity_score: complexity,
            };
        }
    }

    // For medium tasks, prefer larger local models
    if complexity > 0.3 {
        for model in &candidates {
            let lower = model.to_lowercase();
            if lower.contains("70b") || lower.contains("72b") || lower.contains("large") {
                return ModelRoute {
                    model: model.to_string(),
                    is_cloud: is_cloud_model(model),
                    reasoning: format!("Medium task (score {:.1}) routed to larger model", complexity),
                    complexity_score: complexity,
                };
            }
        }
    }

    // Default: use first available model
    ModelRoute {
        model: candidates[0].to_string(),
        is_cloud: is_cloud_model(candidates[0]),
        reasoning: format!("Simple task (score {:.1}) using default model", complexity),
        complexity_score: complexity,
    }
}

// ──────────────────────────────────── Prompt Cache ────────────────────────────
/// Caches system prompts and repeated context to reduce token usage.
/// Uses content-addressable hashing for cache keys.

use std::collections::HashMap as StdHashMap;

pub struct PromptCache {
    entries: StdHashMap<String, CachedPrompt>,
    max_entries: usize,
}

#[derive(Clone)]
struct CachedPrompt {
    #[allow(dead_code)]
    content: String,
    token_estimate: usize,
    hit_count: u64,
}

impl PromptCache {
    pub fn new(max_entries: usize) -> Self {
        Self {
            entries: StdHashMap::new(),
            max_entries,
        }
    }

    /// Get a cached prompt by content hash. Returns None on miss.
    pub fn get(&mut self, content: &str) -> Option<usize> {
        let key = Self::hash(content);
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.hit_count += 1;
            Some(entry.token_estimate)
        } else {
            None
        }
    }

    /// Insert or update a cached prompt.
    pub fn insert(&mut self, content: &str, token_estimate: usize) {
        let key = Self::hash(content);
        if self.entries.len() >= self.max_entries {
            if let Some(evict_key) = self.entries.iter()
                .min_by_key(|(_, v)| v.hit_count)
                .map(|(k, _)| k.clone())
            {
                self.entries.remove(&evict_key);
            }
        }
        self.entries.insert(key, CachedPrompt {
            content: content.to_string(),
            token_estimate,
            hit_count: 1,
        });
    }

    /// Clear the cache.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    fn hash(content: &str) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        content.hash(&mut hasher);
        format!("{:x}", hasher.finish())
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_cloud_model_identifies_cloud_tags() {
        assert!(is_cloud_model("nemotron-3-super:cloud"));
        assert!(is_cloud_model("qwen3-coder:cloud-latest"));
        assert!(!is_cloud_model("llama3:8b"));
        assert!(!is_cloud_model("qwen3:4b"));
    }

    #[test]
    fn recommend_model_strict_local_excludes_cloud() {
        let available = vec![
            "llama3:8b".to_string(),
            "nemotron:cloud".to_string(),
            "qwen3:4b".to_string(),
        ];
        let (model, is_cloud) = recommend_model("chat", Some("strict_local"), &available);
        assert!(!is_cloud);
        assert!(!model.contains(":cloud"));
    }

    #[test]
    fn recommend_model_prefers_cloud_for_complex_tasks() {
        let available = vec![
            "llama3:8b".to_string(),
            "nemotron:cloud".to_string(),
        ];
        let (model, is_cloud) = recommend_model("research analysis", None, &available);
        assert!(is_cloud);
        assert_eq!(model, "nemotron:cloud");
    }

    #[test]
    fn recommend_model_fallback_on_empty() {
        let available: Vec<String> = vec![];
        let (model, _) = recommend_model("chat", None, &available);
        assert_eq!(model, "llama3");
    }

    #[test]
    fn inference_config_default_uses_ollama() {
        let cfg = InferenceConfig::default();
        assert!(cfg.base_url.contains("11434"));
        assert_eq!(cfg.model, "llama3.1:8b");
    }

    #[test]
    fn inference_request_defaults() {
        let req = InferenceRequest::default();
        assert!(req.prompt.is_empty());
        assert!(req.max_tokens.is_none());
        assert!(req.privacy_level.is_none());
        assert!(req.json_schema.is_none());
        assert!(req.thinking.is_none());
    }

    #[test]
    fn chat_request_structured_output_fields() {
        let req = ChatRequest {
            messages: vec![ChatMessage { role: "user".into(), content: "hello".into() }],
            tools: None,
            max_tokens: Some(100),
            temperature: Some(0.5),
            stream: Some(true),
            model: Some("nemotron:cloud".into()),
            privacy_level: Some("cloud_first".into()),
            json_schema: Some(serde_json::json!({"type": "object"})),
            thinking: Some("medium".into()),
            web_search: Some(true),
            challenge_level: None,
        };
        assert_eq!(req.model.as_deref(), Some("nemotron:cloud"));
        assert_eq!(req.thinking.as_deref(), Some("medium"));
        assert_eq!(req.web_search, Some(true));
    }

    #[test]
    fn openai_provider_model_id_parsing() {
        let payload = serde_json::json!({
            "data": [
                {"id": "llama3:8b"},
                {"id": "qwen3:4b"},
            ]
        });
        let ids = OpenAIProvider::parse_model_ids_pub(&payload);
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"llama3:8b".to_string()));
        assert!(ids.contains(&"qwen3:4b".to_string()));
    }

    #[test]
    fn openai_provider_model_id_parsing_ollama_format() {
        let payload = serde_json::json!({
            "models": [
                {"name": "gemma3:1b"},
                {"name": "deepseek-r1:14b"},
            ]
        });
        let ids = OpenAIProvider::parse_model_ids_pub(&payload);
        assert_eq!(ids.len(), 2);
    }

    #[test]
    fn openai_provider_model_id_deduplication() {
        let payload = serde_json::json!({
            "data": [
                {"id": "llama3:8b"},
                {"id": "llama3:8b"},
                {"id": "qwen3:4b"},
            ]
        });
        let ids = OpenAIProvider::parse_model_ids_pub(&payload);
        assert_eq!(ids.len(), 2);
    }

    #[test]
    fn temperature_from_challenge_level_1() {
        assert!((temperature_from_challenge(Some(1), 0.7) - 0.3).abs() < f32::EPSILON);
    }

    #[test]
    fn temperature_from_challenge_level_2() {
        assert!((temperature_from_challenge(Some(2), 0.7) - 0.6).abs() < f32::EPSILON);
    }

    #[test]
    fn temperature_from_challenge_level_3() {
        assert!((temperature_from_challenge(Some(3), 0.7) - 0.9).abs() < f32::EPSILON);
    }

    #[test]
    fn temperature_from_challenge_high_levels_cap() {
        assert!((temperature_from_challenge(Some(5), 0.7) - 0.9).abs() < f32::EPSILON);
    }

    #[test]
    fn temperature_from_challenge_none_uses_default() {
        assert!((temperature_from_challenge(None, 0.7) - 0.7).abs() < f32::EPSILON);
        assert!((temperature_from_challenge(None, 0.4) - 0.4).abs() < f32::EPSILON);
    }

    #[test]
    fn analyze_task_complexity_simple() {
        let score = analyze_task_complexity("hello");
        assert!(score < 0.3, "simple greeting should be low complexity");
    }

    #[test]
    fn analyze_task_complexity_code_review() {
        let score = analyze_task_complexity("Please analyze and refactor this architecture to optimize the security algorithm");
        assert!(score > 0.5, "complex code task should be high complexity");
    }

    #[test]
    fn analyze_task_complexity_caps_at_one() {
        let score = analyze_task_complexity("reason analyze research architecture design debug refactor algorithm optimize security test review plan strategy system");
        assert!(score <= 1.0, "complexity should never exceed 1.0");
    }

    #[test]
    fn route_request_prefers_local_for_simple() {
        let models = vec!["llama3:8b".to_string(), "nemotron:cloud".to_string()];
        let route = route_request("hello", None, &models, false);
        assert_eq!(route.model, "llama3:8b");
        assert!(!route.is_cloud);
    }

    #[test]
    fn route_request_prefers_cloud_for_complex() {
        let models = vec!["llama3:8b".to_string(), "nemotron:cloud".to_string()];
        let route = route_request("Please analyze this complex architecture and reason about the algorithm design", None, &models, false);
        assert!(route.is_cloud);
    }

    #[test]
    fn route_request_respects_strict_local() {
        let models = vec!["llama3:8b".to_string(), "nemotron:cloud".to_string()];
        let route = route_request("analyze this complex architecture", Some("strict_local"), &models, false);
        assert!(!route.is_cloud);
        assert_eq!(route.model, "llama3:8b");
    }

    #[test]
    fn route_request_empty_models_falls_back() {
        let models: Vec<String> = vec![];
        let route = route_request("hello", None, &models, false);
        assert_eq!(route.model, "llama3.1:8b");
    }

    #[test]
    fn prompt_cache_miss_returns_none() {
        let mut cache = PromptCache::new(10);
        assert!(cache.get("not cached").is_none());
    }

    #[test]
    fn prompt_cache_hit_returns_value() {
        let mut cache = PromptCache::new(10);
        cache.insert("system prompt", 100);
        assert_eq!(cache.get("system prompt"), Some(100));
    }

    #[test]
    fn prompt_cache_evicts_least_hit() {
        let mut cache = PromptCache::new(2);
        cache.insert("a", 10);
        cache.insert("b", 20);
        cache.get("a");
        cache.insert("c", 30);
        assert!(cache.get("a").is_some());
        assert!(cache.get("b").is_none());
        assert!(cache.get("c").is_some());
    }

    #[test]
    fn prompt_cache_clear() {
        let mut cache = PromptCache::new(10);
        cache.insert("a", 10);
        cache.insert("b", 20);
        cache.clear();
        assert!(cache.get("a").is_none());
        assert!(cache.get("b").is_none());
    }

    // ── FallbackProvider tests ───────────────────────────────────────────

    /// An always-failing provider used to test fallback behavior.
    struct FailingProvider;
    #[async_trait]
    impl InferenceProvider for FailingProvider {
        async fn complete(&self, _request: InferenceRequest) -> Result<InferenceResponse> {
            Err(InferenceError::Provider("simulated failure".into()))
        }
        async fn complete_chat_stream(&self, _request: ChatRequest) -> Result<InferenceStream> {
            Err(InferenceError::Provider("simulated failure".into()))
        }
        async fn embed(&self, _text: &str) -> Result<Vec<f64>> {
            Ok(vec![])
        }
        async fn list_models(&self) -> Result<Vec<String>> {
            Ok(vec![])
        }
        fn default_model(&self) -> String {
            "failing".into()
        }
    }

    /// A simple provider that always returns a fixed response.
    struct OkProvider(String);
    #[async_trait]
    impl InferenceProvider for OkProvider {
        async fn complete(&self, _request: InferenceRequest) -> Result<InferenceResponse> {
            Ok(InferenceResponse { text: self.0.clone(), tokens: 1, finish_reason: "stop".into() })
        }
        async fn complete_chat_stream(&self, _request: ChatRequest) -> Result<InferenceStream> {
            use futures_util::stream;
            let bytes = bytes::Bytes::from("data: [DONE]\n\n");
            Ok(Box::pin(stream::once(async move { Ok(bytes) })))
        }
        async fn embed(&self, _text: &str) -> Result<Vec<f64>> { Ok(vec![]) }
        async fn list_models(&self) -> Result<Vec<String>> { Ok(vec![]) }
        fn default_model(&self) -> String { self.0.clone() }
    }

    #[tokio::test]
    async fn fallback_provider_switches_on_primary_failure() {
        let fallback = FallbackProvider::new(
            Arc::new(FailingProvider),
            Arc::new(OkProvider("fallback-response".into())),
        );
        let resp = fallback.complete(InferenceRequest::default()).await.unwrap();
        assert_eq!(&resp.text, "fallback-response");
    }

    #[tokio::test]
    async fn fallback_provider_uses_primary_when_healthy() {
        let fallback = FallbackProvider::new(
            Arc::new(OkProvider("primary-response".into())),
            Arc::new(OkProvider("fallback-response".into())),
        );
        let resp = fallback.complete(InferenceRequest::default()).await.unwrap();
        assert_eq!(&resp.text, "primary-response");
    }

    #[tokio::test]
    async fn fallback_provider_delegates_embed_to_primary() {
        let fallback = FallbackProvider::new(
            Arc::new(OkProvider("primary".into())),
            Arc::new(OkProvider("fallback".into())),
        );
        let result: Vec<f64> = fallback.embed("text").await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn fallback_provider_default_model_uses_primary() {
        let fallback = FallbackProvider::new(
            Arc::new(OkProvider("primary-model".into())),
            Arc::new(OkProvider("fallback-model".into())),
        );
        assert_eq!(&fallback.default_model(), "primary-model");
    }
}