//! 加密实现集中于此 crate，Desktop 与未来 Web WASM 共用同一算法。
//!
//! TLS 与应用层 E2EE 是两层保护。本 crate 只处理应用层。

pub mod aead;
pub mod error;
pub mod hash;
pub mod item;
pub mod keys;

pub use aead::{CHUNK_SIZE, EncryptedChunk, decrypt_chunk, encrypt_chunk, encrypt_small};
pub use error::CryptoError;
pub use hash::{blake3_bytes, blake3_reader};
pub use item::{EncryptedPayload, decrypt_metadata, encrypt_metadata};
pub use keys::{
    AccountVaultKey, DeviceIdentity, ItemKey, RecoveryKey, WrappedItemKey, derive_search_cache_key,
    local_dedup_tag, unwrap_item_key, wrap_item_key,
};
