use asterism_core::OutboxPayload;
use asterism_core::content::{ContentFlags, ContentItem};
use asterism_core::id::ContentId;
use rusqlite::{Connection, OptionalExtension, params};

use crate::error::{Result, StorageError};

pub const EVENT_COMMITTED: &str = "content.committed.v1";
pub const EVENT_DELETED: &str = "content.deleted.v1";
pub const CONSUMER_LAN: &str = "asterism.sync.lan";
pub const CONSUMER_HUB: &str = "asterism.sync.hub";
pub const CONSUMER_HUB_DELETE: &str = "asterism.sync.hub_delete";

#[derive(Clone, Debug)]
pub struct OutboxEvent {
    pub id: i64,
    pub event_id: [u8; 16],
    pub event_type: String,
    pub aggregate_id: ContentId,
    pub payload_json: String,
    pub created_at_ms: i64,
    pub attempts: i64,
}

pub fn ensure_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS domain_outbox (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            event_id BLOB NOT NULL UNIQUE,
            event_type TEXT NOT NULL,
            aggregate_id BLOB NOT NULL,
            payload_json TEXT NOT NULL,
            created_at_ms INTEGER NOT NULL,
            available_at_ms INTEGER NOT NULL,
            attempts INTEGER NOT NULL DEFAULT 0,
            acked_at_ms INTEGER
        );
        CREATE INDEX IF NOT EXISTS idx_outbox_pending
            ON domain_outbox(available_at_ms, id)
            WHERE acked_at_ms IS NULL;
        CREATE TABLE IF NOT EXISTS domain_outbox_delivery (
            event_id BLOB NOT NULL,
            consumer_id TEXT NOT NULL,
            acked_at_ms INTEGER,
            available_at_ms INTEGER NOT NULL,
            attempts INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (event_id, consumer_id)
        );
        CREATE INDEX IF NOT EXISTS idx_outbox_delivery_pending
            ON domain_outbox_delivery(consumer_id, available_at_ms)
            WHERE acked_at_ms IS NULL;
        "#,
    )?;
    Ok(())
}

pub fn enqueue_committed(conn: &Connection, item: &ContentItem, now_ms: i64) -> Result<()> {
    let payload = OutboxPayload {
        aggregate_id: item.id().to_string(),
        origin_device_id: item.origin_device_id().to_string(),
        kind: item.kind().as_str().to_string(),
        from_remote: item.flags().contains(ContentFlags::FROM_REMOTE),
    };
    enqueue(conn, EVENT_COMMITTED, item.id(), &payload, now_ms)
}

pub fn enqueue_deleted(conn: &Connection, id: ContentId, now_ms: i64) -> Result<()> {
    let payload = OutboxPayload {
        aggregate_id: id.to_string(),
        origin_device_id: String::new(),
        kind: String::new(),
        from_remote: false,
    };
    enqueue(conn, EVENT_DELETED, id, &payload, now_ms)
}

fn enqueue(
    conn: &Connection,
    event_type: &str,
    aggregate_id: ContentId,
    payload: &OutboxPayload,
    now_ms: i64,
) -> Result<()> {
    let event_id = ContentId::new();
    conn.execute(
        r#"
        INSERT INTO domain_outbox (
            event_id, event_type, aggregate_id, payload_json, created_at_ms, available_at_ms, attempts
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?5, 0)
        "#,
        params![
            event_id.as_bytes().as_slice(),
            event_type,
            aggregate_id.as_bytes().as_slice(),
            serde_json::to_string(payload)?,
            now_ms,
        ],
    )?;
    Ok(())
}

pub fn pending(conn: &Connection, limit: u32, now_ms: i64) -> Result<Vec<OutboxEvent>> {
    pending_for(conn, None, None, limit, now_ms)
}

