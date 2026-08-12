use crate::routes::err_json;
use crate::{AppState, LiveEvent};
use axum::{
    extract::State,
    routing::{get, post},
    Json, Router,
};
use serde::Serialize;
use serde_json::json;

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
        let run_type = if r.id.starts_with("decay") { "decay" } else { "weekly" };
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

async fn narratives(
    State(_state): State<AppState>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, Json<serde_json::Value>)> {
    // Narrative distillation: group episodic memories into narrative summaries.
    // This is a future feature — requires LLM integration to synthesize stories.
    Ok(Json(json!({
        "ok": true,
        "message": "Narrative distillation requires LLM integration. Planned for future release.",
        "narratives_created": 0,
    })))
}
