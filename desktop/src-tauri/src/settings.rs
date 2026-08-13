use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SyncSettings {
    pub hub_url: Option<String>,
    pub token: Option<String>,
    pub lan_port: u16,
    pub auto_sync: bool,
    #[serde(default = "default_true")]
    pub auto_receive: bool,
    #[serde(default)]
    pub pending_pair_code: Option<String>,
    #[serde(default)]
    pub pending_pair_salt: Option<String>,
}

fn default_true() -> bool {
    true
}

impl Default for SyncSettings {
    fn default() -> Self {
        Self {
            hub_url: None,
            token: None,
            lan_port: 47820,
            auto_sync: true,
            auto_receive: true,
            pending_pair_code: None,
            pending_pair_salt: None,
        }
    }
}

impl SyncSettings {
    pub fn path(config_dir: &Path) -> std::path::PathBuf {
        config_dir.join("sync.toml")
    }

    pub fn load(config_dir: &Path) -> Self {
        let path = Self::path(config_dir);
        let Ok(raw) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        toml::from_str(&raw).unwrap_or_default()
    }

    pub fn save(&self, config_dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(config_dir)?;
        std::fs::write(Self::path(config_dir), toml::to_string_pretty(self).unwrap_or_default())
    }

    pub fn hub_ready(&self) -> bool {
        self.auto_sync
            && self.hub_url.as_ref().is_some_and(|u| !u.is_empty())
            && self.token.is_some()
    }
}
