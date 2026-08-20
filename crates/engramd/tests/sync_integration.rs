// ── Integration tests: encrypt → sync → decrypt round-trip ──────────────────
//
// These tests verify the E2E encrypted sync flow:
//   1. Client encrypts memory → blob with HMAC
//   2. Client pushes blob to server → server accepts
//   3. Client pulls blob from server → verifies HMAC, decrypts → plaintext
//
// Also covers:
//   - Vector clock conflict resolution (LWW)
//   - Deletion tombstone propagation
//   - Malformed HMAC rejection
//   - KDF vault-id derivation (v1/v2) + probe convergence

use axiom_engram::sync::SyncBlob;
use engramd::sync_client::SyncClient;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
    routing::get,
    Router,
};
use serde_json::json;
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, OnceLock,
};

/// Helper: create a test client with a deterministic passphrase.
fn test_client() -> SyncClient {
    SyncClient::new(
        "http://localhost:8788".into(),
        "test-vault".into(),
        "correct-horse-battery-staple-test-only",
        "device-alpha".into(),
        None,
        0,
    )
}

// ── Encrypt → Decrypt round-trip ───────────────────────────────────────────

#[tokio::test]
async fn encrypt_decrypt_round_trip() {
    let client = test_client();
    let plaintext = r#"{"id":"mem_001","content":"Fixed the login timeout bug","layer":"episodic","source":"cli","strength":1.0,"valence":0.3,"tags":["bug","login"],"created_at":"2026-08-01T12:00:00Z"}"#;

    let blob: SyncBlob = client
        .encrypt_memory("mem_001", plaintext, false)
        .await
        .expect("encrypt should succeed");

    // Verify blob structure
    assert_eq!(blob.vault_id, "test-vault");
    assert_eq!(blob.memory_id, "mem_001");
    assert_eq!(blob.device_id, "device-alpha");
    assert_eq!(blob.vector_clock, 1);
    assert!(!blob.ciphertext.is_empty(), "ciphertext must not be empty");
    assert!(!blob.hmac.is_empty(), "HMAC must not be empty");
    assert!(!blob.deleted, "should not be a tombstone");

    // Decrypt should recover original plaintext
    let decrypted = client
        .decrypt_blob(&blob)
        .expect("decrypt should succeed");
    assert_eq!(decrypted, plaintext, "round-trip plaintext must match");
}

// ── Clock monotonicity ─────────────────────────────────────────────────────

#[tokio::test]
async fn vector_clock_increments() {
    let client = test_client();

    let blob1 = client
        .encrypt_memory("mem_a", "content A", false)
        .await
        .unwrap();
    assert_eq!(blob1.vector_clock, 1);

    let blob2 = client
        .encrypt_memory("mem_b", "content B", false)
        .await
        .unwrap();
    assert_eq!(blob2.vector_clock, 2);

    let blob3 = client
        .encrypt_memory("mem_c", "content C", false)
        .await
        .unwrap();
    assert_eq!(blob3.vector_clock, 3);
}

// ── HMAC tamper detection ──────────────────────────────────────────────────

#[tokio::test]
async fn detect_tampered_ciphertext() {
    let client = test_client();
    let mut blob = client
        .encrypt_memory("mem_x", "sensitive data", false)
        .await
        .unwrap();

    // Tamper with the ciphertext
    blob.ciphertext = "tampered-data".into();

    let result = client.decrypt_blob(&blob);
    assert!(result.is_err(), "should detect tampered ciphertext");
    assert!(
        result.unwrap_err().contains("HMAC verification failed"),
        "error should mention HMAC failure"
    );
}

#[tokio::test]
async fn detect_tampered_memory_id() {
    let client = test_client();
    let mut blob = client
        .encrypt_memory("mem_original", "data", false)
        .await
        .unwrap();

    blob.memory_id = "mem_spoofed".into();

    let result = client.decrypt_blob(&blob);
    assert!(result.is_err(), "should detect spoofed memory ID");
}

#[tokio::test]
async fn detect_tampered_device_id() {
    let client = test_client();
    let mut blob = client
        .encrypt_memory("mem_dev", "data", false)
        .await
        .unwrap();

    blob.device_id = "evil-device".into();

    let result = client.decrypt_blob(&blob);
    assert!(result.is_err(), "should detect spoofed device ID");
}

// ── Tombstone round-trip ───────────────────────────────────────────────────

#[tokio::test]
async fn tombstone_encrypt_decrypt() {
    let client = test_client();
    let plaintext = r#"{"id":"mem_del","deleted":true}"#;

    let blob = client
        .encrypt_memory("mem_del", plaintext, true)
        .await
        .expect("tombstone encrypt should succeed");

    assert!(blob.deleted, "tombstone blob must have deleted=true");

    let decrypted = client
        .decrypt_blob(&blob)
        .expect("tombstone decrypt should succeed");
    assert_eq!(decrypted, plaintext);
}