pub fn pending_for(
    conn: &Connection,
    event_type: Option<&str>,
    consumer_id: Option<&str>,
    limit: u32,
    now_ms: i64,
) -> Result<Vec<OutboxEvent>> {
    let limit = if limit == 0 { 50 } else { limit.min(200) } as i64;
    let sql = match (event_type.is_some(), consumer_id.is_some()) {
        (true, true) => {
            r#"
            SELECT o.id, o.event_id, o.event_type, o.aggregate_id, o.payload_json, o.created_at_ms,
                   COALESCE(d.attempts, 0)
            FROM domain_outbox o
            LEFT JOIN domain_outbox_delivery d
              ON d.event_id = o.event_id AND d.consumer_id = ?3
            WHERE o.event_type = ?2
              AND (d.acked_at_ms IS NULL)
              AND COALESCE(d.available_at_ms, o.available_at_ms) <= ?1
            ORDER BY o.id ASC
            LIMIT ?4
            "#
        }
        (true, false) => {
            r#"
            SELECT o.id, o.event_id, o.event_type, o.aggregate_id, o.payload_json, o.created_at_ms, o.attempts
            FROM domain_outbox o
            WHERE o.event_type = ?2 AND o.available_at_ms <= ?1
            ORDER BY o.id ASC
            LIMIT ?4
            "#
        }
        _ => {
            r#"
            SELECT id, event_id, event_type, aggregate_id, payload_json, created_at_ms, attempts
            FROM domain_outbox
            WHERE available_at_ms <= ?1
            ORDER BY id ASC
            LIMIT ?4
            "#
        }
    };
    let mut stmt = conn.prepare(sql)?;
    let type_or_empty = event_type.unwrap_or("");
    let consumer_or_empty = consumer_id.unwrap_or("");
    let rows = stmt.query_map(params![now_ms, type_or_empty, consumer_or_empty, limit], |row| {
        let event_raw: Vec<u8> = row.get(1)?;
        let agg_raw: Vec<u8> = row.get(3)?;
        Ok(OutboxEvent {
            id: row.get(0)?,
            event_id: blob16(&event_raw)?,
            event_type: row.get(2)?,
            aggregate_id: ContentId::from_bytes(blob16(&agg_raw)?),
            payload_json: row.get(4)?,
            created_at_ms: row.get(5)?,
            attempts: row.get(6)?,
        })
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>().map_err(StorageError::from)
}

pub fn ack(conn: &Connection, id: i64, now_ms: i64) -> Result<bool> {
    ack_consumer(conn, id, CONSUMER_HUB, now_ms)
}

pub fn ack_consumer(conn: &Connection, id: i64, consumer_id: &str, now_ms: i64) -> Result<bool> {
    let Some(event_id) = event_id_of(conn, id)? else {
        return Ok(false);
    };
    let n = conn.execute(
        r#"
        INSERT INTO domain_outbox_delivery (event_id, consumer_id, acked_at_ms, available_at_ms, attempts)
        VALUES (?1, ?2, ?3, ?3, 0)
        ON CONFLICT(event_id, consumer_id) DO UPDATE SET
            acked_at_ms = excluded.acked_at_ms
        WHERE domain_outbox_delivery.acked_at_ms IS NULL
        "#,
        params![event_id.as_slice(), consumer_id, now_ms],
    )?;
    Ok(n > 0)
}

pub fn retry(conn: &Connection, id: i64, available_at_ms: i64) -> Result<()> {
    retry_consumer(conn, id, CONSUMER_HUB, available_at_ms)
}

pub fn retry_consumer(
    conn: &Connection,
    id: i64,
    consumer_id: &str,
    available_at_ms: i64,
) -> Result<()> {
    let Some(event_id) = event_id_of(conn, id)? else {
        return Ok(());
    };
    conn.execute(
        r#"
        INSERT INTO domain_outbox_delivery (event_id, consumer_id, acked_at_ms, available_at_ms, attempts)
        VALUES (?1, ?2, NULL, ?3, 1)
        ON CONFLICT(event_id, consumer_id) DO UPDATE SET
            attempts = domain_outbox_delivery.attempts + 1,
            available_at_ms = excluded.available_at_ms
        WHERE domain_outbox_delivery.acked_at_ms IS NULL
        "#,
        params![event_id.as_slice(), consumer_id, available_at_ms],
    )?;
    Ok(())
}

fn event_id_of(conn: &Connection, id: i64) -> Result<Option<[u8; 16]>> {
    conn.query_row("SELECT event_id FROM domain_outbox WHERE id = ?1", params![id], |row| {
        let raw: Vec<u8> = row.get(0)?;
        blob16(&raw)
    })
    .optional()
    .map_err(StorageError::from)
}

fn blob16(raw: &[u8]) -> rusqlite::Result<[u8; 16]> {
    if raw.len() != 16 {
        return Err(rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Blob,
            Box::new(asterism_core::CoreError::InvalidUuid),
        ));
    }
    let mut arr = [0u8; 16];
    arr.copy_from_slice(raw);
    Ok(arr)
}

impl OutboxEvent {
    pub fn payload(&self) -> Result<OutboxPayload> {
        serde_json::from_str(&self.payload_json).map_err(StorageError::from)
    }
}

pub fn latest_unacked(
    conn: &Connection,
    event_type: &str,
    aggregate_id: ContentId,
) -> Result<Option<i64>> {
    conn.query_row(
        r#"
        SELECT id FROM domain_outbox
        WHERE event_type = ?1 AND aggregate_id = ?2
        ORDER BY id DESC LIMIT 1
        "#,
        params![event_type, aggregate_id.as_bytes().as_slice()],
        |row| row.get(0),
    )
    .optional()
    .map_err(StorageError::from)
}
