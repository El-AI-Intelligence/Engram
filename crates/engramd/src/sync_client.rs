// ── Client-side sync module ─────────────────────────────────────────────────
// Handles E2E encrypted push/pull to the sync server.
//
// Design:
//   - AES-256-GCM encryption with random nonces (prepended to ciphertext)
//   - HMAC-SHA256 integrity (vault_id + memory_id + device_id + clock + ciphertext)
//   - Monotonic vector clocks for last-write-wins conflict resolution
//   - Device ID is the persisted one from device.json (survives restarts)

use aes_gcm::aead::{Aead, OsRng};
use aes_gcm::{AeadCore, Aes256Gcm, Key, KeyInit, Nonce};
use axiom_engram::sync::{PullResponse, PushRequest, PushResponse, SyncBlob};
use base64::Engine;
use hmac::Mac;
use sha2::Sha256;
use std::sync::Arc;
use tokio::sync::Mutex;

type HmacSha256 = hmac::Hmac<Sha256>;

/// Client for pushing/pulling encrypted memory blobs to the sync server.
pub struct SyncClient {
    http: reqwest::Client,
    server_url: String,
    vault_id: String,
    device_id: String,
    encryption_key: [u8; 32],
    hmac_key: [u8; 32],
    clock: Arc<Mutex<u64>>,
    /// Authorization header value to send with requests (optional).
    api_key: Option<String>,
}

impl SyncClient {
    /// Create a new sync client.
    ///
    /// `passphrase` is the vault passphrase, used to derive AES-256 and HMAC keys
    /// via Argon2id (memory-hard KDF, ~64 MiB, 3 iterations).
    /// `device_id` should be the persisted device identity from device.json —
    /// it must survive restarts so vector clocks remain meaningful.
    /// `initial_clock` should be the persisted vector clock from a previous run,
    /// or 0 for a fresh start.
    pub fn new(
        server_url: String,
        vault_id: String,
        passphrase: &str,
        device_id: String,
        api_key: Option<String>,
        initial_clock: u64,
    ) -> Self {
        // Derive two 256-bit keys from the passphrase using Argon2id.
        // enc_key = Argon2id(passphrase, salt="axiom-sync-enc-v2")
        // hmac_key = Argon2id(passphrase, salt="axiom-sync-hmac-v2")
        //
        // We use domain-separated salts so the sync keys are cryptographically
        // independent from the vault encryption key.
        let enc_key = derive_sync_key(passphrase, b"axiom-sync-enc-v2");
        let hmac_key = derive_sync_key(passphrase, b"axiom-sync-hmac-v2");

        Self {
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("reqwest client builder with timeout"),
            server_url: server_url.trim_end_matches('/').to_string(),
            vault_id,
            device_id,
            encryption_key: enc_key,
            hmac_key,
            clock: Arc::new(Mutex::new(initial_clock)),
            api_key,
        }
    }

    // ── Public API ────────────────────────────────────────────────────────

