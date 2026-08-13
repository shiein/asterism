use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use rand::RngCore;
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};

use crate::error::{CryptoError, Result};
use crate::keys::ItemKey;

/// 大文件不能作为单个 AEAD message。固定分块独立认证加密。
pub const CHUNK_SIZE: usize = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EncryptedChunk {
    pub blob_id: [u8; 32],
    pub chunk_index: u32,
    pub nonce: [u8; 24],
    pub ciphertext: Vec<u8>,
}

fn aad(blob_id: &[u8; 32], chunk_index: u32) -> [u8; 36] {
    let mut out = [0u8; 36];
    out[..32].copy_from_slice(blob_id);
    out[32..].copy_from_slice(&chunk_index.to_le_bytes());
    out
}

pub fn encrypt_chunk(
    item_key: &ItemKey,
    blob_id: [u8; 32],
    chunk_index: u32,
    plaintext: &[u8],
) -> Result<EncryptedChunk> {
    if plaintext.len() > CHUNK_SIZE {
        return Err(CryptoError::InvalidChunk);
    }
    let cipher = XChaCha20Poly1305::new(Key::from_slice(item_key.as_bytes()));
    let mut nonce_bytes = [0u8; 24];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = XNonce::from_slice(&nonce_bytes);
    let aad = aad(&blob_id, chunk_index);
    let ciphertext = cipher
        .encrypt(nonce, Payload { msg: plaintext, aad: &aad })
        .map_err(|_| CryptoError::Decrypt)?;
    Ok(EncryptedChunk { blob_id, chunk_index, nonce: nonce_bytes, ciphertext })
}

pub fn decrypt_chunk(item_key: &ItemKey, chunk: &EncryptedChunk) -> Result<Vec<u8>> {
    let cipher = XChaCha20Poly1305::new(Key::from_slice(item_key.as_bytes()));
    let nonce = XNonce::from_slice(&chunk.nonce);
    let aad = aad(&chunk.blob_id, chunk.chunk_index);
    cipher
        .decrypt(nonce, Payload { msg: &chunk.ciphertext, aad: &aad })
        .map_err(|_| CryptoError::Decrypt)
}

/// 小文本可直接作为单个加密 payload。
pub fn encrypt_small(
    item_key: &ItemKey,
    blob_id: [u8; 32],
    plaintext: &[u8],
) -> Result<EncryptedChunk> {
    encrypt_chunk(item_key, blob_id, 0, plaintext)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_roundtrip_binds_index() {
        let key = ItemKey::generate();
        let blob = [7u8; 32];
        let chunk = encrypt_chunk(&key, blob, 3, b"payload-bytes").unwrap();
        assert_eq!(decrypt_chunk(&key, &chunk).unwrap(), b"payload-bytes");

        let mut tampered = chunk.clone();
        tampered.chunk_index = 4;
        assert!(decrypt_chunk(&key, &tampered).is_err());
    }

    #[test]
    fn rejects_oversized_chunk() {
        let key = ItemKey::generate();
        let big = vec![0u8; CHUNK_SIZE + 1];
        assert!(encrypt_chunk(&key, [0; 32], 0, &big).is_err());
    }
}
