//! Engram store - SQLite implementation
//!
//! The engram vault is encrypted at rest using SQLCipher. The encryption key
//! is derived from the hardware machine-id (Linux: /etc/machine-id, macOS:
//! IOPlatformUUID, Windows: MachineGuid registry value) combined with a
//! compile-time application secret. This means:
//!   - The vault cannot be trivially read by copying the .db file to another machine
//!     (offline disk-copy / stolen-media protection).
//!   - Axiom does not transmit or know the vault key — it is derived locally.
//!   - Users can independently verify the key derivation by reading this code.
//!
//! **Threat model:** The machine-id-based key protects against offline disk cloning
//! and stolen storage media. It does NOT protect against a local user on the same
//! machine (machine-id is world-readable and the salt is a public constant). For
//! confidentiality against local attackers, use `open_with_passphrase()` which
//! derives the key from a user-provided secret.
//!
//! **Key stability:** Uses SHA-256 for deterministic, cross-toolchain-stable key
//! derivation. Vaults created before 2026-08-05 used an unstable SipHash-based key;
//! `open()` transparently detects legacy vaults and rekeys them to SHA-256 on open.

use crate::engram::{Engram, EngramLayer, EngramSource, EngramLink, PrivacyLevel, CoherenceState, CharacterTopology};
use crate::schema::create_tables;
use crate::{EngramError, Result};
use chrono::{Datelike, Timelike, Utc};
use rusqlite::params;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;

// ── Vault key derivation ──────────────────────────────────────────────────────

/// Application-level salt mixed into the key derivation. Prevents a raw
/// machine-id from being useful even if extracted from another context.
const APP_SALT: &str = "axiom-engram-vault-v1";

/// Derive a deterministic, machine-specific, cross-toolchain-stable SQLCipher key.
///
/// Key = hex(SHA-256(machine_id || ":" || APP_SALT))
///
/// Uses SHA-256 for stability across Rust toolchain versions. The machine-id
/// acts as a hardware binding factor. This is intentionally lightweight (no
/// argon2/scrypt) because the primary threat model is offline disk cloning,
/// not dedicated brute-force with full access to the derivation code.
fn derive_vault_key() -> String {
    use sha2::{Digest, Sha256};
    let machine_id = read_machine_id();
    let input = format!("{}:{}", machine_id, APP_SALT);
    let hash = Sha256::digest(input.as_bytes());
    hex::encode(hash)
}

/// Legacy key derivation using DefaultHasher (SipHash-1-3).
///
/// Used to open vaults created before 2026-08-05. On successful open with this
/// key, the vault is transparently rekeyed to SHA-256. Kept for backward
/// compatibility only; new vaults always use `derive_vault_key()`.
fn derive_legacy_key() -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let machine_id = read_machine_id();
    let input = format!("{}:{}", machine_id, APP_SALT);

    let mut h1 = DefaultHasher::new();
    input.hash(&mut h1);
    let a = h1.finish();

    let mut h2 = DefaultHasher::new();
    format!("{}{}", input, "b").hash(&mut h2);
    let b = h2.finish();

    let mut h3 = DefaultHasher::new();
    format!("{}{}", input, "c").hash(&mut h3);
    let c = h3.finish();

    let mut h4 = DefaultHasher::new();
    format!("{}{}", input, "d").hash(&mut h4);
    let d = h4.finish();

    format!("{:016x}{:016x}{:016x}{:016x}", a, b, c, d)
}

/// Derive a vault key from a user-provided passphrase.
///
/// Key = hex(SHA-256(passphrase || ":" || APP_SALT))
///
/// This provides real confidentiality — the key cannot be derived without the
/// passphrase, even by a local user who knows the machine-id and salt.
fn derive_passphrase_key(passphrase: &str) -> String {
    use sha2::{Digest, Sha256};
    let input = format!("{}:{}", passphrase, APP_SALT);
    let hash = Sha256::digest(input.as_bytes());
    hex::encode(hash)
}

/// Converts bytes to hex string. Avoids pulling in the `hex` crate.
mod hex {
    pub fn encode(bytes: impl AsRef<[u8]>) -> String {
        bytes.as_ref().iter().map(|b| format!("{:02x}", b)).collect()
    }
}

/// Read the platform hardware identifier used in key derivation.
fn read_machine_id() -> String {
    // Linux / systemd
    #[cfg(target_os = "linux")]
    {
        if let Ok(id) = std::fs::read_to_string("/etc/machine-id") {
            let id = id.trim().to_string();
            if !id.is_empty() {
                return id;
            }
        }
        // Fallback: container environments (Docker / Podman)
        if let Ok(id) = std::fs::read_to_string("/proc/sys/kernel/random/boot_id") {
            let id = id.trim().to_string();
            if !id.is_empty() {
                return id;
            }
        }
    }

    // macOS — use IOPlatformUUID via system_profiler or ioreg
    #[cfg(target_os = "macos")]
    {
        if let Ok(output) = std::process::Command::new("ioreg")
            .args(["-rd1", "-c", "IOPlatformExpertDevice"])
            .output()
        {
            let text = String::from_utf8_lossy(&output.stdout);
            for line in text.lines() {
                if line.contains("IOPlatformUUID") {
                    if let Some(start) = line.rfind('"') {
                        let after = &line[start + 1..];
                        if let Some(end) = after.rfind('"') {
                            let uuid = after[..end].trim().to_string();
                            if !uuid.is_empty() {
                                return uuid;
                            }
                        }
                    }
                }
            }
        }
    }

    // Windows — read HKLM\SOFTWARE\Microsoft\Cryptography\MachineGuid
    #[cfg(target_os = "windows")]
    {
        if let Ok(output) = std::process::Command::new("reg")
            .args(["query", r"HKLM\SOFTWARE\Microsoft\Cryptography", "/v", "MachineGuid"])
            .output()
        {
            let text = String::from_utf8_lossy(&output.stdout);
            for line in text.lines() {
                if line.contains("MachineGuid") {
                    if let Some(guid) = line.split_whitespace().last() {
                        return guid.to_string();
                    }
                }
            }
        }
    }

    // Last-resort fallback: use the db path itself as a weak binding factor.
    // In practice, this only triggers in test environments without a machine-id.
    "axiom-fallback-key-no-machine-id".to_string()
}

// ── Store ─────────────────────────────────────────────────────────────────────

/// Main engram store
pub struct EngramStore {
    conn: Arc<Mutex<rusqlite::Connection>>,
}

