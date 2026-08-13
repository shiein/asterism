use std::sync::Arc;
use std::time::{Duration, SystemTime};

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::auth_token;
use crate::device::bearer;
use crate::state::HubState;

const MAX_CHUNK: usize = 2 * 1024 * 1024;
const MAX_CHUNKS: u32 = 50_000;

#[derive(Serialize)]
pub struct BeginRes {
    pub blob_id: String,
}

#[derive(Deserialize)]
pub struct CommitReq {
    pub chunks: u32,
}

pub async fn begin(
    State(state): State<Arc<HubState>>,
    headers: HeaderMap,
) -> Result<Json<BeginRes>, StatusCode> {
    let _ = auth_token(&state, bearer(&headers), None).ok_or(StatusCode::UNAUTHORIZED)?;
    let id = Uuid::now_v7().to_string();
    let dir = state.config.blob_root().join(&id);
    std::fs::create_dir_all(dir).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(BeginRes { blob_id: id }))
}

pub async fn put_chunk(
    State(state): State<Arc<HubState>>,
    headers: HeaderMap,
    Path((id, index)): Path<(String, u32)>,
    body: Bytes,
) -> Result<StatusCode, StatusCode> {
    let _ = auth_token(&state, bearer(&headers), None).ok_or(StatusCode::UNAUTHORIZED)?;
    if index > MAX_CHUNKS || body.len() > MAX_CHUNK {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }
    if id.contains("..") || id.contains('/') || id.contains('\\') {
        return Err(StatusCode::BAD_REQUEST);
    }
    let dir = state.config.blob_root().join(&id);
    if !dir.exists() {
        return Err(StatusCode::NOT_FOUND);
    }
    let path = dir.join(format!("chunk_{index}"));
    std::fs::write(path, &body).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn get_chunk(
    State(state): State<Arc<HubState>>,
    headers: HeaderMap,
    Path((id, index)): Path<(String, u32)>,
) -> Result<Vec<u8>, StatusCode> {
    let _ = auth_token(&state, bearer(&headers), None).ok_or(StatusCode::UNAUTHORIZED)?;
    if id.contains("..") || id.contains('/') {
        return Err(StatusCode::BAD_REQUEST);
    }
    let path = state.config.blob_root().join(&id).join(format!("chunk_{index}"));
    std::fs::read(path).map_err(|_| StatusCode::NOT_FOUND)
}

pub async fn commit(
    State(state): State<Arc<HubState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<CommitReq>,
) -> Result<StatusCode, StatusCode> {
    let _ = auth_token(&state, bearer(&headers), None).ok_or(StatusCode::UNAUTHORIZED)?;
    if req.chunks > MAX_CHUNKS {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }
    if id.contains("..") || id.contains('/') {
        return Err(StatusCode::BAD_REQUEST);
    }
    let dir = state.config.blob_root().join(&id);
    if !dir.exists() {
        return Err(StatusCode::NOT_FOUND);
    }
    std::fs::write(dir.join("committed"), req.chunks.to_string())
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::NO_CONTENT)
}

pub fn gc_unused(state: &HubState, grace: Duration) -> anyhow::Result<u64> {
    let released_before_ms = chrono::Utc::now()
        .timestamp_millis()
        .saturating_sub(i64::try_from(grace.as_millis()).unwrap_or(i64::MAX));
    let orphan_before = SystemTime::now().checked_sub(grace).unwrap_or(SystemTime::UNIX_EPOCH);
    let db = state.db.lock();
    gc_unused_locked(&db, &state.config.blob_root(), released_before_ms, orphan_before)
}

fn gc_unused_locked(
    db: &rusqlite::Connection,
    root: &std::path::Path,
    released_before_ms: i64,
    orphan_before: SystemTime,
) -> anyhow::Result<u64> {
    let mut stmt = db.prepare(
        "SELECT blob_id FROM blob_refs WHERE ref_count = 0 AND last_released_at_ms <= ?1",
    )?;
    let ids = stmt
        .query_map([released_before_ms], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    drop(stmt);
    let mut removed = 0u64;
    for id in ids {
        let path = root.join(&id);
        if path.exists() {
            std::fs::remove_dir_all(&path)?;
        }
        db.execute(
            "DELETE FROM blob_refs WHERE blob_id = ?1 AND ref_count = 0 AND last_released_at_ms <= ?2",
            rusqlite::params![id, released_before_ms],
        )?;
        removed += 1;
    }

    if root.exists() {
        for entry in std::fs::read_dir(root)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir()
                || entry.metadata()?.modified().unwrap_or(SystemTime::now()) > orphan_before
            {
                continue;
            }
            let id = entry.file_name().to_string_lossy().into_owned();
            let referenced: bool = db.query_row(
                "SELECT EXISTS(SELECT 1 FROM blob_refs WHERE blob_id = ?1)",
                [&id],
                |row| row.get(0),
            )?;
            if !referenced {
                std::fs::remove_dir_all(entry.path())?;
                removed += 1;
            }
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gc_removes_released_and_orphan_blob_directories() {
        let root = tempfile::tempdir().unwrap();
        let released = root.path().join("released");
        let orphan = root.path().join("orphan");
        std::fs::create_dir(&released).unwrap();
        std::fs::create_dir(&orphan).unwrap();
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE blob_refs (blob_id TEXT PRIMARY KEY, ref_count INTEGER NOT NULL, created_at_ms INTEGER NOT NULL, last_released_at_ms INTEGER);",
        )
        .unwrap();
        db.execute("INSERT INTO blob_refs VALUES ('released', 0, 1, 10)", []).unwrap();

        let removed = gc_unused_locked(&db, root.path(), 10, SystemTime::now()).unwrap();

        assert_eq!(removed, 2);
        assert!(!released.exists());
        assert!(!orphan.exists());
    }
}
