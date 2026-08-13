use asterism_core::content::{
    ContentFlags, ContentItem, ContentKind, ContentStatus, FileManifest, ItemMetadata, PayloadRef,
};
use asterism_core::id::{BlobId, ContentId, DeviceId, ManifestId};
use rusqlite::{Connection, OptionalExtension, params};

use crate::error::{Result, StorageError};

#[derive(Clone, Debug, Default)]
pub struct HistoryQuery {
    pub kind: Option<ContentKind>,
    pub favorite_only: bool,
    pub query: Option<String>,
    pub limit: u32,
    pub before_ms: Option<i64>,
}

impl HistoryQuery {
    pub fn recent(limit: u32) -> Self {
        Self { limit, ..Self::default() }
    }
}

pub fn insert_item(
    conn: &Connection,
    item: &ContentItem,
    manifest: Option<&FileManifest>,
) -> Result<()> {
    let (payload_kind, payload_inline, blob_id, manifest_id) = match &item.payload_ref {
        PayloadRef::Inline { bytes } => ("inline", Some(bytes.as_ref()), None, None),
        PayloadRef::Blob { blob_id } => ("blob", None, Some(blob_id.as_str().to_string()), None),
        PayloadRef::FileManifest { manifest_id } => {
            ("file_manifest", None, None, Some(manifest_id.as_bytes()))
        }
    };

    let preview_text = item.metadata.text_preview.clone();
    let file_names = item.metadata.files.as_ref().map(|f| f.root_name.clone());
    let metadata_json = serde_json::to_string(&item.metadata)?;

    conn.execute(
        r#"
        INSERT INTO content_items (
            id, origin_device_id, kind, created_at_ms, logical_size, payload_size,
            dedup_tag, flags, status, metadata_json, payload_kind, payload_inline,
            blob_id, manifest_id, preview_text, file_names
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
        "#,
        params![
            item.id.as_bytes().as_slice(),
            item.origin_device_id.as_bytes().as_slice(),
            item.kind.as_str(),
            item.created_at_ms,
            item.logical_size as i64,
            item.payload_size as i64,
            item.dedup_tag.as_slice(),
            item.flags.bits() as i64,
            item.status.as_str(),
            metadata_json,
            payload_kind,
            payload_inline,
            blob_id,
            manifest_id,
            preview_text,
            file_names,
        ],
    )?;

    if let Some(manifest) = manifest {
        conn.execute(
            "INSERT INTO file_manifests (id, content_id, manifest_json) VALUES (?1, ?2, ?3)",
            params![
                manifest.id.as_bytes().as_slice(),
                item.id.as_bytes().as_slice(),
                serde_json::to_string(manifest)?,
            ],
        )?;
    }

    if let PayloadRef::Blob { blob_id } = &item.payload_ref {
        bump_blob_ref(conn, blob_id, item.created_at_ms)?;
    }
    Ok(())
}

fn bump_blob_ref(conn: &Connection, blob_id: &BlobId, now_ms: i64) -> Result<()> {
    conn.execute(
        r#"
        INSERT INTO blob_refs (blob_id, ref_count, created_at_ms, last_released_at_ms)
        VALUES (?1, 1, ?2, NULL)
        ON CONFLICT(blob_id) DO UPDATE SET ref_count = ref_count + 1
        "#,
        params![blob_id.as_str(), now_ms],
    )?;
    Ok(())
}

pub fn get_item(conn: &Connection, id: ContentId) -> Result<ContentItem> {
    let item = conn
        .query_row(
            r#"
            SELECT id, origin_device_id, kind, created_at_ms, logical_size, payload_size,
                   dedup_tag, flags, status, metadata_json, payload_kind, payload_inline,
                   blob_id, manifest_id
            FROM content_items WHERE id = ?1
            "#,
            params![id.as_bytes().as_slice()],
            map_item,
        )
        .optional()?
        .ok_or(StorageError::NotFound)?;
    Ok(item)
}

pub fn contains_item(conn: &Connection, id: ContentId) -> Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM content_items WHERE id = ?1)",
        params![id.as_bytes().as_slice()],
        |row| row.get(0),
    )
    .map_err(StorageError::from)
}

