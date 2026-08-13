use std::sync::Arc;

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