    /// Encrypt a plaintext memory entry into a `SyncBlob` ready for push.
    ///
    /// Increments the local vector clock and uses it for this blob.
    /// Uses `lock().await` on the tokio Mutex — must be called from within
    /// a tokio runtime context (spawned task or async handler).
    pub async fn encrypt_memory(
        &self,
        memory_id: &str,
        plaintext: &str,
        deleted: bool,
    ) -> Result<SyncBlob, String> {
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&self.encryption_key));
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let ciphertext_bytes = cipher
            .encrypt(&nonce, plaintext.as_bytes())
            .map_err(|e| format!("encryption failed: {e}"))?;

        // Prepend nonce so decryption can recover it (12 bytes nonce + ciphertext).
        let mut combined = nonce.to_vec();
        combined.extend_from_slice(&ciphertext_bytes);
        let ciphertext_b64 =
            base64::engine::general_purpose::STANDARD.encode(&combined);

        let vector_clock = {
            let mut c = self.clock.lock().await;
            *c += 1;
            *c
        };

        let blob_created_at = chrono::Utc::now().to_rfc3339();
        let hmac = self.compute_hmac(
            &self.vault_id,
            memory_id,
            &self.device_id,
            vector_clock,
            &ciphertext_b64,
            deleted,
            &blob_created_at,
        );

        Ok(SyncBlob {
            vault_id: self.vault_id.clone(),
            memory_id: memory_id.to_string(),
            device_id: self.device_id.clone(),
            vector_clock,
            ciphertext: ciphertext_b64,
            hmac,
            created_at: blob_created_at,
            deleted,
        })
    }

    /// Decrypt a `SyncBlob` pulled from the server.
    ///
    /// Verifies HMAC before decrypting. Returns the plaintext on success.
    pub fn decrypt_blob(&self, blob: &SyncBlob) -> Result<String, String> {
        // Verify HMAC
        let expected = self.compute_hmac(
            &blob.vault_id,
            &blob.memory_id,
            &blob.device_id,
            blob.vector_clock,
            &blob.ciphertext,
            blob.deleted,
            &blob.created_at,
        );
        if !constant_time_eq(&expected, &blob.hmac) {
            return Err("HMAC verification failed — blob may be tampered".into());
        }

        // Decode ciphertext
        let combined = base64::engine::general_purpose::STANDARD
            .decode(&blob.ciphertext)
            .map_err(|e| format!("base64 decode failed: {e}"))?;

        if combined.len() < 12 {
            return Err("ciphertext too short".into());
        }

        let (nonce_bytes, ct) = combined.split_at(12);
        let nonce = Nonce::from_slice(nonce_bytes);
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&self.encryption_key));

        let plaintext = cipher
            .decrypt(nonce, ct)
            .map_err(|e| format!("decryption failed: {e}"))?;

        String::from_utf8(plaintext).map_err(|e| format!("invalid UTF-8: {e}"))
    }

    /// Push a batch of encrypted blobs to the sync server.
    pub async fn push(&self, blobs: Vec<SyncBlob>) -> Result<PushResponse, String> {
        let url = format!(
            "{}/v1/vaults/{}/push",
            self.server_url,
            urlencoding(&self.vault_id)
        );
        let mut req = self.http.post(&url).json(&PushRequest { blobs });
        if let Some(ref key) = self.api_key {
            req = req.header("Authorization", format!("Bearer {key}"));
        }
        req.send()
            .await
            .map_err(|e| format!("push failed: {e}"))?
            .error_for_status()
            .map_err(|e| format!("push rejected: {e}"))?
            .json::<PushResponse>()
            .await
            .map_err(|e| format!("push parse: {e}"))
    }

    /// Pull blobs newer than `since` (RFC 3339 timestamp).
    pub async fn pull(
        &self,
        since: Option<&str>,
        limit: usize,
    ) -> Result<PullResponse, String> {
        let vault_id_enc = urlencoding(&self.vault_id);
        let mut url = format!(
            "{}/v1/vaults/{}/pull?limit={}",
            self.server_url, vault_id_enc, limit
        );
        if let Some(s) = since {
            url.push_str("&since=");
            url.push_str(&urlencoding(s));
        }

        let mut req = self.http.get(&url);
        if let Some(ref key) = self.api_key {
            req = req.header("Authorization", format!("Bearer {key}"));
        }
        req.send()
            .await
            .map_err(|e| format!("pull failed: {e}"))?
            .error_for_status()
            .map_err(|e| format!("pull rejected: {e}"))?
            .json::<PullResponse>()
            .await
            .map_err(|e| format!("pull parse: {e}"))
    }

    /// Server health check.
    #[allow(dead_code)]
    pub async fn health(&self) -> Result<axiom_engram::sync::SyncHealth, String> {
        let url = format!("{}/health", self.server_url);
        self.http
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("health check failed: {e}"))?
            .error_for_status()
            .map_err(|e| format!("health rejected: {e}"))?
            .json()
            .await
            .map_err(|e| format!("health parse: {e}"))
    }

    /// Current vector clock value.
    pub async fn current_clock(&self) -> u64 {
        *self.clock.lock().await
    }

    /// Persist the vector clock to device.json so it survives restarts.
    /// Called after each successful push cycle.
    ///
    /// Uses atomic write-via-tempfile so a crash mid-write or serialization
    /// failure never wipes the existing device.json (which holds the device
    /// identity and fingerprint).
    pub async fn persist_clock(&self, vault_path: &std::path::Path) {
        let clock = self.current_clock().await;
        let path = vault_path.join("device.json");
        // Read existing device.json to preserve other fields
        let mut json = if path.exists() {
            std::fs::read_to_string(&path)
                .ok()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                .unwrap_or(serde_json::json!({}))
        } else {
            serde_json::json!({})
        };
        json["vector_clock"] = serde_json::json!(clock);
        let serialized = match serde_json::to_string_pretty(&json) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "Failed to serialize vector clock — device.json untouched");
                return;
            }
        };
        // Atomic write: temp file then rename — crash-safe.
        let tmp = vault_path.join("device.json.tmp");
        if let Err(e) = std::fs::write(&tmp, &serialized) {
            tracing::warn!(error = %e, "Failed to write vector clock temp file");
            return;
        }
        if let Err(e) = std::fs::rename(&tmp, &path) {
            tracing::warn!(error = %e, "Failed to atomically persist vector clock");
        }
    }

    /// Load the vector clock from device.json on startup.
    /// Returns the stored clock or 0 if not found.
    pub fn load_clock(vault_path: &std::path::Path) -> u64 {
        let path = vault_path.join("device.json");
        if let Ok(data) = std::fs::read_to_string(&path) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&data) {
                return json.get("vector_clock").and_then(|v| v.as_u64()).unwrap_or(0);
            }
        }
        0
    }

    /// Load persisted known_clocks from sync_state.json on startup.
    /// Returns empty map if the file is missing or unparseable.
    pub fn load_known_clocks(
        vault_path: &std::path::Path,
    ) -> std::collections::HashMap<String, u64> {
        let path = vault_path.join("sync_state.json");
        if let Ok(data) = std::fs::read_to_string(&path) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&data) {
                if let Some(clocks) = json.get("known_clocks").and_then(|v| v.as_object()) {
                    return clocks
                        .iter()
                        .filter_map(|(k, v)| v.as_u64().map(|n| (k.clone(), n)))
                        .collect();
                }
            }
        }
        std::collections::HashMap::new()
    }

    /// Persist known_clocks to sync_state.json so they survive restarts.
    pub async fn persist_known_clocks(
        &self,
        vault_path: &std::path::Path,
        known_clocks: &std::collections::HashMap<String, u64>,
    ) {
        let path = vault_path.join("sync_state.json");
        let clock = self.current_clock().await;
        let json = serde_json::json!({
            "vector_clock": clock,
            "known_clocks": known_clocks,
            "updated_at": chrono::Utc::now().to_rfc3339(),
        });
        if let Err(e) =
            std::fs::write(&path, serde_json::to_string_pretty(&json).unwrap_or_default())
        {
            tracing::warn!(error = %e, "Failed to persist sync state");
        }
    }

    /// Bump the local vector clock to at least `min_val` — called after
    /// pulling remote blobs so subsequent local pushes use clocks higher
    /// than anything the server has seen.
    pub async fn bump_clock(&self, min_val: u64) {
        let mut c = self.clock.lock().await;
        *c = (*c).max(min_val);
    }

    // ── Private helpers ───────────────────────────────────────────────────

    fn compute_hmac(
        &self,
        vault_id: &str,
        memory_id: &str,
        device_id: &str,
        clock: u64,
        ciphertext: &str,
        deleted: bool,
        created_at: &str,
    ) -> String {
        let mut mac =
            <HmacSha256 as KeyInit>::new_from_slice(&self.hmac_key).expect("HMAC key is 32 bytes");
        mac.update(vault_id.as_bytes());
        mac.update(memory_id.as_bytes());
        mac.update(device_id.as_bytes());
        mac.update(&clock.to_le_bytes());
        mac.update(ciphertext.as_bytes());
        mac.update(&[deleted as u8]);
        mac.update(created_at.as_bytes());
        let result = mac.finalize();
        base64::engine::general_purpose::STANDARD.encode(result.into_bytes())
    }
}