pub fn list_history(conn: &Connection, query: &HistoryQuery) -> Result<Vec<ContentItem>> {
    let limit = if query.limit == 0 { 50 } else { query.limit.min(200) } as i64;
    let text_q = query.query.as_ref().map(|q| q.trim().to_string()).filter(|q| !q.is_empty());
    // FTS5 trigram 对少于 3 个字符的查询几乎无效，短查询回退 LIKE。
    let use_fts = text_q.as_ref().is_some_and(|q| q.chars().count() >= 3);
    let use_like = text_q.is_some() && !use_fts;

    let mut sql = String::from(
        r#"
        SELECT c.id, c.origin_device_id, c.kind, c.created_at_ms, c.logical_size, c.payload_size,
               c.dedup_tag, c.flags, c.status, c.metadata_json, c.payload_kind, c.payload_inline,
               c.blob_id, c.manifest_id
        FROM content_items c
        "#,
    );
    let mut where_parts = Vec::new();
    if use_fts {
        sql.push_str(" JOIN content_search s ON s.rowid = c.rowid ");
        where_parts.push("content_search MATCH ?");
    }
    if use_like {
        where_parts.push("(c.preview_text LIKE ? ESCAPE '\\' OR c.file_names LIKE ? ESCAPE '\\')");
    }
    if query.kind.is_some() {
        where_parts.push("c.kind = ?");
    }
    if query.favorite_only {
        where_parts.push("(c.flags & ?) != 0");
    }
    if query.before_ms.is_some() {
        where_parts.push("c.created_at_ms < ?");
    }
    if !where_parts.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&where_parts.join(" AND "));
    }
    sql.push_str(" ORDER BY c.created_at_ms DESC LIMIT ?");

    let mut stmt = conn.prepare(&sql)?;
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    if let Some(q) = text_q {
        if use_fts {
            params.push(Box::new(escape_fts(&q)));
        } else {
            let like = format!("%{}%", escape_like(&q));
            params.push(Box::new(like.clone()));
            params.push(Box::new(like));
        }
    }
    if let Some(kind) = query.kind {
        params.push(Box::new(kind.as_str().to_string()));
    }
    if query.favorite_only {
        params.push(Box::new(ContentFlags::FAVORITE.bits() as i64));
    }
    if let Some(before) = query.before_ms {
        params.push(Box::new(before));
    }
    params.push(Box::new(limit));

    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let rows = stmt.query_map(param_refs.as_slice(), map_item)?;
    rows.collect::<std::result::Result<Vec<_>, _>>().map_err(StorageError::from)
}

pub fn set_favorite(conn: &Connection, id: ContentId, favorite: bool) -> Result<()> {
    let item = get_item(conn, id)?;
    let mut flags = item.flags;
    flags.set(ContentFlags::FAVORITE, favorite);
    let n = conn.execute(
        "UPDATE content_items SET flags = ?1 WHERE id = ?2",
        params![flags.bits() as i64, id.as_bytes().as_slice()],
    )?;
    if n == 0 {
        return Err(StorageError::NotFound);
    }
    Ok(())
}

pub fn set_status(conn: &Connection, id: ContentId, status: ContentStatus) -> Result<()> {
    let changed = conn.execute(
        "UPDATE content_items SET status = ?1 WHERE id = ?2",
        params![status.as_str(), id.as_bytes().as_slice()],
    )?;
    if changed == 0 {
        return Err(StorageError::NotFound);
    }
    Ok(())
}

pub fn list_pending_sync(conn: &Connection, limit: u32) -> Result<Vec<ContentItem>> {
    let limit = if limit == 0 { 50 } else { limit.min(200) } as i64;
    let excluded = (ContentFlags::SENSITIVE | ContentFlags::LOCAL_ONLY | ContentFlags::TRANSIENT)
        .bits() as i64;
    let mut stmt = conn.prepare(
        r#"
        SELECT id, origin_device_id, kind, created_at_ms, logical_size, payload_size,
               dedup_tag, flags, status, metadata_json, payload_kind, payload_inline,
               blob_id, manifest_id
        FROM content_items
        WHERE status IN ('LOCAL', 'UPLOADING', 'FAILED')
          AND (flags & ?1) != 0
          AND (flags & ?2) = 0
        ORDER BY created_at_ms ASC, id ASC
        LIMIT ?3
        "#,
    )?;
    let rows = stmt.query_map(
        params![ContentFlags::REMOTE_ALLOWED.bits() as i64, excluded, limit],
        map_item,
    )?;
    rows.collect::<std::result::Result<Vec<_>, _>>().map_err(StorageError::from)
}

