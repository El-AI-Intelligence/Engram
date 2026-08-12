// ── User-facing error codes ──────────────────────────────────────────────────
//
// Structured error responses for the Engram API. Every error returned to
// API consumers includes:
//   - `error.code`:    machine-readable kebab-case code (stable, safe to match on)
//   - `error.message`: human-readable description (may change between versions)
//   - `error.details`: optional extra context (field name, validation failure, etc.)
//
// Error codes are stable — clients can rely on them for control flow.
// Messages may be improved over time but the intent stays the same.

use axum::body::Body;
use axum::http::{Response, StatusCode};
use axum::Json;

/// Well-known error codes for the Engram API.
///
/// These are the codes API consumers should match on. The string values
/// are part of the public API contract.
pub mod code {
    #![allow(dead_code)] // API contract — codes used by consumers, not all internally
    // ── 4xx Client errors ────────────────────────────────────────────────
    pub const BAD_REQUEST: &str = "bad_request";
    pub const INVALID_JSON: &str = "invalid_json";
    pub const INVALID_LAYER: &str = "invalid_layer";
    pub const INVALID_SOURCE: &str = "invalid_source";
    pub const INVALID_PRIVACY: &str = "invalid_privacy_level";
    pub const INVALID_SCOPE: &str = "invalid_scope";
    pub const INVALID_CONTENT_TYPE: &str = "invalid_content_type";
    pub const INVALID_LINK_TYPE: &str = "invalid_link_type";
    pub const INVALID_DATE: &str = "invalid_date_format";
    pub const INVALID_VALENCE: &str = "invalid_valence";
    pub const INVALID_STRENGTH: &str = "invalid_strength";
    pub const CONTENT_EMPTY: &str = "content_empty";
    pub const CONTENT_TOO_LARGE: &str = "content_too_large";

    pub const NOT_FOUND: &str = "not_found";
    pub const MEMORY_NOT_FOUND: &str = "memory_not_found";
    pub const SEARCH_NOT_FOUND: &str = "saved_search_not_found";
    pub const LINK_NOT_FOUND: &str = "link_not_found";

    pub const UNAUTHORIZED: &str = "unauthorized";
    pub const FORBIDDEN: &str = "forbidden";
    pub const RATE_LIMITED: &str = "rate_limited";

    // ── 5xx Server errors ────────────────────────────────────────────────
    pub const INTERNAL: &str = "internal_error";
    pub const DATABASE_ERROR: &str = "database_error";
    pub const VAULT_LOCKED: &str = "vault_locked";
    pub const VAULT_CORRUPT: &str = "vault_corrupt";
    pub const STORAGE_FULL: &str = "storage_full";
}

/// Build a structured error JSON response.
///
/// ```ignore
/// error(code::MEMORY_NOT_FOUND, 404, "No memory with id mem_abc123");
/// ```
pub fn error(
    code: &str,
    status: u16,
    message: impl Into<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    let status_code =
        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let body = serde_json::json!({
        "error": {
            "code": code,
            "message": message.into(),
        }
    });
    (status_code, Json(body))
}

/// Build a structured error with additional details.
pub fn error_with_details(
    code: &str,
    status: u16,
    message: impl Into<String>,
    details: serde_json::Value,
) -> (StatusCode, Json<serde_json::Value>) {
    let status_code =
        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let body = serde_json::json!({
        "error": {
            "code": code,
            "message": message.into(),
            "details": details,
        }
    });
    (status_code, Json(body))
}

/// Convenience: 400 Bad Request with a structured error code.
pub fn bad_request(code: &str, message: impl Into<String>) -> (StatusCode, Json<serde_json::Value>) {
    error(code, 400, message)
}

/// Convenience: 404 Not Found with a structured error code.
pub fn not_found(code: &str, message: impl Into<String>) -> (StatusCode, Json<serde_json::Value>) {
    error(code, 404, message)
}

/// Convenience: 500 Internal Server Error with a structured error code.
pub fn internal(code: &str, message: impl Into<String>) -> (StatusCode, Json<serde_json::Value>) {
    error(code, 500, message)
}

