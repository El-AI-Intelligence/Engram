// ── Weekly digest endpoint ──────────────────────────────────────────────────
//
// GET /digest/weekly?days=7&prose=1 — "what your AI learned about you this
// week": deterministic stats (new / reinforced / fading / quarantined) plus
// themes clustered from local embeddings.
//
// The digest core is fully local and free. `?prose=1` upgrades the response
// with LLM-written prose via the user's BYO-key OpenAI-compatible endpoint
// (`digest.llm` in config.json — also works with a local Ollama at
// http://localhost:11434/v1). Prose is generated ONLY on that explicit flag:
// BYO-key calls bill the user's own key, so they are never automatic.

use std::collections::{BTreeMap, HashMap};

use crate::AppState;
use crate::errors;

use axum::{
    extract::{Query, State},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};

use axiom_engram::Engram;

/// Hard cap per memory slice returned in the digest (the slices are highlights,
/// not full dumps — counts are exact regardless).
const SLICE_LIMIT: usize = 50;
/// Maximum number of themes reported.
const MAX_THEMES: usize = 5;
/// Cosine similarity a memory must share with a theme anchor to join it.
const SIM_THRESHOLD: f64 = 0.35;
/// Theme labels are truncated medoid contents.
const LABEL_CHARS: usize = 48;
/// Example contents are truncated harder.
const EXAMPLE_CHARS: usize = 80;
/// Max themes' worth of examples each.
const EXAMPLES_PER_THEME: usize = 3;
/// Timeout for the BYO-key prose call.
const LLM_TIMEOUT_SECS: u64 = 45;

pub fn router() -> Router<AppState> {
    Router::new().route("/digest/weekly", get(weekly))
}

#[derive(Debug, Deserialize)]
struct DigestQuery {
    /// Window length in days (clamped to 1–90).
    #[serde(default = "default_days")]
    days: u32,
    /// Generate LLM prose (requires `digest.llm` in config.json).
    #[serde(default, deserialize_with = "deserialize_flag")]
    prose: bool,
}

fn default_days() -> u32 { 7 }

/// Accepts the common boolean spellings ("1", "true", "yes", "on") so both
/// `?prose=1` and `?prose=true` work.
fn deserialize_flag<'de, D: serde::Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    let s = String::deserialize(d)?;
    Ok(matches!(s.as_str(), "1" | "true" | "yes" | "on"))
}

/// BYO-key LLM block as read raw from config.json (the /config route owns the
/// typed PersistedConfig; this route follows the sync_status pattern of raw
/// reads so the two never drift out of lockstep).
#[derive(Debug, Default, Clone)]
struct RawLlmConfig {
    /// OpenAI-compatible base URL ending in /v1 (e.g. "http://localhost:11434/v1").
    base_url: String,
    api_key: Option<String>,
    model: String,
}

impl RawLlmConfig {
    fn from_json(v: &serde_json::Value) -> Option<Self> {
        let llm = v.get("digest")?.get("llm")?;
        Some(Self {
            base_url: llm.get("base_url").and_then(|x| x.as_str()).unwrap_or("").into(),
            api_key: llm
                .get("api_key")
                .and_then(|x| x.as_str())
                .filter(|k| !k.is_empty() && *k != "••••••••")
                .map(String::from),
            model: llm.get("model").and_then(|x| x.as_str()).unwrap_or("").into(),
        })
    }

    fn usable(&self) -> bool {
        !self.base_url.is_empty() && !self.model.is_empty()
    }
}

/// Raw `digest` section read from config.json (defaults when absent).
fn read_digest_config(vault_path: &std::path::Path) -> (bool, Option<RawLlmConfig>) {
    let cfg: serde_json::Value = std::fs::read_to_string(vault_path.join("config.json"))
        .ok()
        .and_then(|data| serde_json::from_str(&data).ok())
        .unwrap_or(serde_json::Value::Null);
    let enabled = cfg
        .get("digest")
        .and_then(|d| d.get("enabled"))
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    (enabled, RawLlmConfig::from_json(&cfg))
}

