use rusqlite::{Connection, OptionalExtension};

use crate::error::Result;

const SCHEMA_VERSION: i64 = 1;

pub fn initialize(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        PRAGMA journal_mode=WAL;
        PRAGMA synchronous=NORMAL;
        PRAGMA busy_timeout=5000;
        PRAGMA foreign_keys=ON;
        PRAGMA temp_store=MEMORY;
        "#,
    )?;

    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS meta (
            key TEXT PRIMARY KEY NOT NULL,
            value TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS content_items (
            id BLOB PRIMARY KEY NOT NULL,
            origin_device_id BLOB NOT NULL,
            kind TEXT NOT NULL,
            created_at_ms INTEGER NOT NULL,
            logical_size INTEGER NOT NULL,
            payload_size INTEGER NOT NULL,
            dedup_tag BLOB NOT NULL,
            flags INTEGER NOT NULL,
            status TEXT NOT NULL,
            metadata_json TEXT NOT NULL,
            payload_kind TEXT NOT NULL,
            payload_inline BLOB,
            blob_id TEXT,
            manifest_id BLOB,
            preview_text TEXT,
            file_names TEXT
        );

        CREATE INDEX IF NOT EXISTS idx_content_created ON content_items(created_at_ms DESC);
        CREATE INDEX IF NOT EXISTS idx_content_kind ON content_items(kind);
        CREATE INDEX IF NOT EXISTS idx_content_device ON content_items(origin_device_id);
        CREATE INDEX IF NOT EXISTS idx_content_flags ON content_items(flags);
        CREATE INDEX IF NOT EXISTS idx_content_dedup ON content_items(dedup_tag);
        CREATE INDEX IF NOT EXISTS idx_content_status ON content_items(status);

        CREATE TABLE IF NOT EXISTS file_manifests (
            id BLOB PRIMARY KEY NOT NULL,
            content_id BLOB NOT NULL,
            manifest_json TEXT NOT NULL,
            FOREIGN KEY(content_id) REFERENCES content_items(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS blob_refs (
            blob_id TEXT PRIMARY KEY NOT NULL,
            ref_count INTEGER NOT NULL,
            created_at_ms INTEGER NOT NULL,
            last_released_at_ms INTEGER
        );

        CREATE TABLE IF NOT EXISTS sync_cursors (
            scope TEXT PRIMARY KEY NOT NULL,
            cursor TEXT NOT NULL,
            updated_at_ms INTEGER NOT NULL
        );
        "#,
    )?;

    ensure_fts(conn)?;
    set_schema_version(conn, SCHEMA_VERSION)?;
    Ok(())
}

fn ensure_fts(conn: &Connection) -> Result<()> {
    let exists: Option<String> = conn
        .query_row(
            "SELECT name FROM sqlite_master WHERE type='table' AND name='content_search'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if exists.is_some() {
        return Ok(());
    }

    // trigram 优先；旧 SQLite 回退 unicode61。回退可观测：写入 meta。
    let trigram = conn.execute_batch(
        r#"
        CREATE VIRTUAL TABLE content_search USING fts5(
            preview_text,
            file_names,
            content='content_items',
            content_rowid='rowid',
            tokenize='trigram'
        );
        "#,
    );
    match trigram {
        Ok(()) => {
            conn.execute(
                "INSERT OR REPLACE INTO meta(key, value) VALUES ('fts_tokenizer', 'trigram')",
                [],
            )?;
        }
        Err(err) => {
            tracing::warn!(error = %err, "FTS5 trigram unavailable, falling back to unicode61");
            conn.execute_batch(
                r#"
                CREATE VIRTUAL TABLE content_search USING fts5(
                    preview_text,
                    file_names,
                    content='content_items',
                    content_rowid='rowid',
                    tokenize='unicode61'
                );
                "#,
            )?;
            conn.execute(
                "INSERT OR REPLACE INTO meta(key, value) VALUES ('fts_tokenizer', 'unicode61')",
                [],
            )?;
        }
    }

    conn.execute_batch(
        r#"
        CREATE TRIGGER IF NOT EXISTS content_items_ai AFTER INSERT ON content_items BEGIN
            INSERT INTO content_search(rowid, preview_text, file_names)
            VALUES (new.rowid, new.preview_text, new.file_names);
        END;
        CREATE TRIGGER IF NOT EXISTS content_items_ad AFTER DELETE ON content_items BEGIN
            INSERT INTO content_search(content_search, rowid, preview_text, file_names)
            VALUES ('delete', old.rowid, old.preview_text, old.file_names);
        END;
        CREATE TRIGGER IF NOT EXISTS content_items_au AFTER UPDATE ON content_items BEGIN
            INSERT INTO content_search(content_search, rowid, preview_text, file_names)
            VALUES ('delete', old.rowid, old.preview_text, old.file_names);
            INSERT INTO content_search(rowid, preview_text, file_names)
            VALUES (new.rowid, new.preview_text, new.file_names);
        END;
        "#,
    )?;
    Ok(())
}

fn set_schema_version(conn: &Connection, version: i64) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO meta(key, value) VALUES ('schema_version', ?1)",
        [version.to_string()],
    )?;
    Ok(())
}

pub fn schema_version(conn: &Connection) -> Result<i64> {
    let value: Option<String> = conn
        .query_row("SELECT value FROM meta WHERE key = 'schema_version'", [], |row| row.get(0))
        .optional()?;
    Ok(value.and_then(|v| v.parse().ok()).unwrap_or(0))
}
