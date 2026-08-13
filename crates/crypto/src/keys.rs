use ed25519_dalek::{SigningKey, VerifyingKey};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use rand::RngCore;
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use x25519_dalek::{PublicKey as X25519Public, StaticSecret};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::error::{CryptoError, Result};
use crate::hash::blake3_bytes;

type HmacSha256 = Hmac<Sha256>;

const HKDF_INFO_SEARCH: &[u8] = b"asterism/search-cache/v1";
const HKDF_INFO_WRAP: &[u8] = b"asterism/item-key-wrap/v1";

/// Account Vault Key，随机 256-bit。新设备通过配对获取。
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct AccountVaultKey([u8; 32]);

impl AccountVaultKey {
    pub fn generate() -> Self {
        let mut key = [0u8; 32];
        OsRng.fill_bytes(&mut key);
        Self(key)
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// `dedup_tag = HMAC(AVK, BLAKE3(plaintext))`
    ///
    /// 同一账户内可判断重复，且不向 Hub 泄漏全局明文 Hash。
    pub fn dedup_tag(&self, plaintext_blake3: &[u8; 32]) -> [u8; 32] {
        let mut mac = HmacSha256::new_from_slice(&self.0).expect("HMAC-SHA256 accepts 32-byte key");
        mac.update(plaintext_blake3);
        let result = mac.finalize().into_bytes();
        let mut out = [0u8; 32];
        out.copy_from_slice(&result);
        out
    }
}

/// 没有 Vault 时的本地去重：直接使用明文 BLAKE3。上线 Hub 后改用 AVK HMAC。
pub fn local_dedup_tag(plaintext: &[u8]) -> [u8; 32] {
    blake3_bytes(plaintext)
}

#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct ItemKey([u8; 32]);

impl ItemKey {
    pub fn generate() -> Self {
        let mut key = [0u8; 32];
        OsRng.fill_bytes(&mut key);
        Self(key)
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WrappedItemKey {
    pub nonce: [u8; 24],
    pub ciphertext: Vec<u8>,
}

pub fn wrap_item_key(avk: &AccountVaultKey, item: &ItemKey) -> Result<WrappedItemKey> {
    use chacha20poly1305::aead::{Aead, KeyInit};
    use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};

    let hk = Hkdf::<Sha256>::new(None, avk.as_bytes());
    let mut wrap_key = [0u8; 32];
    hk.expand(HKDF_INFO_WRAP, &mut wrap_key).map_err(|_| CryptoError::InvalidKeyLength)?;
    let cipher = XChaCha20Poly1305::new(Key::from_slice(&wrap_key));
    wrap_key.zeroize();

    let mut nonce_bytes = [0u8; 24];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = XNonce::from_slice(&nonce_bytes);
    let ciphertext =
        cipher.encrypt(nonce, item.as_bytes().as_slice()).map_err(|_| CryptoError::Decrypt)?;
    Ok(WrappedItemKey { nonce: nonce_bytes, ciphertext })
}

pub fn unwrap_item_key(avk: &AccountVaultKey, wrapped: &WrappedItemKey) -> Result<ItemKey> {
    use chacha20poly1305::aead::{Aead, KeyInit};
    use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};

    let hk = Hkdf::<Sha256>::new(None, avk.as_bytes());
    let mut wrap_key = [0u8; 32];
    hk.expand(HKDF_INFO_WRAP, &mut wrap_key).map_err(|_| CryptoError::InvalidKeyLength)?;
    let cipher = XChaCha20Poly1305::new(Key::from_slice(&wrap_key));
    wrap_key.zeroize();

