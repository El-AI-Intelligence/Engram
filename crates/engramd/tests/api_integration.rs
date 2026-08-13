// ── Integration tests: Engramd HTTP API ────────────────────────────────────
//
// These tests verify the HTTP API by starting an actual engramd server
// on a random port and making real HTTP requests via reqwest.
//
// Coverage:
//   - Health endpoint shape
//   - Memory lifecycle: capture → search → get → link → delete
//   - Error responses with correct status codes

use reqwest::StatusCode;
use serde_json::{json, Value};
use std::net::TcpListener;
use std::process::{Child, Command};
use std::time::Duration;

// ── Test harness ──────────────────────────────────────────────────────────

struct TestServer {
    child: Child,
    base_url: String,
}

impl TestServer {
    /// Start engramd on a random port with a temporary vault.
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let tmp = std::env::temp_dir().join(format!("engramd-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).unwrap();

        let child = Command::new(
            std::env::var("CARGO_BIN_EXE_engramd")
                .unwrap_or_else(|_| "target/debug/engramd".into()),
        )
        .arg("--vault")
        .arg(tmp.to_str().unwrap())
        .arg("--bind")
        .arg(format!("127.0.0.1:{}", port))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("failed to start engramd");

        let base_url = format!("http://127.0.0.1:{}", port);
        let server = TestServer { child, base_url };
        server.wait_ready();
        server
    }

    fn wait_ready(&self) {
        let client = reqwest::blocking::Client::new();
        for _ in 0..50 {
            if let Ok(resp) = client.get(&format!("{}/health", self.base_url)).send() {
                if resp.status().is_success() {
                    return;
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("engramd did not start within 5 seconds");
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn api() -> TestServer {
    TestServer::start()
}

fn post(url: &str, body: Value) -> (StatusCode, Value) {
    let client = reqwest::blocking::Client::new();
    let resp = client.post(url).json(&body).send().expect("POST failed");
    let status = resp.status();
    let json = resp.json().unwrap_or(Value::Null);
    (status, json)
}

fn get(url: &str) -> (StatusCode, Value) {
    let client = reqwest::blocking::Client::new();
    let resp = client.get(url).send().expect("GET failed");
    let status = resp.status();
    let json = resp.json().unwrap_or(Value::Null);
    (status, json)
}

fn del(url: &str) -> (StatusCode, Value) {
    let client = reqwest::blocking::Client::new();
    let resp = client.delete(url).send().expect("DELETE failed");
    let status = resp.status();
    let json = resp.json().unwrap_or(Value::Null);
    (status, json)
}

// ── Health endpoint ───────────────────────────────────────────────────────

#[test]
fn health_returns_expected_shape() {
    let srv = api();
    let (status, body) = get(&srv.url("/health"));
    assert_eq!(status, StatusCode::OK);
    assert!(body.get("status").is_some());
    assert!(body.get("version").is_some());
    assert_eq!(body["status"].as_str().unwrap(), "ok");
}

// ── Memory lifecycle ─────────────────────────────────────────────────────

#[test]
fn capture_search_get_delete_lifecycle() {
    let srv = api();

    // 1. Capture
    let (status, body) = post(
        &srv.url("/memories"),
        json!({"content": "Fixed the login timeout with 30s deadline", "tags": ["bug","login"], "source": "test"}),
    );
    assert_eq!(status, StatusCode::OK);
    let memory_id = body["id"].as_str().expect("should return id").to_string();
    assert!(!memory_id.is_empty(), "id must not be empty");

    // 2. Search
    let (status, body) = post(
        &srv.url("/memories/search"),
        json!({"query": "login timeout", "limit": 10, "search_mode": "fts5"}),
    );
    assert_eq!(status, StatusCode::OK);
    assert!(!body["results"].as_array().unwrap().is_empty());

    // 3. Get by ID
    let (status, body) = get(&srv.url(&format!("/memories/{}", memory_id)));
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["id"].as_str().unwrap(), memory_id);

    // 4. Link: POST /memories/link (global link endpoint)
    let (status, _) = post(
        &srv.url("/memories/link"),
        json!({"source_id": memory_id, "target_id": memory_id, "weight": 0.5, "link_type": "related"}),
    );
    assert_eq!(status, StatusCode::OK);

    // 5. Delete
    let (status, _) = del(&srv.url(&format!("/memories/{}", memory_id)));
    assert_eq!(status, StatusCode::OK);

    // 6. Verify deleted
    let (status, _) = get(&srv.url(&format!("/memories/{}", memory_id)));
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[test]
fn capture_multiple_and_search() {
    let srv = api();

    for i in 0..5 {
        let (status, _) = post(
            &srv.url("/memories"),
            json!({"content": format!("Memory number {}", i), "source": "test"}),
        );
        assert_eq!(status, StatusCode::OK);
    }

    // Search with empty query should return recent results
    let (status, body) = post(
        &srv.url("/memories/search"),
        json!({"query": "Memory", "limit": 20, "search_mode": "fts5"}),
    );
    assert_eq!(status, StatusCode::OK);
    assert!(body["results"].as_array().unwrap().len() >= 5);
}

// ── Error handling ────────────────────────────────────────────────────────

#[test]
fn not_found_returns_404_with_error_code() {
    let srv = api();
    let (status, body) = get(&srv.url("/memories/nonexistent-id-12345"));
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body["error"]["code"].as_str().is_some());
}

#[test]
fn missing_content_returns_error() {
    let srv = api();
    let (status, body) = post(&srv.url("/memories"), json!({"source": "test"}));
    assert!(status.is_client_error());
    assert!(body["error"].is_object());
}

#[test]
fn invalid_json_returns_400() {
    let srv = api();
    let client = reqwest::blocking::Client::new();
    let resp = client
        .post(srv.url("/memories"))
        .header("content-type", "application/json")
        .body("this is not json")
        .send()
        .unwrap();
    assert!(resp.status().is_client_error());
}

#[test]
fn health_always_returns_200() {
    let srv = api();
    let (status, _) = get(&srv.url("/health"));
    assert_eq!(status, StatusCode::OK);
}

// ── Context assembly ──────────────────────────────────────────────────────

#[test]
fn assemble_context_returns_messages() {
    let srv = api();

    post(&srv.url("/memories"), json!({"content": "src/auth.rs: Added JWT validation", "source": "test", "context": {"file": "src/auth.rs"}}));

    let (status, body) = post(
        &srv.url("/context/assemble"),
        json!({"query": "JWT validation", "dimensions": ["file_aware"], "token_budget": 4096}),
    );
    assert_eq!(status, StatusCode::OK);
    assert!(body.get("messages").is_some(), "response should have messages");
    assert!(body["messages"].as_array().is_some());
}

// ── Privacy endpoints ────────────────────────────────────────────────────

#[test]
fn privacy_audit_returns_expected_shape() {
    let srv = api();
    post(&srv.url("/memories"), json!({"content": "Test A", "source": "test-a"}));
    post(&srv.url("/memories"), json!({"content": "Test B", "source": "test-b"}));

    let (status, body) = get(&srv.url("/privacy/audit"));
    assert_eq!(status, StatusCode::OK);
    assert!(body.get("total_memories").is_some());
    assert!(body.get("breakdown").is_some());
}

#[test]
fn privacy_purge_requires_criteria() {
    let srv = api();
    let (status, body) = post(&srv.url("/privacy/purge"), json!({}));
    assert!(status.is_client_error());
    assert!(body["error"]["message"].as_str().unwrap_or("").contains("criterion"));
}

// ── Sync status ──────────────────────────────────────────────────────────

#[test]
fn sync_status_returns_expected_shape() {
    let srv = api();
    let (status, body) = get(&srv.url("/sync/status"));
    assert_eq!(status, StatusCode::OK);
    assert!(body.get("configured").is_some());
    assert!(body.get("device_id").is_some());
}

// ── Config ─────────────────────────────────────────────────────────────────

#[test]
fn get_config_returns_expected_shape() {
    let srv = api();
    let (status, body) = get(&srv.url("/config"));
    assert_eq!(status, StatusCode::OK);
    // Top-level config keys
    assert!(body.get("vault_path").is_some(), "config must include vault_path");
    assert!(body.get("sync").is_some(), "config must include sync block");
    assert!(body.get("schedule").is_some(), "config must include schedule block");
    // API key masked when present
    if let Some(k) = body["sync"]["api_key"].as_str() {
        assert_eq!(k, "••••••••", "api_key must be masked in GET response");
    }
}

#[test]
fn patch_config_updates_schedule() {
    let srv = api();

    // Use reqwest directly since config endpoint is PATCH, not POST
    let client = reqwest::blocking::Client::new();
    let resp = client
        .patch(srv.url("/config"))
        .json(&json!({"schedule": {"decay_cron": "0 3 * * *"}}))
        .send()
        .expect("PATCH config failed");

    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = resp.json().unwrap();
    assert!(body.get("vault_path").is_some());
}

// ── Analytics ──────────────────────────────────────────────────────────────

#[test]
fn analytics_stats_returns_expected_shape() {
    let srv = api();
    // Put some data in
    post(&srv.url("/memories"), json!({"content": "Stats test memory", "source": "test"}));

    let (status, body) = get(&srv.url("/analytics/stats"));
    assert_eq!(status, StatusCode::OK);
    assert!(body.get("total_memories").is_some());
    assert!(body.get("by_layer").is_some());
    assert!(body["by_layer"].is_object());
    assert!(body.get("total_links").is_some());
    assert!(body.get("avg_strength").is_some());
    assert!(body.get("db_size_bytes").is_some());
}

#[test]
fn analytics_activity_returns_date_counts() {
    let srv = api();

    // Capture memories with known source so they show up in activity
    for i in 0..3 {
        post(&srv.url("/memories"), json!({"content": format!("Activity day {}", i), "source": "test"}));
    }

    let (status, body) = get(&srv.url("/analytics/activity?days=7"));
    assert_eq!(status, StatusCode::OK);
    assert!(body.get("activity").is_some());
    assert!(body["activity"].is_array());
    // Each entry should have date + count
    if let Some(entries) = body["activity"].as_array() {
        if !entries.is_empty() {
            let first = &entries[0];
            assert!(first.get("day").is_some(), "activity entry must have day");
            assert!(first.get("count").is_some(), "activity entry must have count");
        }
    }
}

#[test]
fn analytics_co2_returns_shape() {
    let srv = api();
    let (status, body) = get(&srv.url("/analytics/co2"));
    assert_eq!(status, StatusCode::OK);
    // CO2 endpoint returns estimates
    assert!(body.get("estimated_co2_grams").is_some());
}

// ── Consolidation ──────────────────────────────────────────────────────────

#[test]
fn consolidation_history_returns_runs() {
    let srv = api();
    let (status, body) = get(&srv.url("/consolidate/history"));
    assert_eq!(status, StatusCode::OK);
    assert!(body.get("runs").is_some());
    assert!(body["runs"].is_array());
}

#[test]
fn consolidation_decay_succeeds() {
    let srv = api();

    // Add some memories so there's something to decay
    for i in 0..3 {
        post(&srv.url("/memories"), json!({"content": format!("Decay candidate {}", i), "source": "test"}));
    }

    let (status, body) = post(&srv.url("/consolidate/decay"), json!({}));
    assert_eq!(status, StatusCode::OK);
    assert!(body.get("ok").is_some());
    assert!(body["ok"].as_bool().unwrap_or(false));
    // Should report strengthened + decayed counts
    assert!(body.get("strengthened").is_some());
    assert!(body.get("decayed").is_some());
}

#[test]
fn consolidation_weekly_succeeds() {
    let srv = api();

    // Add some episodic memories for weekly consolidation to process
    for i in 0..5 {
        post(&srv.url("/memories"), json!({"content": format!("Weekly item {}", i), "source": "test", "layer": "episodic"}));
    }

    let (status, body) = post(&srv.url("/consolidate/weekly"), json!({}));
    assert_eq!(status, StatusCode::OK);
    assert!(body.get("ok").is_some());
}

// ── Export/Import ──────────────────────────────────────────────────────────

#[test]
fn export_returns_jsonl() {
    let srv = api();
    post(&srv.url("/memories"), json!({"content": "Export test", "source": "test"}));

    let (status, body) = post(&srv.url("/export"), json!({"limit": 5}));
    assert_eq!(status, StatusCode::OK);
    assert!(body.get("memories").is_some());
    assert!(body["memories"].is_array());
    assert!(body.get("exported_at").is_some());
}

#[test]
fn import_reimport_roundtrip() {
    let srv = api();

    // Export some data first (even if empty)
    let (_, export_body) = post(&srv.url("/export"), json!({"limit": 5}));
    let memories = export_body["memories"].as_array().unwrap();

    // Import them back (idempotent — already exist or dedup)
    let (status, body) = post(&srv.url("/import"), json!({"memories": memories}));
    assert_eq!(status, StatusCode::OK);
    assert!(body.get("imported").is_some());
}

// ── Saved searches ─────────────────────────────────────────────────────────

#[test]
fn saved_searches_lifecycle() {
    let srv = api();

    // Create a saved search
    let (status, body) = post(
        &srv.url("/searches"),
        json!({"name": "My bugs", "query": "bug fix", "search_mode": "fts5"}),
    );
    assert_eq!(status, StatusCode::OK);
    let search_id = body["id"].as_str().expect("should return id").to_string();

    // List saved searches — endpoint returns a bare array
    let (s2, body2) = get(&srv.url("/searches"));
    assert_eq!(s2, StatusCode::OK);
    assert!(body2.is_array(), "searches endpoint returns an array");

    // Delete saved search
    let (s3, _) = del(&srv.url(&format!("/searches/{}", search_id)));
    assert_eq!(s3, StatusCode::OK);
}