// ── Sync loop (integrated with vault) ────────────────────────────────────────

/// Start a background sync loop that pulls remote changes and writes them
/// into the local vault, and pushes local changes on each cycle.
///
/// Returns immediately; the loop runs in a spawned tokio task.
pub fn spawn_sync_loop(
    client: Arc<SyncClient>,
    vault: Arc<Mutex<axiom_engram::EngramStore>>,
    vault_path: std::path::PathBuf,
    interval: std::time::Duration,
    mut trigger: tokio::sync::watch::Receiver<u64>,
    events_tx: tokio::sync::broadcast::Sender<crate::app_state::LiveEvent>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // Delay first sync so the server has time to start.
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;

        // Load persisted state so restarts don't lose sync progress.
        let last_push_path = vault_path.join("sync_state.json");
        let mut last_push: Option<String> = last_push_path
            .metadata()
            .ok()
            .and_then(|_| {
                std::fs::read_to_string(&last_push_path).ok().and_then(|s| {
                    serde_json::from_str::<serde_json::Value>(&s)
                        .ok()
                        .and_then(|v| v.get("last_push").cloned())
                        .and_then(|v| v.as_str().map(String::from))
                })
            });

        let mut last_sync: Option<String> = None;
        // Track (memory_id, vector_clock) pairs we've already processed for
        // dedup across paginated batches. Uses (id, clock) instead of just
        // created_at so we correctly handle blobs that share a timestamp.
        let mut seen_blobs: std::collections::HashSet<(String, u64)> =
            std::collections::HashSet::new();
        // Seed known_clocks from persisted sync state so the first pull after
        // restart doesn't overwrite local data with stale remote blobs.
        let mut known_clocks: std::collections::HashMap<String, u64> =
            SyncClient::load_known_clocks(&vault_path);

        loop {
            // ── Pull remote changes ──────────────────────────────────────
            match client.pull(last_sync.as_deref(), 500).await {
                Ok(resp) => {
                    let mut latest_seen: Option<String> = None;
                    // Highest modified_at among blobs imported this pull — the
                    // push cursor advances past it below so pulled rows are
                    // never re-selected and re-pushed (echo churn).
                    let mut max_pulled_modified: Option<String> = None;

                    for blob in &resp.blobs {
                        // Track the latest timestamp for pagination cursor advancement
                        if latest_seen.as_deref().unwrap_or("") < blob.created_at.as_str() {
                            latest_seen = Some(blob.created_at.clone());
                        }

                        // Skip blobs we've already processed (dedup across paginated batches)
                        if !seen_blobs.insert((blob.memory_id.clone(), blob.vector_clock)) {
                            continue;
                        }

                        match client.decrypt_blob(blob) {
                            Ok(plaintext) => {
                                if let Ok(json) = serde_json::from_str::<
                                    serde_json::Value,
                                >(&plaintext)
                                {
                                    let memory_id = json
                                        .get("id")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or(&blob.memory_id);
                                    let content = json
                                        .get("content")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or(&plaintext);

                                    let vault = vault.lock().await;

                                    // Handle deletion tombstones
                                    if blob.deleted {
                                        let known_clock =
                                            known_clocks.get(memory_id).copied().unwrap_or(0);
                                        // LWW: only delete if remote clock is newer
                                        if blob.vector_clock > known_clock {
                                            if let Err(e) = vault.delete(memory_id).await {
                                                tracing::warn!(
                                                    "sync: failed to delete {}: {e}",
                                                    memory_id
                                                );
                                            } else {
                                                tracing::debug!(
                                                    "sync: deleted {} per remote tombstone (clock={})",
                                                    memory_id, blob.vector_clock
                                                );
                                                known_clocks.insert(
                                                    memory_id.to_string(),
                                                    blob.vector_clock,
                                                );
                                            }
                                        }
                                        continue;
                                    }

                                    // LWW conflict resolution: compare vector clocks.
                                    let known_clock =
                                        known_clocks.get(memory_id).copied().unwrap_or(0);
                                    if blob.vector_clock <= known_clock {
                                        tracing::debug!(
                                            "sync: skipping {} — known clock {} >= remote {}",
                                            memory_id,
                                            known_clock,
                                            blob.vector_clock
                                        );
                                        continue;
                                    }

                                    // Build a full engram from the JSON envelope,
                                    // preserving all fields for faithful replication.
                                    let layer_str = json
                                        .get("layer")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("episodic");
                                    let source_str = json
                                        .get("source")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("system");
                                    let tags: Vec<String> = json
                                        .get("tags")
                                        .and_then(|v| v.as_array())
                                        .map(|a| {
                                            a.iter()
                                                .filter_map(|t| t.as_str().map(String::from))
                                                .collect()
                                        })
                                        .unwrap_or_default();

                                    let mut engram = axiom_engram::Engram::new_episodic(
                                        content.to_string(),
                                        axiom_engram::EngramSource::from_str(source_str)
                                            .unwrap_or(axiom_engram::EngramSource::System),
                                        json.get("context")
                                            .cloned()
                                            .unwrap_or(serde_json::json!({})),
                                    );
                                    engram.id = memory_id.to_string();
                                    engram.layer =
                                        axiom_engram::EngramLayer::from_str(layer_str)
                                            .unwrap_or(axiom_engram::EngramLayer::Episodic);
                                    engram.tags = tags;

                                    // Preserve all optional fields from the JSON envelope
                                    if let Some(v) = json.get("valence").and_then(|v| v.as_f64()) {
                                        engram.valence = v.clamp(-1.0, 1.0);
                                    }
                                    if let Some(s) = json.get("strength").and_then(|v| v.as_f64()) {
                                        engram.strength = s.max(0.0).min(2.0);
                                    }
                                    if let Some(p) = json.get("project").and_then(|v| v.as_str()) {
                                        engram.project = Some(p.to_string());
                                    }
                                    if let Some(s) = json.get("scope").and_then(|v| v.as_str()) {
                                        engram.scope = s.to_string();
                                    }
                                    if let Some(p) = json.get("privacy_level").and_then(|v| v.as_str()) {
                                        engram.privacy_level =
                                            axiom_engram::PrivacyLevel::from_str(p)
                                                .unwrap_or_default();
                                    }
                                    if let Some(ct) = json.get("content_type").and_then(|v| v.as_str()) {
                                        engram.content_type = ct.to_string();
                                    }
                                    if let Some(oa) = json.get("occurred_at").and_then(|v| v.as_str()) {
                                        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(oa) {
                                            engram.occurred_at = Some(dt.with_timezone(&chrono::Utc));
                                        }
                                    }
                                    if let Some(ca) = json.get("created_at").and_then(|v| v.as_str()) {
                                        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ca) {
                                            engram.created_at = dt.with_timezone(&chrono::Utc);
                                        }
                                    }
                                    // Preserve the remote modified_at — the
                                    // write_inner upsert stamps whatever the
                                    // Engram carries, so this is what stops
                                    // the echo: a fresh local now() here would
                                    // re-select the row for push every cycle.
                                    // Pre-v5 envelopes fall back to created_at,
                                    // matching the schema-v5 backfill.
                                    let pulled_modified = json
                                        .get("modified_at")
                                        .and_then(|v| v.as_str())
                                        .or_else(|| json.get("created_at").and_then(|v| v.as_str()));
                                    if let Some(ma) = pulled_modified {
                                        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ma) {
                                            engram.modified_at = dt.with_timezone(&chrono::Utc);
                                        }
                                    }
                                    if let Some(img) = json.get("imagined").and_then(|v| v.as_bool()) {
                                        engram.imagined = img;
                                    }
                                    if let Some(g) = json.get("grounded").and_then(|v| v.as_bool()) {
                                        engram.grounded = g;
                                    }
                                    if let Some(r) = json.get("retrievals").and_then(|v| v.as_u64()) {
                                        engram.retrievals = r as i32;
                                    }
                                    if let Some(lr) = json.get("last_retrieved").and_then(|v| v.as_str()) {
                                        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(lr) {
                                            engram.last_retrieved =
                                                Some(dt.with_timezone(&chrono::Utc));
                                        }
                                    }
                                    // Links and the embedding ride along in
                                    // the blob so the graph round-trips.
                                    if let Some(links) = json.get("links").and_then(|v| v.as_array()) {
                                        engram.links = links
                                            .iter()
                                            .filter_map(|l| {
                                                let target = l.get("target_id")?.as_str()?;
                                                let weight = l.get("weight")?.as_f64()?;
                                                let ty = l.get("link_type")?.as_str()?;
                                                Some(axiom_engram::EngramLink {
                                                    target_id: target.to_string(),
                                                    weight,
                                                    link_type: axiom_engram::LinkType::from_str(ty)?,
                                                })
                                            })
                                            .collect();
                                    }
                                    let embedding: Option<Vec<f64>> = json
                                        .get("embedding")
                                        .and_then(|v| v.as_array())
                                        .map(|a| a.iter().filter_map(|x| x.as_f64()).collect());
                                    let embedding_model = json
                                        .get("embedding_model")
                                        .and_then(|v| v.as_str())
                                        .map(String::from);

                                    let write_result = match &embedding {
                                        Some(emb) if !emb.is_empty() => {
                                            vault
                                                .write_with_embedding(
                                                    &engram,
                                                    Some(emb),
                                                    embedding_model.as_deref(),
                                                    None,
                                                )
                                                .await
                                        }
                                        _ => vault.write(&engram).await,
                                    };
                                    if let Err(e) = write_result {
                                        tracing::warn!(
                                            "sync: failed to write pulled memory {}: {e}",
                                            memory_id
                                        );
                                    } else {
                                        tracing::debug!(
                                            "sync: imported memory {} from remote (clock={})",
                                            memory_id,
                                            blob.vector_clock
                                        );
                                        known_clocks.insert(
                                            memory_id.to_string(),
                                            blob.vector_clock,
                                        );
                                        // Track the imported row's modified_at
                                        // so the push cursor skips it below.
                                        let m = engram.modified_at.to_rfc3339();
                                        if max_pulled_modified.as_deref().unwrap_or("") < m.as_str() {
                                            max_pulled_modified = Some(m);
                                        }
                                        // Teammate activity: broadcast the
                                        // imported memory so the SPA live feed
                                        // updates without a reload. The UI
                                        // dedupes by memory id.
                                        let _ = events_tx.send(crate::app_state::LiveEvent::Capture {
                                            memory: serde_json::to_value(&engram).unwrap_or_else(
                                                |_| serde_json::json!({"id": memory_id}),
                                            ),
                                            timestamp: chrono::Utc::now().to_rfc3339(),
                                        });
                                    }
                                } else {
                                    tracing::warn!(
                                        "sync: blob {} decrypted but plaintext is not valid JSON envelope — discarding",
                                        blob.memory_id
                                    );
                                }
                            }
                            Err(e) => {
                                tracing::warn!(
                                    "sync: failed to decrypt blob {}: {e}",
                                    blob.memory_id
                                );
                            }
                        }
                    }

                    // Bump local clock to max remote clock so subsequent
                    // local pushes win LWW against remote blobs we just
                    // ingested. Without this, edits to pulled memories
                    // are rejected by the server as stale.
                    if let Some(max_clock) = resp.blobs.iter().map(|b| b.vector_clock).max() {
                        client.bump_clock(max_clock).await;
                    }

                    if !resp.blobs.is_empty() {
                        tracing::info!(
                            "sync: pulled {} blobs (has_more={})",
                            resp.blobs.len(),
                            resp.has_more
                        );
                    }

                    // Advance cursor: use the latest blob's created_at so we
                    // don't re-pull the same blobs on the next cycle.
                    // When has_more is false, use now() to catch any blobs
                    // that arrived between our pull and now.
                    if !resp.has_more {
                        last_sync = Some(chrono::Utc::now().to_rfc3339());
                        // Reset dedup set for the next full sweep
                        seen_blobs.clear();
                    } else if let Some(ts) = latest_seen {
                        // Advance to the latest blob we've seen so we don't
                        // re-pull the same batch forever.
                        last_sync = Some(ts);
                    }

                    // Echo-churn fix (pull side): advance the push cursor past
                    // the modified_at of every row imported this pull. Without
                    // this, the imported rows re-satisfy `modified_at > cutoff`
                    // on the next cycle and get re-pushed — the server rejects
                    // them as stale, but the churn and noise remain.
                    if let Some(mp) = max_pulled_modified {
                        if last_push.as_deref().unwrap_or("") < mp.as_str() {
                            last_push = Some(mp);
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("sync pull error: {e}");
                }
            }

            // ── Push local changes ───────────────────────────────────────
            match push_local_changes(&client, &vault, last_push.as_deref()).await {
                Ok((clocks, new_last_push)) => {
                    for (id, clock) in clocks {
                        let entry = known_clocks.entry(id).or_insert(0);
                        *entry = (*entry).max(clock);
                    }
                    // Advance last_push so we only push newly-modified memories
                    // on subsequent cycles. This replaces the 5-minute window.
                    if let Some(ts) = new_last_push {
                        last_push = Some(ts);
                    }
                    client.persist_clock(&vault_path).await;
                }
                Err(e) => {
                    tracing::warn!("sync push error: {e}");
                }
            }

            // ── Push tombstones for locally-deleted memories ──────────
            let tombstone_clocks = push_tombstones(&client, &vault_path).await;
            for (id, clock) in &tombstone_clocks {
                let entry = known_clocks.entry(id.clone()).or_insert(0);
                *entry = (*entry).max(*clock);
            }
            if !tombstone_clocks.is_empty() {
                client.persist_clock(&vault_path).await;
            }

            // Persist known_clocks and last_push so they survive restarts.
            // This fixes the known_clocks-not-seeded issue (Sync-C4) and the
            // last_push-not-persisted issue (Sync-C2).
            client.persist_known_clocks(&vault_path, &known_clocks).await;
            // Also persist last_push to sync_state.json
            if let Some(ref ts) = last_push {
                let path = vault_path.join("sync_state.json");
                if let Ok(data) = std::fs::read_to_string(&path) {
                    if let Ok(mut json) = serde_json::from_str::<serde_json::Value>(&data) {
                        json["last_push"] = serde_json::json!(ts);
                        let _ = std::fs::write(
                            &path,
                            serde_json::to_string_pretty(&json).unwrap_or_default(),
                        );
                    }
                }
            }

            // Wait for the next cycle: the interval elapses, or /sync/now
            // bumps the trigger counter for an immediate cycle. A watch
            // channel (not a Notify) so a trigger fired mid-cycle isn't lost.
            tokio::select! {
                _ = trigger.changed() => {}
                _ = tokio::time::sleep(interval) => {}
            }
        }
    })
}

