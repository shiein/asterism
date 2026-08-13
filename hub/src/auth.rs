use std::sync::Arc;

use asterism_core::id::{AccountId, DeviceId};
use asterism_sync::hub_client::TrustedDeviceDto;
use asterism_sync::pairing::{
    PairingFinish, PairingOffer, generate_code, generate_kdf_salt, generate_session_token,
    hash_bootstrap, hash_code, hash_token,
};
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use crate::state::HubState;

const PAIRING_TTL_MS: i64 = 10 * 60 * 1000;
const SESSION_TTL_MS: i64 = 30 * 24 * 60 * 60 * 1000;

#[derive(Deserialize)]
pub struct SessionReq {
    pub token: Option<String>,
}

#[derive(Serialize)]
pub struct SessionRes {
    pub token: String,
    pub account_id: AccountId,
    pub device_id: DeviceId,
    pub avk_wrapped_hex: Option<String>,
    #[serde(default)]
    pub kdf_salt_hex: Option<String>,
    #[serde(default)]
    pub trusted_devices: Vec<TrustedDeviceDto>,
}

const MAX_CODE_FAILURES: i64 = 5;
const GLOBAL_FAIL_WINDOW_MS: i64 = 10 * 60 * 1000;
const GLOBAL_FAIL_LIMIT: usize = 20;

pub async fn pairing_start(
    State(state): State<Arc<HubState>>,
    headers: HeaderMap,
) -> Result<Json<PairingOffer>, StatusCode> {
    let token_ok = auth_token(&state, crate::device::bearer(&headers), None).is_some();
    let bootstrap_ok = bootstrap_ok(&state, &headers);
    if !token_ok && !bootstrap_ok {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let now = now_ms();
    let db = state.db.lock();
    if bootstrap_ok && !token_ok && has_active_devices(&db) {
        return Err(StatusCode::FORBIDDEN);
    }
    let account = ensure_account(&db, now).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let code = generate_code();
    let hash = hash_code(&code);
    let salt = generate_kdf_salt();
    db.execute(
        "INSERT INTO pairings (code_hash, account_id, expires_at_ms, consumed_at_ms, kdf_salt, fail_count) VALUES (?1, ?2, ?3, NULL, ?4, 0)",
        rusqlite::params![hash.as_slice(), account.as_bytes().as_slice(), now + PAIRING_TTL_MS, salt.as_slice()],
    )
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(PairingOffer {
        code,
        expires_at_ms: now + PAIRING_TTL_MS,
        account_id: account,
        kdf_salt_hex: hex::encode(salt),
    }))
}

