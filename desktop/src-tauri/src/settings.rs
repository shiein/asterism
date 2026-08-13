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
    #[serde(default)]
    pub hub_cert_sha256: Option<String>,
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
            hub_cert_sha256: None,
        }
    }
}

impl SyncSettings {
    pub fn path(config_dir: &Path) -> std::path::PathBuf {
        config_dir.join("sync.toml")
    }

    pub fn load(config_dir: &Path) -> Self {
        match Self::try_load(config_dir) {
            Ok(settings) => settings,
            Err(err) => {
                tracing::error!(error = %err, "refusing to overwrite a corrupt sync.toml");
                Self::default()
            }
        }
    }

    pub fn try_load(config_dir: &Path) -> std::io::Result<Self> {
        let path = Self::path(config_dir);
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(&path)?;
        toml::from_str(&raw).map_err(|err| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("invalid sync.toml: {err}"),
            )
        })
    }

    pub fn save(&self, config_dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(config_dir)?;
        let path = Self::path(config_dir);
        let bytes = toml::to_string_pretty(self)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err.to_string()))?;
        asterism_platform::atomic::atomic_write(&path, bytes.as_bytes())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }

    pub fn hub_ready(&self) -> bool {
        self.auto_sync
            && self.hub_url.as_ref().is_some_and(|u| !u.is_empty())
            && self.token.is_some()
    }
}
