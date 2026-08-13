use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{CoreError, Result};

macro_rules! uuid_id {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }

            pub fn from_uuid(id: Uuid) -> Self {
                Self(id)
            }

            pub fn as_uuid(self) -> Uuid {
                self.0
            }

            pub fn as_bytes(self) -> [u8; 16] {
                *self.0.as_bytes()
            }

            pub fn from_bytes(bytes: [u8; 16]) -> Self {
                Self(Uuid::from_bytes(bytes))
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl FromStr for $name {
            type Err = CoreError;

            fn from_str(s: &str) -> Result<Self> {
                Ok(Self(Uuid::parse_str(s).map_err(|_| CoreError::InvalidUuid)?))
            }
        }
    };
}

uuid_id!(ContentId);
uuid_id!(DeviceId);
uuid_id!(AccountId);
uuid_id!(ManifestId);

/// 内容寻址 Blob 标识。本地为 BLAKE3 hex；Hub 侧为加密后寻址标识。
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BlobId(String);

impl BlobId {
    pub fn from_hex(hex: impl Into<String>) -> Result<Self> {
        let hex = hex.into();
        if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(CoreError::InvalidHex(hex));
        }
        Ok(Self(hex.to_ascii_lowercase()))
    }

    pub fn from_blake3(hash: &[u8; 32]) -> Self {
        Self(hex_encode(hash))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 两级目录分片：`aa/aabbcc...`
    pub fn shard_dir(&self) -> &str {
        &self.0[..2]
    }
}

impl fmt::Display for BlobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn hex_encode(bytes: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(64);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_id_is_uuid_v7() {
        let id = ContentId::new();
        assert_eq!(id.as_uuid().get_version_num(), 7);
    }

    #[test]
    fn blob_id_rejects_non_hex() {
        assert!(BlobId::from_hex("zz").is_err());
        assert!(BlobId::from_hex("ab").is_err());
    }

    #[test]
    fn blob_id_shards_first_two_hex() {
        let id = BlobId::from_blake3(&[0xab; 32]);
        assert_eq!(id.shard_dir(), "ab");
        assert_eq!(id.as_str().len(), 64);
    }
}
