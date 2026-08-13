use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HubConfig {
    pub bind: String,
    pub data_dir: PathBuf,
    pub tls: TlsConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TlsConfig {
    pub cert: PathBuf,
    pub key: PathBuf,
}

impl HubConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("read config {}", path.display()))?;
        let config: Self = toml::from_str(&raw).context("parse config.toml")?;
        Ok(config)
    }

    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("hub.db")
    }

    pub fn blob_root(&self) -> PathBuf {
        self.data_dir.join("blobs")
    }
}
