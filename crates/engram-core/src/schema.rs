//! SQLite schema definitions

use rusqlite::Connection;

/// Create all tables
pub fn create_tables(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS engrams (
            id              TEXT PRIMARY KEY,
            layer           TEXT NOT NULL CHECK(layer IN ('episodic','semantic','imagined')),
            source          TEXT NOT NULL DEFAULT 'interaction' CHECK(source IN ('interaction','sensor','consolidation','imagined','chat','window','mic','agent','research','system','user')),
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

        -- FTS5 virtual table for full-text search
        CREATE VIRTUAL TABLE IF NOT EXISTS engrams_fts USING fts5(
            id,
            content,
            content_rowid='rowid'
        );
        
        -- Triggers to keep FTS in sync
        CREATE TRIGGER IF NOT EXISTS engrams_ai AFTER INSERT ON engrams BEGIN
            INSERT INTO engrams_fts(id, content) VALUES (new.id, new.content);
        END;
        
        CREATE TRIGGER IF NOT EXISTS engrams_ad AFTER DELETE ON engrams BEGIN
            INSERT INTO engrams_fts(engrams_fts, id, content) VALUES('delete', old.id, old.content);
        END;
        
        CREATE TRIGGER IF NOT EXISTS engrams_au AFTER UPDATE ON engrams BEGIN
            INSERT INTO engrams_fts(engrams_fts, id, content) VALUES('delete', old.id, old.content);
            INSERT INTO engrams_fts(id, content) VALUES (new.id, new.content);
        END;
        "#
    )?;
    
    Ok(())
}