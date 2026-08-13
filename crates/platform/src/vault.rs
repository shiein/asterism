use std::fs;
use std::path::Path;

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
            if let Ok(file) = serde_json::from_slice::<VaultFile>(&raw)
                && let Ok(key) = RecoveryKey::decode_hex(&file.recovery_hex)
            {
                return Ok(Self { avk: AccountVaultKey::from_bytes(*key.avk().as_bytes()) });
            }
        }
        let avk = AccountVaultKey::generate();
        let rec = RecoveryKey::from_avk(AccountVaultKey::from_bytes(*avk.as_bytes()));
        let file = VaultFile { recovery_hex: rec.encode_hex() };
        fs::write(&path, serde_json::to_vec_pretty(&file).expect("vault json"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
        }
        Ok(Self { avk })
    }

    pub fn recovery_hex(&self) -> String {
        RecoveryKey::from_avk(AccountVaultKey::from_bytes(*self.avk.as_bytes())).encode_hex()
    }
}
