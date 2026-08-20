// ── Privacy audit & purge routes ─────────────────────────────────────────────
//
// GET  /privacy/audit  — full breakdown of what data is stored
// POST /privacy/purge  — delete engrams matching criteria
//
// These endpoints give users transparency and control over their memory data.

use crate::routes::err_json;
use crate::AppState;
use axum::{
    extract::State,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/privacy/audit", get(audit))
        .route("/privacy/purge", post(purge))
}

// ── Audit ──────────────────────────────────────────────────────────────────

async fn audit(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().await;
    let conn = vault.conn().await;

    // Total count
    let total: i64 = conn
        .query_row("SELECT COUNT(*) FROM engrams", [], |r| r.get(0))
        .map_err(|e| err_json(500, format!("Database error: {e}")))?;

    // Breakdown by layer
    let by_layer: Vec<serde_json::Value> = {
        let mut stmt = conn
            .prepare(
                "SELECT layer, COUNT(*) as cnt \
                 FROM engrams GROUP BY layer ORDER BY cnt DESC",
            )
            .map_err(|e| err_json(500, format!("Database error: {e}")))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                ))
            })
            .map_err(|e| err_json(500, format!("Database error: {e}")))?;
        let mut v = Vec::new();
        for row in rows {
            let (layer, count) = row.map_err(|e| err_json(500, format!("Database error: {e}")))?;
            v.push(serde_json::json!({"layer": layer, "count": count}));
        }
        v
    };

    // Breakdown by source
    let by_source: Vec<serde_json::Value> = {
        let mut stmt = conn
            .prepare(
                "SELECT source, COUNT(*) as cnt \
                 FROM engrams GROUP BY source ORDER BY cnt DESC",
            )
            .map_err(|e| err_json(500, format!("Database error: {e}")))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                ))
            })
            .map_err(|e| err_json(500, format!("Database error: {e}")))?;
        let mut v = Vec::new();
        for row in rows {
            let (source, count) = row.map_err(|e| err_json(500, format!("Database error: {e}")))?;
            v.push(serde_json::json!({"source": source, "count": count}));
        }
        v
    };

    // Breakdown by project
    let by_project: Vec<serde_json::Value> = {
        let mut stmt = conn
            .prepare(
                "SELECT COALESCE(project, '(none)') as proj, COUNT(*) as cnt \
                 FROM engrams GROUP BY project ORDER BY cnt DESC",
            )
            .map_err(|e| err_json(500, format!("Database error: {e}")))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                ))
            })
            .map_err(|e| err_json(500, format!("Database error: {e}")))?;
        let mut v = Vec::new();
        for row in rows {
            let (project, count) = row.map_err(|e| err_json(500, format!("Database error: {e}")))?;
            v.push(serde_json::json!({"project": project, "count": count}));
        }
        v
    };

    // Oldest memory
    let oldest: Option<serde_json::Value> = {
        let mut stmt = conn
            .prepare(
                "SELECT id, content, created_at FROM engrams \
                 ORDER BY created_at ASC LIMIT 1",
            )
            .map_err(|e| err_json(500, format!("Database error: {e}")))?;
        stmt.query_row([], |row| {
            Ok(serde_json::json!({
                "id": row.get::<_, String>(0)?,
                "snippet": row.get::<_, String>(1)?.chars().take(200).collect::<String>(),
                "created_at": row.get::<_, String>(2)?,
            }))
        })
        .ok()
    };

    // Newest memory
    let newest: Option<serde_json::Value> = {
        let mut stmt = conn
            .prepare(
                "SELECT id, content, created_at FROM engrams \
                 ORDER BY created_at DESC LIMIT 1",
            )
            .map_err(|e| err_json(500, format!("Database error: {e}")))?;
        stmt.query_row([], |row| {
            Ok(serde_json::json!({
                "id": row.get::<_, String>(0)?,
                "snippet": row.get::<_, String>(1)?.chars().take(200).collect::<String>(),
                "created_at": row.get::<_, String>(2)?,
            }))
        })
        .ok()
    };

    // Average age in days (using julianday for SQLite date math)
    let avg_age_days: Option<f64> = conn
        .query_row(
            "SELECT AVG(julianday('now') - julianday(created_at)) FROM engrams",
            [],
            |r| r.get(0),
        )
        .ok()
        .flatten();

    // Date range (oldest and newest timestamp)
    let (oldest_date, newest_date): (Option<String>, Option<String>) = {
        let mut stmt = conn
            .prepare(
                "SELECT MIN(created_at), MAX(created_at) FROM engrams",
            )
            .map_err(|e| err_json(500, format!("Database error: {e}")))?;
        stmt.query_row([], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .map_err(|e| err_json(500, format!("Database error: {e}")))?
    };

    // DB size
    let db_size_bytes = state
        .vault_path
        .join("engrams.db")
        .metadata()
        .map(|m| m.len())
        .unwrap_or(0);

    // Total links and embeddings
    let total_links: i64 = conn
        .query_row("SELECT COUNT(*) FROM engram_links", [], |r| r.get(0))
        .unwrap_or(0);
    let total_embeddings: i64 = conn
        .query_row("SELECT COUNT(*) FROM engram_embeddings", [], |r| r.get(0))
        .unwrap_or(0);

    // Retention settings from config
    let retention_days: Option<u32> = {
        let config_path = state.vault_path.join("config.json");
        if let Ok(data) = std::fs::read_to_string(&config_path) {
            if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&data) {
                cfg.get("privacy")
                    .and_then(|p| p.get("retention_days"))
                    .and_then(|v| v.as_u64())
                    .map(|v| v as u32)
            } else {
                None
            }
        } else {
            None
        }
    };

    Ok(Json(serde_json::json!({
        "total_memories": total,
        "breakdown": {
            "by_layer": by_layer,
            "by_source": by_source,
            "by_project": by_project,
        },
        "oldest_memory": oldest,
        "newest_memory": newest,
        "oldest_date": oldest_date,
        "newest_date": newest_date,
        "avg_age_days": avg_age_days,
        "total_links": total_links,
        "total_embeddings": total_embeddings,
        "db_size_bytes": db_size_bytes,
        "estimated_db_size_human": format_size(db_size_bytes),
        "retention_days": retention_days,
        "sync_enabled": state.sync_enabled,
        "local_only": !state.sync_enabled,
    })))
}

