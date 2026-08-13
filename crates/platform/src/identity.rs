use std::io::{Error, ErrorKind};
use std::path::Path;

use asterism_core::id::{AccountId, DeviceId};
use serde::{Deserialize, Serialize};

use crate::atomic::atomic_write;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LocalIdentity {
    pub device_id: DeviceId,
    pub account_id: AccountId,
    pub device_name: String,
}

impl LocalIdentity {
    pub fn load_or_create(config_dir: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(config_dir)?;
        let path = config_dir.join("identity.json");
        if path.exists() {
            let bytes = std::fs::read(&path)?;
            return serde_json::from_slice(&bytes).map_err(|err| {
                Error::new(ErrorKind::InvalidData, format!("invalid identity.json: {err}"))
            });
        }
        let identity = Self {
            device_id: DeviceId::new(),
            account_id: AccountId::new(),
            device_name: default_device_name(),
        };
        identity.save(config_dir)?;
        Ok(identity)
    }

    pub fn save(&self, config_dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(config_dir)?;
        let path = config_dir.join("identity.json");
        atomic_write(&path, &serde_json::to_vec_pretty(self).expect("identity json"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }
}

fn default_device_name() -> String {
    hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            if cfg!(target_os = "macos") {
                "Mac".into()
            } else if cfg!(windows) {
                "Windows PC".into()
            } else {
                "Asterism".into()
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corrupt_identity_is_rejected_without_overwrite() {
        let dir = std::env::temp_dir().join(format!("asterism-id-corrupt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("identity.json");
        std::fs::write(&path, b"not-json").unwrap();

        let err = LocalIdentity::load_or_create(&dir).err().unwrap();
        assert_eq!(err.kind(), ErrorKind::InvalidData);
        assert_eq!(std::fs::read(&path).unwrap(), b"not-json");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
