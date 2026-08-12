//! LlamaCpp provider for in-process GGUF model inference.
//!
//! Uses the `llama-cpp-2` crate to load GGUF models and run inference
//! directly in-process without requiring an external API server.
//!
//! Feature-gated behind the `llama-cpp` feature flag.
//!
//! # Thread safety
//!
//! `LlamaModel` in `llama-cpp-2` is explicitly `Send + Sync`, but
//! `LlamaContext` is **not** `Send`.  All context-level operations
//! (decode, sampling, embedding extraction) MUST run via
//! `tokio::task::spawn_blocking` so they execute on a dedicated
//! blocking thread where !Send types are acceptable.  A fresh context
//! is created for each invocation and dropped before the blocking
//! closure returns.

use std::num::NonZeroU32;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::context::LlamaContext;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaChatMessage, LlamaModel, Special};
use llama_cpp_2::token::data::LlamaTokenData;
use llama_cpp_2::token::data_array::LlamaTokenDataArray;
use llama_cpp_2::token::LlamaToken;

use crate::{
    ChatRequest, ChatMessage, InferenceError, InferenceProvider,
    InferenceRequest, InferenceResponse, InferenceStream, Result,
};

// ─────────────────────────────────────────────────────────────────────────────
//  LlamaCppProvider
// ─────────────────────────────────────────────────────────────────────────────

/// LlamaCpp inference provider.
///
/// Loads a single GGUF model and provides text completion, streaming chat,
/// and embedding capabilities.  All context-level model operations run via
/// `spawn_blocking` to avoid blocking the async runtime.
pub struct LlamaCppProvider {
    /// Human-readable model identifier (derived from the GGUF filename).
    model_id: String,
    /// Shared inner state held behind a `std::sync::Mutex` because all access
    /// goes through `spawn_blocking` (never the async executor directly).
    inner: Arc<std::sync::Mutex<LlamaCppInner>>,
    /// Context size (number of tokens) for newly created contexts.
    n_ctx: u32,
    /// Number of CPU threads to use for evaluation.
    n_threads: u32,
}

/// The !Send-able state that must stay on a blocking thread.
struct LlamaCppInner {
    backend: LlamaBackend,
    model:   LlamaModel,
}

// ─────────────────────────────────────────────────────────────────────────────
//  Construction
// ─────────────────────────────────────────────────────────────────────────────

