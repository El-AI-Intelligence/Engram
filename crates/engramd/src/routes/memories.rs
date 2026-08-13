use crate::errors::{self, code};
use crate::routes::err_json;
use crate::{AppState, LiveEvent};
use axiom_engram::{Engram, EngramLayer, EngramSource, EngramLink, WriteOutcome};
use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/memories", post(capture))
        .route("/memories/search", post(search))
        .route("/memories/link", post(link))
        .route("/memories/{id}", get(get_one).patch(patch_one).delete(delete_one))
        .route("/memories/{id}/links", get(get_links))
        .route("/memories/{id}/related", get(get_related))
        .route("/memories/{id}/ground", post(ground))
        .route("/memories/{id}/mark-noise", post(mark_noise))
}

// ── DTOs ────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct CaptureBody {
    content: String,
    #[serde(default)]
    layer: Option<String>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    context: Option<serde_json::Value>,
    #[serde(default)]
    tags: Option<Vec<String>>,
    #[serde(default)]
    valence: Option<f64>,
    #[serde(default)]
    strength: Option<f64>,
    #[serde(default)]
    privacy_level: Option<String>,
    #[serde(default)]
    project: Option<String>,
    #[serde(default)]
    imagined: bool,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    content_type: Option<String>,
    #[serde(default)]
    occurred_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SearchQuery {
    query: Option<String>,
    #[serde(default)]
    layer: Option<String>,
    #[serde(default)]
    tags: Option<Vec<String>>,
    #[serde(default = "default_limit")]
    limit: usize,
    #[serde(default)]
    offset: usize,
    #[serde(default)]
    sort_by: Option<String>,
    /// Filter out memories below this strength threshold
    #[serde(default)]
    min_strength: Option<f64>,
    /// Text to embed for vector similarity search.
    /// When provided and an embedder is configured, performs semantic search.
    #[serde(default)]
    vector_query: Option<String>,
    /// Search mode: "fts5" (default), "vector", or "hybrid".
    /// - fts5: keyword-based full-text search only
    /// - vector: semantic similarity search only
    /// - hybrid: merge both, weighted 0.6 vector + 0.4 FTS5
    #[serde(default = "default_search_mode")]
    search_mode: String,
    /// B7: filter to quarantined memories (imagined && !grounded) when Some(true),
    /// exclude them when Some(false), no filter when None.
    #[serde(default)]
    quarantined: Option<bool>,
}

fn default_search_mode() -> String { "fts5".to_string() }

fn default_limit() -> usize { 20 }

#[derive(Debug, Serialize)]
struct SearchResponse {
    results: Vec<MemoryResponse>,
    total: usize,
    search_type: String,
    took_ms: u64,
}

#[derive(Debug, Deserialize)]
struct LinkBody {
    source_id: String,
    target_id: String,
    #[serde(default = "default_weight")]
    weight: f64,
    #[serde(default)]
    link_type: Option<String>,
}

fn default_weight() -> f64 { 0.5 }

#[derive(Debug, Deserialize)]
struct RelatedQuery {
    #[serde(default = "default_limit")]
    limit: usize,
}

#[derive(Debug, Deserialize)]
struct PatchBody {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tags: Option<Vec<String>>,
    #[serde(default)]
    valence: Option<f64>,
    #[serde(default)]
    strength: Option<f64>,
    #[serde(default)]
    layer: Option<String>,
    #[serde(default)]
    project: Option<String>,
    #[serde(default)]
    privacy_level: Option<String>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    content_type: Option<String>,
    #[serde(default)]
    occurred_at: Option<String>,
}

