//! SQLite schema definitions

use rusqlite::Connection;

/// Create all tables (idempotent via IF NOT EXISTS).
/// Call `migrate()` after `create_tables()` to add columns added in newer versions.
pub fn create_tables(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS engrams (
            id              TEXT PRIMARY KEY,
            layer           TEXT NOT NULL CHECK(layer IN ('episodic','semantic','imagined')),
            source          TEXT NOT NULL DEFAULT 'interaction' CHECK(source IN ('interaction','sensor','consolidation','imagined','chat','window','mic','agent','research','system','user','observation','ai-session','ai-tool')),
            privacy_level   TEXT NOT NULL DEFAULT 'cloud_first' CHECK(privacy_level IN ('strict_local','hybrid','cloud_first','enterprise')),
            content         TEXT NOT NULL,
            context         TEXT NOT NULL,
            strength        REAL NOT NULL DEFAULT 1.0,
            valence         REAL NOT NULL DEFAULT 0.0 CHECK(valence BETWEEN -1.0 AND 1.0),
            retrievals      INTEGER NOT NULL DEFAULT 0,
            imagined        INTEGER NOT NULL DEFAULT 0,
            grounded        INTEGER NOT NULL DEFAULT 0,
            created_at      TEXT NOT NULL,
            last_retrieved  TEXT,
            project         TEXT,
            tags            TEXT
        );

        CREATE TABLE IF NOT EXISTS engram_links (
            source_id   TEXT NOT NULL REFERENCES engrams(id) ON DELETE CASCADE,
            target_id   TEXT NOT NULL REFERENCES engrams(id) ON DELETE CASCADE,
            weight      REAL NOT NULL DEFAULT 0.5,
            link_type   TEXT NOT NULL CHECK(link_type IN ('associative','causal','analogical','temporal')),
            PRIMARY KEY (source_id, target_id)
        );

        CREATE TABLE IF NOT EXISTS coherence_state (
            id                  INTEGER PRIMARY KEY DEFAULT 1,
            baseline_valence    REAL NOT NULL DEFAULT 0.3,
            character_strengths TEXT NOT NULL,
            purpose_vector      TEXT NOT NULL,
            last_hygiene_daily  TEXT,
            last_hygiene_weekly TEXT,
            drift_score         REAL DEFAULT 0.0,
            updated_at          TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS goals (
            id              TEXT PRIMARY KEY,
            description     TEXT NOT NULL,
            pathways        TEXT NOT NULL,
            agency_score    REAL DEFAULT 0.5,
            created_at      TEXT NOT NULL,
            status          TEXT DEFAULT 'active' CHECK(status IN ('active','achieved','released'))
        );

        CREATE TABLE IF NOT EXISTS consolidation_runs (
            id                  TEXT PRIMARY KEY,
            run_at              TEXT NOT NULL,
            episodes_processed  INTEGER,
            semantics_created   INTEGER,
            engrams_decayed     INTEGER,
            notes               TEXT
        );

        CREATE INDEX IF NOT EXISTS idx_engrams_layer ON engrams(layer);
        CREATE INDEX IF NOT EXISTS idx_engrams_source ON engrams(source);
        CREATE INDEX IF NOT EXISTS idx_engrams_created_at ON engrams(created_at);
        CREATE INDEX IF NOT EXISTS idx_engrams_imagined ON engrams(imagined);
        CREATE INDEX IF NOT EXISTS idx_engram_links_source ON engram_links(source_id);
        CREATE INDEX IF NOT EXISTS idx_engram_links_target ON engram_links(target_id);

        -- Semantic embeddings for vector search
        CREATE TABLE IF NOT EXISTS engram_embeddings (
            engram_id    TEXT PRIMARY KEY REFERENCES engrams(id) ON DELETE CASCADE,
            embedding    BLOB NOT NULL,  -- serialized f64 vector
            model        TEXT NOT NULL DEFAULT 'text-embedding-3-small',
            dimensions   INTEGER NOT NULL DEFAULT 1536,
            created_at  TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_embeddings_created ON engram_embeddings(created_at);

        -- Memory evidence (provenance chain)
        CREATE TABLE IF NOT EXISTS memory_evidence (
            memory_id    TEXT NOT NULL REFERENCES engrams(id) ON DELETE CASCADE,
            evidence_id  TEXT NOT NULL REFERENCES engrams(id) ON DELETE CASCADE,
            relationship TEXT NOT NULL DEFAULT 'supports',
            PRIMARY KEY (memory_id, evidence_id)
        );

        CREATE INDEX IF NOT EXISTS idx_memory_evidence_memory ON memory_evidence(memory_id);
        CREATE INDEX IF NOT EXISTS idx_memory_evidence_evidence ON memory_evidence(evidence_id);

        -- Annotations (user notes on memories)
        CREATE TABLE IF NOT EXISTS annotations (
            id          TEXT PRIMARY KEY,
            memory_id   TEXT NOT NULL REFERENCES engrams(id) ON DELETE CASCADE,
            content     TEXT NOT NULL,
            created_at  TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_annotations_memory ON annotations(memory_id);

        -- Saved searches (watchlists)
        CREATE TABLE IF NOT EXISTS saved_searches (
            id          TEXT PRIMARY KEY,
            query       TEXT NOT NULL DEFAULT '',
            layer       TEXT,
            tags        TEXT,
            notify      INTEGER NOT NULL DEFAULT 0,
            created_at  TEXT NOT NULL,
            last_checked_at TEXT
        );

        -- FTS5 virtual table for full-text search.
        -- FTS sync is managed in Rust code (store.rs) rather than SQLite triggers
        -- because the FTS 'delete' command is incompatible with SQLCipher's
        -- virtual table handling.
        CREATE VIRTUAL TABLE IF NOT EXISTS engrams_fts USING fts5(
            id,
            content
        );
        "#
    )?;

    Ok(())
}

/// Apply schema migrations for columns added after the initial release.
///
/// Current schema version. Increment this when adding new migrations below.
const CURRENT_SCHEMA_VERSION: i32 = 3;

/// Versioned schema migrations using SQLite's `PRAGMA user_version`.
///
/// Each migration is applied exactly once — the schema version tracks which
/// migrations have been applied.
///
/// Migrations are wrapped in a transaction so partial failures roll back
/// cleanly. Column additions use `PRAGMA table_info` to detect columns that
/// already exist (vaults created between the column introduction and the
/// `user_version` migration that missed the version bump), making the
/// migration idempotent.
///
/// Migration history:
///   v0 → v1: Added scope, content_type, occurred_at columns (2026-08-09)
///   v1 → v2: Fixed FTS5 content_rowid mismatch — engrams uses TEXT PK, not INTEGER PK (2026-08-11)
///   v2 → v3: Added ai-session, ai-tool to source CHECK constraint (2026-08-11)
#[allow(clippy::needless_return)]
pub fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let version: i32 = conn.query_row(
        "PRAGMA user_version",
        [],
        |row| row.get(0),
    )?;

    if version < 1 {
        // Wrap in a transaction so partial ALTER failure rolls back cleanly.
        conn.execute_batch("BEGIN")?;

        // Check which columns already exist (idempotent for vaults that
        // already have the columns but missed the user_version bump).
        let has_column = |name: &str| -> rusqlite::Result<bool> {
            let mut stmt = conn.prepare("PRAGMA table_info('engrams')")?;
            let exists = stmt
                .query_map([], |row| row.get::<_, String>(1))
                .unwrap()
                .filter_map(|r| r.ok())
                .any(|col| col == name);
            Ok(exists)
        };

        if !has_column("scope")? {
            conn.execute(
                "ALTER TABLE engrams ADD COLUMN scope TEXT NOT NULL DEFAULT 'moment'",
                [],
            )?;
            conn.execute(
                "UPDATE engrams SET scope = 'moment' WHERE scope IS NULL OR scope = ''",
                [],
            )?;
        }
        if !has_column("content_type")? {
            conn.execute(
                "ALTER TABLE engrams ADD COLUMN content_type TEXT NOT NULL DEFAULT 'text'",
                [],
            )?;
            conn.execute(
                "UPDATE engrams SET content_type = 'text' WHERE content_type IS NULL OR content_type = ''",
                [],
            )?;
        }
        if !has_column("occurred_at")? {
            conn.execute(
                "ALTER TABLE engrams ADD COLUMN occurred_at TEXT",
                [],
            )?;
        }

        conn.execute(
            &format!("PRAGMA user_version = {CURRENT_SCHEMA_VERSION}"),
            [],
        )?;
        conn.execute_batch("COMMIT")?;
    }

    if version < 2 {
        // v2: Fix FTS5 — drop triggers (FTS 'delete' command is incompatible
        // with SQLCipher's virtual table handling; sync is now done in Rust).
        // Also remove broken content_rowid='rowid' by recreating the FTS table.
        conn.execute_batch("BEGIN")?;

        // 1. Drop old triggers (may or may not exist)
        conn.execute_batch("DROP TRIGGER IF EXISTS engrams_ai;")?;
        conn.execute_batch("DROP TRIGGER IF EXISTS engrams_ad;")?;
        conn.execute_batch("DROP TRIGGER IF EXISTS engrams_au;")?;

        // 2. Drop old FTS table and recreate without content_rowid
        conn.execute_batch("DROP TABLE IF EXISTS engrams_fts;")?;
        conn.execute_batch(
            "CREATE VIRTUAL TABLE engrams_fts USING fts5(id, content);"
        )?;

        // 3. Re-index all existing engrams
        conn.execute_batch(
            "INSERT INTO engrams_fts(rowid, id, content) SELECT rowid, id, content FROM engrams;"
        )?;

        conn.execute(
            &format!("PRAGMA user_version = {CURRENT_SCHEMA_VERSION}"),
            [],
        )?;
        conn.execute_batch("COMMIT")?;
    }

    if version < 3 {
        // v3: Add ai-session, ai-tool to source CHECK constraint.
        // SQLite doesn't support ALTER TABLE to change CHECK constraints,
        // so we recreate the table.
        //
        // Foreign keys must be disabled BEFORE the transaction because
        // PRAGMA foreign_keys is a no-op inside a transaction. Disabling
        // FKs prevents ON DELETE CASCADE from wiping dependent tables
        // (engram_links, engram_embeddings, memory_evidence, annotations)
        // when we drop the old engrams table.
        //
        // We save/restore the previous FK state so a migration failure
        // doesn't leave FKs permanently disabled on this connection.

        // Save current FK state and disable before starting transaction
        let fk_was_on: bool = conn
            .query_row("PRAGMA foreign_keys", [], |row| row.get::<_, i64>(0))
            .map(|v| v != 0)
            .unwrap_or(true);
        if fk_was_on {
            conn.execute_batch("PRAGMA foreign_keys = OFF")?;
        }

        // Wrap migration in a closure so we can restore FK on failure
        let result = (|| -> rusqlite::Result<()> {
            conn.execute_batch("BEGIN")?;

            // 1. Create new table with updated CHECK constraint
            conn.execute_batch(
                "CREATE TABLE engrams_v3 (
                    id              TEXT PRIMARY KEY,
                    layer           TEXT NOT NULL CHECK(layer IN ('episodic','semantic','imagined')),
                    source          TEXT NOT NULL DEFAULT 'interaction' CHECK(source IN ('interaction','sensor','consolidation','imagined','chat','window','mic','agent','research','system','user','observation','ai-session','ai-tool')),
                    privacy_level   TEXT NOT NULL DEFAULT 'cloud_first' CHECK(privacy_level IN ('strict_local','hybrid','cloud_first','enterprise')),
                    content         TEXT NOT NULL,
                    context         TEXT NOT NULL,
                    strength        REAL NOT NULL DEFAULT 1.0,
                    valence         REAL NOT NULL DEFAULT 0.0 CHECK(valence BETWEEN -1.0 AND 1.0),
                    retrievals      INTEGER NOT NULL DEFAULT 0,
                    imagined        INTEGER NOT NULL DEFAULT 0,
                    grounded        INTEGER NOT NULL DEFAULT 0,
                    created_at      TEXT NOT NULL,
                    last_retrieved  TEXT,
                    project         TEXT,
                    tags            TEXT,
                    scope           TEXT NOT NULL DEFAULT 'moment',
                    content_type    TEXT NOT NULL DEFAULT 'text',
                    occurred_at     TEXT
                );"
            )?;

            // 2. Copy data from old table
            conn.execute_batch(
                "INSERT INTO engrams_v3 SELECT * FROM engrams;"
            )?;

            // 3. Drop old table (FKs disabled so no cascade) and rename
            conn.execute_batch("DROP TABLE engrams;")?;
            conn.execute_batch("ALTER TABLE engrams_v3 RENAME TO engrams;")?;

            // 4. Recreate indexes (lost when we dropped the old table)
            conn.execute_batch("CREATE INDEX IF NOT EXISTS idx_engrams_layer ON engrams(layer);")?;
            conn.execute_batch("CREATE INDEX IF NOT EXISTS idx_engrams_source ON engrams(source);")?;
            conn.execute_batch("CREATE INDEX IF NOT EXISTS idx_engrams_created_at ON engrams(created_at);")?;
            conn.execute_batch("CREATE INDEX IF NOT EXISTS idx_engrams_imagined ON engrams(imagined);")?;

            // 5. Rebuild FTS index
            conn.execute_batch("DROP TABLE IF EXISTS engrams_fts;")?;
            conn.execute_batch(
                "CREATE VIRTUAL TABLE engrams_fts USING fts5(id, content);"
            )?;
            conn.execute_batch(
                "INSERT INTO engrams_fts(rowid, id, content) SELECT rowid, id, content FROM engrams;"
            )?;

            conn.execute(
                &format!("PRAGMA user_version = {CURRENT_SCHEMA_VERSION}"),
                [],
            )?;
            conn.execute_batch("COMMIT")?;
            Ok(())
        })();

        // Always restore FK state
        if fk_was_on {
            conn.execute_batch("PRAGMA foreign_keys = ON")?;
        }

        // Propagate any error from the migration
        result?;
    }

    Ok(())
}