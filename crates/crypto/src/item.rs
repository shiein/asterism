use serde::{Deserialize, Serialize};

use crate::aead::{EncryptedChunk, decrypt_chunk, encrypt_small};
use crate::error::Result;
use crate::hash::blake3_bytes;
use crate::keys::{AccountVaultKey, ItemKey, WrappedItemKey, unwrap_item_key, wrap_item_key};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EncryptedPayload {
    pub wrapped_key: WrappedItemKey,
    pub chunk: EncryptedChunk,
}

pub fn encrypt_metadata(avk: &AccountVaultKey, plaintext: &[u8]) -> Result<EncryptedPayload> {
    let item = ItemKey::generate();
    let blob_id = avk.dedup_tag(&blake3_bytes(plaintext));
    let chunk = encrypt_small(&item, blob_id, plaintext)?;
    let wrapped_key = wrap_item_key(avk, &item)?;
    Ok(EncryptedPayload { wrapped_key, chunk })
}

pub fn decrypt_metadata(avk: &AccountVaultKey, payload: &EncryptedPayload) -> Result<Vec<u8>> {
    let item = unwrap_item_key(avk, &payload.wrapped_key)?;
    let plain = decrypt_chunk(&item, &payload.chunk)?;
    let hash = blake3_bytes(&plain);
    if payload.chunk.blob_id != avk.dedup_tag(&hash) && payload.chunk.blob_id != hash {
        return Err(crate::error::CryptoError::Decrypt);
    }
    Ok(plain)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_roundtrip() {
        let avk = AccountVaultKey::generate();
        let enc = encrypt_metadata(&avk, br#"{"text_preview":"hi"}"#).unwrap();
        let plain = decrypt_metadata(&avk, &enc).unwrap();
        assert_eq!(plain, br#"{"text_preview":"hi"}"#);
    }
}
