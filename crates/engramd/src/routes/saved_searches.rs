use crate::routes::err_json;
use crate::AppState;
use axum::{
    extract::{Path, State},
    routing::{delete, get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Serialize)]
struct SavedSearch {
    id: String,
    query: String,
    layer: Option<String>,
    tags: Option<String>,
    notify: bool,
    created_at: String,
    last_checked_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreateSavedSearch {
    #[serde(default)]
    query: String,
    layer: Option<String>,
    tags: Option<String>,
    #[serde(default)]
    notify: bool,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/searches", post(create).get(list))
        .route("/searches/{id}", get(get_one))
        .route("/searches/{id}/update", post(update_search))
        .route("/searches/{id}", delete(delete_search))
}

async fn create(
    State(state): State<AppState>,
    Json(body): Json<CreateSavedSearch>,
) -> Result<Json<SavedSearch>, (axum::http::StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().await;
    let id = format!("ss_{}", Uuid::new_v4().to_string().replace('-', "")[..20].to_string());
    let created_at = chrono::Utc::now().to_rfc3339();
    vault
        .conn()
        .await
        .execute(
            "INSERT INTO saved_searches (id, query, layer, tags, notify, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![id, body.query, body.layer, body.tags, body.notify as i32, created_at],
        )
        .map_err(|e| err_json(500, format!("Database error: {e}")))?;
    Ok(Json(SavedSearch {
        id,
        query: body.query,
        layer: body.layer,
        tags: body.tags,
        notify: body.notify,
        created_at,
        last_checked_at: None,
    }))
}

async fn list(
    State(state): State<AppState>,
) -> Result<Json<Vec<SavedSearch>>, (axum::http::StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().await;
    let conn_guard = vault.conn().await;
    let mut stmt = conn_guard
        .prepare(
            "SELECT id, query, layer, tags, notify, created_at, last_checked_at \
             FROM saved_searches ORDER BY created_at DESC",
        )
        .map_err(|e| err_json(500, format!("Database error: {e}")))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(SavedSearch {
                id: row.get(0)?,
                query: row.get::<_, String>(1).unwrap_or_default(),
                layer: row.get(2)?,
                tags: row.get(3)?,
                notify: row.get::<_, i32>(4).unwrap_or(0) != 0,
                created_at: row.get(5)?,
                last_checked_at: row.get(6)?,
            })
        })
        .map_err(|e| err_json(500, format!("Database error: {e}")))?;
    let mut searches = Vec::new();
    for row in rows {
        searches.push(row.map_err(|e| err_json(500, format!("Database error: {e}")))?);
    }
    Ok(Json(searches))
}

async fn get_one(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SavedSearch>, (axum::http::StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().await;
    let result = vault
        .conn()
        .await
        .query_row(
            "SELECT id, query, layer, tags, notify, created_at, last_checked_at \
             FROM saved_searches WHERE id = ?1",
            [&id],
            |row| {
                Ok(SavedSearch {
                    id: row.get(0)?,
                    query: row.get::<_, String>(1).unwrap_or_default(),
                    layer: row.get(2)?,
                    tags: row.get(3)?,
                    notify: row.get::<_, i32>(4).unwrap_or(0) != 0,
                    created_at: row.get(5)?,
                    last_checked_at: row.get(6)?,
                })
            },
        )
        .map_err(|e| err_json(404, format!("saved search not found: {e}")))?;
    Ok(Json(result))
}

async fn update_search(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<CreateSavedSearch>,
) -> Result<Json<SavedSearch>, (axum::http::StatusCode, Json<serde_json::Value>)> {
    // Parameterized UPDATE — no SQL string building.
    // Build the SET clause with only the non-empty/Some fields.
    let mut set_parts: Vec<String> = Vec::new();
    let mut param_values: Vec<String> = Vec::new();

    if !body.query.is_empty() {
        set_parts.push("query = ?".to_string());
        param_values.push(body.query.clone());
    }
    if let Some(ref l) = body.layer {
        set_parts.push("layer = ?".to_string());
        param_values.push(l.clone());
    }
    if let Some(ref t) = body.tags {
        set_parts.push("tags = ?".to_string());
        param_values.push(t.clone());
    }
    // Always set notify so it can be toggled both directions.
    set_parts.push("notify = ?".to_string());
    param_values.push(if body.notify { "1".to_string() } else { "0".to_string() });

    if set_parts.is_empty() {
        return get_one(State(state), Path(id)).await;
    }

    // Build parameterized SQL
    let sql = format!(
        "UPDATE saved_searches SET {} WHERE id = ?{}",
        set_parts.join(", "),
        set_parts.len() + 1
    );

    // Scoped to ensure conn guard is dropped before we re-acquire vault in get_one
    {
        let vault = state.vault.lock().await;
        let conn = vault.conn().await;

        let mut all_params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        for v in &param_values {
            all_params.push(Box::new(v.clone()));
        }
        all_params.push(Box::new(id.clone()));
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            all_params.iter().map(|p| p.as_ref()).collect();
        conn.execute(&sql, param_refs.as_slice())
            .map_err(|e| err_json(500, format!("Database error: {e}")))?;
    }

    get_one(State(state), Path(id)).await
}

async fn delete_search(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().await;
    let affected = vault
        .conn()
        .await
        .execute("DELETE FROM saved_searches WHERE id = ?1", [&id])
        .map_err(|e| err_json(500, format!("Database error: {e}")))?;
    if affected == 0 {
        return Err(err_json(404, "saved search not found"));
    }
    Ok(Json(serde_json::json!({"ok": true})))
}
