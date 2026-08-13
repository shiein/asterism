use std::fs;
use std::io::{Error, ErrorKind};
use std::path::Path;

use asterism_crypto::{AccountVaultKey, RecoveryKey};
use serde::{Deserialize, Serialize};

use crate::atomic::atomic_write;

#[derive(Serialize, Deserialize)]
struct VaultFile {
    recovery_hex: String,
}

/// 本地 AVK。Hub 不得持有明文。文件权限由调用方保证；后续可迁到系统钥匙串。
pub struct LocalVault {
    pub avk: AccountVaultKey,
}

impl LocalVault {
    pub fn load_or_create(config_dir: &Path) -> std::io::Result<Self> {
        fs::create_dir_all(config_dir)?;
        let path = config_dir.join("vault.json");
        if path.exists() {
            let raw = fs::read(&path)?;
            let file = serde_json::from_slice::<VaultFile>(&raw).map_err(|err| {
                Error::new(ErrorKind::InvalidData, format!("invalid vault.json: {err}"))
            })?;
            let key = RecoveryKey::decode_hex(&file.recovery_hex).map_err(|err| {
                Error::new(ErrorKind::InvalidData, format!("invalid recovery key: {err}"))
            })?;
            return Ok(Self { avk: AccountVaultKey::from_bytes(*key.avk().as_bytes()) });
        }
        let avk = AccountVaultKey::generate();
        let vault = Self { avk };
        vault.save(config_dir)?;
        Ok(vault)
    }

    pub fn save(&self, config_dir: &Path) -> std::io::Result<()> {
        fs::create_dir_all(config_dir)?;
        let path = config_dir.join("vault.json");
        let file = VaultFile { recovery_hex: self.recovery_hex() };
        atomic_write(&path, &serde_json::to_vec_pretty(&file).expect("vault json"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }

    pub fn recovery_hex(&self) -> String {
        RecoveryKey::from_avk(AccountVaultKey::from_bytes(*self.avk.as_bytes())).encode_hex()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("asterism-vault-{name}-{}", std::process::id()))
    }

    #[test]
    fn corrupt_vault_is_rejected_without_overwrite() {
        let dir = test_dir("corrupt");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("vault.json");
        fs::write(&path, b"not-json").unwrap();

        let err = LocalVault::load_or_create(&dir).err().unwrap();

        assert_eq!(err.kind(), ErrorKind::InvalidData);
        assert_eq!(fs::read(&path).unwrap(), b"not-json");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn missing_vault_is_created_and_round_trips() {
        let dir = test_dir("create");
        let _ = fs::remove_dir_all(&dir);

        let created = LocalVault::load_or_create(&dir).unwrap();
        let loaded = LocalVault::load_or_create(&dir).unwrap();

        assert_eq!(created.avk.as_bytes(), loaded.avk.as_bytes());
        fs::remove_dir_all(dir).unwrap();
    }
}
