use std::sync::Arc;

use asterism_core::id::DeviceId;
use asterism_sync::hub_client::DeviceDto;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};

use crate::auth::auth_token;
use crate::state::HubState;

pub async fn list(
    State(state): State<Arc<HubState>>,
    headers: HeaderMap,
) -> Result<Json<Vec<DeviceDto>>, StatusCode> {
    let auth = bearer(&headers);
    let (account, _) = auth_token(&state, auth, None).ok_or(StatusCode::UNAUTHORIZED)?;
    let db = state.db.lock();
    let mut stmt = db
        .prepare(
            "SELECT id, name, platform, last_seen_at_ms, revoked_at_ms, cert_fingerprint FROM devices WHERE account_id = ?1",
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let rows = stmt
        .query_map([account.as_bytes().as_slice()], |row| {
            let raw: Vec<u8> = row.get(0)?;
            let mut id = [0u8; 16];
            if raw.len() == 16 {
                id.copy_from_slice(&raw);
            }
            let fp: Option<Vec<u8>> = row.get(5)?;
            Ok(DeviceDto {
                id: DeviceId::from_bytes(id),
                name: row.get(1)?,
                platform: row.get(2)?,
                last_seen_at_ms: row.get(3)?,
                revoked: row.get::<_, Option<i64>>(4)?.is_some(),
                cert_fingerprint: fp.filter(|b| b.len() == 32).map(hex::encode),
            })
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let list =
        rows.collect::<Result<Vec<_>, _>>().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(list))
}

pub async fn revoke(
    State(state): State<Arc<HubState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<StatusCode, StatusCode> {
    let auth = bearer(&headers);
    let (account, _) = auth_token(&state, auth, None).ok_or(StatusCode::UNAUTHORIZED)?;
    let device: DeviceId = id.parse().map_err(|_| StatusCode::BAD_REQUEST)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let db = state.db.lock();
    db.execute(
        "UPDATE devices SET revoked_at_ms = ?1 WHERE id = ?2 AND account_id = ?3",
        rusqlite::params![now, device.as_bytes().as_slice(), account.as_bytes().as_slice()],
    )
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    db.execute(
        "UPDATE sessions SET revoked_at_ms = ?1 WHERE device_id = ?2",
        rusqlite::params![now, device.as_bytes().as_slice()],
    )
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::NO_CONTENT)
}

pub fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers.get(axum::http::header::AUTHORIZATION).and_then(|v| v.to_str().ok())
}
