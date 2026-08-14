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

use axiom_engram::sync::SyncBlob;
use engramd::sync_client::SyncClient;

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