#[derive(Debug, Serialize)]
struct MemoryResponse {
    id: String,
    layer: String,
    source: String,
    privacy_level: String,
    content: String,
    context: serde_json::Value,
    strength: f64,
    valence: f64,
    retrievals: i32,
    #[serde(rename = "retrieval_count")]
    retrieval_count: i32,
    imagined: bool,
    grounded: bool,
    created_at: String,
    last_retrieved: Option<String>,
    project: Option<String>,
    tags: Vec<String>,
    links: Vec<LinkResponse>,
    scope: String,
    content_type: String,
    occurred_at: Option<String>,
    evidence: Vec<EvidenceResponse>,
    /// True when this capture was filtered (noise) or merged (duplicate)
    /// instead of being written.
    skipped: bool,
    skip_reason: Option<String>,
    matched_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct EvidenceResponse {
    memory_id: String,
    relationship: String,
}

#[derive(Debug, Serialize)]
struct LinkResponse {
    target_id: String,
    weight: f64,
    link_type: String,
}

impl From<Engram> for MemoryResponse {
    fn from(e: Engram) -> Self {
        let rc = e.retrievals;
        Self {
            id: e.id,
            layer: e.layer.as_str().to_string(),
            source: e.source.as_str().to_string(),
            privacy_level: e.privacy_level.as_str().to_string(),
            content: e.content,
            context: e.context,
            strength: e.strength,
            valence: e.valence,
            retrievals: rc,
            retrieval_count: rc,
            imagined: e.imagined,
            grounded: e.grounded,
            created_at: e.created_at.to_rfc3339(),
            last_retrieved: e.last_retrieved.map(|d| d.to_rfc3339()),
            project: e.project,
            tags: e.tags,
            links: e.links.into_iter().map(LinkResponse::from).collect(),
            scope: e.scope,
            content_type: e.content_type,
            occurred_at: e.occurred_at.map(|d| d.to_rfc3339()),
            evidence: Vec::new(),
            skipped: false,
            skip_reason: None,
            matched_id: None,
        }
    }
}

impl From<EngramLink> for LinkResponse {
    fn from(l: EngramLink) -> Self {
        Self {
            target_id: l.target_id,
            weight: l.weight,
            link_type: l.link_type.as_str().to_string(),
        }
    }
}

impl From<axiom_engram::MemoryEntry> for MemoryResponse {
    fn from(m: axiom_engram::MemoryEntry) -> Self {
        let rc = m.retrieval_count as i32;
        Self {
            id: m.id.to_string(),
            layer: m.layer.as_str().to_string(),
            source: m.source.as_str().to_string(),
            privacy_level: m.privacy_level.as_str().to_string(),
            content: m.content,
            context: m.context,
            strength: m.strength,
            valence: m.valence,
            retrievals: rc,
            retrieval_count: rc,
            imagined: m.imagined,
            grounded: m.grounded,
            created_at: m.created_at.to_rfc3339(),
            last_retrieved: m.last_retrieved.map(|d| d.to_rfc3339()),
            project: m.project,
            tags: m.tags,
            links: m.links_out.into_iter().map(|l| LinkResponse {
                target_id: l.target_id.to_string(),
                weight: l.weight,
                link_type: l.link_type.as_str().to_string(),
            }).collect(),
            scope: m.scope.as_str().to_string(),
            content_type: m.content_type.as_str().to_string(),
            occurred_at: m.occurred_at.map(|d| d.to_rfc3339()),
            evidence: m.evidence.into_iter().map(|e| EvidenceResponse {
                memory_id: e.memory_id.to_string(),
                relationship: e.relationship,
            }).collect(),
            skipped: false,
            skip_reason: None,
            matched_id: None,
        }
    }
}

// ── Handlers ────────────────────────────────────────────────────────────────

async fn capture(
    State(state): State<AppState>,
    Json(body): Json<CaptureBody>,
) -> Result<Json<MemoryResponse>, (axum::http::StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().await;
    let layer = body.layer.as_deref()
        .and_then(EngramLayer::from_str)
        .unwrap_or(EngramLayer::Episodic);
    let source = body.source.as_deref()
        .and_then(EngramSource::from_str)
        .unwrap_or(EngramSource::Interaction);
    let privacy = body.privacy_level.as_deref()
        .and_then(axiom_engram::PrivacyLevel::from_str)
        .unwrap_or_default();

    let mut engram = if body.imagined {
        Engram::new_imagined(
            body.content,
            body.context.unwrap_or(json!({})),
        )
    } else {
        Engram::new_episodic(body.content, source, body.context.unwrap_or(json!({})))
    };

    engram.layer = layer;
    engram.privacy_level = privacy;
    if let Some(v) = body.valence { engram.valence = v.clamp(-1.0, 1.0); }
    if let Some(s) = body.strength { engram.strength = s.clamp(0.0, 1.0); }
    if let Some(t) = body.tags { engram.tags = t; }
    if let Some(p) = body.project { engram.project = Some(p); }
    if let Some(ref s) = body.scope { engram.scope = s.clone(); }
    if let Some(ref ct) = body.content_type { engram.content_type = ct.clone(); }
    if let Some(ref oa) = body.occurred_at {
        engram.occurred_at = chrono::DateTime::parse_from_rfc3339(oa)
            .map(|d| d.with_timezone(&chrono::Utc)).ok();
    }

    // B1/B2: noise captures and duplicates don't produce a new row — report
    // the outcome instead of pretending the memory was stored.
    let outcome = vault.write(&engram).await.map_err(|e| errors::db_error(e))?;
    match outcome {
        WriteOutcome::NoiseSkipped { reason } => {
            return Ok(Json(MemoryResponse {
                skipped: true,
                skip_reason: Some(reason),
                matched_id: None,
                ..engram.into()
            }));
        }
        WriteOutcome::Duplicate { matched_id } => {
            let matched = vault.get(&matched_id).await.map_err(|e| errors::db_error(e))?;
            return Ok(Json(MemoryResponse {
                skipped: true,
                skip_reason: Some(format!("duplicate of {matched_id}")),
                matched_id: Some(matched_id),
                ..matched.into()
            }));
        }
        WriteOutcome::Inserted => {}
    }
    let saved = vault.get(&engram.id).await.map_err(|e| errors::db_error(e))?;
    // Write-through to QEM L1 cache
    let entry: axiom_engram::MemoryEntry = saved.clone().into();
    state.qem.populate_l1(&entry);
    // Broadcast capture event to WebSocket clients (full memory payload —
    // the UI's live feed renders `msg.memory` directly)
    let _ = state.events_tx.send(LiveEvent::Capture {
        memory: serde_json::to_value(&saved).unwrap_or_default(),
        timestamp: saved.created_at.to_rfc3339(),
    });
    Ok(Json(saved.into()))
}

async fn get_one(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<MemoryResponse>, (axum::http::StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().await;
    let e = vault.get(&id).await.map_err(|e| {
        if format!("{e}").contains("Not found") {
            errors::not_found(code::MEMORY_NOT_FOUND, format!("No memory found with id: {id}"))
        } else {
            errors::db_error(e)
        }
    })?;
    Ok(Json(e.into()))
}

async fn search(
    State(state): State<AppState>,
    Json(query): Json<SearchQuery>,
) -> Result<Json<SearchResponse>, (axum::http::StatusCode, Json<serde_json::Value>)> {
    // Clamp to prevent accidental/unbounded full-table scans
    let limit = query.limit.min(100);
    let offset = query.offset;
    let vault = state.vault.lock().await;
    let start = std::time::Instant::now();

    let _is_layer_search = query.layer.is_some();
    let _is_content_search = query.query.as_ref().map(|q| !q.trim().is_empty()).unwrap_or(false);
    let _is_tag_search = query.tags.as_ref().map(|t| !t.is_empty()).unwrap_or(false);
    let use_vector = query.search_mode == "vector" || query.search_mode == "hybrid";
    let use_fts5 = query.search_mode == "fts5" || query.search_mode == "hybrid";

    let mut search_type = String::from("list");
    let mut scored: std::collections::HashMap<String, (Engram, f64)> = std::collections::HashMap::new();

    // ── FTS5 / layer / tags search ──────────────────────────────────────
    let fts5_results: Vec<Engram> = if let Some(ref layer_str) = query.layer {
        search_type = "layer".into();
        let layer = EngramLayer::from_str(layer_str)
            .ok_or_else(|| errors::bad_request(code::INVALID_LAYER, format!("Unknown memory layer: {layer_str}. Valid layers: episodic, semantic, imagined")))?;
        vault.search_by_layer(layer, limit).await.map_err(|e| errors::db_error(e))?
    } else if let Some(ref content_query) = query.query {
        if content_query.trim().is_empty() && !use_vector {
            return Err(errors::bad_request(code::CONTENT_EMPTY, "Search query cannot be empty"));
        }
        if use_fts5 && !content_query.trim().is_empty() {
            search_type = "fts5".into();
            vault.search_by_content(content_query, limit).await.map_err(|e| errors::db_error(e))?
        } else {
            Vec::new()
        }
    } else if let Some(ref tags) = query.tags {
        search_type = "tags".into();
        let tag_refs: Vec<&str> = tags.iter().map(|t| t.as_str()).collect();
        vault.search_by_tags(&tag_refs, limit).await.map_err(|e| errors::db_error(e))?
    } else {
        search_type = "list".into();
        vault.list(limit, offset).await.map_err(|e| errors::db_error(e))?
    };

    // Score FTS5/layer/tag results
    for (i, e) in fts5_results.into_iter().enumerate() {
        let score = e.strength as f64 + (1.0 / (i as f64 + 1.0)) * 0.5;
        scored.insert(e.id.clone(), (e, score));
    }

    // ── Vector search ───────────────────────────────────────────────────
    if use_vector {
        if let Some(ref embedder) = state.embedder {
            let vector_text = query.vector_query.as_deref()
                .or(query.query.as_deref())
                .unwrap_or("");
            if !vector_text.is_empty() {
                match embedder.embed(vector_text).await {
                    Ok(embedding) if !embedding.is_empty() => {
                        match vault.vector_search(&embedding, limit).await {
                            Ok(vector_results) => {
                                if search_type == "list" || search_type == "fts5" && query.search_mode == "vector" {
                                    search_type = "vector".into();
                                } else if query.search_mode == "hybrid" {
                                    search_type = "hybrid".into();
                                }
                                let vector_weight: f64 = if query.search_mode == "hybrid" { 0.6 } else { 1.0 };
                                let fts5_weight: f64 = if query.search_mode == "hybrid" { 0.4 } else { 0.0 };

                                for (e, sim) in vector_results {
                                    let score = sim as f64 * vector_weight;
                                    match scored.entry(e.id.clone()) {
                                        std::collections::hash_map::Entry::Occupied(mut o) => {
                                            o.get_mut().1 += score;
                                        }
                                        std::collections::hash_map::Entry::Vacant(v) => {
                                            v.insert((e, score));
                                        }
                                    }
                                }

                                // Re-weight existing FTS5 results for hybrid mode
                                if query.search_mode == "hybrid" {
                                    for (_, (_, s)) in scored.iter_mut() {
                                        *s *= fts5_weight;
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::warn!("Vector search failed: {e}");
                            }
                        }
                    }
                    Ok(_) => {} // empty embedding — noop embedder
                    Err(e) => {
                        tracing::warn!("Embedding generation failed: {e}");
                    }
                }
            }
        }
    }

    // ── QEM associative lookup (via holographic XOR codes) ──────────────
    if let Some(ref tags) = query.tags {
        let qem_codes: Vec<u32> = tags.iter()
            .filter_map(|t| t.strip_prefix("qem:0x"))
            .filter_map(|hex| u32::from_str_radix(hex, 16).ok())
            .collect();
        if !qem_codes.is_empty() {
            // For each QEM code, look up cached entries by exact code match
            // and try associative lookup between pairs of codes
            let mut assoc_memory_ids: Vec<String> = Vec::new();

            for &code in &qem_codes {
                // Direct code lookup
                let hits = state.qem.lookup_by_code(code);
                for mid in hits {
                    let id_str = mid.to_string();
                    if !assoc_memory_ids.contains(&id_str) {
                        assoc_memory_ids.push(id_str);
                    }
                }
            }

            // Associative lookup: try pairs of QEM codes as subject+relation
            if qem_codes.len() >= 2 {
                let subject = qem_codes[0];
                let relation = qem_codes[1];
                if let Some(mid) = state.qem.associative_lookup(subject, relation) {
                    let id_str = mid.to_string();
                    if !assoc_memory_ids.contains(&id_str) {
                        assoc_memory_ids.push(id_str);
                    }
                }
            }

            // Account the L1 probes we just made. The REST route reads the
            // vault directly (not through QemCache::search), so without this
            // the hit counters stay structurally 0. Captured before the
            // consuming loop below moves the Vec.
            let qem_hit = !assoc_memory_ids.is_empty();
            if qem_hit {
                // Fetch the full engrams for QEM hits
                for mid in assoc_memory_ids {
                    if let Ok(engram) = vault.get(&mid).await {
                        let boost = 3.0; // QEM associative hits get highest weight
                        match scored.entry(engram.id.clone()) {
                            std::collections::hash_map::Entry::Occupied(mut o) => {
                                o.get_mut().1 += boost;
                            }
                            std::collections::hash_map::Entry::Vacant(v) => {
                                v.insert((engram, boost));
                            }
                        }
                    }
                }
                search_type = if search_type == "list" || search_type == "fts5" {
                    "qem_associative".into()
                } else {
                    format!("{}+qem_associative", search_type)
                };
            }

            state.qem.record_lookup(qem_hit);
        }
    }

    // Sort by score descending, create result list
    let mut ranked: Vec<(Engram, f64)> = scored.into_values().collect();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    ranked.truncate(limit);

    let took_ms = start.elapsed().as_millis() as u64;
    let total = ranked.len();

    let mut memory_results: Vec<MemoryResponse> = ranked.into_iter()
        .map(|(e, _)| MemoryResponse::from(e))
        .collect();

    // Apply min_strength filter if specified
    if let Some(min_s) = query.min_strength {
        memory_results.retain(|m| m.strength >= min_s);
    }

    // B7: quarantine filter — lets users review/restore noise-marked memories
    if let Some(q) = query.quarantined {
        memory_results.retain(|m| (m.imagined && !m.grounded) == q);
    }

    // Apply sort if requested (default is relevance/recency from the query)
    if let Some(ref sort) = query.sort_by {
        match sort.as_str() {
            "strength" => memory_results.sort_by(|a, b| b.strength.partial_cmp(&a.strength).unwrap_or(std::cmp::Ordering::Equal)),
            "valence" => memory_results.sort_by(|a, b| b.valence.partial_cmp(&a.valence).unwrap_or(std::cmp::Ordering::Equal)),
            "recency" => memory_results.sort_by(|a, b| b.created_at.cmp(&a.created_at)),
            _ => {} // "relevance" is default — no re-sort needed
        }
    }

    Ok(Json(SearchResponse {
        results: memory_results,
        total,
        search_type,
        took_ms,
    }))
}

async fn link(
    State(state): State<AppState>,
    Json(body): Json<LinkBody>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().await;
    let lt = body.link_type.as_deref()
        .and_then(axiom_engram::engram::LinkType::from_str)
        .unwrap_or(axiom_engram::engram::LinkType::Associative);
    vault.link(&body.source_id, &body.target_id, body.weight, lt)
        .await
        .map_err(|e| {
            let msg = e.to_string();
            if msg.contains("Not found") {
                errors::not_found(code::MEMORY_NOT_FOUND, format!("Source or target memory not found"))
            } else {
                errors::db_error(e)
            }
        })?;
    Ok(Json(json!({"ok": true})))
}

async fn get_links(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().await;
    let links = vault.get_links(&id).await.map_err(|e| err_json(500, e.to_string()))?;
    let outgoing: Vec<LinkResponse> = links.into_iter().map(LinkResponse::from).collect();
    // Return { outgoing, incoming } object — the UI (graph page) accesses l.outgoing
    Ok(Json(json!({
        "outgoing": outgoing,
        "incoming": [],
    })))
}

async fn get_related(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<RelatedQuery>,
) -> Result<Json<Vec<MemoryResponse>>, (axum::http::StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().await;
    let related = vault.search_related(&id, q.limit).await.map_err(|e| err_json(500, e.to_string()))?;
    Ok(Json(related.into_iter().map(MemoryResponse::from).collect()))
}

async fn ground(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<MemoryResponse>, (axum::http::StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().await;
    let mut engram = vault.get(&id).await.map_err(|e| err_json(404, e.to_string()))?;
    engram.grounded = true;
    vault.write(&engram).await.map_err(|e| err_json(500, e.to_string()))?;
    let updated = vault.get(&id).await.map_err(|e| err_json(500, e.to_string()))?;
    // Update QEM L1 cache
    let entry: axiom_engram::MemoryEntry = updated.clone().into();
    state.qem.populate_l1(&entry);
    Ok(Json(updated.into()))
}

/// B7: mark a memory as noise — moves it to the imagined layer, clears
/// grounded, and tags it "noise". Restore via the existing ground flow.
async fn mark_noise(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<MemoryResponse>, (axum::http::StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().await;
    let mut engram = vault.get(&id).await.map_err(|e| err_json(404, e.to_string()))?;
    engram.imagined = true;
    engram.grounded = false;
    engram.layer = EngramLayer::Imagined;
    if !engram.tags.iter().any(|t| t == "noise") {
        engram.tags.push("noise".to_string());
    }
    vault.write(&engram).await.map_err(|e| err_json(500, e.to_string()))?;
    let updated = vault.get(&id).await.map_err(|e| err_json(500, e.to_string()))?;
    // Update QEM L1 cache
    let entry: axiom_engram::MemoryEntry = updated.clone().into();
    state.qem.populate_l1(&entry);
    Ok(Json(updated.into()))
}

async fn patch_one(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<PatchBody>,
) -> Result<Json<MemoryResponse>, (axum::http::StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().await;
    let mut engram = vault.get(&id).await.map_err(|e| err_json(404, e.to_string()))?;
    if let Some(c) = body.content { engram.content = c; }
    if let Some(t) = body.tags { engram.tags = t; }
    if let Some(v) = body.valence { engram.valence = v.clamp(-1.0, 1.0); }
    if let Some(s) = body.strength { engram.strength = s.clamp(0.0, 1.0); }
    if let Some(ref l) = body.layer {
        engram.layer = EngramLayer::from_str(l)
            .ok_or_else(|| err_json(400, format!("invalid layer: {l}")))?;
    }
    if let Some(p) = body.project { engram.project = Some(p); }
    if let Some(ref pl) = body.privacy_level {
        engram.privacy_level = axiom_engram::PrivacyLevel::from_str(pl)
            .ok_or_else(|| err_json(400, format!("invalid privacy_level: {pl}")))?;
    }
    if let Some(ref s) = body.scope { engram.scope = s.clone(); }
    if let Some(ref ct) = body.content_type { engram.content_type = ct.clone(); }
    if let Some(ref oa) = body.occurred_at {
        engram.occurred_at = chrono::DateTime::parse_from_rfc3339(oa)
            .map(|d| d.with_timezone(&chrono::Utc)).ok();
    }
    vault.write(&engram).await.map_err(|e| err_json(500, e.to_string()))?;
    let updated = vault.get(&id).await.map_err(|e| err_json(500, e.to_string()))?;
    // Update QEM L1 cache
    let entry: axiom_engram::MemoryEntry = updated.clone().into();
    state.qem.populate_l1(&entry);
    Ok(Json(updated.into()))
}

async fn delete_one(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().await;
    vault.delete(&id).await.map_err(|e| {
        if format!("{e}").contains("Not found") {
            errors::not_found(code::MEMORY_NOT_FOUND, format!("No memory found with id: {id}"))
        } else {
            errors::db_error(e)
        }
    })?;
    // Evict from QEM L1 cache so stale data isn't returned
    state.qem.evict_by_id(&id);
    // Record deletion for sync tombstone propagation
    crate::sync_client::record_deletion(&state.vault_path, &id);
    Ok(Json(json!({"ok": true})))
}
