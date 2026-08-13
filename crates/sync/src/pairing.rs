use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use asterism_core::id::{AccountId, DeviceId};

/// 一次性配对码，有时效。只回传明文一次，存储仅保留 hash。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PairingOffer {
    pub code: String,
    pub expires_at_ms: i64,
    pub account_id: AccountId,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PairingFinish {
    pub code: String,
    pub device_id: DeviceId,
    pub device_name: String,
    pub platform: String,
    pub identity_public_key: Vec<u8>,
}

pub fn generate_code() -> String {
    const ALPH: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut rng = rand::thread_rng();
    (0..8).map(|_| ALPH[rng.gen_range(0..ALPH.len())] as char).collect()
}

pub fn hash_code(code: &str) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(code.trim().to_ascii_uppercase().as_bytes());
    h.finalize().into()
}

pub fn hash_token(token: &str) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(token.as_bytes());
    h.finalize().into()
}

pub fn generate_session_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill(&mut bytes);
    hex::encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_hash_is_case_insensitive() {
        let c = generate_code();
        assert_eq!(hash_code(&c), hash_code(&c.to_ascii_lowercase()));
        assert_eq!(c.len(), 8);
    }
}
