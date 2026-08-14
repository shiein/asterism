use asterism_crypto::{
    AccountVaultKey, CHUNK_SIZE, EncryptedChunk, EncryptedPayload, ItemKey, WrappedItemKey,
    blake3_bytes, decrypt_chunk, decrypt_metadata, encrypt_chunk, encrypt_metadata,
    unwrap_item_key, wrap_item_key,
};
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

const BLOB_CHUNK_MAGIC: &[u8; 4] = b"ASB1";
const BLOB_CHUNK_HEADER: usize = 4 + 24 + 48 + 32 + 4 + 24 + 4;

pub struct BlobChunkEncryptor {
    item_key: ItemKey,
    wrapped: WrappedItemKey,
    blob_id: [u8; 32],
}

pub struct BlobChunkDecryptor {
    expected_blob_id: Option<[u8; 32]>,
    hasher: blake3::Hasher,
}

impl Default for BlobChunkDecryptor {
    fn default() -> Self {
        Self { expected_blob_id: None, hasher: blake3::Hasher::new() }
    }
}

impl BlobChunkDecryptor {
    pub fn decrypt(
        &mut self,
        avk: &AccountVaultKey,
        expected_index: u32,
        encoded: &[u8],
    ) -> Result<Vec<u8>> {
        let (wrapped, chunk) = decode_blob_chunk(encoded)?;
        if chunk.chunk_index != expected_index {
            return Err(SyncError::Protocol("blob chunk index is not contiguous".into()));
        }
        if self.expected_blob_id.is_some_and(|id| id != chunk.blob_id) {
            return Err(SyncError::Protocol("blob id changed between chunks".into()));
        }
        self.expected_blob_id = Some(chunk.blob_id);
        let item_key = unwrap_item_key(avk, &wrapped).map_err(crypto_failed)?;
        let plaintext = decrypt_chunk(&item_key, &chunk).map_err(crypto_failed)?;
        self.hasher.update(&plaintext);
        Ok(plaintext)
    }

    pub fn finish(self, avk: &AccountVaultKey) -> Result<()> {
        let hash = *self.hasher.finalize().as_bytes();
        let expected = self
            .expected_blob_id
            .ok_or_else(|| SyncError::Protocol("encrypted blob has no chunks".into()))?;
        if expected != avk.dedup_tag(&hash) && expected != hash {
            return Err(SyncError::Failed("blob plaintext hash mismatch".into()));
        }
        Ok(())
    }
}

impl BlobChunkEncryptor {
    pub fn new(avk: &AccountVaultKey, plaintext_hash: [u8; 32]) -> Result<Self> {
        let item_key = ItemKey::generate();
        let wrapped = wrap_item_key(avk, &item_key).map_err(crypto_failed)?;
        Ok(Self { item_key, wrapped, blob_id: avk.dedup_tag(&plaintext_hash) })
    }

    pub fn encrypt(&self, index: u32, plaintext: &[u8]) -> Result<Vec<u8>> {
        let chunk =
            encrypt_chunk(&self.item_key, self.blob_id, index, plaintext).map_err(crypto_failed)?;
        encode_blob_chunk(&self.wrapped, &chunk)
    }
}

pub fn encrypt_blob_chunks(avk: &AccountVaultKey, plaintext: &[u8]) -> Result<Vec<Vec<u8>>> {
    let encryptor = BlobChunkEncryptor::new(avk, blake3_bytes(plaintext))?;
    plaintext
        .chunks(CHUNK_SIZE)
        .enumerate()
        .map(|(index, bytes)| {
            let index = u32::try_from(index)
                .map_err(|_| SyncError::Protocol("blob has too many chunks".into()))?;
            encryptor.encrypt(index, bytes)
        })
        .collect()
}