/// Push locally-modified memories that haven't been synced yet.
///
/// Uses a persisted `last_push` cursor against `modified_at` to push only
/// memories modified since the last successful push cycle — including edits
/// to old memories, which the previous `created_at` + top-200 filter dropped.
/// On first push (no persisted `last_push`), uses a 24-hour window.
///
/// Returns the set of (memory_id, clock) pairs for blobs that were **accepted**
/// by the server (not rejected), plus the new `last_push` timestamp to persist.
///
/// The vault lock is held only while collecting fresh memories; it is dropped
/// before the network call so an unreachable sync server doesn't stall the
/// daemon's vault access.
async fn push_local_changes(
    client: &SyncClient,
    vault: &Arc<Mutex<axiom_engram::EngramStore>>,
    last_push: Option<&str>,
) -> Result<(std::collections::HashMap<String, u64>, Option<String>), String> {
    // Use the persisted last_push timestamp as the cutoff, or a broad
    // window on first push. This replaces the old 5-minute freshness window
    // which silently dropped memories created while the daemon was down.
    let cutoff = last_push
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .unwrap_or_else(|| chrono::Utc::now() - chrono::Duration::hours(24));

    // Collect fresh memories under the vault lock, then drop it before I/O.
    let mut batch_latest: Option<String> = last_push.map(String::from);
    let fresh_jsons: Vec<(String, String)> = {
        let vault = vault.lock().await;
        // modified_at-based cursor: edits to old memories re-propagate even
        // when they fall outside the recency top-N (reads never bump it).
        let fresh: Vec<axiom_engram::Engram> = vault
            .list_modified_since(&cutoff.to_rfc3339(), 500)
            .await
            .map_err(|e| format!("list_modified_since failed: {e}"))?;

        if fresh.is_empty() {
            return Ok((std::collections::HashMap::new(), batch_latest));
        }

        // Track the latest modified_at in the batch for advancing last_push
        let mut jsons = Vec::with_capacity(fresh.len());
        for mem in &fresh {
            let created_at = mem.created_at.to_rfc3339();
            let modified_at = mem.modified_at.to_rfc3339();
            if batch_latest.as_deref().unwrap_or("") < modified_at.as_str() {
                batch_latest = Some(modified_at.clone());
            }
            // Links and the embedding ride along in the blob so the graph
            // (and vector fallback) round-trip across devices.
            let links = vault.get_links(&mem.id).await.unwrap_or_default();
            let links_json: Vec<serde_json::Value> = links
                .iter()
                .map(|l| {
                    serde_json::json!({
                        "target_id": l.target_id,
                        "weight": l.weight,
                        "link_type": l.link_type.as_str(),
                    })
                })
                .collect();
            let embedding = vault.get_embedding(&mem.id).await.unwrap_or(None);
            let (embedding_json, embedding_model) = match &embedding {
                Some((model, vec)) => (serde_json::json!(vec), serde_json::json!(model)),
                None => (serde_json::Value::Null, serde_json::Value::Null),
            };
            let json = serde_json::json!({
                "id": mem.id,
                "content": mem.content,
                "layer": mem.layer.as_str(),
                "source": mem.source.as_str(),
                "strength": mem.strength,
                "valence": mem.valence,
                "tags": mem.tags,
                "project": mem.project,
                "scope": mem.scope,
                "privacy_level": mem.privacy_level.as_str(),
                "content_type": mem.content_type,
                "context": mem.context,
                "imagined": mem.imagined,
                "grounded": mem.grounded,
                "retrievals": mem.retrievals,
                "created_at": created_at,
                "modified_at": modified_at,
                "last_retrieved": mem.last_retrieved.map(|d| d.to_rfc3339()),
                "occurred_at": mem.occurred_at.map(|d| d.to_rfc3339()),
                "links": links_json,
                "embedding": embedding_json,
                "embedding_model": embedding_model,
            });
            jsons.push((mem.id.clone(), json.to_string()));
        }
        jsons
        // vault lock dropped here
    };

    // Encrypt outside the vault lock
    let mut blobs = Vec::new();
    for (id, plaintext) in &fresh_jsons {
        match client.encrypt_memory(id, plaintext, false).await {
            Ok(blob) => blobs.push(blob),
            Err(e) => {
                tracing::warn!("sync: failed to encrypt memory {id}: {e}");
            }
        }
    }

    if blobs.is_empty() {
        return Ok((std::collections::HashMap::new(), batch_latest));
    }

    // Track each blob's own vector clock so we only record clocks for
    // accepted blobs, not rejected ones.
    let blob_clocks: std::collections::HashMap<String, u64> = blobs
        .iter()
        .map(|b| (b.memory_id.clone(), b.vector_clock))
        .collect();

    let count = blobs.len();
    let mut pushed_clocks = std::collections::HashMap::new();

    match client.push(blobs).await {
        Ok(resp) => {
            tracing::info!(
                "sync: pushed {}/{} blobs ({} rejected)",
                resp.accepted,
                count,
                resp.rejected.len()
            );
            // Only record clocks for accepted blobs (not rejected ones).
            // Rejected blobs had a lower-or-equal clock than what the server
            // already has, so recording an inflated clock would mask remote updates.
            let rejected: std::collections::HashSet<&str> =
                resp.rejected.iter().map(|s| s.as_str()).collect();
            for (id, clock) in &blob_clocks {
                if !rejected.contains(id.as_str()) {
                    pushed_clocks.insert(id.clone(), *clock);
                }
            }
        }
        Err(e) => {
            return Err(format!("push failed: {e}"));
        }
    }

    Ok((pushed_clocks, batch_latest))
}