#[derive(Debug, Serialize)]
struct WeeklyDigest {
    generated_at: String,
    window_start: String,
    window_end: String,
    stats: DigestStats,
    themes: Vec<DigestTheme>,
    new_memories: Vec<DigestMemory>,
    reinforced: Vec<DigestMemory>,
    fading: Vec<DigestMemory>,
    /// LLM-written narrative — present only when explicitly requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    prose: Option<String>,
    /// Whether a usable BYO-key LLM is configured (the UI shows "Generate
    /// prose" only when true).
    llm_configured: bool,
}

#[derive(Debug, Serialize)]
struct DigestStats {
    live_total: usize,
    new: usize,
    reinforced: usize,
    fading: usize,
    quarantined: usize,
    quarantined_new: usize,
}

#[derive(Debug, Serialize)]
struct DigestTheme {
    label: String,
    count: usize,
    examples: Vec<String>,
}

#[derive(Debug, Serialize)]
struct DigestMemory {
    id: String,
    content: String,
    layer: String,
    tags: Vec<String>,
    strength: f64,
}

impl From<&Engram> for DigestMemory {
    fn from(e: &Engram) -> Self {
        DigestMemory {
            id: e.id.clone(),
            content: e.content.clone(),
            layer: e.layer.as_str().to_string(),
            tags: e.tags.clone(),
            strength: e.strength,
        }
    }
}

async fn weekly(
    State(state): State<AppState>,
    Query(q): Query<DigestQuery>,
) -> Result<Json<WeeklyDigest>, (axum::http::StatusCode, Json<serde_json::Value>)> {
    let (enabled, llm) = read_digest_config(&state.vault_path);
    if !enabled {
        return Err((
            axum::http::StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": {
                    "code": "digest_disabled",
                    "message": "The weekly digest is disabled in config.json (digest.enabled = false)"
                }
            })),
        ));
    }

    let days = q.days.clamp(1, 90);
    let window_end = chrono::Utc::now();
    let window_start = window_end - chrono::Duration::days(i64::from(days));

    let window = {
        let vault = state.vault.lock().await;
        vault
            .digest_window(&window_start.to_rfc3339(), SLICE_LIMIT)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "digest_window failed");
                errors::internal(errors::code::INTERNAL, "Failed to assemble digest window")
            })?
    };

    // Themes cluster the new memories; a quiet week falls back to what the AI
    // actually re-used (reinforced).
    let theme_source = if !window.new.is_empty() { &window.new } else { &window.reinforced };
    let embeddings = if theme_source.is_empty() {
        HashMap::new()
    } else {
        let ids: Vec<String> = theme_source.iter().map(|m| m.id.clone()).collect();
        {
            let vault = state.vault.lock().await;
            vault.get_embeddings(&ids).await.unwrap_or_default()
        }
    };
    let themes = cluster_themes(theme_source, &embeddings, MAX_THEMES);

    let stats = DigestStats {
        live_total: window.live_total,
        new: window.new_count,
        reinforced: window.reinforced_count,
        fading: window.fading_count,
        quarantined: window.quarantined_count,
        quarantined_new: window.quarantined_new_count,
    };
    let new_memories: Vec<DigestMemory> = window.new.iter().map(DigestMemory::from).collect();
    let reinforced: Vec<DigestMemory> = window.reinforced.iter().map(DigestMemory::from).collect();
    let fading: Vec<DigestMemory> = window.fading.iter().map(DigestMemory::from).collect();

    let llm_configured = llm.as_ref().map(|l| l.usable()).unwrap_or(false);

    let prose = if q.prose {
        let llm = llm.as_ref().filter(|l| l.usable()).ok_or_else(|| {
            (
                axum::http::StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "error": {
                        "code": "llm_not_configured",
                        "message": "Prose requires a BYO-key LLM in config.json: set digest.llm.base_url (OpenAI-compatible, e.g. http://localhost:11434/v1) and digest.llm.model"
                    }
                })),
            )
        })?;
        let prompt = digest_prompt(
            days,
            &stats,
            &themes,
            &new_memories,
            &reinforced,
            &fading,
        );
        let text = generate_prose(llm, &prompt).await.map_err(|e| {
            tracing::error!(error = %e, "BYO-key prose generation failed");
            (
                axum::http::StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({
                    "error": { "code": "llm_error", "message": e }
                })),
            )
        })?;
        Some(text)
    } else {
        None
    };

    Ok(Json(WeeklyDigest {
        generated_at: window_end.to_rfc3339(),
        window_start: window_start.to_rfc3339(),
        window_end: window_end.to_rfc3339(),
        stats,
        themes,
        new_memories,
        reinforced,
        fading,
        prose,
        llm_configured,
    }))
}

