use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension};

pub fn migrate(db_path: &Path) -> Result<()> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(db_path)?;
    conn.execute_batch(
        r#"
        PRAGMA journal_mode=WAL;
        PRAGMA synchronous=NORMAL;
        PRAGMA busy_timeout=5000;
        PRAGMA foreign_keys=ON;
        "#,
    )?;
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS accounts (
            id BLOB PRIMARY KEY NOT NULL,
            created_at_ms INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS devices (
            id BLOB PRIMARY KEY NOT NULL,
            account_id BLOB NOT NULL,
            name TEXT NOT NULL,
            platform TEXT NOT NULL,
            identity_public_key BLOB NOT NULL,
            capabilities INTEGER NOT NULL,
            created_at_ms INTEGER NOT NULL,
            last_seen_at_ms INTEGER NOT NULL,
            revoked_at_ms INTEGER,
            FOREIGN KEY(account_id) REFERENCES accounts(id)
        );

        CREATE TABLE IF NOT EXISTS sessions (
            token_hash BLOB PRIMARY KEY NOT NULL,
            device_id BLOB NOT NULL,
            created_at_ms INTEGER NOT NULL,
            expires_at_ms INTEGER NOT NULL,
            revoked_at_ms INTEGER,
            FOREIGN KEY(device_id) REFERENCES devices(id)
        );

        CREATE TABLE IF NOT EXISTS pairings (
            code_hash BLOB PRIMARY KEY NOT NULL,
            account_id BLOB NOT NULL,
            expires_at_ms INTEGER NOT NULL,
            consumed_at_ms INTEGER
        );

        CREATE TABLE IF NOT EXISTS content_refs (
            id BLOB PRIMARY KEY NOT NULL,
            account_id BLOB NOT NULL,
            origin_device_id BLOB NOT NULL,
            kind TEXT NOT NULL,
            created_at_ms INTEGER NOT NULL,
            logical_size INTEGER NOT NULL,
            payload_size INTEGER NOT NULL,
            dedup_tag BLOB NOT NULL,
            flags INTEGER NOT NULL,
            encrypted_metadata BLOB NOT NULL,
            blob_id TEXT
        );

        CREATE TABLE IF NOT EXISTS blob_refs (
            blob_id TEXT PRIMARY KEY NOT NULL,
            ref_count INTEGER NOT NULL,
            created_at_ms INTEGER NOT NULL,
            last_released_at_ms INTEGER
        );

        CREATE INDEX IF NOT EXISTS idx_content_account_created
            ON content_refs(account_id, created_at_ms DESC);
        "#,
    )?;
    let _ = conn.execute("ALTER TABLE pairings ADD COLUMN avk_wrap TEXT", []);
    let _ = conn.execute("ALTER TABLE pairings ADD COLUMN kdf_salt BLOB", []);
    let _ =
        conn.execute("ALTER TABLE pairings ADD COLUMN fail_count INTEGER NOT NULL DEFAULT 0", []);
    let _ = conn.execute("ALTER TABLE devices ADD COLUMN cert_fingerprint BLOB", []);
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS schema_meta (
            version INTEGER PRIMARY KEY
        );
        INSERT OR IGNORE INTO schema_meta(version) VALUES (2);
        "#,
    )?;
    let stored: i64 =
        conn.query_row("SELECT MAX(version) FROM schema_meta", [], |r| r.get(0)).unwrap_or(2);
    const SCHEMA_VERSION: i64 = 2;
    if stored > SCHEMA_VERSION {
        anyhow::bail!("hub schema {stored} is newer than supported {SCHEMA_VERSION}");
    }
    let _ = schema_ready(&conn)?;
    Ok(())
}

fn schema_ready(conn: &Connection) -> Result<bool> {
    let name: Option<String> = conn
        .query_row(
            "SELECT name FROM sqlite_master WHERE type='table' AND name='devices'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    Ok(name.is_some())
}

pub fn backup(src: &Path, dest: &Path) -> Result<()> {
    let src_conn = Connection::open(src).with_context(|| format!("open {}", src.display()))?;
    let mut dest_conn = Connection::open(dest)?;
    let backup = rusqlite::backup::Backup::new(&src_conn, &mut dest_conn)?;
    backup.run_to_completion(64, std::time::Duration::from_millis(16), None)?;
    Ok(())
}

pub fn referenced_blob_ids(snapshot: &Path) -> Result<Vec<String>> {
    let conn = Connection::open(snapshot)?;
    let mut stmt = conn.prepare(
        "SELECT DISTINCT blob_id FROM content_refs WHERE blob_id IS NOT NULL ORDER BY blob_id",
    )?;
    let ids = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(ids)
}
