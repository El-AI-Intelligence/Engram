use crate::routes::err_json;
use crate::{AppState, LiveEvent};
use axum::{
    extract::State,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;

use axiom_engram::{Engram, EngramLayer, EngramSource};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/consolidate/decay", post(trigger_decay))
        .route("/consolidate/weekly", post(trigger_weekly))
        .route("/consolidate/history", get(get_history))
        .route("/consolidate/narratives", post(narratives))
}

#[derive(Debug, Serialize)]
struct ConsolidationRunResponse {
    id: Option<String>,
    #[serde(rename = "type")]
    run_type: String,
    run_at: Option<String>,
    episodes_processed: Option<i32>,
    semantics_created: Option<i32>,
    engrams_decayed: Option<i32>,
    notes: Option<String>,
}

async fn trigger_decay(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().await;
    let (strengthened, decayed) = vault.apply_daily_hygiene()
        .await
        .map_err(|e| err_json(500, e.to_string()))?;

    // Record the run in history
    let run = axiom_engram::ConsolidationRun {
        id: format!("decay_{}", chrono::Utc::now().timestamp_millis()),
        run_at: chrono::Utc::now(),
        episodes_processed: Some((strengthened + decayed) as i32),
        semantics_created: Some(strengthened),
        engrams_decayed: Some(decayed),
        notes: Some(format!("Daily hygiene: strengthened {}, decayed {}", strengthened, decayed)),
    };
    let _ = vault.record_consolidation_run(&run).await;

    // Broadcast to WebSocket clients (mirrors background_scheduler in main.rs)
    let _ = state.events_tx.send(LiveEvent::Decay {
        strengthened: strengthened as usize,
        decayed: decayed as usize,
        timestamp: chrono::Utc::now().to_rfc3339(),
    });

    Ok(Json(json!({
        "ok": true,
        "strengthened": strengthened,
        "decayed": decayed,
        "message": format!("Strengthened {} engrams, decayed {}", strengthened, decayed),
    })))
}

async fn trigger_weekly(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().await;
    let (promoted, pruned) = vault.apply_weekly_consolidation()
        .await
        .map_err(|e| err_json(500, e.to_string()))?;

    // Record the run in history
    let run = axiom_engram::ConsolidationRun {
        id: format!("weekly_{}", chrono::Utc::now().timestamp_millis()),
        run_at: chrono::Utc::now(),
        episodes_processed: Some((promoted + pruned) as i32),
        semantics_created: Some(promoted),
        engrams_decayed: None,
        notes: Some(format!("Weekly consolidation: promoted {}, pruned {}", promoted, pruned)),
    };
    let _ = vault.record_consolidation_run(&run).await;

    // Broadcast consolidation event to WebSocket clients
    let _ = state.events_tx.send(LiveEvent::Consolidation {
        promoted: promoted as usize,
        pruned: pruned as usize,
        timestamp: chrono::Utc::now().to_rfc3339(),
    });

    Ok(Json(json!({
        "ok": true,
        "promoted": promoted,
        "pruned": pruned,
        "message": format!("Promoted {} engrams to semantic, pruned {} imagined", promoted, pruned),
    })))
}

async fn get_history(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().await;
    let runs = vault.get_consolidation_history(50)
        .await
        .map_err(|e| err_json(500, e.to_string()))?;
    let run_list: Vec<ConsolidationRunResponse> = runs.into_iter().map(|r| {
        let run_type = if r.id.starts_with("decay") {
            "decay"
        } else if r.id.starts_with("narr") {
            "narratives"
        } else {
            "weekly"
        };
        ConsolidationRunResponse {
            id: Some(r.id),
            run_type: run_type.to_string(),
            run_at: Some(r.run_at.to_rfc3339()),
            episodes_processed: r.episodes_processed,
            semantics_created: r.semantics_created,
            engrams_decayed: r.engrams_decayed,
            notes: r.notes,
        }
    }).collect();
    Ok(Json(json!({ "runs": run_list })))
}

#[derive(Debug, Deserialize)]
struct NarrativesBody {
    /// Lookback window in days (clamped 1–30; default 7)
    #[serde(default = "default_narratives_days")]
    days: u32,
}

fn default_narratives_days() -> u32 { 7 }