// ── Theme clustering ────────────────────────────────────────────────────────
//
// Deterministic, local, free: greedy medoid clustering over the vault's own
// embeddings, with tag grouping as the fallback when embeddings are absent
// (short notes aren't embedded). Leftover singletons are omitted from themes
// — they still appear in the memory lists, and themes are highlights, not a
// partition guarantee.

fn cluster_themes(
    memories: &[Engram],
    embeddings: &HashMap<String, Vec<f64>>,
    max_themes: usize,
) -> Vec<DigestTheme> {
    let mut themes: Vec<DigestTheme> = Vec::new();
    let mut unassigned: Vec<&Engram> = memories.iter().collect();

    // Round 1: embedding clusters. Each round picks the medoid — the memory
    // most similar to everything left — and claims every memory within
    // SIM_THRESHOLD of it.
    while themes.len() < max_themes && !unassigned.is_empty() {
        let Some((medoid_idx, _best_sum)) = pick_medoid(&unassigned, embeddings) else {
            break;
        };
        let medoid = unassigned.swap_remove(medoid_idx);
        let mut members = vec![medoid];
        let mut i = 0;
        while i < unassigned.len() {
            let pair = embeddings
                .get(&medoid.id)
                .zip(embeddings.get(&unassigned[i].id));
            if let Some((ea, eb)) = pair {
                if cosine(ea, eb) >= SIM_THRESHOLD {
                    members.push(unassigned.swap_remove(i));
                    continue;
                }
            }
            i += 1;
        }
        if members.len() == 1 {
            // The medoid has no neighbors, so no further cluster is possible
            // — put it back and fall through to tag grouping.
            unassigned.push(medoid);
            break;
        }
        themes.push(theme_from_medoid(members));
    }

    // Round 2: tag grouping for what's left (also the no-embeddings path).
    let mut by_tag: BTreeMap<String, Vec<&Engram>> = BTreeMap::new();
    for m in &unassigned {
        if let Some(tag) = m.tags.first() {
            by_tag.entry(tag.clone()).or_default().push(m);
        }
    }
    for (tag, members) in by_tag {
        if themes.len() >= max_themes {
            break;
        }
        if members.len() >= 2 {
            themes.push(DigestTheme {
                label: tag,
                count: members.len(),
                examples: theme_examples(&members),
            });
        }
    }

    themes
}

/// The unassigned memory with the highest summed similarity to the rest.
/// `None` when fewer than two unassigned memories have embeddings.
fn pick_medoid<'a>(
    memories: &[&'a Engram],
    embeddings: &HashMap<String, Vec<f64>>,
) -> Option<(usize, f64)> {
    let embedded = memories.iter().filter(|m| embeddings.contains_key(&m.id)).count();
    if embedded < 2 {
        return None;
    }
    let mut best: Option<(usize, f64)> = None;
    for (i, m) in memories.iter().enumerate() {
        let Some(ea) = embeddings.get(&m.id) else { continue };
        let mut sum = 0.0;
        for (j, other) in memories.iter().enumerate() {
            if i == j {
                continue;
            }
            if let Some(eb) = embeddings.get(&other.id) {
                sum += cosine(ea, eb);
            }
        }
        match best {
            // Strict >= keeps the first medoid on ties — deterministic.
            Some((_, b)) if b >= sum => {}
            _ => best = Some((i, sum)),
        }
    }
    best
}

fn theme_from_medoid(members: Vec<&Engram>) -> DigestTheme {
    DigestTheme {
        label: truncate(&members[0].content, LABEL_CHARS),
        count: members.len(),
        examples: theme_examples(&members),
    }
}