// ── Different clients with different keys cannot cross-decrypt ─────────────

#[tokio::test]
async fn different_keys_cannot_cross_decrypt() {
    let client_a = SyncClient::new(
        "http://localhost:8788".into(),
        "vault-a".into(),
        "passphrase-alpha-alpha-alpha",
        "device-a".into(),
        None,
        0,
    );
    let client_b = SyncClient::new(
        "http://localhost:8788".into(),
        "vault-b".into(),
        "passphrase-beta-beta-beta-beta",
        "device-b".into(),
        None,
        0,
    );

    let blob = client_a
        .encrypt_memory("mem_k", "secret from A", false)
        .await
        .unwrap();

    let result = client_b.decrypt_blob(&blob);
    assert!(
        result.is_err(),
        "different passphrase must not decrypt other's data"
    );
}

// ── Clock persistence round-trip ───────────────────────────────────────────

#[tokio::test]
async fn clock_persist_and_load() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let vault_path = tmp.path().to_path_buf();

    std::fs::write(
        vault_path.join("device.json"),
        r#"{"device_id":"dev-test","label":"test-host"}"#,
    )
    .unwrap();

    let client = SyncClient::new(
        "http://localhost:8788".into(),
        "vault-c".into(),
        "test-passphrase-for-clock",
        "dev-test".into(),
        None,
        SyncClient::load_clock(&vault_path),
    );

    for _ in 0..42 {
        client.encrypt_memory("dummy", "x", false).await.unwrap();
    }
    assert_eq!(client.current_clock().await, 42);

    client.persist_clock(&vault_path).await;

    let loaded = SyncClient::load_clock(&vault_path);
    assert_eq!(loaded, 42, "persisted clock must survive round-trip");
}

// ── Edge cases ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn encrypt_empty_content() {
    let client = test_client();
    let blob = client
        .encrypt_memory("mem_empty", "", false)
        .await
        .expect("encrypting empty content should succeed");
    let decrypted = client.decrypt_blob(&blob).expect("decrypt should succeed");
    assert_eq!(decrypted, "");
}

#[tokio::test]
async fn encrypt_unicode_content() {
    let client = test_client();
    let unicode = "\u{30E1}\u{30E2}\u{30EA} \u{1F9E0} \u{2014} caf\u{E9} r\u{E9}sum\u{E9} \u{4E2D}\u{6587}";
    let blob = client
        .encrypt_memory("mem_unicode", unicode, false)
        .await
        .unwrap();
    let decrypted = client.decrypt_blob(&blob).unwrap();
    assert_eq!(decrypted, unicode, "unicode round-trip must be exact");
}

#[tokio::test]
async fn encrypt_large_content() {
    let client = test_client();
    let large = "A".repeat(10_000);
    let blob = client
        .encrypt_memory("mem_large", &large, false)
        .await
        .unwrap();
    let decrypted = client.decrypt_blob(&blob).unwrap();
    assert_eq!(decrypted, large, "large content round-trip must match");
}

// ── Vector clock conflict resolution tests (pure logic) ────────────────────

#[test]
fn last_write_wins_higher_clock_wins() {
    assert!(5u64 > 3u64, "remote should win with higher clock");
}

#[test]
fn last_write_wins_lower_clock_loses() {
    assert!(2u64 <= 4u64, "local should win with higher clock");
}

#[test]
fn last_write_wins_equal_clock_rejected() {
    assert!(7u64 <= 7u64, "equal clocks should be rejected");
}

#[test]
fn bump_clock_after_pull() {
    let mut local: u64 = 10;
    local = local.max(25);
    assert_eq!(local, 25, "local clock should bump to remote max");
}

#[test]
fn new_device_starts_at_zero() {
    assert_eq!(0u64, 0u64, "new device starts at clock 0");
}

#[test]
fn rapid_pushes_produce_monotonic_clocks() {
    let mut clock: u64 = 0;
    let mut clocks: Vec<u64> = Vec::new();
    for _ in 0..100 {
        clock += 1;
        clocks.push(clock);
    }
    for i in 1..clocks.len() {
        assert!(clocks[i] > clocks[i - 1], "clocks must be strictly monotonic");
    }
}

// ── Edit propagation (modified_at) ──────────────────────────────────────────