pub fn delete_item(conn: &Connection, id: ContentId) -> Result<Option<BlobId>> {
    let item = get_item(conn, id)?;
    conn.execute("DELETE FROM content_items WHERE id = ?1", params![id.as_bytes().as_slice()])?;
    if let PayloadRef::Blob { blob_id } = item.payload_ref {
        conn.execute(
            r#"
            UPDATE blob_refs
            SET ref_count = MAX(ref_count - 1, 0), last_released_at_ms = ?2
            WHERE blob_id = ?1
            "#,
            params![blob_id.as_str(), now_ms()],
        )?;
        return Ok(Some(blob_id));
    }
    Ok(None)
}

pub fn load_manifest(conn: &Connection, id: ManifestId) -> Result<FileManifest> {
    let json: String = conn
        .query_row(
            "SELECT manifest_json FROM file_manifests WHERE id = ?1",
            params![id.as_bytes().as_slice()],
            |row| row.get(0),
        )
        .optional()?
        .ok_or(StorageError::NotFound)?;
    Ok(serde_json::from_str(&json)?)
}

fn map_item(row: &rusqlite::Row<'_>) -> rusqlite::Result<ContentItem> {
    let id = blob16(row, 0)?;
    let device = blob16(row, 1)?;
    let kind: String = row.get(2)?;
    let created_at_ms: i64 = row.get(3)?;
    let logical_size: i64 = row.get(4)?;
    let payload_size: i64 = row.get(5)?;
    let dedup_raw: Vec<u8> = row.get(6)?;
    let flags: i64 = row.get(7)?;
    let status: String = row.get(8)?;
    let metadata_json: String = row.get(9)?;
    let payload_kind: String = row.get(10)?;
    let payload_inline: Option<Vec<u8>> = row.get(11)?;
    let blob_id: Option<String> = row.get(12)?;
    let manifest_id: Option<Vec<u8>> = row.get(13)?;

    let mut dedup_tag = [0u8; 32];
    if dedup_raw.len() == 32 {
        dedup_tag.copy_from_slice(&dedup_raw);
    }

    let payload_ref = match payload_kind.as_str() {
        "inline" => {
            PayloadRef::Inline { bytes: bytes::Bytes::from(payload_inline.unwrap_or_default()) }
        }
        "blob" => PayloadRef::Blob {
            blob_id: BlobId::from_hex(blob_id.unwrap_or_default()).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    12,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?,
        },
        _ => {
            let bytes = manifest_id.unwrap_or_default();
            let mut arr = [0u8; 16];
            if bytes.len() == 16 {
                arr.copy_from_slice(&bytes);
            }
            PayloadRef::FileManifest { manifest_id: ManifestId::from_bytes(arr) }
        }
    };

    Ok(ContentItem {
        id: ContentId::from_bytes(id),
        origin_device_id: DeviceId::from_bytes(device),
        kind: ContentKind::parse(&kind).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(e))
        })?,
        created_at_ms,
        logical_size: logical_size as u64,
        payload_size: payload_size as u64,
        dedup_tag,
        flags: ContentFlags::from_bits_truncate(flags as u32),
        status: ContentStatus::parse(&status).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(8, rusqlite::types::Type::Text, Box::new(e))
        })?,
        metadata: serde_json::from_str::<ItemMetadata>(&metadata_json).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(9, rusqlite::types::Type::Text, Box::new(e))
        })?,
        payload_ref,
        encrypted_metadata: bytes::Bytes::new(),
    })
}

fn blob16(row: &rusqlite::Row<'_>, idx: usize) -> rusqlite::Result<[u8; 16]> {
    let raw: Vec<u8> = row.get(idx)?;
    if raw.len() != 16 {
        return Err(rusqlite::Error::FromSqlConversionFailure(
            idx,
            rusqlite::types::Type::Blob,
            Box::new(asterism_core::CoreError::InvalidUuid),
        ));
    }
    let mut arr = [0u8; 16];
    arr.copy_from_slice(&raw);
    Ok(arr)
}

fn escape_fts(raw: &str) -> String {
    let cleaned: String = raw.chars().filter(|c| !matches!(c, '"' | '*' | '(' | ')')).collect();
    format!("\"{}\"", cleaned.replace('"', ""))
}

fn escape_like(raw: &str) -> String {
    raw.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