impl LlamaCppProvider {
    /// Create a new `LlamaCppProvider`.
    ///
    /// # Arguments
    ///
    /// * `model_path`   – Path to the GGUF model file.
    /// * `n_ctx`        – Context size in tokens (clamped to >= 1).
    /// * `n_gpu_layers` – Number of layers to offload to the GPU (0 = CPU-only).
    /// * `n_threads`    – Number of CPU threads (0 = auto-detect via `num_cpus`).
    ///
    /// # Errors
    ///
    /// Returns `InferenceError::Provider` if the llama backend cannot be
    /// initialised or the model file cannot be loaded.
    pub fn new(
        model_path: &str,
        n_ctx: u32,
        n_gpu_layers: u32,
        n_threads: usize,
    ) -> Result<Self> {
        let backend = LlamaBackend::init().map_err(|e| {
            InferenceError::Provider(format!(
                "Failed to initialise llama backend: {e}"
            ))
        })?;

        let model_params = LlamaModelParams::default().with_n_gpu_layers(n_gpu_layers);

        let model =
            LlamaModel::load_from_file(&backend, Path::new(model_path), &model_params)
                .map_err(|e| {
                    InferenceError::Provider(format!(
                        "Failed to load model from '{model_path}': {e}"
                    ))
                })?;

        let model_id = Path::new(model_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();

        let n_threads = if n_threads == 0 {
            num_cpus::get()
        } else {
            n_threads
        };

        let n_ctx = if n_ctx == 0 { 512 } else { n_ctx };

        Ok(Self {
            model_id,
            inner: Arc::new(std::sync::Mutex::new(LlamaCppInner { backend, model })),
            n_ctx,
            n_threads: n_threads as u32,
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Private helpers
// ─────────────────────────────────────────────────────────────────────────────

impl LlamaCppProvider {
    /// Create a fresh context for a single inference call.
    ///
    /// MUST be called on a blocking thread (`spawn_blocking`) because
    /// `LlamaContext` binds to a thread-local memory arena.
    fn create_context(
        inner: &LlamaCppInner,
        n_ctx: u32,
        n_threads: u32,
        embeddings: bool,
    ) -> Result<LlamaContext<'_>> {
        let ctx_params = LlamaContextParams::default()
            .with_n_ctx(NonZeroU32::new(n_ctx))
            .with_embeddings(embeddings)
            .with_n_threads(n_threads)
            .with_n_threads_batch(n_threads);

        inner
            .model
            .new_context(&inner.backend, ctx_params)
            .map_err(|e| InferenceError::Provider(format!("Failed to create context: {e}")))
    }

    /// Sample one token from the context's logits.
    ///
    /// When `temperature > 0.0` the logits are scaled and then a token is
    /// drawn from the resulting probability distribution.  When `temperature
    /// == 0.0` the token with the highest logit is returned (greedy).
    fn sample_token(context: &mut LlamaContext, temperature: f32) -> LlamaToken {
        let candidates: Vec<LlamaTokenData> = context.candidates_ith(0).collect();
        let mut arr = LlamaTokenDataArray::from_iter(candidates, false);

        if temperature > 0.0 {
            // sample_temp divides logits by temperature.
            // sample_token then internally applies softmax and picks at random.
            context.sample_temp(&mut arr, temperature);
            arr.sample_token(context)
        } else {
            // Greedy: directly pick the token with the highest logit.
            context.sample_token_greedy(arr)
        }
    }

    /// Autoregressive token generation.
    ///
    /// Encodes `input_tokens`, then generates up to `max_tokens` new tokens
    /// until EOS or the limit is reached.
    ///
    /// Returns `(output_tokens, text, finish_reason)`.
    ///
    /// MUST be called on a blocking thread.
    #[allow(clippy::cast_possible_truncation)]
    fn generate(
        inner: &LlamaCppInner,
        n_ctx: u32,
        n_threads: u32,
        input_tokens: &[LlamaToken],
        max_tokens: usize,
        temperature: f32,
        eos_token: LlamaToken,
    ) -> Result<(Vec<LlamaToken>, String, String)> {
        let mut ctx = Self::create_context(inner, n_ctx, n_threads, false)?;
        let mut output_tokens: Vec<LlamaToken> = Vec::new();

        // ── Encode the prompt ────────────────────────────────────────────
        let batch_size = input_tokens.len().max(1);
        let mut batch = LlamaBatch::new(batch_size, 1);
        for (i, &token) in input_tokens.iter().enumerate() {
            let is_last = i == input_tokens.len() - 1;
            batch.add(token, i as i32, &[0], is_last).map_err(|e| {
                InferenceError::Provider(format!("Failed to add token to batch: {e}"))
            })?;
        }

        ctx.decode(&mut batch)
            .map_err(|e| InferenceError::Provider(format!("Failed to decode prompt: {e}")))?;

        // ── Autoregressive loop ──────────────────────────────────────────
        let mut pos = input_tokens.len() as i32;
        let mut generated: usize = 0;

        loop {
            let token = Self::sample_token(&mut ctx, temperature);

            if token == eos_token || generated >= max_tokens {
                break;
            }

            output_tokens.push(token);
            generated += 1;

            let mut next_batch = LlamaBatch::new(1, 1);
            next_batch.add(token, pos, &[0], true).map_err(|e| {
                InferenceError::Provider(format!("Failed to add token to batch: {e}"))
            })?;
            pos += 1;

            ctx.decode(&mut next_batch)
                .map_err(|e| InferenceError::Provider(format!("Failed to decode: {e}")))?;
        }

        // ── Decode output tokens ─────────────────────────────────────────
        let text = inner
            .model
            .tokens_to_str(&output_tokens, Special::Tokenize)
            .map_err(|e| {
                InferenceError::Provider(format!("Failed to convert tokens to string: {e}"))
            })?;

        let finish_reason = if generated >= max_tokens {
            "length".to_string()
        } else {
            "stop".to_string()
        };

        Ok((output_tokens, text, finish_reason))
    }

    /// Convert our public `ChatMessage` into `LlamaChatMessage`.
    fn to_llama_messages(messages: &[ChatMessage]) -> Result<Vec<LlamaChatMessage>> {
        messages
            .iter()
            .map(|m| {
                LlamaChatMessage::new(m.role.clone(), m.content.clone()).map_err(|e| {
                    InferenceError::Provider(format!("Invalid chat message: {e}"))
                })
            })
            .collect()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  InferenceProvider trait implementation
// ─────────────────────────────────────────────────────────────────────────────

#[async_trait]
impl InferenceProvider for LlamaCppProvider {
    // ── complete ─────────────────────────────────────────────────────────
    async fn complete(&self, request: InferenceRequest) -> Result<InferenceResponse> {
        let max_tokens = request.max_tokens.unwrap_or(512);
        let temperature = request.temperature.unwrap_or(0.7);
        let prompt = request.prompt;

        let inner = self.inner.clone();
        let n_ctx = self.n_ctx;
        let n_threads = self.n_threads;

        let result = tokio::task::spawn_blocking(move || {
            let guard = inner.lock().unwrap();
            let inner = &*guard;

            // Tokenize the prompt
            let input_tokens = inner
                .model
                .str_to_token(&prompt, AddBos::Always)
                .map_err(|e| {
                    InferenceError::Provider(format!("Failed to tokenize prompt: {e}"))
                })?;

            let prompt_len = input_tokens.len();
            if prompt_len >= n_ctx as usize {
                return Err(InferenceError::InvalidRequest(format!(
                    "Prompt of {prompt_len} tokens exceeds context of {n_ctx}"
                )));
            }

            let max_gen: usize =
                (n_ctx as usize - prompt_len).min(max_tokens);
            let eos_token = inner.model.token_eos();

            let (output_tokens, text, finish_reason) = Self::generate(
                inner,
                n_ctx,
                n_threads,
                &input_tokens,
                max_gen,
                temperature,
                eos_token,
            )?;

            Ok(InferenceResponse {
                text,
                tokens: output_tokens.len(),
                finish_reason,
            }) as Result<InferenceResponse>
        })
        .await
        .map_err(|e| InferenceError::Provider(format!("Blocking task failed: {e}")))??;

        Ok(result)
    }

    // ── embed ────────────────────────────────────────────────────────────
    async fn embed(&self, text: &str) -> Result<Vec<f64>> {
        let inner = self.inner.clone();
        let n_ctx = self.n_ctx;
        let n_threads = self.n_threads;
        let input = text.to_string();

        let result = tokio::task::spawn_blocking(move || {
            let guard = inner.lock().unwrap();
            let inner = &*guard;

            let input_tokens = inner
                .model
                .str_to_token(&input, AddBos::Always)
                .map_err(|e| {
                    InferenceError::Provider(format!("Failed to tokenize: {e}"))
                })?;

            if input_tokens.len() >= n_ctx as usize {
                return Err(InferenceError::InvalidRequest(format!(
                    "Input too long: {} tokens, context is {}",
                    input_tokens.len(),
                    n_ctx
                )));
            }

            let mut ctx =
                Self::create_context(inner, n_ctx, n_threads, true)?;

            let mut batch = LlamaBatch::new(input_tokens.len().max(1), 1);
            for (i, &token) in input_tokens.iter().enumerate() {
                let is_last = i == input_tokens.len() - 1;
                batch.add(token, i as i32, &[0], is_last).map_err(|e| {
                    InferenceError::Provider(format!("Batch add error: {e}"))
                })?;
            }

            ctx.decode(&mut batch)
                .map_err(|e| InferenceError::Provider(format!("Decode error: {e}")))?;

            let emb = ctx.embeddings_seq_ith(0).map_err(|e| {
                InferenceError::Provider(format!(
                    "Embedding extraction failed (model may not support embeddings): {e}"
                ))
            })?;

            Ok(emb.iter().map(|&v| v as f64).collect()) as Result<Vec<f64>>
        })
        .await
        .map_err(|e| InferenceError::Provider(format!("Blocking task failed: {e}")))??;

        Ok(result)
    }

    // ── list_models ──────────────────────────────────────────────────────
    async fn list_models(&self) -> Result<Vec<String>> {
        Ok(vec![self.model_id.clone()])
    }

    // ── default_model ────────────────────────────────────────────────────
    fn default_model(&self) -> String {
        self.model_id.clone()
    }

    // ── complete_chat_stream ─────────────────────────────────────────────
    async fn complete_chat_stream(&self, request: ChatRequest) -> Result<InferenceStream> {
        let max_tokens = request.max_tokens.unwrap_or(512);
        let temperature = request.temperature.unwrap_or(0.7);

        // Convert messages outside the blocking closure (pure data transformation).
        let chat_messages = Self::to_llama_messages(&request.messages)?;

        let inner = self.inner.clone();
        let n_ctx = self.n_ctx;
        let n_threads = self.n_threads;

        // Apply chat template and tokenize (model operations safe outside
        // spawn_blocking because LlamaModel is Send + Sync).
        let (input_tokens, eos_token) = {
            let guard = inner.lock().unwrap();
            let formatted = guard
                .model
                .apply_chat_template(None, chat_messages, true)
                .map_err(|e| {
                    InferenceError::Provider(format!("Chat template error: {e}"))
                })?;

            let eos = guard.model.token_eos();
            let tokens = guard
                .model
                .str_to_token(&formatted, AddBos::Always)
                .map_err(|e| {
                    InferenceError::Provider(format!("Tokenize error: {e}"))
                })?;
            (tokens, eos)
        };

        let prompt_len = input_tokens.len();
        if prompt_len >= n_ctx as usize {
            return Err(InferenceError::InvalidRequest(format!(
                "Prompt of {prompt_len} tokens exceeds context of {n_ctx}"
            )));
        }

        let max_gen: usize = (n_ctx as usize - prompt_len).min(max_tokens);

        // Channel to bridge the blocking generation thread → async stream.
        let (tx, rx) =
            tokio::sync::mpsc::channel::<std::result::Result<bytes::Bytes, InferenceError>>(64);

        tokio::task::spawn_blocking(move || {
            // Helper: send an error and abort.
            let send_err = |tx: &tokio::sync::mpsc::Sender<
                std::result::Result<bytes::Bytes, InferenceError>,
            >,
                             e: InferenceError| {
                let _ = tx.blocking_send(Err(e));
            };

            let guard = match inner.lock() {
                Ok(g) => g,
                Err(e) => {
                    send_err(
                        &tx,
                        InferenceError::Provider(format!("Lock error: {e}")),
                    );
                    return;
                }
            };
            let inner = &*guard;

            let mut ctx = match Self::create_context(inner, n_ctx, n_threads, false) {
                Ok(c) => c,
                Err(e) => {
                    send_err(&tx, e);
                    return;
                }
            };

            // ── Encode prompt ────────────────────────────────────────────
            let batch_size = input_tokens.len().max(1);
            let mut batch = LlamaBatch::new(batch_size, 1);
            for (i, &token) in input_tokens.iter().enumerate() {
                let is_last = i == input_tokens.len() - 1;
                if let Err(e) = batch.add(token, i as i32, &[0], is_last) {
                    send_err(
                        &tx,
                        InferenceError::Provider(format!("Batch add error: {e}")),
                    );
                    return;
                }
            }

            if let Err(e) = ctx.decode(&mut batch) {
                send_err(
                    &tx,
                    InferenceError::Provider(format!("Decode error: {e}")),
                );
                return;
            }

            // ── Autoregressive loop + SSE emission ───────────────────────
            let mut pos = input_tokens.len() as i32;
            let mut generated: usize = 0;

            loop {
                let token = Self::sample_token(&mut ctx, temperature);

                if token == eos_token || generated >= max_gen {
                    let _ = tx.blocking_send(Ok(bytes::Bytes::from("data: [DONE]\n\n")));
                    break;
                }

                // Decode token to string for SSE payload.
                let token_str = match inner.model.token_to_str(token, Special::Tokenize) {
                    Ok(s) => s,
                    Err(_) => {
                        // Skip tokens that cannot be decoded.
                        generated += 1;

                        let mut next_batch = LlamaBatch::new(1, 1);
                        if let Err(e) = next_batch.add(token, pos, &[0], true) {
                            send_err(
                                &tx,
                                InferenceError::Provider(format!("Batch add error: {e}")),
                            );
                            return;
                        }
                        pos += 1;

                        if let Err(e) = ctx.decode(&mut next_batch) {
                            send_err(
                                &tx,
                                InferenceError::Provider(format!("Decode error: {e}")),
                            );
                            return;
                        }
                        continue;
                    }
                };

                let sse_payload = serde_json::json!({
                    "choices": [{
                        "delta": { "content": token_str }
                    }]
                });

                let sse_line = format!(
                    "data: {}\n\n",
                    serde_json::to_string(&sse_payload).unwrap_or_default()
                );

                // If the receiver has been dropped, stop generating.
                if tx.blocking_send(Ok(bytes::Bytes::from(sse_line))).is_err() {
                    break;
                }

                generated += 1;

                let mut next_batch = LlamaBatch::new(1, 1);
                if let Err(e) = next_batch.add(token, pos, &[0], true) {
                    send_err(
                        &tx,
                        InferenceError::Provider(format!("Batch add error: {e}")),
                    );
                    return;
                }
                pos += 1;

                if let Err(e) = ctx.decode(&mut next_batch) {
                    send_err(
                        &tx,
                        InferenceError::Provider(format!("Decode error: {e}")),
                    );
                    return;
                }
            }
        });

        // Convert the `mpsc::Receiver` into a `futures_util::Stream`.
        let stream = futures_util::stream::unfold(rx, |mut rx| async {
            rx.recv().await.map(|item| (item, rx))
        });

        Ok(Box::pin(stream))
    }
}
