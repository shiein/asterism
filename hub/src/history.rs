use std::sync::Arc;

use asterism_core::id::DeviceId;
use asterism_sync::hub_client::HistoryDto;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use rusqlite::OptionalExtension;
use serde::Deserialize;

use crate::auth::auth_token;
use crate::device::bearer;
use crate::state::HubState;

#[derive(Deserialize)]
pub struct HistoryQuery {
    pub cursor: Option<String>,
    pub limit: Option<u32>,
}

struct HistoryCursor {
    created_at_ms: i64,
    id: Vec<u8>,
}

pub async fn list(
    State(state): State<Arc<HubState>>,
    headers: HeaderMap,
    Query(q): Query<HistoryQuery>,
) -> Result<Json<Vec<HistoryDto>>, StatusCode> {
    let (account, _) =
        auth_token(&state, bearer(&headers), None).ok_or(StatusCode::UNAUTHORIZED)?;
    let limit = q.limit.unwrap_or(50).min(200) as i64;
    let db = state.db.lock();
    let mut sql = String::from(
        r#"
        SELECT id, origin_device_id, kind, created_at_ms, logical_size, payload_size,
               dedup_tag, flags, encrypted_metadata, blob_id
        FROM content_refs WHERE account_id = ?1
        "#,
    );
    let cursor = q.cursor.as_deref().map(parse_cursor).transpose()?;
    if cursor.is_some() {
        sql.push_str(" AND (created_at_ms > ?2 OR (created_at_ms = ?2 AND id > ?3))");
    }
    sql.push_str(" ORDER BY created_at_ms ASC, id ASC LIMIT ?");
    let mut stmt = db.prepare(&sql).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let map = |row: &rusqlite::Row<'_>| -> rusqlite::Result<HistoryDto> {
        let id: Vec<u8> = row.get(0)?;
        let origin: Vec<u8> = row.get(1)?;
        let mut oid = [0u8; 16];
        if origin.len() == 16 {
            oid.copy_from_slice(&origin);
        }
        Ok(HistoryDto {
            id: hex::encode(id),
            origin_device_id: DeviceId::from_bytes(oid),
            kind: row.get(2)?,
            created_at_ms: row.get(3)?,
            logical_size: row.get::<_, i64>(4)? as u64,
            payload_size: row.get::<_, i64>(5)? as u64,
            dedup_tag: hex::encode(row.get::<_, Vec<u8>>(6)?),
            flags: row.get::<_, i64>(7)? as u32,
            encrypted_metadata: hex::encode(row.get::<_, Vec<u8>>(8)?),
            blob_id: row.get(9)?,
        })
    };
    let rows = if let Some(cursor) = cursor {
        stmt.query_map(
            rusqlite::params![
                account.as_bytes().as_slice(),
                cursor.created_at_ms,
                cursor.id,
                limit
            ],
            map,
        )
    } else {
        stmt.query_map(rusqlite::params![account.as_bytes().as_slice(), limit], map)
    }
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(rows.collect::<Result<Vec<_>, _>>().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?))
}

fn parse_cursor(raw: &str) -> Result<HistoryCursor, StatusCode> {
    if let Some((created, id)) = raw.split_once(':') {
        let created_at_ms = created.parse().map_err(|_| StatusCode::BAD_REQUEST)?;
        let id = hex::decode(id).map_err(|_| StatusCode::BAD_REQUEST)?;
        if id.len() != 16 {
            return Err(StatusCode::BAD_REQUEST);
        }
        return Ok(HistoryCursor { created_at_ms, id });
    }
    let created_at_ms = raw.parse().map_err(|_| StatusCode::BAD_REQUEST)?;
    Ok(HistoryCursor { created_at_ms, id: Vec::new() })
}