pub fn decrypt_blob_chunks(avk: &AccountVaultKey, encoded: &[Vec<u8>]) -> Result<Vec<u8>> {
    if encoded.is_empty() {
        return Err(SyncError::Protocol("encrypted blob has no chunks".into()));
    }
    let mut plaintext = Vec::new();
    let mut decryptor = BlobChunkDecryptor::default();
    for (expected_index, bytes) in encoded.iter().enumerate() {
        let expected_index = u32::try_from(expected_index)
            .map_err(|_| SyncError::Protocol("blob has too many chunks".into()))?;
        plaintext.extend(decryptor.decrypt(avk, expected_index, bytes)?);
    }
    decryptor.finish(avk)?;
    Ok(plaintext)
}

fn encode_blob_chunk(wrapped: &WrappedItemKey, chunk: &EncryptedChunk) -> Result<Vec<u8>> {
    if wrapped.ciphertext.len() != 48 {
        return Err(SyncError::Protocol("unexpected wrapped item key length".into()));
    }
    let ciphertext_len = u32::try_from(chunk.ciphertext.len())
        .map_err(|_| SyncError::Protocol("encrypted chunk is too large".into()))?;
    let mut out = Vec::with_capacity(BLOB_CHUNK_HEADER + chunk.ciphertext.len());
    out.extend_from_slice(BLOB_CHUNK_MAGIC);
    out.extend_from_slice(&wrapped.nonce);
    out.extend_from_slice(&wrapped.ciphertext);
    out.extend_from_slice(&chunk.blob_id);
    out.extend_from_slice(&chunk.chunk_index.to_le_bytes());
    out.extend_from_slice(&chunk.nonce);
    out.extend_from_slice(&ciphertext_len.to_le_bytes());
    out.extend_from_slice(&chunk.ciphertext);
    Ok(out)
}

fn decode_blob_chunk(bytes: &[u8]) -> Result<(WrappedItemKey, EncryptedChunk)> {
    if bytes.len() < BLOB_CHUNK_HEADER || &bytes[..4] != BLOB_CHUNK_MAGIC {
        return Err(SyncError::Protocol("invalid encrypted blob chunk".into()));
    }
    let mut wrapped_nonce = [0u8; 24];
    wrapped_nonce.copy_from_slice(&bytes[4..28]);
    let wrapped_ciphertext = bytes[28..76].to_vec();
    let mut blob_id = [0u8; 32];
    blob_id.copy_from_slice(&bytes[76..108]);
    let chunk_index = u32::from_le_bytes(bytes[108..112].try_into().expect("fixed slice"));
    let mut nonce = [0u8; 24];
    nonce.copy_from_slice(&bytes[112..136]);
    let ciphertext_len =
        u32::from_le_bytes(bytes[136..140].try_into().expect("fixed slice")) as usize;
    if bytes.len() != BLOB_CHUNK_HEADER + ciphertext_len {
        return Err(SyncError::Protocol("encrypted blob chunk length mismatch".into()));
    }
    Ok((
        WrappedItemKey { nonce: wrapped_nonce, ciphertext: wrapped_ciphertext },
        EncryptedChunk {
            blob_id,
            chunk_index,
            nonce,
            ciphertext: bytes[BLOB_CHUNK_HEADER..].to_vec(),
        },
    ))
}

fn crypto_failed(err: impl std::fmt::Display) -> SyncError {
    SyncError::Failed(err.to_string())
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

    #[test]
    fn multi_megabyte_blob_roundtrip_uses_bounded_chunks() {
        let avk = AccountVaultKey::generate();
        let plaintext = vec![0x5a; CHUNK_SIZE * 2 + 17];

        let chunks = encrypt_blob_chunks(&avk, &plaintext).unwrap();

        assert_eq!(chunks.len(), 3);
        assert!(chunks.iter().all(|chunk| chunk.len() <= CHUNK_SIZE + BLOB_CHUNK_HEADER + 16));
        assert_eq!(decrypt_blob_chunks(&avk, &chunks).unwrap(), plaintext);
    }

    #[test]
    fn blob_rejects_reordered_chunks() {
        let avk = AccountVaultKey::generate();
        let plaintext = vec![0x3c; CHUNK_SIZE + 1];
        let mut chunks = encrypt_blob_chunks(&avk, &plaintext).unwrap();
        chunks.swap(0, 1);

        assert!(decrypt_blob_chunks(&avk, &chunks).is_err());
    }
}