    let nonce = XNonce::from_slice(&wrapped.nonce);
    let plain =
        cipher.decrypt(nonce, wrapped.ciphertext.as_ref()).map_err(|_| CryptoError::Decrypt)?;
    if plain.len() != 32 {
        return Err(CryptoError::InvalidKeyLength);
    }
    let mut bytes = [0u8; 32];
    bytes.copy_from_slice(&plain);
    Ok(ItemKey::from_bytes(bytes))
}

pub fn derive_search_cache_key(avk: &AccountVaultKey) -> [u8; 32] {
    let hk = Hkdf::<Sha256>::new(None, avk.as_bytes());
    let mut out = [0u8; 32];
    hk.expand(HKDF_INFO_SEARCH, &mut out).expect("32-byte OKM fits HKDF-SHA256");
    out
}

/// 每设备独立身份：Ed25519 签名 + X25519 密钥交换。
pub struct DeviceIdentity {
    pub signing: SigningKey,
    pub ecdh: StaticSecret,
}

impl DeviceIdentity {
    pub fn generate() -> Self {
        let mut sign_seed = [0u8; 32];
        OsRng.fill_bytes(&mut sign_seed);
        let signing = SigningKey::from_bytes(&sign_seed);
        sign_seed.zeroize();
        let ecdh = StaticSecret::random_from_rng(OsRng);
        Self { signing, ecdh }
    }

    pub fn verifying_key(&self) -> VerifyingKey {
        self.signing.verifying_key()
    }

    pub fn ecdh_public(&self) -> X25519Public {
        X25519Public::from(&self.ecdh)
    }

    pub fn public_identity_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(64);
        out.extend_from_slice(self.verifying_key().as_bytes());
        out.extend_from_slice(self.ecdh_public().as_bytes());
        out
    }
}

/// Recovery Key 导出：本质是 AVK 的可备份编码。
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct RecoveryKey(AccountVaultKey);

impl RecoveryKey {
    pub fn from_avk(avk: AccountVaultKey) -> Self {
        Self(avk)
    }

    pub fn encode_hex(&self) -> String {
        hex_lower(self.0.as_bytes())
    }

    pub fn decode_hex(s: &str) -> Result<Self> {
        let raw = decode_hex32(s)?;
        Ok(Self(AccountVaultKey::from_bytes(raw)))
    }

    pub fn avk(&self) -> &AccountVaultKey {
        &self.0
    }
}

fn hex_lower(bytes: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(64);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

fn decode_hex32(s: &str) -> Result<[u8; 32]> {
    let s = s.trim();
    if s.len() != 64 {
        return Err(CryptoError::InvalidKeyLength);
    }
    let mut out = [0u8; 32];
    for i in 0..32 {
        let byte = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16)
            .map_err(|_| CryptoError::InvalidKeyLength)?;
        out[i] = byte;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_roundtrip() {
        let avk = AccountVaultKey::generate();
        let item = ItemKey::generate();
        let wrapped = wrap_item_key(&avk, &item).unwrap();
        let opened = unwrap_item_key(&avk, &wrapped).unwrap();
        assert_eq!(item.as_bytes(), opened.as_bytes());
    }

    #[test]
    fn wrong_avk_cannot_unwrap() {
        let item = ItemKey::generate();
        let wrapped = wrap_item_key(&AccountVaultKey::generate(), &item).unwrap();
        assert!(unwrap_item_key(&AccountVaultKey::generate(), &wrapped).is_err());
    }

    #[test]
    fn dedup_tag_hides_raw_hash_from_same_input_without_key() {
        let avk = AccountVaultKey::generate();
        let hash = blake3_bytes(b"hello");
        let tag = avk.dedup_tag(&hash);
        assert_ne!(tag, hash);
        assert_eq!(tag, avk.dedup_tag(&hash));
        assert_ne!(tag, AccountVaultKey::generate().dedup_tag(&hash));
    }

    #[test]
    fn recovery_hex_roundtrip() {
        let avk = AccountVaultKey::generate();
        let encoded =
            RecoveryKey::from_avk(AccountVaultKey::from_bytes(*avk.as_bytes())).encode_hex();
        let decoded = RecoveryKey::decode_hex(&encoded).unwrap();
        assert_eq!(decoded.avk().as_bytes(), avk.as_bytes());
    }
}
