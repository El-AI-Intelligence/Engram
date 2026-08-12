use crate::errors::{self};
use crate::AppState;
use axiom_engram::Engram;
use serde_json::json;
use axum::{
    extract::{Query, State},
    response::sse::{Event, KeepAlive, Sse},
    routing::{get, post},
    Json, Router,
};
use futures::stream::Stream;
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::sync::Arc;
use tokio::sync::mpsc;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/context/assemble", post(assemble))
        .route("/context/stream", get(stream_context))
}

// ── DTOs ────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct AssembleRequest {
    /// The user's current query/message
    query: String,
    /// Token budget for the assembled context window (default 8192).
    #[serde(default = "default_token_budget")]
    token_budget: usize,
    /// Max engrams to retrieve (default 12, max 200).
    #[serde(default = "default_max_engrams")]
    max_engrams: usize,
    /// Max recent conversation turns to include
    #[serde(default = "default_max_turns")]
    max_recent_turns: usize,
    /// Override priorities for specific sources
    #[serde(default)]
    priorities: Option<std::collections::HashMap<String, String>>,
    /// Files the user is currently working on (for file-aware retrieval).
    /// Memories tagged with these files get a relevance boost.
    #[serde(default)]
    current_files: Vec<String>,
    /// Current error the user is facing (for error-aware retrieval).
    /// Memories tagged with this error pattern get a relevance boost.
    #[serde(default)]
    current_error: Option<String>,
    /// Enable vector similarity search as an additional retrieval dimension.
    /// Default: true when an embedder is configured.
    #[serde(default = "default_use_vector")]
    use_vector: bool,
}

fn default_use_vector() -> bool { true }

fn default_token_budget() -> usize { 8192 }
fn default_max_engrams() -> usize { 12 }
fn default_max_turns() -> usize { 5 }

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct StreamQuery {
    /// Optional session identifier for filtering. When provided, results
    /// are scoped to memories captured in this session.
    session_id: Option<String>,
    #[serde(default = "default_token_budget")]
    token_budget: usize,
    /// Maximum number of memories to stream
    #[serde(default = "default_stream_limit")]
    limit: usize,
}

/// Default stream limit raised from 50→100 for richer context.
fn default_stream_limit() -> usize { 100 }

#[derive(Debug, Serialize)]
struct Message {
    role: String,
    content: String,
    token_count: Option<usize>,
}

// ── Handlers ────────────────────────────────────────────────────────────────

