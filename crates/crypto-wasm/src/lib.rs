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
    let s = s.trim();
    if s.len() != 64 {
        return Err(JsValue::from_str("recovery key must be 64 hex chars"));
    }
    let mut out = [0u8; 32];
    for i in 0..32 {
        out[i] = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16)
            .map_err(|_| JsValue::from_str("invalid hex"))?;
    }
    Ok(out)
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