/// Narrative distillation: group recent episodic memories by session and
/// synthesize a five-section narrative per group (accomplished / decisions /
/// blockers / next steps / why it matters). Uses the configured local Ollama
/// model when available, otherwise the deterministic heuristic summarizer.
///
/// Manual-only by design — the background consolidator never calls this
/// endpoint, so the LLM path never triggers on a schedule.
async fn narratives(
    State(state): State<AppState>,
    body: Option<Json<NarrativesBody>>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, Json<serde_json::Value>)> {
    let days = body.map(|b| b.days.clamp(1, 30)).unwrap_or(7);
    let cutoff = (chrono::Utc::now() - chrono::Duration::days(days as i64)).to_rfc3339();
    let vault = state.vault.lock().await;

    // ── Group episodes: context.session_id → project → one misc bucket ─────
    // Keyed with an index into `groups` so group order follows created_at ASC.
    struct Group { key: String, ids: Vec<String>, contents: Vec<String> }
    let groups: Vec<Group> = {
        let conn = vault.conn().await;
        let mut stmt = conn.prepare(
            "SELECT id, content, context, project FROM engrams \
             WHERE layer = 'episodic' AND created_at >= ?1 ORDER BY created_at ASC",
        ).map_err(|e| err_json(500, format!("Failed to prepare episode query: {e}")))?;
        let rows = stmt.query_map(rusqlite::params![cutoff], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        }).map_err(|e| err_json(500, format!("Failed to load episodes: {e}")))?;

        let mut groups: Vec<Group> = Vec::new();
        let mut index: HashMap<String, usize> = HashMap::new();
        for row in rows {
            let (id, content, context, project) = match row {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(error = %e, "Skipping unreadable episode row");
                    continue;
                }
            };
            let session_id = serde_json::from_str::<serde_json::Value>(&context)
                .ok()
                .and_then(|c| c.get("session_id").and_then(|s| s.as_str()).map(String::from));
            let key = session_id.or(project).unwrap_or_else(|| "misc".into());
            let idx = *index.entry(key.clone()).or_insert_with(|| {
                groups.push(Group { key: key.clone(), ids: Vec::new(), contents: Vec::new() });
                groups.len() - 1
            });
            groups[idx].ids.push(id);
            groups[idx].contents.push(content);
        }
        groups
    }; // conn guard dropped here — vault.write below re-locks the same mutex

    let mut created = 0;
    let mut duplicates = 0;
    let mut failed = 0;
    let mut model_used: Option<String> = None;

    for group in groups.iter().take(10) {
        // Most recent 30 episodes per group, each truncated for prompt size.
        let episodes: Vec<String> = group.contents.iter().rev().take(30).rev()
            .map(|c| c.chars().take(400).collect::<String>())
            .collect();
        if episodes.is_empty() {
            continue;
        }

        let (narrative, model) = match &state.inference {
            Some(inference) => {
                let prompt = narrative_prompt(&group.key, &episodes);
                match inference.complete(axiom_inference::InferenceRequest {
                    prompt,
                    max_tokens: Some(800),
                    temperature: Some(0.3),
                    ..Default::default()
                }).await {
                    Ok(resp) if !resp.text.trim().is_empty() => {
                        (resp.text, Some(inference.default_model()))
                    }
                    Ok(_) => (heuristic_narrative(&episodes), None),
                    Err(e) => {
                        tracing::warn!(error = %e, group = %group.key,
                            "LLM narrative failed — falling back to heuristic for this group");
                        (heuristic_narrative(&episodes), None)
                    }
                }
            }
            None => (heuristic_narrative(&episodes), None),
        };
        model_used = model_used.or(model);

        let mut engram = Engram::new_episodic(
            narrative,
            EngramSource::Consolidation,
            json!({
                "kind": "narrative",
                "session_id": group.key,
                "source_engram_ids": group.ids,
                "model": model_used.clone(),
            }),
        );
        engram.layer = EngramLayer::Semantic;
        engram.tags = vec!["consolidation".into(), "narrative".into()];
        engram.scope = "session".into();

        match vault.write(&engram).await {
            Ok(axiom_engram::WriteOutcome::Inserted) => created += 1,
            Ok(axiom_engram::WriteOutcome::Duplicate { .. }) => duplicates += 1,
            Ok(axiom_engram::WriteOutcome::NoiseSkipped { .. }) => failed += 1,
            Err(e) => {
                tracing::warn!(error = %e, group = %group.key, "Failed to store narrative");
                failed += 1;
            }
        }
    }

    // Record the run so /consolidate/history and /analytics/stats reflect it
    let run = axiom_engram::ConsolidationRun {
        id: format!("narr_{}", chrono::Utc::now().timestamp_millis()),
        run_at: chrono::Utc::now(),
        episodes_processed: Some(groups.iter().map(|g| g.ids.len() as i32).sum()),
        semantics_created: Some(created),
        engrams_decayed: None,
        notes: Some(format!(
            "Narrative distillation ({days}d window): {} groups, {} created, {} duplicates, {} failed",
            groups.len(), created, duplicates, failed,
        )),
    };
    let _ = vault.record_consolidation_run(&run).await;

    Ok(Json(json!({
        "ok": true,
        "groups": groups.len(),
        "narratives_created": created,
        "duplicates_skipped": duplicates,
        "failed": failed,
        "model": model_used.unwrap_or_else(|| "heuristic".into()),
        "message": format!("Created {created} narrative memory(s) from {} group(s) ({} duplicate, {} failed)",
            groups.len(), duplicates, failed),
    })))
}