// ── Tombstone tracking ───────────────────────────────────────────────────

/// Record a memory deletion so the sync loop can push a tombstone.
/// Appends to a JSON-lines file in the vault directory.
pub fn record_deletion(vault_path: &std::path::Path, memory_id: &str) {
    let path = vault_path.join("tombstones.jsonl");
    let entry = serde_json::json!({"id": memory_id, "deleted_at": chrono::Utc::now().to_rfc3339()});
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path);
    match file.as_mut() {
        Ok(f) => {
            use std::io::Write;
            let _ = writeln!(f, "{}", entry);
        }
        Err(e) => {
            tracing::warn!(error = %e, "Failed to record tombstone for sync");
        }
    }
}

/// Load pending tombstone IDs from the tracking file.
/// Returns (memory_id, deleted_at) pairs.
fn load_tombstones(vault_path: &std::path::Path) -> Vec<(String, String)> {
    let path = vault_path.join("tombstones.jsonl");
    match std::fs::read_to_string(&path) {
        Ok(data) => data
            .lines()
            .filter_map(|line| {
                let json: serde_json::Value = serde_json::from_str(line).ok()?;
                let id = json.get("id")?.as_str()?.to_string();
                let deleted_at = json
                    .get("deleted_at")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                Some((id, deleted_at))
            })
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// Remove accepted tombstone IDs from the tracking file.
/// Uses atomic write-via-tempfile to avoid data loss on crash and to prevent
/// races with concurrent `record_deletion` (HTTP handler) from the main task.
fn clear_tombstones(vault_path: &std::path::Path, accepted: &std::collections::HashSet<String>) {
    if accepted.is_empty() {
        return;
    }
    let all = load_tombstones(vault_path);
    let remaining: Vec<_> = all
        .into_iter()
        .filter(|(id, _)| !accepted.contains(id))
        .collect();
    let path = vault_path.join("tombstones.jsonl");
    let tmp_path = vault_path.join("tombstones.jsonl.tmp");
    if remaining.is_empty() {
        let _ = std::fs::remove_file(&path);
    } else {
        use std::io::Write;
        // Write to temp file, then atomically rename — prevents partial
        // writes from corrupting the tombstone tracking and avoids races
        // with record_deletion appending concurrently.
        if let Ok(mut f) = std::fs::File::create(&tmp_path) {
            for (id, deleted_at) in &remaining {
                let entry = serde_json::json!({"id": id, "deleted_at": deleted_at});
                let _ = writeln!(f, "{}", entry);
            }
            let _ = std::fs::rename(&tmp_path, &path);
        }
    }
}

/// Push tombstone blobs for locally-deleted memories that haven't been
/// synced yet. Returns a map of memory_id → vector_clock for tombstones
/// that were **accepted** by the server, so the caller can update per-memory
/// clock tracking. Uses each blob's own vector clock (not the global counter)
/// so clock tracking is precise.
async fn push_tombstones(
    client: &SyncClient,
    vault_path: &std::path::Path,
) -> std::collections::HashMap<String, u64> {
    let pending = load_tombstones(vault_path);
    if pending.is_empty() {
        return std::collections::HashMap::new();
    }

    let mut blobs = Vec::new();
    // Track per-blob (id, clock) pairs for precise clock tracking
    let mut blob_clocks: Vec<(String, u64)> = Vec::new();
    for (id, _deleted_at) in &pending {
        let plaintext = serde_json::json!({"id": id, "deleted": true}).to_string();
        match client.encrypt_memory(id, &plaintext, true).await {
            Ok(blob) => {
                let clock = blob.vector_clock;
                blobs.push(blob);
                blob_clocks.push((id.clone(), clock));
            }
            Err(e) => {
                tracing::warn!("sync: failed to encrypt tombstone for {id}: {e}");
            }
        }
    }

    if blobs.is_empty() {
        return std::collections::HashMap::new();
    }

    let count = blobs.len();
    let mut accepted = std::collections::HashMap::new();
    match client.push(blobs).await {
        Ok(resp) => {
            tracing::info!(
                "sync: pushed {}/{} tombstones ({} rejected)",
                resp.accepted,
                count,
                resp.rejected.len()
            );
            let rejected: std::collections::HashSet<&str> =
                resp.rejected.iter().map(|s| s.as_str()).collect();
            let mut accepted_ids = std::collections::HashSet::new();
            for (id, clock) in &blob_clocks {
                if !rejected.contains(id.as_str()) {
                    accepted.insert(id.clone(), *clock);
                    accepted_ids.insert(id.clone());
                }
            }
            // Clear accepted tombstones from tracking
            clear_tombstones(vault_path, &accepted_ids);
        }
        Err(e) => {
            tracing::warn!("sync: tombstone push failed: {e}");
        }
    }

    accepted
}

// ── Constant-time comparison ────────────────────────────────────────────────

fn constant_time_eq(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    let mut acc = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        acc |= x ^ y;
    }
    acc == 0
}

/// Minimal percent-encoding for URL path/query components.
/// Encodes characters that are not unreserved per RFC 3986.
fn urlencoding(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push_str(&format!("%{:02X}", b));
            }
        }
    }
    out
}