pub async fn pairing_finish(
    State(state): State<Arc<HubState>>,
    Json(req): Json<PairingFinish>,
) -> Result<Json<SessionRes>, StatusCode> {
    let now = now_ms();
    if too_many_global_failures(&state, now) {
        return Err(StatusCode::TOO_MANY_REQUESTS);
    }
    let hash = hash_code(&req.code);
    let db = state.db.lock();
    type PairRow = (Vec<u8>, i64, Option<i64>, Option<String>, Option<Vec<u8>>, i64);
    let row: Option<PairRow> = db
        .query_row(
            "SELECT account_id, expires_at_ms, consumed_at_ms, avk_wrap, kdf_salt, fail_count FROM pairings WHERE code_hash = ?1",
            [hash.as_slice()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .optional()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let Some((account_raw, exp, consumed, avk_wrap, kdf_salt, fail_count)) = row else {
        record_global_failure(&state, now);
        return Err(StatusCode::NOT_FOUND);
    };
    if fail_count >= MAX_CODE_FAILURES || consumed.is_some() || exp < now {
        record_global_failure(&state, now);
        return Err(StatusCode::GONE);
    }
    let mut account_bytes = [0u8; 16];
    account_bytes.copy_from_slice(&account_raw);
    let account = AccountId::from_bytes(account_bytes);
    db.execute(
        "UPDATE pairings SET consumed_at_ms = ?1, avk_wrap = NULL WHERE code_hash = ?2",
        rusqlite::params![now, hash.as_slice()],
    )
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let fp = hex::decode(req.cert_fingerprint.trim()).ok().filter(|b| b.len() == 32);
    db.execute(
        r#"
        INSERT INTO devices (id, account_id, name, platform, identity_public_key, capabilities, created_at_ms, last_seen_at_ms, revoked_at_ms, cert_fingerprint)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, NULL, ?8)
        "#,
        rusqlite::params![
            req.device_id.as_bytes().as_slice(),
            account.as_bytes().as_slice(),
            req.device_name,
            req.platform,
            req.identity_public_key,
            asterism_core::DeviceCapabilities::desktop_v1().bits() as i64,
            now,
            fp.as_deref()
        ],
    )
    .map_err(|_| StatusCode::CONFLICT)?;
    let trusted = list_trusted(&db, account).unwrap_or_default();
    let salt_hex = kdf_salt.filter(|s| !s.is_empty()).map(hex::encode);
    issue_session(&db, req.device_id, account, now, avk_wrap, salt_hex, trusted)
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

pub async fn session(
    State(state): State<Arc<HubState>>,
    Json(req): Json<SessionReq>,
) -> Result<Json<SessionRes>, StatusCode> {
    let token = req.token.ok_or(StatusCode::UNAUTHORIZED)?;
    let now = now_ms();
    let hash = hash_token(&token);
    let db = state.db.lock();
    type SessionRow = (Vec<u8>, Vec<u8>, i64, Option<i64>);
    let row: Option<SessionRow> = db
        .query_row(
            r#"
            SELECT s.device_id, d.account_id, s.expires_at_ms, s.revoked_at_ms
            FROM sessions s JOIN devices d ON d.id = s.device_id
            WHERE s.token_hash = ?1
            "#,
            [hash.as_slice()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let Some((dev, acc, exp, revoked)) = row else {
        return Err(StatusCode::UNAUTHORIZED);
    };
    if revoked.is_some() || exp < now {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let device_id = bytes16(&dev).ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    let account_id = bytes16(&acc).ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(SessionRes {
        token,
        account_id: AccountId::from_bytes(account_id),
        device_id: DeviceId::from_bytes(device_id),
        avk_wrapped_hex: None,
        kdf_salt_hex: None,
        trusted_devices: Vec::new(),
    }))
}

#[derive(Deserialize)]
pub struct AvkDeposit {
    pub code: String,
    pub wrapped_hex: String,
}

pub async fn deposit_avk(
    State(state): State<Arc<HubState>>,
    headers: axum::http::HeaderMap,
    Json(req): Json<AvkDeposit>,
) -> Result<StatusCode, StatusCode> {
    let _ = auth_token(&state, crate::device::bearer(&headers), None)
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let hash = hash_code(&req.code);
    let db = state.db.lock();
    db.execute(
        "UPDATE pairings SET avk_wrap = ?1 WHERE code_hash = ?2 AND consumed_at_ms IS NULL",
        rusqlite::params![req.wrapped_hex, hash.as_slice()],
    )
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::NO_CONTENT)
}

pub fn auth_token(
    state: &HubState,
    header: Option<&str>,
    query: Option<&str>,
) -> Option<(AccountId, DeviceId)> {
    let raw = header.and_then(|h| h.strip_prefix("Bearer ")).or(query)?;
    let hash = hash_token(raw);
    let db = state.db.lock();
    let row = db
        .query_row(
            r#"
            SELECT d.account_id, s.device_id, s.expires_at_ms, s.revoked_at_ms, d.revoked_at_ms
            FROM sessions s JOIN devices d ON d.id = s.device_id
            WHERE s.token_hash = ?1
            "#,
            [hash.as_slice()],
            |r| {
                Ok((
                    r.get::<_, Vec<u8>>(0)?,
                    r.get::<_, Vec<u8>>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, Option<i64>>(3)?,
                    r.get::<_, Option<i64>>(4)?,
                ))
            },
        )
        .ok()?;
    if row.2 < now_ms() || row.3.is_some() || row.4.is_some() {
        return None;
    }
    Some((AccountId::from_bytes(bytes16(&row.0)?), DeviceId::from_bytes(bytes16(&row.1)?)))
}

fn issue_session(
    db: &rusqlite::Connection,
    device: DeviceId,
    account: AccountId,
    now: i64,
    avk_wrapped_hex: Option<String>,
    kdf_salt_hex: Option<String>,
    trusted_devices: Vec<TrustedDeviceDto>,
) -> anyhow::Result<SessionRes> {
    let token = generate_session_token();
    let hash = hash_token(&token);
    db.execute(
        "INSERT INTO sessions (token_hash, device_id, created_at_ms, expires_at_ms, revoked_at_ms) VALUES (?1, ?2, ?3, ?4, NULL)",
        rusqlite::params![hash.as_slice(), device.as_bytes().as_slice(), now, now + SESSION_TTL_MS],
    )?;
    Ok(SessionRes {
        token,
        account_id: account,
        device_id: device,
        avk_wrapped_hex,
        kdf_salt_hex,
        trusted_devices,
    })
}

fn bootstrap_ok(state: &HubState, headers: &HeaderMap) -> bool {
    let Some(expected) = state.config.bootstrap_secret_hash.as_deref() else {
        return false;
    };
    let Some(got) = headers.get("x-asterism-bootstrap").and_then(|v| v.to_str().ok()) else {
        return false;
    };
    let actual = hex::encode(hash_bootstrap(got));
    constant_eq(actual.as_bytes(), expected.as_bytes())
}

fn has_active_devices(db: &rusqlite::Connection) -> bool {
    db.query_row(
        "SELECT COUNT(*) FROM devices WHERE revoked_at_ms IS NULL",
        [],
        |r| r.get::<_, i64>(0),
    )
    .ok()
    .is_some_and(|n| n > 0)
}

fn list_trusted(db: &rusqlite::Connection, account: AccountId) -> rusqlite::Result<Vec<TrustedDeviceDto>> {
    let mut stmt = db.prepare(
        "SELECT id, name, cert_fingerprint FROM devices WHERE account_id = ?1 AND revoked_at_ms IS NULL",
    )?;
    let rows = stmt.query_map([account.as_bytes().as_slice()], |row| {
        let raw: Vec<u8> = row.get(0)?;
        let mut id = [0u8; 16];
        if raw.len() == 16 {
            id.copy_from_slice(&raw);
        }
        let fp: Option<Vec<u8>> = row.get(2)?;
        Ok(TrustedDeviceDto {
            device_id: DeviceId::from_bytes(id),
            name: row.get(1)?,
            cert_fingerprint: fp.filter(|b| b.len() == 32).map(hex::encode),
        })
    })?;
    rows.collect()
}

fn too_many_global_failures(state: &HubState, now: i64) -> bool {
    let mut fails = state.pairing_fails.lock();
    fails.retain(|t| now - *t < GLOBAL_FAIL_WINDOW_MS);
    fails.len() >= GLOBAL_FAIL_LIMIT
}

fn record_global_failure(state: &HubState, now: i64) {
    state.pairing_fails.lock().push(now);
}

fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn ensure_account(db: &rusqlite::Connection, now: i64) -> anyhow::Result<AccountId> {
    if let Some(raw) = db
        .query_row("SELECT id FROM accounts LIMIT 1", [], |r| r.get::<_, Vec<u8>>(0))
        .optional()?
    {
        let mut id = [0u8; 16];
        id.copy_from_slice(&raw);
        return Ok(AccountId::from_bytes(id));
    }
    let id = AccountId::new();
    db.execute(
        "INSERT INTO accounts (id, created_at_ms) VALUES (?1, ?2)",
        rusqlite::params![id.as_bytes().as_slice(), now],
    )?;
    Ok(id)
}

fn bytes16(raw: &[u8]) -> Option<[u8; 16]> {
    (raw.len() == 16).then(|| {
        let mut a = [0u8; 16];
        a.copy_from_slice(raw);
        a
    })
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
