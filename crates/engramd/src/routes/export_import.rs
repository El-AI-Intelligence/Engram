use crate::routes::err_json;
use crate::AppState;
use axiom_engram::Engram;
use axum::{
    extract::State,
    routing::post,
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/export", post(export_memories))
        .route("/import", post(import_memories))
}

#[derive(Debug, Deserialize)]
struct ExportQuery {
    /// Optional: filter by layer
    #[serde(default)]
    layer: Option<String>,
    /// Optional: filter by tags (AND match)
    #[serde(default)]
    tags: Option<Vec<String>>,
    /// Max engrams to export
    #[serde(default = "default_export_limit")]
    limit: usize,
    /// Format: "jsonl" (default) or "json"
    #[serde(default)]
    format: Option<String>,
}

fn default_export_limit() -> usize { 10_000 }

#[derive(Debug, Deserialize)]
struct ImportBody {
    /// Array of memory objects (JSONL format — one object per line parsed
    /// into an array, or native JSON array)
    memories: Vec<ImportMemory>,
}

#[derive(Debug, Deserialize)]
struct ImportMemory {
    #[serde(default)]
    id: Option<String>,
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
    grounded: bool,
}

/// Import-field hardening. The SPA escapes everything it renders, but an
/// import must not plant values that would be storable raw: ids must be
/// plain (they land in hrefs and attributes), content must be non-empty
/// and bounded, project is bounded. Returns Err(reason) — the caller skips
/// the memory and reports the reason.
fn validate_import(m: &ImportMemory) -> Result<(), String> {
    if let Some(ref id) = m.id {
        let plain = id.len() <= 64
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !plain {
            return Err(format!("invalid id: {}", &id[..id.len().min(24)]));
        }
    }
    if m.content.trim().is_empty() {
        return Err("empty content".to_string());
    }
    if m.content.len() > 1_000_000 {
        return Err("content exceeds 1 MiB".to_string());
    }
    if let Some(ref p) = m.project {
        if p.len() > 512 {
            return Err("project exceeds 512 chars".to_string());
        }
    }
    Ok(())
}

async fn export_memories(
    State(state): State<AppState>,
    Json(q): Json<ExportQuery>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().await;
    let engrams = if let Some(ref layer_str) = q.layer {
        let layer = axiom_engram::EngramLayer::from_str(layer_str)
            .ok_or_else(|| err_json(400, format!("invalid layer: {layer_str}")))?;
        vault.search_by_layer(layer, q.limit).await.map_err(|e| err_json(500, e.to_string()))?
    } else if let Some(ref tags) = q.tags {
        let tag_refs: Vec<&str> = tags.iter().map(|t| t.as_str()).collect();
        vault.search_by_tags(&tag_refs, q.limit).await.map_err(|e| err_json(500, e.to_string()))?
    } else {
        vault.list(q.limit, 0).await.map_err(|e| err_json(500, e.to_string()))?
    };

    let data: Vec<serde_json::Value> = engrams.into_iter().map(|e| {
        json!({
            "id": e.id,
            "layer": e.layer.as_str(),
            "source": e.source.as_str(),
            "privacy_level": e.privacy_level.as_str(),
            "content": e.content,
            "context": e.context,
            "strength": e.strength,
            "valence": e.valence,
            "retrievals": e.retrievals,
            "imagined": e.imagined,
            "grounded": e.grounded,
            "created_at": e.created_at.to_rfc3339(),
            "last_retrieved": e.last_retrieved.map(|d| d.to_rfc3339()),
            "project": e.project,
            "tags": e.tags,
            "links": e.links.iter().map(|l| {
                json!({
                    "target_id": l.target_id,
                    "weight": l.weight,
                    "link_type": l.link_type.as_str(),
                })
            }).collect::<Vec<_>>(),
        })
    }).collect();

    Ok(Json(json!({
        "exported_at": chrono::Utc::now().to_rfc3339(),
        "count": data.len(),
        "format": q.format.as_deref().unwrap_or("json"),
        "memories": data,
    })))
}

