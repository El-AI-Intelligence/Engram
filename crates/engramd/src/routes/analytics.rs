use crate::routes::err_json;
use crate::AppState;
use axum::{
    extract::{Query, State},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/analytics/patterns", post(patterns))
        .route("/analytics/stats", get(stats))
        .route("/analytics/activity", get(activity))
        .route("/analytics/co2", get(co2))
}

#[derive(Debug, Deserialize)]
struct PatternsQuery {
    /// Optional context query to scope pattern detection
    #[serde(default)]
    query: Option<String>,
    /// Minimum engrams required to detect a pattern
    #[serde(default = "default_min_engrams")]
    min_engrams: usize,
}

fn default_min_engrams() -> usize { 5 }

async fn patterns(
    State(state): State<AppState>,
    Json(q): Json<PatternsQuery>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().await;
    let query_str = q.query.as_deref().unwrap_or("");
    let pattern = vault.detect_temporal_patterns(query_str, q.min_engrams)
        .await
        .map_err(|e| err_json(500, e.to_string()))?;

    match pattern {
        Some(p) => Ok(Json(serde_json::json!({
            "pattern": {
                "found": true,
                "description": p.describe(),
                "peak_day": p.peak_day,
                "peak_period": p.peak_period,
                "day_strength": p.day_strength,
                "period_strength": p.period_strength,
                "sample_size": p.sample_size,
            }
        }))),
        None => Ok(Json(serde_json::json!({
            "pattern": {
                "found": false,
                "description": "Insufficient data to detect temporal patterns. Continue capturing memories to enable pattern detection.",
            }
        }))),
    }
}

async fn stats(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().await;
    let total = vault.count().await.map_err(|e| err_json(500, e.to_string()))?;

    // Count by layer
    let episodic = vault.search_by_layer(
        axiom_engram::EngramLayer::Episodic, 100_000
    ).await.map(|v| v.len()).unwrap_or(0);

    let semantic = vault.search_by_layer(
        axiom_engram::EngramLayer::Semantic, 100_000
    ).await.map(|v| v.len()).unwrap_or(0);

    let imagined = vault.search_by_layer(
        axiom_engram::EngramLayer::Imagined, 100_000
    ).await.map(|v| v.len()).unwrap_or(0);

    // Additional stats the UI expects
    let conn = vault.conn().await;
    let total_links: i64 = conn
        .query_row("SELECT COUNT(*) FROM engram_links", [], |r| r.get(0))
        .unwrap_or(0);
    let total_embeddings: i64 = conn
        .query_row("SELECT COUNT(*) FROM engram_embeddings", [], |r| r.get(0))
        .unwrap_or(0);
    let avg_strength: f64 = conn
        .query_row("SELECT COALESCE(AVG(strength), 0) FROM engrams", [], |r| r.get(0))
        .unwrap_or(0.0);
    let avg_valence: f64 = conn
        .query_row("SELECT COALESCE(AVG(valence), 0) FROM engrams", [], |r| r.get(0))
        .unwrap_or(0.0);
    // Last run timestamps from consolidation history
    let last_consolidation: Option<String> = conn
        .query_row(
            "SELECT run_at FROM consolidation_runs WHERE id LIKE 'weekly_%' ORDER BY run_at DESC LIMIT 1",
            [], |r| r.get(0),
        ).ok();
    let last_decay: Option<String> = conn
        .query_row(
            "SELECT run_at FROM consolidation_runs WHERE id LIKE 'decay_%' ORDER BY run_at DESC LIMIT 1",
            [], |r| r.get(0),
        ).ok();

    Ok(Json(serde_json::json!({
        "total": total,
        "total_memories": total,
        "by_layer": {
            "episodic": episodic,
            "semantic": semantic,
            "imagined": imagined,
        },
        "total_links": total_links,
        "total_embeddings": total_embeddings,
        "avg_strength": avg_strength,
        "avg_valence": avg_valence,
        "last_consolidation": last_consolidation,
        "last_decay": last_decay,
        "qem_hit_rate": state.qem.hit_rate(),
        "qem_cache_entries": state.qem.cache_size(),
        "db_size_bytes": state.vault_path.join("engrams.db")
            .metadata()
            .map(|m| m.len())
            .unwrap_or(0),
    })))
}

// ── Activity (captures per day over a window) ────────────────────────────

#[derive(Debug, Deserialize)]
struct ActivityQuery {
    #[serde(default = "default_activity_days")]
    days: u32,
}

fn default_activity_days() -> u32 { 30 }

async fn activity(
    State(state): State<AppState>,
    Query(q): Query<ActivityQuery>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().await;
    let conn = vault.conn().await;

    let mut stmt = conn
        .prepare(
            "SELECT DATE(created_at) as day, COUNT(*) as cnt \
             FROM engrams \
             WHERE created_at >= DATE('now', ?1) \
             GROUP BY day ORDER BY day ASC",
        )
        .map_err(|e| err_json(500, format!("Database error: {e}")))?;

    let days_ago = format!("-{} days", q.days);
    let rows = stmt
        .query_map([&days_ago], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(|e| err_json(500, format!("Database error: {e}")))?;

    let mut activity = Vec::new();
    for row in rows {
        let (day, count) = row.map_err(|e| err_json(500, format!("Database error: {e}")))?;
        activity.push(serde_json::json!({"day": day, "count": count}));
    }

    Ok(Json(serde_json::json!({ "activity": activity, "days": q.days })))
}

// ── CO2 estimate ─────────────────────────────────────────────────────────

async fn co2(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().await;
    let conn = vault.conn().await;

    // Count total captures (each capture avoids re-generating ~same tokens)
    let total_captures: i64 = conn
        .query_row("SELECT COUNT(*) FROM engrams", [], |r| r.get(0))
        .unwrap_or(0);

    // Count retrievals (each retrieval saves ~context_window tokens)
    let total_retrievals: i64 = conn
        .query_row(
            "SELECT COALESCE(SUM(retrieval_count), 0) FROM engrams",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    // Conservative estimates: each capture saves ~500 tokens (avoid re-deriving),
    // each retrieval saves ~2000 tokens (avoid full context re-assembly)
    let estimated_tokens_saved = (total_captures * 500) + (total_retrievals * 2000);
    // Public estimate: 0.4 g CO2e per 1K tokens
    let estimated_co2_grams = (estimated_tokens_saved as f64 / 1000.0) * 0.4;

    Ok(Json(serde_json::json!({
        "total_captures": total_captures,
        "total_retrievals": total_retrievals,
        "estimated_tokens_saved": estimated_tokens_saved,
        "estimated_co2_grams": estimated_co2_grams,
        "co2_constant_g_per_1k_tokens": 0.4,
        "disclaimer": "Estimates based on public research (0.4 g CO2e per 1K tokens). Actual savings vary by model and workload."
    })))
}
