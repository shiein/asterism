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
    #[serde(default)]
    pub webdav: Option<asterism_sync::WebdavConfig>,
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
            webdav: None,
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

    #[allow(dead_code)]
    pub fn webdav_ready(&self) -> bool {
        self.auto_sync && self.webdav.as_ref().is_some_and(|w| w.enabled && !w.url.is_empty())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShortcutSettings {
    #[serde(default = "default_shortcut_toggle_window")]
    pub toggle_window: String,
    #[serde(default = "default_shortcut_capture_region")]
    pub capture_region: String,
    #[serde(default = "default_shortcut_capture_fullscreen")]
    pub capture_fullscreen: String,
    #[serde(default = "default_shortcut_record_gif")]
    pub record_gif: String,
    #[serde(default = "default_shortcut_record_video")]
    pub record_video: String,
}

fn default_shortcut_toggle_window() -> String {
    "Alt+V".into()
}

fn default_shortcut_capture_region() -> String {
    "Alt+A".into()
}

fn default_shortcut_capture_fullscreen() -> String {
    "Alt+S".into()
}

fn default_shortcut_record_gif() -> String {
    "Alt+G".into()
}

fn default_shortcut_record_video() -> String {
    "Alt+R".into()
}

impl Default for ShortcutSettings {
    fn default() -> Self {
        Self {
            toggle_window: default_shortcut_toggle_window(),
            capture_region: default_shortcut_capture_region(),
            capture_fullscreen: default_shortcut_capture_fullscreen(),
            record_gif: default_shortcut_record_gif(),
            record_video: default_shortcut_record_video(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppSettings {
    #[serde(default = "default_true")]
    pub close_to_tray: bool,
    #[serde(default = "default_true")]
    pub minimize_to_tray: bool,
    #[serde(default = "default_false")]
    pub autostart: bool,
    #[serde(default)]
    pub shortcuts: ShortcutSettings,
}

fn default_false() -> bool {
    false
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            close_to_tray: true,
            minimize_to_tray: true,
            autostart: false,
            shortcuts: ShortcutSettings::default(),
        }
    }
}

impl AppSettings {
    pub fn path(config_dir: &Path) -> std::path::PathBuf {
        config_dir.join("app_settings.toml")
    }

    pub fn load(config_dir: &Path) -> Self {
        match Self::try_load(config_dir) {
            Ok(settings) => settings,
            Err(err) => {
                tracing::warn!(error = %err, "using default app settings");
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
                format!("invalid app_settings.toml: {err}"),
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
}