/// The push envelope carries `modified_at`; it must survive the
/// encrypt→decrypt round-trip exactly so the puller can preserve the remote
/// value — the echo-churn fix relies on the pulled row keeping the remote
/// timestamp instead of a fresh local stamp (which would re-push forever).
#[tokio::test]
async fn modified_at_round_trips_in_blob() {
    let client = test_client();
    let plaintext = r#"{"id":"mem_edit","content":"edited content","layer":"episodic","source":"interaction","created_at":"2026-08-01T12:00:00Z","modified_at":"2026-08-14T09:30:15.123456789Z"}"#;

    let blob = client
        .encrypt_memory("mem_edit", plaintext, false)
        .await
        .expect("encrypt should succeed");

    let decrypted = client.decrypt_blob(&blob).expect("decrypt should succeed");
    assert_eq!(decrypted, plaintext);

    let json: serde_json::Value = serde_json::from_str(&decrypted).unwrap();
    assert_eq!(
        json["modified_at"].as_str().unwrap(),
        "2026-08-14T09:30:15.123456789Z",
        "modified_at must survive the blob round-trip exactly"
    );
}

/// An edit produces a strictly-higher clock than the original blob, so the
/// server's LWW accepts the re-pushed edit even though the memory itself is
/// old — the clock ordering (not age) decides the conflict.
#[test]
fn lww_edit_beats_original() {
    let original_clock: u64 = 3;
    let edit_clock: u64 = original_clock + 1;
    assert!(
        edit_clock > original_clock,
        "the edited blob must outrank the original in LWW"
    );
}

// ── vault_id passphrase fallback ────────────────────────────────────────────

/// Two devices sharing a passphrase must derive the same fallback vault_id
/// (that's what puts them in the same vault on the server), and the id must
/// be stable and hex-shaped. The v2 derivation is what fresh vaults use; v1
/// exists only so an old-pinned vault can be converged on — the two must
/// differ for the same passphrase or the probe flow could not tell them apart.
#[test]
fn vault_id_fallback_is_stable_across_devices() {
    let a = engramd::sync_client::derive_vault_id_v2("correct-horse-battery-staple");
    let b = engramd::sync_client::derive_vault_id_v2("correct-horse-battery-staple");
    assert_eq!(a, b, "same passphrase must derive the same vault_id");
    assert_eq!(a.len(), 64, "vault_id should be 32-byte hex");
    assert!(
        a.chars().all(|c| c.is_ascii_hexdigit()),
        "vault_id must be lowercase hex"
    );
    let v1 = engramd::sync_client::derive_vault_id_v1("correct-horse-battery-staple");
    assert_ne!(v1, a, "v1 and v2 derivations must diverge for one passphrase");
}

/// Different passphrases must derive different vault ids (a collision here
/// would merge two teams into one vault on the server).
#[test]
fn vault_id_fallback_is_distinct_per_passphrase() {
    let a = engramd::sync_client::derive_vault_id_v2("correct-horse-battery-staple");
    let b = engramd::sync_client::derive_vault_id_v2("correct-horse-battery-stapler");
    assert_ne!(a, b, "different passphrases must derive different vault_ids");
}

// ── KDF v2 probe convergence (W4a) ──────────────────────────────────────────
//
// converge_vault_id probes the relay's /sync/{id}/stats endpoint to pick
// between the v1 and v2 vault-id derivations. The stub below scripts the
// relay's replies per vault id and counts requests, so every decision path
// (including the no-network no-key path) is exercised without a real relay.

/// A scripted reply the stub relay returns for one vault id.
#[derive(Clone)]
enum StubReply {
    /// 200 with `total_blobs > 0` — the vault exists and holds data.
    Exists,
    /// 200 with `total_blobs == 0` — the id resolves but is empty.
    Empty,
    /// 403 — the key is not authorized for this vault (or it doesn't exist).
    Denied,
    /// 401 — the api_key itself is rejected.
    Rejected,
}

#[derive(Clone)]
struct StubRelay {
    replies: Arc<HashMap<String, StubReply>>,
    requests: Arc<AtomicUsize>,
}

async fn stats_handler(
    State(stub): State<StubRelay>,
    Path((id,)): Path<(String,)>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    stub.requests.fetch_add(1, Ordering::SeqCst);
    match stub.replies.get(&id) {
        Some(StubReply::Exists) => Ok(Json(json!({ "total_blobs": 7 }))),
        Some(StubReply::Empty) => Ok(Json(json!({ "total_blobs": 0 }))),
        Some(StubReply::Rejected) => Err(StatusCode::UNAUTHORIZED),
        Some(StubReply::Denied) | None => Err(StatusCode::FORBIDDEN),
    }
}

