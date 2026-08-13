use std::path::Path;

use asterism_core::id::{AccountId, DeviceId};
use serde::{Deserialize, Serialize};

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
            if let Ok(id) = serde_json::from_slice(&bytes) {
                return Ok(id);
            }
        }
        let identity = Self {
            device_id: DeviceId::new(),
            account_id: AccountId::new(),
            device_name: default_device_name(),
        };
        std::fs::write(path, serde_json::to_vec_pretty(&identity).expect("identity json"))?;
        Ok(identity)
    }
}

fn default_device_name() -> String {
    hostname::get().ok().and_then(|h| h.into_string().ok()).filter(|s| !s.is_empty()).unwrap_or_else(|| {
        if cfg!(target_os = "macos") {
            "Mac".into()
        } else if cfg!(windows) {
            "Windows PC".into()
        } else {
            "Asterism".into()
        }
    })
}
