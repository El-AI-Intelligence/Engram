// ── Annotation CRUD ──────────────────────────────────────────────────────
// User notes attached to memories.
//
// POST   /memories/:id/annotations  — create
// GET    /memories/:id/annotations  — list
// DELETE /annotations/:id            — delete

use crate::routes::err_json;
use crate::AppState;
use axum::{
    extract::{Path, State},
    routing::{delete, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Serialize)]
struct Annotation {
    id: String,
    memory_id: String,
    content: String,
    created_at: String,
}

#[derive(Debug, Deserialize)]
struct CreateAnnotation {
    content: String,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/memories/{id}/annotations", post(create).get(list))
        .route("/annotations/{annotation_id}", delete(delete_one))
}

async fn create(
    State(state): State<AppState>,
    Path(memory_id): Path<String>,
    Json(body): Json<CreateAnnotation>,
) -> Result<Json<Annotation>, (axum::http::StatusCode, Json<serde_json::Value>)> {
    if body.content.is_empty() {
        return Err(err_json(400, "content is required"));
    }

    let id = format!("ann_{}", Uuid::new_v4().to_string().replace('-', "")[..20].to_string());
    let created_at = chrono::Utc::now().to_rfc3339();

    let vault = state.vault.lock().await;

    // Verify the memory exists
    let exists: bool = vault
        .conn()
        .await
        .query_row(
            "SELECT COUNT(*) > 0 FROM engrams WHERE id = ?1",
            [&memory_id],
            |r| r.get(0),
        )
        .unwrap_or(false);

    if !exists {
        return Err(err_json(404, "memory not found"));
    }

    vault
        .conn()
        .await
        .execute(
            "INSERT INTO annotations (id, memory_id, content, created_at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![id, memory_id, body.content, created_at],
        )
        .map_err(|e| err_json(500, format!("Database error: {e}")))?;

    Ok(Json(Annotation {
        id,
        memory_id,
        content: body.content,
        created_at,
    }))
}

async fn list(
    State(state): State<AppState>,
    Path(memory_id): Path<String>,
) -> Result<Json<Vec<Annotation>>, (axum::http::StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().await;
    let conn_guard = vault.conn().await;

    let mut stmt = conn_guard
        .prepare("SELECT id, memory_id, content, created_at FROM annotations WHERE memory_id = ?1 ORDER BY created_at DESC")
        .map_err(|e| err_json(500, format!("Database error: {e}")))?;

    let rows = stmt
        .query_map([&memory_id], |row| {
            Ok(Annotation {
                id: row.get(0)?,
                memory_id: row.get(1)?,
                content: row.get(2)?,
                created_at: row.get(3)?,
            })
        })
        .map_err(|e| err_json(500, format!("Database error: {e}")))?;

    let mut annotations = Vec::new();
    for row in rows {
        annotations.push(row.map_err(|e| err_json(500, format!("Database error: {e}")))?);
    }

    Ok(Json(annotations))
}

async fn delete_one(
    State(state): State<AppState>,
    Path(annotation_id): Path<String>,
) -> Result<axum::http::StatusCode, (axum::http::StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().await;

    let affected = vault
        .conn()
        .await
        .execute("DELETE FROM annotations WHERE id = ?1", [&annotation_id])
        .map_err(|e| err_json(500, format!("Database error: {e}")))?;

    if affected == 0 {
        return Err(err_json(404, "annotation not found"));
    }

    Ok(axum::http::StatusCode::NO_CONTENT)
}