// ── Sync key derivation (Argon2id) ──────────────────────────────────────────

/// Derive a 256-bit key for sync encryption/HMAC using Argon2id.
///
/// Uses the same parameters as vault key derivation (64 MiB, 3 iterations,
/// 4 lanes) but with a domain-separated salt so sync keys are independent
/// from the vault encryption key.
fn derive_sync_key(passphrase: &str, salt: &[u8]) -> [u8; 32] {
    use argon2::{
        password_hash::{PasswordHasher, SaltString},
        Argon2,
    };
    use sha2::Digest;

    // Derive a stable salt from the domain tag
    let salt_bytes = Sha256::digest(salt);
    let salt_str =
        SaltString::encode_b64(&salt_bytes[..16]).expect("16 bytes is valid salt length");

    // Match axiom-engram's Argon2id params: 64 MiB / 3 iterations / 4 lanes
    let argon2 = Argon2::new(
        argon2::Algorithm::Argon2id,
        argon2::Version::V0x13,
        argon2::Params::new(65536, 3, 4, None).expect("valid Argon2 params"),
    );
    let hash = argon2
        .hash_password(passphrase.as_bytes(), &salt_str)
        .expect("Argon2id hashing is infallible with valid params");

    let mut key = [0u8; 32];
    if let Some(h) = hash.hash.as_ref() {
        let bytes = h.as_bytes();
        let len = bytes.len().min(32);
        key[..len].copy_from_slice(&bytes[..len]);
    }
    key
}