fn theme_examples(members: &[&Engram]) -> Vec<String> {
    members
        .iter()
        .take(EXAMPLES_PER_THEME)
        .map(|m| truncate(&m.content, EXAMPLE_CHARS))
        .collect()
}

/// Cosine similarity of two embedding vectors (zero for empty/mismatched).
fn cosine(a: &[f64], b: &[f64]) -> f64 {
    let (mut dot, mut na, mut nb) = (0.0, 0.0, 0.0);
    for i in 0..a.len().min(b.len()) {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let t: String = s.chars().take(max_chars).collect();
        format!("{t}…")
    }
}

// ── BYO-key prose ───────────────────────────────────────────────────────────

/// Deterministic prompt assembled from the digest data — the LLM only
/// phrases, it never fabricates the numbers.
fn digest_prompt(
    days: u32,
    stats: &DigestStats,
    themes: &[DigestTheme],
    new_memories: &[DigestMemory],
    reinforced: &[DigestMemory],
    fading: &[DigestMemory],
) -> String {
    let mut p = format!(
        "Weekly memory digest (last {days} days).\n\nStats: {} live memories, {} new, \
         {} reinforced by use, {} fading, {} quarantined ({} new).\n",
        stats.live_total, stats.new, stats.reinforced, stats.fading,
        stats.quarantined, stats.quarantined_new,
    );
    if !themes.is_empty() {
        p.push_str("\nThemes:\n");
        for t in themes {
            p.push_str(&format!("- {} ({} memories)\n", t.label, t.count));
        }
    }
    let list = |name: &str, items: &[DigestMemory]| {
        let mut s = format!("\n{name}:\n");
        for m in items {
            s.push_str(&format!("- {}\n", truncate(&m.content, 120)));
        }
        s
    };
    p.push_str(&list("New memories", new_memories));
    p.push_str(&list("Reinforced (used this week)", reinforced));
    p.push_str(&list("Fading (not used recently)", fading));
    p.push_str(
        "\nWrite a warm, concise personal digest (3-5 short paragraphs) for the \
         user: what their AI learned about them this week, what it kept using, \
         and what might be worth revisiting. Do not invent memories or numbers; \
         stick to the material above.",
    );
    p
}