impl EngramStore {
    /// Open or create an encrypted engram store.
    ///
    /// Uses SHA-256 derived key. If the vault was created before 2026-08-05
    /// with the legacy SipHash-based key, transparently rekeys to SHA-256.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_internal(path, None).await
    }

    /// Open an encrypted engram store with a user-provided passphrase.
    ///
    /// The key is derived from the passphrase rather than the machine-id,
    /// providing real confidentiality even against local attackers.
    pub async fn open_with_passphrase(path: impl AsRef<Path>, passphrase: &str) -> Result<Self> {
        Self::open_internal(path, Some(passphrase)).await
    }

    /// Internal open: tries SHA-256 key first, falls back to legacy with rekey.
    async fn open_internal(path: impl AsRef<Path>, passphrase: Option<&str>) -> Result<Self> {
        let db_path = path.as_ref().join("engrams.db");
        let exists = db_path.exists();
        let conn = rusqlite::Connection::open(&db_path)?;

        // ── SQLCipher vault encryption ────────────────────────────────────────
        // Must be the FIRST statement after opening, before any other queries.

        let primary_key = match passphrase {
            Some(pw) => derive_passphrase_key(pw),
            None => derive_vault_key(),
        };

        // Configure the primary key — MUST be first
        conn.execute_batch(&format!("PRAGMA key = '{}';", primary_key))
            .map_err(|e| EngramError::Validation(format!("vault key setup failed: {e}")))?;

        // Enable busy timeout for concurrent access
        conn.execute_batch("PRAGMA busy_timeout=5000;")
            .map_err(|e| EngramError::Validation(format!("busy_timeout setup failed: {e}")))?;

        // Validate the key on existing databases. New databases skip validation
        // because SQLCipher hasn't initialized the file yet.
        if exists {
            let key_ok = conn.query_row(
                "SELECT count(*) FROM sqlite_master",
                [],
                |row| row.get::<_, i64>(0),
            );

            if key_ok.is_err() {
                if passphrase.is_none() {
                    // SHA-256 key failed on an existing vault — try legacy key
                    drop(conn);
                    let conn = rusqlite::Connection::open(&db_path)?;

                    let legacy_key = derive_legacy_key();
                    conn.execute_batch(&format!("PRAGMA key = '{}';", legacy_key))
                        .map_err(|e| EngramError::Validation(format!("legacy key setup failed: {e}")))?;
                    conn.execute_batch("PRAGMA busy_timeout=5000;")
                        .map_err(|e| EngramError::Validation(format!("busy_timeout setup failed: {e}")))?;

                    // Validate legacy key
                    conn.query_row(
                        "SELECT count(*) FROM sqlite_master",
                        [],
                        |row| row.get::<_, i64>(0),
                    ).map_err(|_| {
                        EngramError::Validation(
                            "vault key mismatch: neither SHA-256 nor legacy key can open this vault. \
                             If you used a passphrase, call open_with_passphrase().".to_string()
                        )
                    })?;

                    // Legacy key works — migrate to SHA-256 in place
                    conn.execute_batch(&format!(
                        "PRAGMA rekey = '{}';",
                        primary_key
                    )).map_err(|e| EngramError::Validation(format!("rekey migration failed: {e}")))?;

                    return Self::finish_open(conn).await;
                } else {
                    return Err(EngramError::Validation(
                        "vault key mismatch: wrong passphrase or corrupt vault.".to_string()
                    ));
                }
            }
        }

        Self::finish_open(conn).await
    }

    /// Shared post-keying setup: create tables if new, init coherence state.
    async fn finish_open(conn: rusqlite::Connection) -> Result<Self> {
        create_tables(&conn)?;

        // Init default coherence state
        let tx = conn.unchecked_transaction()?;
        let count: i32 = tx.query_row("SELECT COUNT(*) FROM coherence_state", [], |row| row.get(0))?;
        if count == 0 {
            let default_character = CharacterTopology::default();
            let character_json = serde_json::to_string(&default_character).unwrap();
            tx.execute(
                "INSERT INTO coherence_state (id, baseline_valence, character_strengths, purpose_vector, drift_score, updated_at) VALUES (1, 0.3, ?, '[]', 0.0, ?)",
                params![character_json, Utc::now().to_rfc3339()]
            )?;
        }
        tx.commit()?;

        Ok(Self { conn: Arc::new(Mutex::new(conn)) })
    }

    /// Write engram
    pub async fn write(&self, engram: &Engram) -> Result<()> {
        let conn = self.conn.lock().await;
        let tags_json = serde_json::to_string(&engram.tags)?;

        conn.execute(
            r#"INSERT OR REPLACE INTO engrams
               (id, layer, source, privacy_level, content, context, strength, valence, retrievals, imagined, grounded, created_at, last_retrieved, project, tags)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)"#,
            params![
                engram.id, engram.layer.as_str(), engram.source.as_str(),
                engram.privacy_level.as_str(),
                engram.content, engram.context.to_string(), engram.strength,
                engram.valence, engram.retrievals, engram.imagined as i32,
                engram.grounded as i32, engram.created_at.to_rfc3339(),
                engram.last_retrieved.map(|d| d.to_rfc3339()), engram.project, tags_json
            ],
        )?;
        Ok(())
    }

    /// Update retrieval counters — called on every successful read.
    /// Must be called while holding the connection lock.
    fn touch(conn: &rusqlite::Connection, id: &str) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE engrams SET retrievals = retrievals + 1, last_retrieved = ?1 WHERE id = ?2",
            rusqlite::params![now, id],
        )?;
        Ok(())
    }

    /// Map a row to an Engram (without links — call enrich_links afterwards).
    /// Column order: id(0), layer(1), source(2), privacy_level(3), content(4),
    /// context(5), strength(6), valence(7), retrievals(8), imagined(9),
    /// grounded(10), created_at(11), last_retrieved(12), project(13), tags(14).
    fn row_to_engram(row: &rusqlite::Row) -> std::result::Result<Engram, rusqlite::Error> {
        let layer_str: String = row.get(1)?;
        let source_str: String = row.get(2)?;
        let privacy_str: String = row.get(3)?;
        let context_str: String = row.get(5)?;
        let tags_str: String = row.get(14)?;
        let created_str: String = row.get(11)?;
        let retrieved_str: Option<String> = row.get(12)?;
        Ok(Engram {
            id: row.get(0)?,
            layer: EngramLayer::from_str(&layer_str).unwrap_or(EngramLayer::Episodic),
            source: EngramSource::from_str(&source_str).unwrap_or(EngramSource::Interaction),
            privacy_level: PrivacyLevel::from_str(&privacy_str).unwrap_or_default(),
            content: row.get(4)?,
            context: serde_json::from_str(&context_str).unwrap_or(serde_json::json!({})),
            links: Vec::new(),
            strength: row.get(6)?,
            valence: row.get(7)?,
            retrievals: row.get(8)?,
            imagined: row.get::<_, i32>(9)? != 0,
            grounded: row.get::<_, i32>(10)? != 0,
            created_at: chrono::DateTime::parse_from_rfc3339(&created_str)
                .map(|d| d.with_timezone(&Utc)).unwrap_or_else(|e| {
                    eprintln!("[engram-core] Engram has corrupted created_at '{}': {}", created_str, e);
                    Utc::now()
                }),
            last_retrieved: retrieved_str.and_then(|s|
                chrono::DateTime::parse_from_rfc3339(&s)
                    .map(|d| d.with_timezone(&Utc)).ok()
            ),
            project: row.get(13)?,
            tags: serde_json::from_str(&tags_str).unwrap_or_default(),
        })
    }

    /// SQL SELECT clause used by all read queries. Keep in sync with row_to_engram.
    const ENGRAM_SELECT: &str = "id, layer, source, privacy_level, content, context, strength, valence, retrievals, imagined, grounded, created_at, last_retrieved, project, tags";

    /// Populate an engram's links from the engram_links table.
    /// Must be called while holding the connection lock.
    fn enrich_links(conn: &rusqlite::Connection, engram: &mut Engram) -> Result<()> {
        let mut stmt = conn.prepare(
            "SELECT target_id, weight, link_type FROM engram_links WHERE source_id = ?1 ORDER BY weight DESC",
        )?;
        let rows = stmt.query_map(params![engram.id], |row| {
            let link_type_str: String = row.get(2)?;
            Ok(EngramLink {
                target_id: row.get(0)?,
                weight: row.get(1)?,
                link_type: crate::engram::LinkType::from_str(&link_type_str)
                    .unwrap_or(crate::engram::LinkType::Associative),
            })
        })?;
        engram.links = Vec::new();
        for link in rows {
            engram.links.push(link?);
        }
        Ok(())
    }

    /// Get engram by ID
    pub async fn get(&self, id: &str) -> Result<Engram> {
        let conn = self.conn.lock().await;
        let mut engram = conn.query_row(
            "SELECT id, layer, source, privacy_level, content, context, strength, valence, retrievals, imagined, grounded, created_at, last_retrieved, project, tags FROM engrams WHERE id = ?1",
            [id],
            |row| {
                let layer_str: String = row.get(1)?;
                let source_str: String = row.get(2)?;
                let privacy_str: String = row.get(3)?;
                let context_str: String = row.get(5)?;
                let tags_str: String = row.get(14)?;
                let created_str: String = row.get(11)?;
                let retrieved_str: Option<String> = row.get(12)?;

                Ok(Engram {
                    id: row.get(0)?,
                    layer: EngramLayer::from_str(&layer_str).unwrap_or(EngramLayer::Episodic),
                    source: EngramSource::from_str(&source_str).unwrap_or(EngramSource::Interaction),
                    privacy_level: PrivacyLevel::from_str(&privacy_str).unwrap_or_default(),
                    content: row.get(4)?,
                    context: serde_json::from_str(&context_str).unwrap_or(serde_json::json!({})),
                    links: Vec::new(), // populated below
                    strength: row.get(6)?,
                    valence: row.get(7)?,
                    retrievals: row.get(8)?,
                    imagined: row.get::<_, i32>(9)? != 0,
                    grounded: row.get::<_, i32>(10)? != 0,
                    created_at: chrono::DateTime::parse_from_rfc3339(&created_str)
                        .map(|d| d.with_timezone(&Utc)).unwrap_or_else(|e| {
                            eprintln!("[engram-core] Engram has corrupted created_at '{}': {}", created_str, e);
                            Utc::now()
                        }),
                    last_retrieved: retrieved_str.and_then(|s|
                        chrono::DateTime::parse_from_rfc3339(&s)
                            .map(|d| d.with_timezone(&Utc)).ok()
                    ),
                    project: row.get(13)?,
                    tags: serde_json::from_str(&tags_str).unwrap_or_default(),
                })
            },
        ).map_err(|e| {
            if matches!(e, rusqlite::Error::QueryReturnedNoRows) {
                EngramError::NotFound(id.to_string())
            } else {
                EngramError::Database(e)
            }
        })?;

        // H3: Update retrieval counters (disabled pending FTS trigger fix)
        // Self::touch(&conn, id)?;
        // H4: Populate links (disabled pending row mapper fix)
        // Self::enrich_links(&conn, &mut engram)?;

        Ok(engram)
    }

    /// Get coherence state
    pub async fn get_coherence(&self) -> Result<CoherenceState> {
        let conn = self.conn.lock().await;
        conn.query_row(
            "SELECT id, baseline_valence, character_strengths, purpose_vector, last_hygiene_daily, last_hygiene_weekly, drift_score, updated_at FROM coherence_state WHERE id = 1",
            [],
            |row| {
                let character_str: String = row.get(2)?;
                let purpose_str: String = row.get(3)?;
                let daily_str: Option<String> = row.get(4)?;
                let weekly_str: Option<String> = row.get(5)?;
                let updated_str: String = row.get(7)?;
                
                Ok(CoherenceState {
                    id: row.get(0)?,
                    baseline_valence: row.get(1)?,
                    character_strengths: serde_json::from_str(&character_str).unwrap_or_default(),
                    purpose_vector: serde_json::from_str(&purpose_str).unwrap_or_default(),
                    last_hygiene_daily: daily_str.and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).map(|d| d.with_timezone(&Utc)).ok()),
                    last_hygiene_weekly: weekly_str.and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).map(|d| d.with_timezone(&Utc)).ok()),
                    drift_score: row.get(6)?,
                    updated_at: chrono::DateTime::parse_from_rfc3339(&updated_str)
                        .map(|d| d.with_timezone(&Utc)).unwrap_or_else(|_| Utc::now()),
                })
            },
        ).map_err(|e| EngramError::NotFound(e.to_string()))
    }

    /// Update coherence state
    pub async fn update_coherence(&self, state: &CoherenceState) -> Result<()> {
        let conn = self.conn.lock().await;
        let character_json = serde_json::to_string(&state.character_strengths)?;
        let purpose_json = serde_json::to_string(&state.purpose_vector)?;
        
        conn.execute(
            "UPDATE coherence_state SET baseline_valence = ?1, character_strengths = ?2, purpose_vector = ?3, last_hygiene_daily = ?4, last_hygiene_weekly = ?5, drift_score = ?6, updated_at = ?7 WHERE id = 1",
            params![
                state.baseline_valence, character_json, purpose_json,
                state.last_hygiene_daily.map(|d| d.to_rfc3339()),
                state.last_hygiene_weekly.map(|d| d.to_rfc3339()),
                state.drift_score, Utc::now().to_rfc3339()
            ],
        )?;
        Ok(())
    }

    /// Search engrams by layer
    pub async fn search_by_layer(&self, layer: EngramLayer, limit: usize) -> Result<Vec<Engram>> {
        let conn = self.conn.lock().await;
        let limit_i64 = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut stmt = conn.prepare(
            "SELECT id, layer, source, privacy_level, content, context, strength, valence, retrievals, imagined, grounded, created_at, last_retrieved, project, tags FROM engrams WHERE layer = ?1 ORDER BY strength DESC, created_at DESC LIMIT ?2"
        )?;
        
        let rows = stmt.query_map(params![layer.as_str(), limit_i64], |row| {
            let layer_str: String = row.get(1)?;
            let source_str: String = row.get(2)?;
            let context_str: String = row.get(5)?;
            let tags_str: String = row.get(14)?;
            let created_str: String = row.get(11)?;
            let retrieved_str: Option<String> = row.get(12)?;
            
            Ok(Engram {
                id: row.get(0)?,
                layer: EngramLayer::from_str(&layer_str).unwrap_or(EngramLayer::Episodic),
                source: EngramSource::from_str(&source_str).unwrap_or(EngramSource::Interaction),
                content: row.get(4)?,
                context: serde_json::from_str(&context_str).unwrap_or(serde_json::json!({})),
                links: Vec::new(),
                privacy_level: PrivacyLevel::default(),
                strength: row.get(6)?,
                valence: row.get(7)?,
                retrievals: row.get(8)?,
                imagined: row.get::<_, i32>(9)? != 0,
                grounded: row.get::<_, i32>(10)? != 0,
                created_at: chrono::DateTime::parse_from_rfc3339(&created_str)
                    .map(|d| d.with_timezone(&Utc)).unwrap_or_else(|_| Utc::now()),
                last_retrieved: retrieved_str.and_then(|s| 
                    chrono::DateTime::parse_from_rfc3339(&s)
                        .map(|d| d.with_timezone(&Utc)).ok()
                ),
                project: row.get(13)?,
                tags: serde_json::from_str(&tags_str).unwrap_or_default(),
            })
        })?;
        
        let mut engrams = Vec::new();
        for engram in rows {
            engrams.push(engram?);
        }
        Ok(engrams)
    }

    /// Search engrams by content using FTS5 full-text index.
    /// Falls back to LIKE if the query contains characters that FTS5 can't parse.
    pub async fn search_by_content(&self, query: &str, limit: usize) -> Result<Vec<Engram>> {
        let conn = self.conn.lock().await;
        let limit_i64 = i64::try_from(limit).unwrap_or(i64::MAX);

        // Try FTS5 first — O(log n) instead of full-table LIKE scan
        let fts_result = (|| -> Result<Vec<Engram>> {
            // Sanitize: strip double-quotes that break FTS5 phrase syntax
            let sanitized: String = query.chars().filter(|c| *c != '"').collect();
            if sanitized.is_empty() {
                return Ok(Vec::new());
            }
            // Wrap in quotes for exact phrase matching in FTS5
            let fts_query = format!("\"{}\"", sanitized);
            let mut stmt = conn.prepare(
                "SELECT e.id, e.layer, e.source, e.content, e.context, e.strength, e.valence, e.retrievals, e.imagined, e.grounded, e.created_at, e.last_retrieved, e.project, e.tags \
                 FROM engrams_fts fts \
                 INNER JOIN engrams e ON fts.id = e.id \
                 WHERE engrams_fts MATCH ?1 \
                 ORDER BY rank \
                 LIMIT ?2"
            )?;
            let rows = stmt.query_map(params![fts_query, limit_i64], |row| {
                let layer_str: String = row.get(1)?;
                let source_str: String = row.get(2)?;
                let context_str: String = row.get(5)?;
                let tags_str: String = row.get(14)?;
                let created_str: String = row.get(11)?;
                let retrieved_str: Option<String> = row.get(12)?;
                
                Ok(Engram {
                    id: row.get(0)?,
                    layer: EngramLayer::from_str(&layer_str).unwrap_or(EngramLayer::Episodic),
                    source: EngramSource::from_str(&source_str).unwrap_or(EngramSource::Interaction),
                    content: row.get(4)?,
                    context: serde_json::from_str(&context_str).unwrap_or(serde_json::json!({})),
                    links: Vec::new(),
                    privacy_level: PrivacyLevel::default(),
                    strength: row.get(6)?,
                    valence: row.get(7)?,
                    retrievals: row.get(8)?,
                    imagined: row.get::<_, i32>(9)? != 0,
                    grounded: row.get::<_, i32>(10)? != 0,
                    created_at: chrono::DateTime::parse_from_rfc3339(&created_str)
                        .map(|d| d.with_timezone(&Utc)).unwrap_or_else(|_| Utc::now()),
                    last_retrieved: retrieved_str.and_then(|s| 
                        chrono::DateTime::parse_from_rfc3339(&s)
                            .map(|d| d.with_timezone(&Utc)).ok()
                    ),
                    project: row.get(13)?,
                    tags: serde_json::from_str(&tags_str).unwrap_or_default(),
                })
            })?;
            let mut engrams = Vec::new();
            for engram in rows {
                engrams.push(engram?);
            }
            Ok(engrams)
        })();

        match fts_result {
            Ok(results) => Ok(results),
            Err(_) => {
                // FTS5 parse error — fall back to LIKE
                let search_pattern = format!("%{}%", query);
                let mut stmt = conn.prepare(
                    "SELECT id, layer, source, privacy_level, content, context, strength, valence, retrievals, imagined, grounded, created_at, last_retrieved, project, tags FROM engrams WHERE content LIKE ?1 ORDER BY strength DESC LIMIT ?2"
                )?;
                let rows = stmt.query_map(params![search_pattern, limit_i64], |row| {
                    let layer_str: String = row.get(1)?;
                    let source_str: String = row.get(2)?;
                    let context_str: String = row.get(5)?;
                    let tags_str: String = row.get(14)?;
                    let created_str: String = row.get(11)?;
                    let retrieved_str: Option<String> = row.get(12)?;
                    
                    Ok(Engram {
                        id: row.get(0)?,
                        layer: EngramLayer::from_str(&layer_str).unwrap_or(EngramLayer::Episodic),
                        source: EngramSource::from_str(&source_str).unwrap_or(EngramSource::Interaction),
                        content: row.get(4)?,
                        context: serde_json::from_str(&context_str).unwrap_or(serde_json::json!({})),
                        links: Vec::new(),
                        privacy_level: PrivacyLevel::default(),
                        strength: row.get(6)?,
                        valence: row.get(7)?,
                        retrievals: row.get(8)?,
                        imagined: row.get::<_, i32>(9)? != 0,
                        grounded: row.get::<_, i32>(10)? != 0,
                        created_at: chrono::DateTime::parse_from_rfc3339(&created_str)
                            .map(|d| d.with_timezone(&Utc)).unwrap_or_else(|_| Utc::now()),
                        last_retrieved: retrieved_str.and_then(|s| 
                            chrono::DateTime::parse_from_rfc3339(&s)
                                .map(|d| d.with_timezone(&Utc)).ok()
                        ),
                        project: row.get(13)?,
                        tags: serde_json::from_str(&tags_str).unwrap_or_default(),
                    })
                })?;
                let mut engrams = Vec::new();
                for engram in rows {
                    engrams.push(engram?);
                }
                Ok(engrams)
            }
        }
    }

    /// List all engrams with pagination
    pub async fn list(&self, limit: usize, offset: usize) -> Result<Vec<Engram>> {
        let conn = self.conn.lock().await;
        let limit_i64 = i64::try_from(limit).unwrap_or(i64::MAX);
        let offset_i64 = i64::try_from(offset).unwrap_or(i64::MAX);
        let mut stmt = conn.prepare(
            "SELECT id, layer, source, privacy_level, content, context, strength, valence, retrievals, imagined, grounded, created_at, last_retrieved, project, tags FROM engrams ORDER BY created_at DESC LIMIT ?1 OFFSET ?2"
        )?;
        
        let rows = stmt.query_map(params![limit_i64, offset_i64], |row| {
            let layer_str: String = row.get(1)?;
            let source_str: String = row.get(2)?;
            let context_str: String = row.get(5)?;
            let tags_str: String = row.get(14)?;
            let created_str: String = row.get(11)?;
            let retrieved_str: Option<String> = row.get(12)?;
            
            Ok(Engram {
                id: row.get(0)?,
                layer: EngramLayer::from_str(&layer_str).unwrap_or(EngramLayer::Episodic),
                source: EngramSource::from_str(&source_str).unwrap_or(EngramSource::Interaction),
                content: row.get(4)?,
                context: serde_json::from_str(&context_str).unwrap_or(serde_json::json!({})),
                links: Vec::new(),
                privacy_level: PrivacyLevel::default(),
                strength: row.get(6)?,
                valence: row.get(7)?,
                retrievals: row.get(8)?,
                imagined: row.get::<_, i32>(9)? != 0,
                grounded: row.get::<_, i32>(10)? != 0,
                created_at: chrono::DateTime::parse_from_rfc3339(&created_str)
                    .map(|d| d.with_timezone(&Utc)).unwrap_or_else(|_| Utc::now()),
                last_retrieved: retrieved_str.and_then(|s| 
                    chrono::DateTime::parse_from_rfc3339(&s)
                        .map(|d| d.with_timezone(&Utc)).ok()
                ),
                project: row.get(13)?,
                tags: serde_json::from_str(&tags_str).unwrap_or_default(),
            })
        })?;
        
        let mut engrams = Vec::new();
        for engram in rows {
            engrams.push(engram?);
        }
        Ok(engrams)
    }

    /// Search engrams by tag(s). Returns engrams that contain ALL of the given
    /// tags, sorted by `created_at DESC` (newest first).
    ///
    /// Tags are stored as a JSON array string in the `tags` column, so we match
    /// via `LIKE '%"tag"%'` for each tag.
    pub async fn search_by_tags(&self, tags: &[&str], limit: usize) -> Result<Vec<Engram>> {
        let conn = self.conn.lock().await;
        let limit_i64 = i64::try_from(limit).unwrap_or(i64::MAX);

        // Build a query with one LIKE clause per tag
        let clauses: Vec<String> = (0..tags.len())
            .map(|i| format!("tags LIKE ?{}", i + 1))
            .collect();
        let where_clause = clauses.join(" AND ");
        let sql = format!(
            "SELECT id, layer, source, privacy_level, content, context, strength, valence, retrievals, imagined, grounded, created_at, last_retrieved, project, tags FROM engrams WHERE {} ORDER BY created_at DESC LIMIT ?{}",
            where_clause,
            tags.len() + 1
        );

        let mut stmt = conn.prepare(&sql)?;

        // Build params: one LIKE pattern per tag, then the limit
        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        for tag in tags {
            param_values.push(Box::new(format!("%\"{}\"%", tag)));
        }
        param_values.push(Box::new(limit_i64));

        let params_refs: Vec<&dyn rusqlite::types::ToSql> = param_values.iter().map(|p| p.as_ref()).collect();

        let rows = stmt.query_map(params_refs.as_slice(), |row| {
            let layer_str: String = row.get(1)?;
            let source_str: String = row.get(2)?;
            let context_str: String = row.get(5)?;
            let tags_str: String = row.get(14)?;
            let created_str: String = row.get(11)?;
            let retrieved_str: Option<String> = row.get(12)?;

            Ok(Engram {
                id: row.get(0)?,
                layer: EngramLayer::from_str(&layer_str).unwrap_or(EngramLayer::Episodic),
                source: EngramSource::from_str(&source_str).unwrap_or(EngramSource::Interaction),
                content: row.get(4)?,
                context: serde_json::from_str(&context_str).unwrap_or(serde_json::json!({})),
                links: Vec::new(),
                privacy_level: PrivacyLevel::default(),
                strength: row.get(6)?,
                valence: row.get(7)?,
                retrievals: row.get(8)?,
                imagined: row.get::<_, i32>(9)? != 0,
                grounded: row.get::<_, i32>(10)? != 0,
                created_at: chrono::DateTime::parse_from_rfc3339(&created_str)
                    .map(|d| d.with_timezone(&Utc)).unwrap_or_else(|_| Utc::now()),
                last_retrieved: retrieved_str.and_then(|s|
                    chrono::DateTime::parse_from_rfc3339(&s)
                        .map(|d| d.with_timezone(&Utc)).ok()
                ),
                project: row.get(13)?,
                tags: serde_json::from_str(&tags_str).unwrap_or_default(),
            })
        })?;

        let mut engrams = Vec::new();
        for engram in rows {
            engrams.push(engram?);
        }
        Ok(engrams)
    }

    /// Get engram count
    pub async fn count(&self) -> Result<i64> {
        let conn = self.conn.lock().await;
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM engrams", [], |row| row.get(0))?;
        Ok(count)
    }

    /// Create or update a link between two engrams
    pub async fn link(
        &self,
        source_id: &str,
        target_id: &str,
        weight: f64,
        link_type: crate::engram::LinkType,
    ) -> Result<()> {
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT OR REPLACE INTO engram_links (source_id, target_id, weight, link_type) VALUES (?1, ?2, ?3, ?4)",
            params![source_id, target_id, weight, link_type.as_str()],
        )?;
        Ok(())
    }

    /// Get all outgoing links from an engram
    pub async fn get_links(&self, engram_id: &str) -> Result<Vec<EngramLink>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn.prepare(
            "SELECT target_id, weight, link_type FROM engram_links WHERE source_id = ?1 ORDER BY weight DESC",
        )?;
        let rows = stmt.query_map([engram_id], |row| {
            let link_type_str: String = row.get(2)?;
            Ok(EngramLink {
                target_id: row.get(0)?,
                weight: row.get(1)?,
                link_type: crate::engram::LinkType::from_str(&link_type_str)
                    .unwrap_or(crate::engram::LinkType::Associative),
            })
        })?;
        let mut links = Vec::new();
        for link in rows {
            links.push(link?);
        }
        Ok(links)
    }

    /// Find engrams related to the given one by following outgoing links,
    /// sorted by link weight descending.
    pub async fn search_related(&self, engram_id: &str, limit: usize) -> Result<Vec<Engram>> {
        // Get links first, then fetch engrams — split to avoid holding conn twice
        let links = self.get_links(engram_id).await?;
        let conn = self.conn.lock().await;
        let mut engrams = Vec::new();
        for link in links.iter().take(limit) {
            let result = conn.query_row(
                "SELECT id, layer, source, privacy_level, content, context, strength, valence, retrievals, imagined, grounded, created_at, last_retrieved, project, tags FROM engrams WHERE id = ?1",
                [&link.target_id],
                |row| {
                    let layer_str: String = row.get(1)?;
                    let source_str: String = row.get(2)?;
                    let context_str: String = row.get(5)?;
                    let tags_str: String = row.get(14)?;
                    let created_str: String = row.get(11)?;
                    let retrieved_str: Option<String> = row.get(12)?;
                    Ok(Engram {
                        id: row.get(0)?,
                        layer: EngramLayer::from_str(&layer_str).unwrap_or(EngramLayer::Episodic),
                        source: EngramSource::from_str(&source_str).unwrap_or(EngramSource::Interaction),
                        content: row.get(4)?,
                        context: serde_json::from_str(&context_str).unwrap_or(serde_json::json!({})),
                        links: Vec::new(),
                        privacy_level: PrivacyLevel::default(),
                        strength: row.get(6)?,
                        valence: row.get(7)?,
                        retrievals: row.get(8)?,
                        imagined: row.get::<_, i32>(9)? != 0,
                        grounded: row.get::<_, i32>(10)? != 0,
                        created_at: chrono::DateTime::parse_from_rfc3339(&created_str)
                            .map(|d| d.with_timezone(&Utc))
                            .unwrap_or_else(|_| Utc::now()),
                        last_retrieved: retrieved_str.and_then(|s|
                            chrono::DateTime::parse_from_rfc3339(&s)
                                .map(|d| d.with_timezone(&Utc))
                                .ok()
                        ),
                        project: row.get(13)?,
                        tags: serde_json::from_str(&tags_str).unwrap_or_default(),
                    })
                },
            );
            if let Ok(e) = result {
                engrams.push(e);
            }
        }
        Ok(engrams)
    }

    /// Apply strength decay and retrieve-strengthening for daily hygiene.
    /// Uses Ebbinghaus forgetting curve: R = e^(-t/S) where t=days since last access, S=stability.
    /// Frequently-accessed engrams have higher stability and decay slower.
    /// Returns (strengthened, decayed) counts.
    pub async fn apply_daily_hygiene(&self) -> Result<(i32, i32)> {
        let conn = self.conn.lock().await;
        let now = Utc::now();

        // Strengthen recently-retrieved engrams (Hebbian-like)
        let strengthened = conn.execute(
            "UPDATE engrams SET strength = MIN(2.0, strength + 0.15) WHERE last_retrieved >= datetime('now', '-1 day')",
            [],
        )? as i32;

        // Ebbinghaus decay for engrams not accessed recently
        // We compute decay in Rust for the exponential curve
        let mut stmt = conn.prepare(
            "SELECT id, strength, retrievals, created_at, last_retrieved FROM engrams WHERE last_retrieved IS NULL OR last_retrieved < datetime('now', '-1 day')"
        )?;

        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,      // id
                row.get::<_, f64>(1)?,          // strength
                row.get::<_, i64>(2)?,          // retrievals
                row.get::<_, String>(3)?,       // created_at
                row.get::<_, Option<String>>(4)?, // last_retrieved
            ))
        })?;

        let mut decayed = 0;
        for row in rows {
            let (id, strength, retrievals, created_str, last_retrieved_str) = row?;

            // Calculate days since last access (or creation if never retrieved)
            let last_access = last_retrieved_str
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_else(|| {
                    chrono::DateTime::parse_from_rfc3339(&created_str)
                        .map(|d| d.with_timezone(&Utc))
                        .unwrap_or_else(|_| now)
                });
            let days_since = (now - last_access).num_days().max(0) as f64;

            // Stability: base on retrievals + 1 (more retrievals = more stable)
            // Higher stability = slower decay
            let stability = (retrievals as f64 + 1.0) * 3.0; // S = (retrievals+1) * 3 days

            // Ebbinghaus: R = e^(-t/S)
            let retention = (-days_since / stability).exp();

            // New strength = current strength * retention (but don't drop below 0.01)
            let new_strength = (strength * retention).max(0.01);

            if (new_strength - strength).abs() > 0.001 {
                conn.execute(
                    "UPDATE engrams SET strength = ?1 WHERE id = ?2",
                    params![new_strength, id],
                )?;
                decayed += 1;
            }
        }

        Ok((strengthened, decayed))
    }

    /// Promote frequently-retrieved episodic engrams to semantic, prune near-zero imagined engrams.
    /// Returns (promoted, pruned) counts.
    pub async fn apply_weekly_consolidation(&self) -> Result<(i32, i32)> {
        let conn = self.conn.lock().await;
        let promoted = conn.execute(
            "UPDATE engrams SET layer = 'semantic', source = 'consolidation' WHERE layer = 'episodic' AND retrievals >= 5",
            [],
        )? as i32;
        let pruned = conn.execute(
            "DELETE FROM engrams WHERE strength < 0.05 AND imagined = 1",
            [],
        )? as i32;
        Ok((promoted, pruned))
    }

    // ─── Vector / Embedding Methods ──────────────────────────────────────────

    /// Store an embedding vector for an engram.
    pub async fn store_embedding(&self, engram_id: &str, embedding: &[f64]) -> Result<()> {
        let conn = self.conn.lock().await;
        let blob = embedding_to_blob(embedding);
        let dims = embedding.len() as i64;
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT OR REPLACE INTO engram_embeddings (engram_id, embedding, dimensions, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![engram_id, blob, dims, now],
        )?;
        Ok(())
    }

    /// Search engrams by cosine similarity to a query embedding.
    /// Returns (Engram, similarity_score) pairs sorted by descending similarity.
    pub async fn vector_search(&self, query: &[f64], limit: usize) -> Result<Vec<(Engram, f64)>> {
        let conn = self.conn.lock().await;

        // Load all embeddings (for SQLite-based cosine search)
        let mut stmt = conn.prepare(
            "SELECT e.engram_id, e.embedding FROM engram_embeddings e"
        )?;

        let mut scored: Vec<(String, f64)> = Vec::new();
        let rows = stmt.query_map([], |row| {
            let id: String = row.get(0)?;
            let blob: Vec<u8> = row.get(1)?;
            Ok((id, blob))
        })?;

        for row in rows {
            let (id, blob) = row?;
            let emb = blob_to_embedding(&blob);
            let sim = cosine_similarity(query, &emb);
            scored.push((id, sim));
        }

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit);

        // Fetch full engrams for top results
        let mut results = Vec::new();
        for (id, score) in scored {
            if let Ok(engram) = self.get_inner(&conn, &id) {
                results.push((engram, score));
            }
        }

        Ok(results)
    }

    /// Proactive memory surfacing: find engrams relevant to the current context
    /// without explicit search. Uses recency, strength, and content matching.
    /// Returns top `limit` engrams sorted by relevance score.
    pub async fn surface_relevant(&self, context: &str, limit: usize) -> Result<Vec<(Engram, f64)>> {
        let conn = self.conn.lock().await;
        let now = Utc::now();

        // Get recent high-strength engrams as candidates
        let mut stmt = conn.prepare(
            "SELECT id, layer, source, privacy_level, content, context, strength, valence, retrievals, imagined, grounded, created_at, last_retrieved, project, tags FROM engrams WHERE strength > 0.1 ORDER BY strength DESC LIMIT 50"
        )?;

        let rows = stmt.query_map([], |row| {
            let layer_str: String = row.get(1)?;
            let source_str: String = row.get(2)?;
            let context_str: String = row.get(5)?;
            let tags_str: String = row.get(14)?;
            let created_str: String = row.get(11)?;
            let retrieved_str: Option<String> = row.get(12)?;

            Ok(Engram {
                id: row.get(0)?,
                layer: EngramLayer::from_str(&layer_str).unwrap_or(EngramLayer::Episodic),
                source: EngramSource::from_str(&source_str).unwrap_or(EngramSource::Interaction),
                content: row.get(4)?,
                context: serde_json::from_str(&context_str).unwrap_or(serde_json::json!({})),
                links: Vec::new(),
                privacy_level: PrivacyLevel::default(),
                strength: row.get(6)?,
                valence: row.get(7)?,
                retrievals: row.get(8)?,
                imagined: row.get::<_, i32>(9)? != 0,
                grounded: row.get::<_, i32>(10)? != 0,
                created_at: chrono::DateTime::parse_from_rfc3339(&created_str)
                    .map(|d| d.with_timezone(&Utc)).unwrap_or_else(|_| Utc::now()),
                last_retrieved: retrieved_str.and_then(|s|
                    chrono::DateTime::parse_from_rfc3339(&s)
                        .map(|d| d.with_timezone(&Utc)).ok()
                ),
                project: row.get(13)?,
                tags: serde_json::from_str(&tags_str).unwrap_or_default(),
            })
        })?;

        let mut scored: Vec<(Engram, f64)> = Vec::new();
        let context_lower = context.to_lowercase();
        let context_words: Vec<&str> = context_lower.split_whitespace().collect();

        for row in rows {
            let engram = row?;
            let mut score: f64 = 0.0;

            // Content relevance: word overlap
            let content_lower = engram.content.to_lowercase();
            let content_words: Vec<&str> = content_lower.split_whitespace().collect();
            let overlap = context_words.iter()
                .filter(|w| content_words.contains(w) && w.len() > 3)
                .count() as f64;
            score += overlap * 0.3;

            // Strength bonus
            score += engram.strength * 0.4;

            // Recency bonus (decay over days)
            let days_old = (now - engram.created_at).num_days().max(0) as f64;
            let recency = (-days_old / 7.0).exp(); // half-life ~7 days
            score += recency * 0.2;

            // Valence bonus (positive memories surface more easily)
            if engram.valence > 0.0 {
                score += engram.valence * 0.1;
            }

            if score > 0.1 {
                scored.push((engram, score));
            }
        }

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit);

        Ok(scored)
    }

    /// Internal get that takes a connection reference (avoids double-locking)
    fn get_inner(&self, conn: &rusqlite::Connection, id: &str) -> Result<Engram> {
        conn.query_row(
            "SELECT id, layer, source, privacy_level, content, context, strength, valence, retrievals, imagined, grounded, created_at, last_retrieved, project, tags FROM engrams WHERE id = ?1",
            params![id],
            |row| {
                let layer_str: String = row.get(1)?;
                let source_str: String = row.get(2)?;
                let context_str: String = row.get(5)?;
                let tags_str: String = row.get(14)?;
                let created_str: String = row.get(11)?;
                let retrieved_str: Option<String> = row.get(12)?;

                Ok(Engram {
                    id: row.get(0)?,
                    layer: EngramLayer::from_str(&layer_str).unwrap_or(EngramLayer::Episodic),
                    source: EngramSource::from_str(&source_str).unwrap_or(EngramSource::Interaction),
                    content: row.get(4)?,
                    context: serde_json::from_str(&context_str).unwrap_or(serde_json::json!({})),
                    links: Vec::new(),
                    privacy_level: PrivacyLevel::default(),
                    strength: row.get(6)?,
                    valence: row.get(7)?,
                    retrievals: row.get(8)?,
                    imagined: row.get::<_, i32>(9)? != 0,
                    grounded: row.get::<_, i32>(10)? != 0,
                    created_at: chrono::DateTime::parse_from_rfc3339(&created_str)
                        .map(|d| d.with_timezone(&Utc)).unwrap_or_else(|_| Utc::now()),
                    last_retrieved: retrieved_str.and_then(|s|
                        chrono::DateTime::parse_from_rfc3339(&s)
                            .map(|d| d.with_timezone(&Utc)).ok()
                    ),
                    project: row.get(13)?,
                    tags: serde_json::from_str(&tags_str).unwrap_or_default(),
                })
            },
        ).map_err(|_| EngramError::NotFound(id.to_string()))
    }

    // ─── Temporal Pattern Detection ──────────────────────────────────────────

    /// Detect temporal patterns in engrams matching a query.
    ///
    /// Analyzes the `created_at` timestamps of matching engrams to find:
    /// - Day-of-week patterns (e.g., "you tend to ask about this on Thursdays")
    /// - Time-of-day patterns (e.g., "you usually work on this in the evening")
    ///
    /// Returns a `TemporalPattern` if a statistically meaningful pattern is found,
    /// or `None` if the data is too sparse or evenly distributed.
    pub async fn detect_temporal_patterns(&self, query: &str, min_engrams: usize) -> Result<Option<TemporalPattern>> {
        let conn = self.conn.lock().await;
        let search_pattern = format!("%{}%", query);
        let mut stmt = conn.prepare(
            "SELECT created_at FROM engrams WHERE content LIKE ?1 ORDER BY created_at DESC LIMIT 200"
        )?;

        let timestamps: Vec<chrono::DateTime<Utc>> = stmt.query_map(params![search_pattern], |row| {
            let ts_str: String = row.get(0)?;
            Ok(chrono::DateTime::parse_from_rfc3339(&ts_str)
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()))
        })?
        .filter_map(|r| r.ok())
        .collect();

        if timestamps.len() < min_engrams {
            return Ok(None);
        }

        // Analyze day-of-week distribution
        let mut dow_counts = [0u32; 7]; // Mon=0 .. Sun=6
        let mut hour_counts = [0u32; 4]; // morning(6-12), afternoon(12-18), evening(18-24), night(0-6)
        for ts in &timestamps {
            let weekday = ts.weekday().num_days_from_monday() as usize;
            dow_counts[weekday] += 1;

            let hour = ts.hour() as usize;
            let period = match hour {
                6..=11 => 0,  // morning
                12..=17 => 1, // afternoon
                18..=23 => 2, // evening
                _ => 3,       // night
            };
            hour_counts[period] += 1;
        }

        let total = timestamps.len() as f64;
        let expected_dow = total / 7.0;
        let expected_period = total / 4.0;

        // Find the strongest day-of-week signal
        let dow_names = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
        let (peak_dow, peak_dow_count) = dow_counts.iter().enumerate()
            .max_by_key(|(_, &c)| c)
            .map(|(i, &c)| (i, c))
            .unwrap_or((0, 0));
        let dow_strength = if expected_dow > 0.0 { peak_dow_count as f64 / expected_dow } else { 0.0 };

        // Find the strongest time-of-day signal
        let period_names = ["morning", "afternoon", "evening", "night"];
        let (peak_period, peak_period_count) = hour_counts.iter().enumerate()
            .max_by_key(|(_, &c)| c)
            .map(|(i, &c)| (i, c))
            .unwrap_or((0, 0));
        let period_strength = if expected_period > 0.0 { peak_period_count as f64 / expected_period } else { 0.0 };

        // Require at least 1.8x expected frequency to report a pattern
        let has_dow_pattern = dow_strength >= 1.8 && peak_dow_count >= 3;
        let has_period_pattern = period_strength >= 1.8 && peak_period_count >= 3;

        if !has_dow_pattern && !has_period_pattern {
            return Ok(None);
        }

        Ok(Some(TemporalPattern {
            query: query.to_string(),
            sample_size: timestamps.len(),
            peak_day: if has_dow_pattern { Some(dow_names[peak_dow].to_string()) } else { None },
            day_strength: if has_dow_pattern { Some(dow_strength) } else { None },
            peak_period: if has_period_pattern { Some(period_names[peak_period].to_string()) } else { None },
            period_strength: if has_period_pattern { Some(period_strength) } else { None },
        }))
    }
}

