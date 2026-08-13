use asterism_crypto::{AccountVaultKey, EncryptedPayload, decrypt_metadata, encrypt_metadata};
use serde::{Deserialize, Serialize};

use crate::error::{Result, SyncError};

/// 远程同步封装：Hub 只见密文。小 payload 走 body；大 payload 走分块 Blob。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SyncPackage {
    pub meta: EncryptedPayload,
    pub body: Option<EncryptedPayload>,
    pub chunk_count: u32,
}

pub fn pack(
    avk: &AccountVaultKey,
    metadata_json: &[u8],
    payload: Option<&[u8]>,
) -> Result<SyncPackage> {
    let meta =
        encrypt_metadata(avk, metadata_json).map_err(|e| SyncError::Failed(e.to_string()))?;
    let body = match payload {
        Some(bytes) if bytes.len() <= 256 * 1024 => {
            Some(encrypt_metadata(avk, bytes).map_err(|e| SyncError::Failed(e.to_string()))?)
        }
        _ => None,
    };
    Ok(SyncPackage { meta, body, chunk_count: 0 })
}

pub fn unpack_meta(avk: &AccountVaultKey, pkg: &SyncPackage) -> Result<Vec<u8>> {
    decrypt_metadata(avk, &pkg.meta).map_err(|e| SyncError::Failed(e.to_string()))
}

pub fn unpack_body(avk: &AccountVaultKey, pkg: &SyncPackage) -> Result<Option<Vec<u8>>> {
    match &pkg.body {
        Some(body) => {
            Ok(Some(decrypt_metadata(avk, body).map_err(|e| SyncError::Failed(e.to_string()))?))
        }
        None => Ok(None),
    }
}

pub fn encode_package(pkg: &SyncPackage) -> Result<String> {
    let bytes = serde_json::to_vec(pkg).map_err(|e| SyncError::Protocol(e.to_string()))?;
    Ok(hex::encode(bytes))
}

pub fn decode_package(hex_str: &str) -> Result<SyncPackage> {
    let bytes = hex::decode(hex_str).map_err(|e| SyncError::Protocol(e.to_string()))?;
    serde_json::from_slice(&bytes).map_err(|e| SyncError::Protocol(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use asterism_crypto::AccountVaultKey;

    #[test]
    fn pack_roundtrip() {
        let avk = AccountVaultKey::generate();
        let pkg = pack(&avk, br#"{"k":1}"#, Some(b"hello")).unwrap();
        let hex = encode_package(&pkg).unwrap();
        let back = decode_package(&hex).unwrap();
        assert_eq!(unpack_meta(&avk, &back).unwrap(), br#"{"k":1}"#);
        assert_eq!(unpack_body(&avk, &back).unwrap().unwrap(), b"hello");
    }
}
