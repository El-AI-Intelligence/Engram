// ── API authentication middleware ────────────────────────────────────────────
//
// Bearer token auth that follows the same pattern as the sync server:
//   - Auth is optional when no API key is configured (loopback mode).
//   - When ENGRAMD_API_KEY is set, all non-health endpoints require
//     Authorization: Bearer <key>.
//   - Constant-time key comparison to prevent timing side-channel attacks.
//
// The /health endpoint is always public (no auth required).

use axum::{
    extract::Request,
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Per-key rate limiter: simple token bucket with 1-second refill.
/// Same pattern as engramd-sync's RateLimiter.
pub struct RateLimiter {
    tokens: f64,
    max_tokens: f64,
    last_refill: std::time::Instant,
}

impl RateLimiter {
    fn new(rate: f64) -> Self {
        Self { tokens: rate, max_tokens: rate, last_refill: std::time::Instant::now() }
    }

    /// Returns true if a request is allowed (consumes 1 token).
    fn allow(&mut self) -> bool {
        let now = std::time::Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.tokens = (self.tokens + elapsed * (self.max_tokens / 1.0)).min(self.max_tokens);
        self.last_refill = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// State needed by the auth middleware — shared with the router.
#[derive(Clone)]
pub struct AuthState {
    /// The expected API key (loaded from ENGRAMD_API_KEY env var).
    /// None means auth is disabled (loopback/development mode).
    pub api_key: Option<String>,
    /// Whether the server is bound to loopback (affects startup warnings).
    pub is_loopback: bool,
    /// Rate limiter per configured key (token bucket, 100 req/s default).
    rate_limiter: Arc<Mutex<RateLimiter>>,
}

impl AuthState {
    /// Create auth state from environment.
    ///
    /// Reads `ENGRAMD_API_KEY` from the environment. If not set and binding
    /// is non-loopback, logs a warning. If set, requires all requests to
    /// include the key (except /health).
    pub fn from_env(is_loopback: bool) -> Self {
        let api_key = std::env::var("ENGRAMD_API_KEY").ok().map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty());

        Self {
            api_key,
            is_loopback,
            rate_limiter: Arc::new(Mutex::new(RateLimiter::new(100.0))),
        }
    }

    /// Check that startup is safe. Returns an error string if the daemon
    /// should refuse to start — specifically, when binding to a non-loopback
    /// address without an API key configured. This matches engramd-sync's
    /// default-secure pattern.
    pub fn check_startup_safe(&self, bind: std::net::SocketAddr) -> Result<(), String> {
        if self.api_key.is_none() && !self.is_loopback {
            Err(format!(
                "ENGRAMD_API_KEY is not set but daemon is binding to {bind}. \
                 Refusing to start — authentication is required on non-loopback addresses. \
                 Set ENGRAMD_API_KEY or bind to 127.0.0.1.",
            ))
        } else {
            Ok(())
        }
    }

    /// Check if authentication is required.
    #[allow(dead_code)]
    pub fn requires_auth(&self) -> bool {
        self.api_key.is_some()
    }
}

/// Axum middleware that validates the Bearer token on all non-health routes.
///
/// Skips auth on:
///   - /health (always public)
///   - When no API key is configured (development mode)
///
/// When auth is required, also enforces rate limiting (100 req/s default).
pub async fn auth_middleware(
    axum::extract::State(auth): axum::extract::State<AuthState>,
    request: Request,
    next: Next,
) -> Result<Response, Response> {
    // Always allow /health
    if request.uri().path() == "/health" {
        return Ok(next.run(request).await);
    }

    // Auth is optional when no key is configured
    let Some(ref expected_key) = auth.api_key else {
        return Ok(next.run(request).await);
    };

    // Check Authorization header
    authenticate(&request.headers(), expected_key)?;

    // Rate limiting check (token bucket, 100 req/s)
    {
        let mut limiter = auth.rate_limiter.lock().await;
        if !limiter.allow() {
            return Err(auth_error(429, "rate limit exceeded"));
        }
    }

    Ok(next.run(request).await)
}

/// Validate the Authorization header against the expected key.
/// Uses constant-time comparison to prevent timing attacks.
fn authenticate(headers: &HeaderMap, expected_key: &str) -> Result<(), Response> {
    let auth_header = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| {
            auth_error(401, "missing Authorization header")
        })?;

    let token = auth_header
        .strip_prefix("Bearer ")
        .ok_or_else(|| {
            auth_error(401, "expected Bearer token")
        })?;

    if token.is_empty() {
        return Err(auth_error(401, "empty token"));
    }

    if !constant_time_eq(token.as_bytes(), expected_key.as_bytes()) {
        return Err(auth_error(401, "invalid API key"));
    }

    Ok(())
}

/// Constant-time byte comparison to prevent timing side-channel attacks.
pub(crate) fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut acc = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        acc |= x ^ y;
    }
    std::hint::black_box(acc == 0)
}

/// Build a JSON auth error response (401 or 429).
fn auth_error(status: u16, msg: &str) -> Response {
    let code = StatusCode::from_u16(status).unwrap_or(StatusCode::UNAUTHORIZED);
    let body = axum::Json(json!({"error": msg, "code": if status == 429 { "rate_limited" } else { "unauthorized" }}));
    (code, body).into_response()
}
