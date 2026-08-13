use argon2::{Algorithm, Argon2, Params, Version};
use rand::Rng;
use serde::{Deserialize, Serialize};

use asterism_core::id::{AccountId, DeviceId};

pub const PAIRING_CODE_LEN: usize = 20;
const PAIRING_ALPH: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

/// 一次性配对码，有时效。只回传明文一次，存储仅保留 hash。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PairingOffer {
    pub code: String,
    pub expires_at_ms: i64,
    pub account_id: AccountId,
    #[serde(default)]
    pub kdf_salt_hex: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PairingFinish {
    pub code: String,
    pub device_id: DeviceId,
    pub device_name: String,
    pub platform: String,
    pub identity_public_key: Vec<u8>,
    #[serde(default)]
    pub cert_fingerprint: String,
}

pub fn generate_code() -> String {
    let mut rng = rand::thread_rng();
    (0..PAIRING_CODE_LEN)
        .map(|_| PAIRING_ALPH[rng.gen_range(0..PAIRING_ALPH.len())] as char)
        .collect()
}

pub fn normalize_code(code: &str) -> String {
    code.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_uppercase()).collect()
}

pub fn hash_code(code: &str) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(normalize_code(code).as_bytes());
    h.finalize().into()
}

pub fn generate_kdf_salt() -> [u8; 16] {
    let mut salt = [0u8; 16];
    rand::thread_rng().fill(&mut salt);
    salt
}

/// Argon2id(m=64MiB, t=3, p=1) 从配对码派生 AVK wrap key。
pub fn derive_wrap_key(code: &str, salt: &[u8; 16]) -> [u8; 32] {
    let params = Params::new(64 * 1024, 3, 1, Some(32)).expect("argon2 params");
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut out = [0u8; 32];
    argon
        .hash_password_into(normalize_code(code).as_bytes(), salt, &mut out)
        .expect("argon2id derive");
    out
}

pub fn parse_salt_hex(hex_str: &str) -> Option<[u8; 16]> {
    let raw = hex::decode(hex_str).ok()?;
    (raw.len() == 16).then(|| {
        let mut salt = [0u8; 16];
        salt.copy_from_slice(&raw);
        salt
    })
}

pub fn hash_token(token: &str) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(token.as_bytes());
    h.finalize().into()
}

pub fn generate_session_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill(&mut bytes);
    hex::encode(bytes)
}

pub fn generate_bootstrap_secret() -> String {
    let mut bytes = [0u8; 24];
    rand::thread_rng().fill(&mut bytes);
    hex::encode(bytes)
}

pub fn hash_bootstrap(secret: &str) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(secret.trim().as_bytes());
    h.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_hash_is_case_insensitive() {
        let c = generate_code();
        assert_eq!(hash_code(&c), hash_code(&c.to_ascii_lowercase()));
        assert_eq!(c.len(), PAIRING_CODE_LEN);
    }

    #[test]
    fn wrap_key_depends_on_salt_and_code() {
        let salt = [7u8; 16];
        let a = derive_wrap_key("ABCDEFGHJKLMNPQRSTUV", &salt);
        let b = derive_wrap_key("abcdefghjklmnpqrstuv", &salt);
        let c = derive_wrap_key("ABCDEFGHJKLMNPQRSTUV", &[8u8; 16]);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
}
