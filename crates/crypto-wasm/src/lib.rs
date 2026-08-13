use asterism_crypto::{AccountVaultKey, EncryptedPayload, decrypt_metadata};
use serde::Deserialize;
use wasm_bindgen::prelude::*;

#[derive(Deserialize)]
struct SyncPackage {
    meta: EncryptedPayload,
    body: Option<EncryptedPayload>,
}

fn avk(hex_key: &str) -> Result<AccountVaultKey, JsValue> {
    Ok(AccountVaultKey::from_bytes(decode_hex32(hex_key)?))
}

fn decode_hex32(s: &str) -> Result<[u8; 32], JsValue> {
    let raw = hex::decode(s.trim()).map_err(|_| JsValue::from_str("invalid hex"))?;
    raw.try_into().map_err(|_| JsValue::from_str("recovery key must be 64 hex chars"))
}

fn parse_pkg(package_hex: &str) -> Result<SyncPackage, JsValue> {
    let bytes = hex::decode(package_hex).map_err(|e| JsValue::from_str(&e.to_string()))?;
    serde_json::from_slice(&bytes).map_err(|e| JsValue::from_str(&e.to_string()))
}

#[wasm_bindgen]
pub fn decrypt_package_meta(recovery_hex: &str, package_hex: &str) -> Result<String, JsValue> {
    let pkg = parse_pkg(package_hex)?;
    let plain = decrypt_metadata(&avk(recovery_hex)?, &pkg.meta)
        .map_err(|_| JsValue::from_str("decrypt failed"))?;
    String::from_utf8(plain).map_err(|e| JsValue::from_str(&e.to_string()))
}

#[wasm_bindgen]
pub fn decrypt_package_body_hex(recovery_hex: &str, package_hex: &str) -> Result<String, JsValue> {
    let pkg = parse_pkg(package_hex)?;
    let body = pkg.body.ok_or_else(|| JsValue::from_str("no body"))?;
    let plain = decrypt_metadata(&avk(recovery_hex)?, &body)
        .map_err(|_| JsValue::from_str("decrypt failed"))?;
    Ok(hex::encode(plain))
}