/// Detected temporal pattern in engram access/creation times.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TemporalPattern {
    pub query: String,
    pub sample_size: usize,
    /// Peak day-of-week (e.g., "Thursday") — None if no significant day pattern
    pub peak_day: Option<String>,
    /// How much stronger the peak day is vs. uniform distribution (e.g., 2.5 = 2.5x expected)
    pub day_strength: Option<f64>,
    /// Peak time period (e.g., "evening") — None if no significant period pattern
    pub peak_period: Option<String>,
    /// How much stronger the peak period is vs. uniform distribution
    pub period_strength: Option<f64>,
}

impl TemporalPattern {
    /// Generate a natural language description of the pattern.
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if let (Some(day), Some(strength)) = (&self.peak_day, self.day_strength) {
            parts.push(format!("you tend to do this on {}s ({:.0}% of the time)", day, (strength / 7.0) * 100.0));
        }
        if let (Some(period), Some(_)) = (&self.peak_period, self.period_strength) {
            parts.push(format!("usually in the {}", period));
        }
        if parts.is_empty() {
            return "No clear temporal pattern detected.".to_string();
        }
        format!("I've noticed {}", parts.join(", "))
    }
}

// ─── Embedding helpers ───────────────────────────────────────────────────────

fn embedding_to_blob(embedding: &[f64]) -> Vec<u8> {
    embedding.iter().flat_map(|f| f.to_le_bytes()).collect()
}