/// One-shot chat-completion call against the user's own OpenAI-compatible
/// endpoint. `Err(String)` carries a non-secret failure description.
async fn generate_prose(llm: &RawLlmConfig, prompt: &str) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(LLM_TIMEOUT_SECS))
        .build()
        .map_err(|e| format!("failed to build HTTP client: {e}"))?;
    let url = format!("{}/chat/completions", llm.base_url.trim_end_matches('/'));
    let mut req = client.post(&url).json(&serde_json::json!({
        "model": llm.model,
        "messages": [
            {
                "role": "system",
                "content": "You write weekly digests for a personal, local-first AI memory vault. \
                            Warm, specific, honest. Never invent memories or statistics."
            },
            { "role": "user", "content": prompt }
        ],
        "max_tokens": 600,
        "temperature": 0.7
    }));
    if let Some(ref key) = llm.api_key {
        req = req.header("Authorization", format!("Bearer {key}"));
    }
    let resp = req
        .send()
        .await
        .map_err(|e| format!("LLM request failed: {e}"))?;
    let status = resp.status();
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("LLM response was not valid JSON: {e}"))?;
    if !status.is_success() {
        let detail = body
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .unwrap_or("unknown error");
        return Err(format!("LLM endpoint returned {status}: {detail}"));
    }
    body.pointer("/choices/0/message/content")
        .and_then(|c| c.as_str())
        .filter(|c| !c.trim().is_empty())
        .map(|c| c.trim().to_string())
        .ok_or_else(|| "LLM response had no message content".to_string())
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn mem(content: &str, tags: &[&str]) -> Engram {
        let mut e = Engram::new_episodic(content.to_string(), axiom_engram::EngramSource::Interaction, serde_json::json!({}));
        e.tags = tags.iter().map(|t| t.to_string()).collect();
        e
    }

    #[test]
    fn cluster_themes_groups_similar_embeddings() {
        let coffee_a = mem("coffee order preferences captured", &[]);
        let coffee_b = mem("espresso machine settings noted", &[]);
        let rust_a = mem("async stream design decision", &[]);
        let rust_b = mem("tokio spawn best practice", &[]);
        let memories = [coffee_a, coffee_b, rust_a, rust_b];
        let mut embeddings = HashMap::new();
        embeddings.insert(memories[0].id.clone(), vec![1.0, 0.0, 0.0]);
        embeddings.insert(memories[1].id.clone(), vec![0.9, 0.1, 0.0]);
        embeddings.insert(memories[2].id.clone(), vec![0.0, 1.0, 0.0]);
        embeddings.insert(memories[3].id.clone(), vec![0.1, 0.9, 0.0]);

        let themes = cluster_themes(&memories, &embeddings, MAX_THEMES);

        assert_eq!(themes.len(), 2, "two similarity clusters expected");
        assert!(themes.iter().all(|t| t.count == 2));
        assert!(themes.iter().all(|t| t.examples.len() == 2));
        // Grouping correctness: each cluster holds one pair, whichever member
        // ends up as the medoid label (they tie on similarity sum).
        let joined: Vec<String> = themes
            .iter()
            .flat_map(|t| t.examples.iter().cloned())
            .collect();
        assert!(joined.iter().any(|e| e.contains("coffee")));
        assert!(joined.iter().any(|e| e.contains("espresso")));
        assert!(joined.iter().any(|e| e.contains("async")));
        assert!(joined.iter().any(|e| e.contains("tokio")));
    }

    #[test]
    fn cluster_themes_falls_back_to_tag_grouping() {
        let a = mem("deep work notes one", &["work"]);
        let b = mem("deep work notes two", &["work"]);
        let c = mem("garden plans one", &["home"]);
        let d = mem("garden plans two", &["home"]);
        let untagged = mem("lonely observation", &[]);
        let memories = [a, b, c, d, untagged];
        let embeddings = HashMap::new(); // no embeddings at all

        let themes = cluster_themes(&memories, &embeddings, MAX_THEMES);

        assert_eq!(themes.len(), 2);
        let labels: Vec<&str> = themes.iter().map(|t| t.label.as_str()).collect();
        assert!(labels.contains(&"work") && labels.contains(&"home"));
        assert!(themes.iter().all(|t| t.count == 2));
        // The untagged singleton is omitted from themes (it still shows in the
        // memory lists).
        assert!(themes.iter().all(|t| !t.examples.iter().any(|e| e.contains("lonely"))));
    }

    #[test]
    fn cluster_themes_caps_at_max_themes() {
        let a = mem("grocery list bread and milk", &[]);
        let b = mem("shopping list eggs and cheese", &[]);
        let c = mem("rust borrow checker rule", &[]);
        let d = mem("lifetime annotation trick", &[]);
        let memories = [a, b, c, d];
        let mut embeddings = HashMap::new();
        embeddings.insert(memories[0].id.clone(), vec![1.0, 0.0]);
        embeddings.insert(memories[1].id.clone(), vec![0.9, 0.1]);
        embeddings.insert(memories[2].id.clone(), vec![0.0, 1.0]);
        embeddings.insert(memories[3].id.clone(), vec![0.1, 0.9]);

        let themes = cluster_themes(&memories, &embeddings, 1);

        assert_eq!(themes.len(), 1);
        assert_eq!(themes[0].count, 2);
    }

    #[test]
    fn cosine_zero_for_orthogonal_and_empty() {
        assert!((cosine(&[1.0, 0.0], &[0.0, 1.0])).abs() < 1e-9);
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 0.0]), 0.0);
        assert_eq!(cosine(&[1.0], &[]), 0.0);
    }

    #[test]
    fn truncate_elides_long_content() {
        assert_eq!(truncate("short", 10), "short");
        let long = truncate("this content is much too long", 10);
        assert_eq!(long.chars().count(), 11, "10 chars + ellipsis");
        assert!(long.ends_with('…'));
    }
}
