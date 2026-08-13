use std::fs::{self, OpenOptions};
use std::io::{Error, ErrorKind, Write};
use std::path::{Path, PathBuf};

use asterism_crypto::{AccountVaultKey, RecoveryKey};
use serde::{Deserialize, Serialize};

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

fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let temp = temporary_path(path);
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        replace_file(&temp, path)?;
        #[cfg(unix)]
        if let Some(parent) = path.parent() {
            fs::File::open(parent)?.sync_all()?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn temporary_path(path: &Path) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    path.with_extension(format!("json.{}.{}.tmp", std::process::id(), nonce))
}

#[cfg(unix)]
fn replace_file(temp: &Path, path: &Path) -> std::io::Result<()> {
    fs::rename(temp, path)
}

#[cfg(not(unix))]
fn replace_file(temp: &Path, path: &Path) -> std::io::Result<()> {
    if !path.exists() {
        return fs::rename(temp, path);
    }
    let backup = path.with_extension("json.backup");
    let _ = fs::remove_file(&backup);
    fs::rename(path, &backup)?;
    if let Err(err) = fs::rename(temp, path) {
        let _ = fs::rename(&backup, path);
        return Err(err);
    }
    let _ = fs::remove_file(backup);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