/// LLM prompt: the five-section narrative template. Sections with no data
/// are omitted by instruction (matching the heuristic fallback's convention).
fn narrative_prompt(session: &str, episodes: &[String]) -> String {
    let joined = episodes.iter().map(|e| format!("- {e}")).collect::<Vec<_>>().join("\n");
    format!(
        "You are summarizing a working session for a personal AI memory vault.\n\
         The vault is local-first and private; write nothing that was not in the input.\n\n\
         Session: {session}\n\n\
         Session memories:\n{joined}\n\n\
         Write a concise narrative with exactly these five sections. Omit a section \
         entirely if there is no data for it (do not print an empty section):\n\
         ## What was accomplished\n\
         ## Key decisions\n\
         ## Blockers / open questions\n\
         ## Next steps\n\
         ## Why this matters\n"
    )
}

/// Deterministic 5-section narrative used when no LLM is configured (or the
/// LLM call fails). Episodes are tallied against action/decision/blocker/
/// next-step keyword lists; sections with no matches are omitted, matching
/// the template convention.
fn heuristic_narrative(episodes: &[String]) -> String {
    fn matches(e: &str, keywords: &[&str]) -> bool {
        let l = e.to_lowercase();
        keywords.iter().any(|k| l.contains(k))
    }
    const ACCOMPLISHED: &[&str] = &[
        "fixed", "added", "built", "deployed", "committed", "completed", "passed",
        "resolved", "implemented", "merged", "shipped", "updated", "wrote",
    ];
    const DECISIONS: &[&str] = &[
        "decided", "decision", "chose", "chosen", "switched", "reverted", "opted", "settled on",
    ];
    const BLOCKERS: &[&str] = &[
        "blocked", "blocker", "failing", "failed", "error", "stuck", "pending", "broken", "crash",
    ];
    const NEXT_STEPS: &[&str] = &[
        "next step", "next:", "todo", "follow up", "follow-up", "plan to", "need to", "remaining",
    ];

    let mut out = String::new();
    let push_section = |out: &mut String, title: &str, items: Vec<&String>| {
        if !items.is_empty() {
            out.push_str(&format!("## {title}\n"));
            for item in items.into_iter().take(5) {
                out.push_str(&format!("- {item}\n"));
            }
        }
    };

    let done: Vec<&String> = episodes.iter().filter(|e| matches(e, ACCOMPLISHED)).collect();
    let decided: Vec<&String> = episodes.iter().filter(|e| matches(e, DECISIONS)).collect();
    let blocked: Vec<&String> = episodes.iter().filter(|e| matches(e, BLOCKERS)).collect();
    let next: Vec<&String> = episodes.iter().filter(|e| matches(e, NEXT_STEPS)).collect();

    // A section-less narrative is useless — fall back to listing the episodes
    // themselves as "accomplished" so the memory still captures the session.
    let fallback = done.is_empty() && decided.is_empty() && blocked.is_empty() && next.is_empty();
    if fallback {
        push_section(&mut out, "What was accomplished", episodes.iter().take(5).collect());
        out.push_str("## Why this matters\nRecorded session activity for later recall.\n");
        return out;
    }

    push_section(&mut out, "What was accomplished", done);
    push_section(&mut out, "Key decisions", decided);
    push_section(&mut out, "Blockers / open questions", blocked);
    push_section(&mut out, "Next steps", next);
    // Why this matters: no reliable signal in raw episodes — omit per the
    // "omit empty sections" convention rather than inventing rationale.
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heuristic_narrative_tallies_sections() {
        let episodes = vec![
            "Fixed a deadlock in QemCache".to_string(),
            "Decided to switch from RwLock to parking_lot".to_string(),
            "Blocked by failing integration test".to_string(),
            "Next: deploy the site".to_string(),
        ];
        let out = heuristic_narrative(&episodes);
        assert!(out.contains("## What was accomplished"), "{out}");
        assert!(out.contains("Fixed a deadlock in QemCache"));
        assert!(out.contains("## Key decisions"));
        assert!(out.contains("switch from RwLock to parking_lot"));
        assert!(out.contains("## Blockers / open questions"));
        assert!(out.contains("## Next steps"));
        // Sections with no matches are omitted, not printed empty
        assert!(!out.contains("## Why this matters"), "{out}");
    }

    #[test]
    fn heuristic_narrative_falls_back_when_no_keywords_match() {
        let episodes = vec!["deployed site".to_string(), "the build passes".to_string()];
        let out = heuristic_narrative(&episodes);
        assert!(out.contains("## What was accomplished"), "{out}");
        assert!(out.contains("deployed site"));
    }

    #[test]
    fn narrative_prompt_lists_all_five_sections() {
        let prompt = narrative_prompt("sess_1", &["fixed a bug".to_string()]);
        for section in [
            "## What was accomplished",
            "## Key decisions",
            "## Blockers / open questions",
            "## Next steps",
            "## Why this matters",
        ] {
            assert!(prompt.contains(section), "missing {section} in {prompt}");
        }
        assert!(prompt.contains("sess_1"));
        assert!(prompt.contains("fixed a bug"));
    }
}