pub async fn create(
    State(state): State<Arc<HubState>>,
    headers: HeaderMap,
    Json(item): Json<HistoryDto>,
) -> Result<StatusCode, StatusCode> {
    let (account, device) =
        auth_token(&state, bearer(&headers), None).ok_or(StatusCode::UNAUTHORIZED)?;
    let id = hex::decode(&item.id).map_err(|_| StatusCode::BAD_REQUEST)?;
    let dedup = hex::decode(&item.dedup_tag).map_err(|_| StatusCode::BAD_REQUEST)?;
    let meta = hex::decode(&item.encrypted_metadata).unwrap_or_default();
    let mut db = state.db.lock();
    let tx = db.transaction().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let old_blob: Option<Option<String>> = tx
        .query_row(
            "SELECT blob_id FROM content_refs WHERE id = ?1 AND account_id = ?2",
            rusqlite::params![id, account.as_bytes().as_slice()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let changed = tx
        .execute(
            r#"
        INSERT INTO content_refs (
            id, account_id, origin_device_id, kind, created_at_ms, logical_size, payload_size,
            dedup_tag, flags, encrypted_metadata, blob_id
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
        ON CONFLICT(id) DO UPDATE SET
            origin_device_id = excluded.origin_device_id,
            kind = excluded.kind,
            created_at_ms = excluded.created_at_ms,
            logical_size = excluded.logical_size,
            payload_size = excluded.payload_size,
            dedup_tag = excluded.dedup_tag,
            flags = excluded.flags,
            encrypted_metadata = excluded.encrypted_metadata,
            blob_id = excluded.blob_id
        WHERE content_refs.account_id = excluded.account_id
        "#,
            rusqlite::params![
                id,
                account.as_bytes().as_slice(),
                device.as_bytes().as_slice(),
                item.kind,
                item.created_at_ms,
                item.logical_size as i64,
                item.payload_size as i64,
                dedup,
                item.flags as i64,
                meta,
                item.blob_id,
            ],
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if changed != 1 {
        return Err(StatusCode::CONFLICT);
    }
    reconcile_blob_ref(
        &tx,
        old_blob.flatten().as_deref(),
        item.blob_id.as_deref(),
        item.created_at_ms,
    )
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    tx.commit().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::CREATED)
}

pub async fn delete(
    State(state): State<Arc<HubState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<StatusCode, StatusCode> {
    let (account, _) =
        auth_token(&state, bearer(&headers), None).ok_or(StatusCode::UNAUTHORIZED)?;
    let raw = hex::decode(&id).map_err(|_| StatusCode::BAD_REQUEST)?;
    let mut db = state.db.lock();
    let tx = db.transaction().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let blob: Option<Option<String>> = tx
        .query_row(
            "SELECT blob_id FROM content_refs WHERE id = ?1 AND account_id = ?2",
            rusqlite::params![raw, account.as_bytes().as_slice()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    tx.execute(
        "DELETE FROM content_refs WHERE id = ?1 AND account_id = ?2",
        rusqlite::params![raw, account.as_bytes().as_slice()],
    )
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    reconcile_blob_ref(&tx, blob.flatten().as_deref(), None, chrono::Utc::now().timestamp_millis())
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    tx.commit().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::NO_CONTENT)
}

fn reconcile_blob_ref(
    conn: &rusqlite::Connection,
    old_blob: Option<&str>,
    new_blob: Option<&str>,
    now_ms: i64,
) -> rusqlite::Result<()> {
    if old_blob == new_blob {
        return Ok(());
    }
    if let Some(blob) = old_blob {
        conn.execute(
            r#"
            UPDATE blob_refs
            SET ref_count = MAX(ref_count - 1, 0),
                last_released_at_ms = CASE WHEN ref_count <= 1 THEN ?2 ELSE last_released_at_ms END
            WHERE blob_id = ?1
            "#,
            rusqlite::params![blob, now_ms],
        )?;
    }
    if let Some(blob) = new_blob {
        conn.execute(
            r#"
            INSERT INTO blob_refs (blob_id, ref_count, created_at_ms, last_released_at_ms)
            VALUES (?1, 1, ?2, NULL)
            ON CONFLICT(blob_id) DO UPDATE SET
                ref_count = ref_count + 1,
                last_released_at_ms = NULL
            "#,
            rusqlite::params![blob, now_ms],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_keeps_timestamp_and_id_tie_breaker() {
        let id = "00112233445566778899aabbccddeeff";
        let cursor = parse_cursor(&format!("123:{id}")).unwrap();
        assert_eq!(cursor.created_at_ms, 123);
        assert_eq!(cursor.id, hex::decode(id).unwrap());
    }

    #[test]
    fn legacy_timestamp_cursor_is_still_accepted() {
        let cursor = parse_cursor("123").unwrap();
        assert_eq!(cursor.created_at_ms, 123);
        assert!(cursor.id.is_empty());
    }

    #[test]
    fn blob_reference_reconcile_is_idempotent_and_releases_old_blob() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE blob_refs (blob_id TEXT PRIMARY KEY, ref_count INTEGER NOT NULL, created_at_ms INTEGER NOT NULL, last_released_at_ms INTEGER);",
        )
        .unwrap();

        reconcile_blob_ref(&conn, None, Some("a"), 10).unwrap();
        reconcile_blob_ref(&conn, Some("a"), Some("a"), 20).unwrap();
        assert_eq!(blob_ref(&conn, "a"), (1, None));

        reconcile_blob_ref(&conn, Some("a"), Some("b"), 30).unwrap();
        assert_eq!(blob_ref(&conn, "a"), (0, Some(30)));
        assert_eq!(blob_ref(&conn, "b"), (1, None));

        reconcile_blob_ref(&conn, Some("b"), None, 40).unwrap();
        assert_eq!(blob_ref(&conn, "b"), (0, Some(40)));
    }

    fn blob_ref(conn: &rusqlite::Connection, id: &str) -> (i64, Option<i64>) {
        conn.query_row(
            "SELECT ref_count, last_released_at_ms FROM blob_refs WHERE blob_id = ?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap()
    }
}