async fn import_memories(
    State(state): State<AppState>,
    body: axum::body::Bytes,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, Json<serde_json::Value>)> {
    // Accept both structured JSON { "memories": [...] } and raw JSONL text
    // (one JSON object per line). The UI sends raw JSONL as a stringified
    // JSON value.
    let memories: Vec<ImportMemory> = match serde_json::from_slice::<ImportBody>(&body) {
        Ok(import_body) => import_body.memories,
        Err(_) => {
            // Try parsing as raw JSONL string: the body is JSON-stringified text
            let text: String = serde_json::from_slice(&body).unwrap_or_else(|_| {
                String::from_utf8_lossy(&body).to_string()
            });
            text.lines()
                .filter(|line| !line.trim().is_empty())
                .filter_map(|line| serde_json::from_str::<ImportMemory>(line).ok())
                .collect()
        }
    };

    let vault = state.vault.lock().await;
    let mut imported = 0;
    let mut skipped = 0;
    let mut rejected: Vec<String> = Vec::new();

    for m in memories {
        if let Err(reason) = validate_import(&m) {
            tracing::warn!(reason, "import: rejecting memory");
            if rejected.len() < 50 {
                rejected.push(reason);
            }
            skipped += 1;
            continue;
        }
        let layer = m.layer.as_deref()
            .and_then(axiom_engram::EngramLayer::from_str)
            .unwrap_or(axiom_engram::EngramLayer::Episodic);
        let source = m.source.as_deref()
            .and_then(axiom_engram::EngramSource::from_str)
            .unwrap_or(axiom_engram::EngramSource::Interaction);
        let privacy = m.privacy_level.as_deref()
            .and_then(axiom_engram::PrivacyLevel::from_str)
            .unwrap_or_default();

        let mut engram = if m.imagined {
            Engram::new_imagined(m.content, m.context.unwrap_or(json!({})))
        } else {
            Engram::new_episodic(m.content, source, m.context.unwrap_or(json!({})))
        };

        // Override generated ID with imported ID if provided
        if let Some(id) = m.id {
            engram.id = id;
        }
        engram.layer = layer;
        engram.privacy_level = privacy;
        if let Some(v) = m.valence { engram.valence = v.clamp(-1.0, 1.0); }
        if let Some(s) = m.strength { engram.strength = s.max(0.0).min(2.0); }
        if let Some(t) = m.tags { engram.tags = t; }
        if let Some(p) = m.project { engram.project = Some(p); }
        engram.imagined = m.imagined;
        engram.grounded = m.grounded;

        match vault.write(&engram).await {
            // Noise doesn't count as imported — the memory was not stored.
            Ok(axiom_engram::WriteOutcome::NoiseSkipped { .. }) => skipped += 1,
            // A duplicate means the content is already in the vault, so the
            // import goal is met — but skip the QEM populate (the id wasn't written).
            Ok(axiom_engram::WriteOutcome::Duplicate { .. }) => imported += 1,
            Ok(axiom_engram::WriteOutcome::Inserted) => {
                imported += 1;
                // Populate QEM L1 cache
                let entry: axiom_engram::MemoryEntry = engram.clone().into();
                state.qem.populate_l1(&entry);
            }
            Err(_) => skipped += 1,
        }
    }

    Ok(Json(json!({
        "ok": true,
        "imported": imported,
        "skipped": skipped,
        "rejected": rejected,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem(id: Option<&str>, content: &str) -> ImportMemory {
        ImportMemory {
            id: id.map(String::from),
            content: content.to_string(),
            layer: None,
            source: None,
            context: None,
            tags: None,
            valence: None,
            strength: None,
            privacy_level: None,
            project: None,
            imagined: false,
            grounded: false,
        }
    }

    #[test]
    fn rejects_non_plain_ids() {
        assert!(validate_import(&mem(Some("eng_abc-123"), "hi")).is_ok());
        assert!(validate_import(&mem(Some("ok_9"), "hi")).is_ok());
        assert!(validate_import(&mem(None, "hi")).is_ok());
        // XSS-plant vectors and structural junk are rejected.
        assert!(validate_import(&mem(Some("\"><img src=x onerror=alert(1)>"), "hi")).is_err());
        assert!(validate_import(&mem(Some("a/b"), "hi")).is_err());
        assert!(validate_import(&mem(Some("a b"), "hi")).is_err());
        assert!(validate_import(&mem(Some(&"x".repeat(65)), "hi")).is_err());
    }

    #[test]
    fn rejects_empty_or_huge_content() {
        assert!(validate_import(&mem(None, "   ")).is_err());
        assert!(validate_import(&mem(None, "")).is_err());
        assert!(validate_import(&mem(None, &"x".repeat(1_000_001))).is_err());
        assert!(validate_import(&mem(None, &"x".repeat(1_000_000))).is_ok());
    }
}