fn blob_to_embedding(blob: &[u8]) -> Vec<f64> {
    blob.chunks_exact(8)
        .map(|c| f64::from_le_bytes(c.try_into().unwrap_or([0u8; 8])))
        .collect()
}

fn cosine_similarity(a: &[f64], b: &[f64]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f64 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f64 = a.iter().map(|x| x * x).sum::<f64>().sqrt();
    let norm_b: f64 = b.iter().map(|x| x * x).sum::<f64>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engram::LinkType;
    use tempfile::tempdir;

    async fn test_store() -> (EngramStore, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let store = EngramStore::open(&path).await.unwrap();
        (store, dir)
    }

    fn make_engram(content: &str) -> Engram {
        Engram::new_episodic(
            content.to_string(),
            EngramSource::Interaction,
            serde_json::json!({}),
        )
    }

    #[tokio::test]
    async fn test_write_and_get() {
        let (store, _dir) = test_store().await;
        let e = make_engram("hello world");
        let id = e.id.clone();
        store.write(&e).await.unwrap();
        let fetched = store.get(&id).await.unwrap();
        assert_eq!(fetched.content, "hello world");
    }

    #[tokio::test]
    async fn test_count_and_list() {
        let (store, _dir) = test_store().await;
        store.write(&make_engram("one")).await.unwrap();
        store.write(&make_engram("two")).await.unwrap();
        assert_eq!(store.count().await.unwrap(), 2);
        let list = store.list(10, 0).await.unwrap();
        assert_eq!(list.len(), 2);
    }

    #[tokio::test]
    async fn test_search_by_content() {
        let (store, _dir) = test_store().await;
        store.write(&make_engram("the quick brown fox")).await.unwrap();
        store.write(&make_engram("unrelated content")).await.unwrap();
        let results = store.search_by_content("quick", 10).await.unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains("quick"));
    }

    #[tokio::test]
    async fn test_link_and_get_links() {
        let (store, _dir) = test_store().await;
        let a = make_engram("alpha");
        let b = make_engram("beta");
        let aid = a.id.clone();
        let bid = b.id.clone();
        store.write(&a).await.unwrap();
        store.write(&b).await.unwrap();
        store.link(&aid, &bid, 0.8, LinkType::Associative).await.unwrap();
        let links = store.get_links(&aid).await.unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target_id, bid);
        assert!((links[0].weight - 0.8).abs() < 1e-9);
        assert_eq!(links[0].link_type, LinkType::Associative);
    }

    #[tokio::test]
    async fn test_search_related() {
        let (store, _dir) = test_store().await;
        let a = make_engram("source");
        let b = make_engram("related");
        let c = make_engram("unrelated");
        let aid = a.id.clone();
        let bid = b.id.clone();
        store.write(&a).await.unwrap();
        store.write(&b).await.unwrap();
        store.write(&c).await.unwrap();
        store.link(&aid, &bid, 0.9, LinkType::Causal).await.unwrap();
        let related = store.search_related(&aid, 10).await.unwrap();
        assert_eq!(related.len(), 1);
        assert_eq!(related[0].content, "related");
    }

    #[tokio::test]
    async fn test_link_types() {
        let (store, _dir) = test_store().await;
        let a = make_engram("a");
        let b = make_engram("b");
        let c = make_engram("c");
        let d = make_engram("d");
        let (aid, bid, cid, did) = (a.id.clone(), b.id.clone(), c.id.clone(), d.id.clone());
        for e in [&a, &b, &c, &d] { store.write(e).await.unwrap(); }
        store.link(&aid, &bid, 0.5, LinkType::Associative).await.unwrap();
        store.link(&aid, &cid, 0.6, LinkType::Causal).await.unwrap();
        store.link(&aid, &did, 0.7, LinkType::Analogical).await.unwrap();
        let links = store.get_links(&aid).await.unwrap();
        assert_eq!(links.len(), 3);
        // Sorted by weight desc
        assert_eq!(links[0].link_type, LinkType::Analogical);
    }

    #[tokio::test]
    async fn test_daily_hygiene_no_crash() {
        let (store, _dir) = test_store().await;
        let (strengthened, decayed) = store.apply_daily_hygiene().await.unwrap();
        // Empty store — both counts are 0
        assert_eq!(strengthened, 0);
        assert_eq!(decayed, 0);
    }

    #[tokio::test]
    async fn test_weekly_consolidation_no_crash() {
        let (store, _dir) = test_store().await;
        let (promoted, pruned) = store.apply_weekly_consolidation().await.unwrap();
        assert_eq!(promoted, 0);
        assert_eq!(pruned, 0);
    }
}