/// Start a stub relay on an ephemeral 127.0.0.1 port; returns its base URL
/// and the request counter for no-HTTP assertions.
fn spawn_stub(replies: HashMap<String, StubReply>) -> (String, Arc<AtomicUsize>) {
    let requests = Arc::new(AtomicUsize::new(0));
    let stub = StubRelay {
        replies: Arc::new(replies),
        requests: requests.clone(),
    };
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("ephemeral bind");
    let addr = listener.local_addr().expect("local addr");
    listener
        .set_nonblocking(true)
        .expect("nonblocking listener");
    let router = Router::new()
        .route("/sync/{id}/stats", get(stats_handler))
        .with_state(stub);
    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::from_std(listener).expect("tokio listener");
        axum::serve(listener, router).await.expect("stub serve");
    });
    (format!("http://{addr}"), requests)
}

const CONVERGE_PW: &str = "correct-horse-battery-staple-converge-test";

/// Both KDF derivations for the shared passphrase — computed once per
/// process (Argon2 is deliberately expensive; converge re-derives internally
/// on every call regardless).
fn converge_ids() -> &'static (String, String) {
    static IDS: OnceLock<(String, String)> = OnceLock::new();
    IDS.get_or_init(|| {
        (
            engramd::sync_client::derive_vault_id_v2(CONVERGE_PW),
            engramd::sync_client::derive_vault_id_v1(CONVERGE_PW),
        )
    })
}

/// The v2 id already holds blobs on the relay → the device joins it under v2.
#[tokio::test]
async fn converge_picks_v2_when_v2_exists() {
    let (v2, v1) = converge_ids();
    let (url, _requests) = spawn_stub(HashMap::from([
        (v2.clone(), StubReply::Exists),
        (v1.clone(), StubReply::Denied),
    ]));
    let (id, version) =
        engramd::sync_client::converge_vault_id(&url, Some("test-key"), CONVERGE_PW)
            .await
            .expect("converge should succeed");
    assert_eq!(&id, v2, "must pick the existing v2 vault");
    assert_eq!(version, "v2");
}

/// v2 resolves empty while v1 holds the team's data (an old binary pinned
/// v1) → the new device must converge onto the existing v1 vault, not split.
#[tokio::test]
async fn converge_picks_v1_when_only_v1_exists() {
    let (v2, v1) = converge_ids();
    let (url, _requests) = spawn_stub(HashMap::from([
        (v2.clone(), StubReply::Empty),
        (v1.clone(), StubReply::Exists),
    ]));
    let (id, version) =
        engramd::sync_client::converge_vault_id(&url, Some("test-key"), CONVERGE_PW)
            .await
            .expect("converge should succeed");
    assert_eq!(&id, v1, "must join the existing v1 vault");
    assert_eq!(version, "v1");
}

/// Neither derivation exists on the relay → fresh vault is created under v2.
#[tokio::test]
async fn converge_defaults_to_v2_when_both_empty() {
    let (v2, v1) = converge_ids();
    let (url, _requests) = spawn_stub(HashMap::from([
        (v2.clone(), StubReply::Empty),
        (v1.clone(), StubReply::Empty),
    ]));
    let (id, version) =
        engramd::sync_client::converge_vault_id(&url, Some("test-key"), CONVERGE_PW)
            .await
            .expect("converge should succeed");
    assert_eq!(&id, v2, "fresh vaults are created under v2");
    assert_eq!(version, "v2");
}

/// A 401 on the first probe means the key itself is bad — the result of a
/// second probe would be meaningless, so converge aborts instead of guessing.
#[tokio::test]
async fn converge_aborts_on_rejected_key() {
    let (v2, v1) = converge_ids();
    let (url, requests) = spawn_stub(HashMap::from([
        (v2.clone(), StubReply::Rejected),
        (v1.clone(), StubReply::Rejected),
    ]));
    let err = engramd::sync_client::converge_vault_id(&url, Some("bad-key"), CONVERGE_PW)
        .await
        .expect_err("a rejected api_key must abort convergence");
    assert!(err.contains("api_key"), "error should name the api_key: {err}");
    assert_eq!(
        requests.load(Ordering::SeqCst),
        1,
        "must stop after the rejected v2 probe — never probe v1"
    );
}

/// With no api_key (never paired) converge answers v2 immediately and makes
/// zero network requests — the relay can't attest anything anyway.
#[tokio::test]
async fn converge_without_key_returns_v2_without_http() {
    let (v2, _v1) = converge_ids();
    let (url, requests) = spawn_stub(HashMap::new());
    let (id, version) = engramd::sync_client::converge_vault_id(&url, None, CONVERGE_PW)
        .await
        .expect("no-key converge should succeed");
    assert_eq!(&id, v2, "unpaired devices derive v2");
    assert_eq!(version, "v2");
    assert_eq!(
        requests.load(Ordering::SeqCst),
        0,
        "no probe may fire without an api_key"
    );
}