/// Map a database error to a friendly structured error.
///
/// Uses `DATABASE_ERROR` code by default. The raw DB message goes into
/// `details` for debugging; the user-facing `message` is sanitized.
pub fn db_error(e: impl std::fmt::Display) -> (StatusCode, Json<serde_json::Value>) {
    let msg = e.to_string();
    // Don't leak raw SQL or internal paths to API consumers
    let user_msg = if msg.contains("SQLITE_CANTOPEN") || msg.contains("unable to open") {
        "The vault database could not be opened. It may be locked by another process or the path may not exist."
    } else if msg.contains("SQLITE_CORRUPT") || msg.contains("database disk image") {
        "The vault database appears to be corrupt. Restore from a backup or create a new vault."
    } else if msg.contains("SQLITE_READONLY") {
        "The vault database is read-only. Check file permissions."
    } else if msg.contains("SQLITE_FULL") || msg.contains("database or disk is full") {
        "No space left on device. Free up disk space and try again."
    } else if msg.contains("NOT NULL") {
        "A required field was missing when saving to the database."
    } else if msg.contains("UNIQUE constraint") {
        "A duplicate entry was rejected by the database."
    } else {
        "An internal database error occurred. The vault is intact but this operation could not complete."
    };
    error_with_details(
        code::DATABASE_ERROR,
        500,
        user_msg,
        serde_json::json!({"debug": msg}),
    )
}

// ── Backward compatibility shim ──────────────────────────────────────────────

/// Legacy helper — kept for existing route handlers that haven't been
/// migrated to structured errors yet. Prefer `error()`, `bad_request()`,
/// `not_found()`, or `db_error()` in new code.
///
/// Now infers the correct error code from the HTTP status instead of
/// always returning `internal_error`.
pub fn err_json(
    status: u16,
    msg: impl Into<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    let code = match status {
        400 => code::BAD_REQUEST,
        401 => code::UNAUTHORIZED,
        403 => code::FORBIDDEN,
        404 => code::NOT_FOUND,
        429 => code::RATE_LIMITED,
        500..=599 => code::INTERNAL,
        _ => code::INTERNAL,
    };
    error(code, status, msg)
}

// ── Extractor rejection handler ────────────────────────────────────────────────

/// Map axum extractor rejections (malformed JSON, wrong Content-Type,
/// oversized body, etc.) to structured JSON errors.
///
/// Only rewrites responses that do NOT already have a JSON Content-Type
/// header — handler-produced structured errors (from `bad_request()`,
/// `not_found()`, etc.) are left intact so their specific error codes
/// and messages reach API consumers.
///
/// Wired into the router via `tower::ServiceBuilder::map_response(...)`.
pub fn handle_extractor_rejection(response: Response<Body>) -> Response<Body> {
    let status = response.status();

    // If the handler already produced a structured JSON error, leave it
    // alone — only rewrite extractor-generated plain-text/HTML responses.
    let is_already_json = response
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.starts_with("application/json"))
        .unwrap_or(false);
    if is_already_json {
        return response;
    }

    if status == StatusCode::UNPROCESSABLE_ENTITY
        || status == StatusCode::PAYLOAD_TOO_LARGE
        || (status.is_client_error() && status != StatusCode::TOO_MANY_REQUESTS
            && status != StatusCode::UNAUTHORIZED
            && status != StatusCode::FORBIDDEN
            && status != StatusCode::NOT_FOUND)
    {
        let code = match status {
            s if s == StatusCode::PAYLOAD_TOO_LARGE => code::CONTENT_TOO_LARGE,
            _ => code::BAD_REQUEST,
        };
        let message = match status {
            s if s == StatusCode::PAYLOAD_TOO_LARGE =>
                "Request body too large. Maximum is 10 MiB.",
            _ => "Invalid request body — expected valid JSON with correct Content-Type.",
        };
        let error_body = serde_json::json!({
            "error": {
                "code": code,
                "message": message,
            }
        });
        let json_body = serde_json::to_string(&error_body).unwrap_or_default();
        let (mut parts, _) = response.into_parts();
        parts.headers.insert(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static("application/json"),
        );
        parts.status = status;
        Response::from_parts(parts, Body::from(json_body))
    } else {
        response
    }
}