async fn assemble(
    State(state): State<AppState>,
    Json(req): Json<AssembleRequest>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, Json<serde_json::Value>)> {
    let start = std::time::Instant::now();
    let vault = state.vault.lock().await;
    let max_engrams = req.max_engrams.min(200);

    // Collect engrams from multiple retrieval dimensions.
    // We use a HashMap to accumulate relevance scores and deduplicate.
    let mut scored: std::collections::HashMap<String, (Engram, f64)> = std::collections::HashMap::new();

    // ── Dimension 1: Content-based (FTS5) ──────────────────────────────
    let content_results: Vec<Engram> = vault
        .search_by_content(&req.query, max_engrams)
        .await
        .map_err(|e| errors::db_error(e))?;

    for (i, e) in content_results.into_iter().enumerate() {
        // Score: strength + recency bonus, position-weighted
        let recency_bonus = if e.last_retrieved.is_some() { 0.3 } else { 0.0 };
        let score = e.strength as f64 + recency_bonus + (1.0 / (i as f64 + 1.0)) * 0.5;
        scored.insert(e.id.clone(), (e, score));
    }

    // ── Dimension 2: File-aware retrieval ──────────────────────────────
    let file_tag_count = req.current_files.len();
    if file_tag_count > 0 {
        for file_path in &req.current_files {
            let tag = format!("file:{}", file_path);
            let file_results: Vec<Engram> = vault
                .search_by_tags(&[&tag], max_engrams.min(20))
                .await
                .map_err(|e| errors::db_error(e))?;

            for e in file_results {
                let boost = 2.0; // File match is a strong signal
                let entry = scored.entry(e.id.clone());
                match entry {
                    std::collections::hash_map::Entry::Occupied(mut o) => {
                        o.get_mut().1 += boost;
                    }
                    std::collections::hash_map::Entry::Vacant(v) => {
                        v.insert((e, boost));
                    }
                }
            }
        }
    }

    // ── Dimension 3: Error-aware retrieval ─────────────────────────────
    if let Some(ref error) = req.current_error {
        // Search for session summaries tagged with this error pattern
        let error_tag = format!("error:{}", error);
        let error_results: Vec<Engram> = vault
            .search_by_tags(&[&error_tag], max_engrams.min(10))
            .await
            .map_err(|e| errors::db_error(e))?;

        for e in error_results {
            let boost = 2.5; // Error match is the strongest signal — "have we fixed this before?"
            let entry = scored.entry(e.id.clone());
            match entry {
                std::collections::hash_map::Entry::Occupied(mut o) => {
                    o.get_mut().1 += boost;
                }
                std::collections::hash_map::Entry::Vacant(v) => {
                    v.insert((e, boost));
                }
            }
        }
    }

    // ── Dimension 4: Vector similarity search ───────────────────────────
    let mut vector_used = false;
    if req.use_vector {
        if let Some(ref embedder) = state.embedder {
            match embedder.embed(&req.query).await {
                Ok(embedding) if !embedding.is_empty() => {
                    match vault.vector_search(&embedding, max_engrams.min(20)).await {
                        Ok(vector_results) => {
                            vector_used = true;
                            let boost = 1.5; // Vector match weight
                            for (e, sim) in vector_results {
                                let score = sim as f64 * boost;
                                let entry = scored.entry(e.id.clone());
                                match entry {
                                    std::collections::hash_map::Entry::Occupied(mut o) => {
                                        o.get_mut().1 += score;
                                    }
                                    std::collections::hash_map::Entry::Vacant(v) => {
                                        v.insert((e, score));
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!("Vector search in context/assemble failed: {e}");
                        }
                    }
                }
                Ok(_) => {} // empty — noop embedder
                Err(e) => {
                    tracing::warn!("Embedding generation in context/assemble failed: {e}");
                }
            }
        }
    }

    // Sort by score descending, take top max_engrams
    let mut ranked: Vec<(Engram, f64)> = scored.into_values().collect();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    ranked.truncate(max_engrams);

    let engrams_retrieved = ranked.len();

    // Build OpenAI-format messages
    let mut messages: Vec<Message> = Vec::new();

    // System message with context
    let mut system_parts: Vec<String> = vec![
        "You are an AI assistant with access to an encrypted memory vault.".into(),
        format!("{} relevant memories loaded.", engrams_retrieved),
        "Use these memories to inform your responses. Mark memory-sourced info with [mem].".into(),
    ];

    // Add retrieval dimension summary
    let dims: Vec<&str> = {
        let mut d = vec!["content"];
        if file_tag_count > 0 { d.push("file-aware"); }
        if req.current_error.is_some() { d.push("error-aware"); }
        if vector_used { d.push("vector"); }
        d
    };
    system_parts.push(format!("Retrieval dimensions: {}", dims.join(", ")));

    for (i, (e, score)) in ranked.iter().enumerate() {
        system_parts.push(format!(
            "[Memory {} | {} | strength={:.2} | rel={:.2}] {}",
            i + 1,
            e.layer.as_str(),
            e.strength,
            score,
            e.content
        ));
    }

    messages.push(Message {
        role: "system".into(),
        content: system_parts.join("\n\n"),
        token_count: None,
    });

    messages.push(Message {
        role: "user".into(),
        content: req.query,
        token_count: None,
    });

    // Rough token count: ~4 chars per token
    let token_count: usize = messages.iter()
        .map(|m| m.content.len() / 4)
        .sum();

    let took_ms = start.elapsed().as_millis() as u64;

    Ok(Json(json!({
        "messages": messages,
        "metadata": {
            "total_tokens": token_count,
            "budget": req.token_budget,
            "engrams_retrieved": engrams_retrieved,
            "retrieval_took_ms": took_ms,
            "dimensions": {
                "content": true,
                "file_aware": file_tag_count > 0,
                "error_aware": req.current_error.is_some(),
                "vector": vector_used,
            },
        },
    })))
}

// ── Real-time SSE context stream ────────────────────────────────────────────

/// Stream memories as Server-Sent Events. Uses a tokio channel for real
/// async streaming — memories are emitted as they're fetched from the DB
/// rather than being collected into a buffer first.
///
/// When `session_id` is provided, events include it as metadata so clients
/// can correlate memories with a specific session.
///
/// The stream sends a `done` event with count when complete, or an `error`
/// event if the vault can't be read.
async fn stream_context(
    State(state): State<AppState>,
    Query(q): Query<StreamQuery>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let session_id = q.session_id.unwrap_or_else(|| "default".into());
    let limit = q.limit.min(200);

    // Channel: the producer task sends events; the stream consumes them
    let (tx, rx) = mpsc::channel::<Result<Event, Infallible>>(32);

    // Spawn a task that fetches and sends memories
    let vault = state.vault.clone();
    tokio::spawn(async move {
        let result = stream_memories_to_channel(vault, session_id.clone(), limit, &tx).await;

        // Always send a terminal event
        match result {
            Ok(count) => {
                let _ = tx.send(Ok(Event::default()
                    .event("done")
                    .data(json!({"session_id": session_id, "count": count}).to_string())))
                .await;
            }
            Err(e) => {
                let _ = tx.send(Ok(Event::default()
                    .event("error")
                    .data(json!({"session_id": session_id, "error": e}).to_string())))
                .await;
            }
        }
    });

    // Convert channel receiver to SSE stream
    let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// Fetch memories from the vault and send them one-by-one through the channel.
/// Returns the total count on success.
async fn stream_memories_to_channel(
    vault: Arc<tokio::sync::Mutex<axiom_engram::EngramStore>>,
    session_id: String,
    limit: usize,
    tx: &mpsc::Sender<Result<Event, Infallible>>,
) -> Result<usize, String> {
    let vault = vault.lock().await;
    let recent = vault.list(limit, 0)
        .await
        .map_err(|e| format!("Failed to read vault: {e}"))?;

    let count = recent.len();

    // Send each memory as it becomes available
    for (i, e) in recent.into_iter().enumerate() {
        let json = serde_json::to_string(&serde_json::json!({
            "id": e.id,
            "index": i,
            "session_id": session_id,
            "layer": e.layer.as_str(),
            "content": e.content,
            "strength": e.strength,
            "valence": e.valence,
            "tags": e.tags,
            "created_at": e.created_at.to_rfc3339(),
        }))
        .unwrap_or_default();

        let event = Event::default()
            .event("memory")
            .id(e.id)
            .data(json);

        if tx.send(Ok(event)).await.is_err() {
            // Client disconnected — stop sending
            return Ok(i);
        }
    }

    Ok(count)
}