// ── Purge ──────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct PurgeRequest {
    /// Only delete engrams from this source (e.g. "window", "clipboard", "cli")
    #[serde(default)]
    source: Option<String>,
    /// Only delete engrams of this layer ("episodic", "semantic", "imagined")
    #[serde(default)]
    layer: Option<String>,
    /// Only delete engrams from this project
    #[serde(default)]
    project: Option<String>,
    /// Only delete engrams created before this ISO-8601 date
    #[serde(default)]
    before_date: Option<String>,
}

async fn purge(
    State(state): State<AppState>,
    Json(req): Json<PurgeRequest>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, Json<serde_json::Value>)> {
    // Require at least one criterion — safety catch against accidental full-deletion
    if req.source.is_none()
        && req.layer.is_none()
        && req.project.is_none()
        && req.before_date.is_none()
    {
        return Err(err_json(
            400,
            "At least one purge criterion is required (source, layer, project, or before_date). \
             To delete everything, use POST /memories/clear instead.",
        ));
    }

    // `before_date` feeds a lexical SQL comparison against stored RFC3339
    // timestamps — a non-timestamp string ("z", "today", …) sorts AFTER every
    // real date and would match the entire vault. Validate before anything
    // touches the store.
    if let Some(d) = &req.before_date {
        if chrono::DateTime::parse_from_rfc3339(d).is_err() {
            return Err(err_json(
                400,
                format!(
                    "before_date must be an RFC3339 timestamp (e.g. 2026-08-01T00:00:00Z), got: {d}"
                ),
            ));
        }
    }

    let vault = state.vault.lock().await;
    let count = vault
        .purge_by_criteria(
            req.source.as_deref(),
            req.layer.as_deref(),
            req.project.as_deref(),
            req.before_date.as_deref(),
        )
        .await
        .map_err(|e| match e {
            axiom_engram::EngramError::Validation(msg) => err_json(400, msg),
            other => err_json(500, format!("Purge failed: {other}")),
        })?;

    Ok(Json(serde_json::json!({
        "deleted": count,
        "criteria": {
            "source": req.source,
            "layer": req.layer,
            "project": req.project,
            "before_date": req.before_date,
        },
    })))
}

// ── Helpers ────────────────────────────────────────────────────────────────

fn format_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB"];
    let mut size = bytes as f64;
    let mut unit_idx = 0;
    while size >= 1024.0 && unit_idx < UNITS.len() - 1 {
        size /= 1024.0;
        unit_idx += 1;
    }
    format!("{:.1} {}", size, UNITS[unit_idx])
}
