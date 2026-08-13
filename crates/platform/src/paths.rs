use std::path::PathBuf;

use directories::ProjectDirs;

const QUALIFIER: &str = "dev";
const ORGANIZATION: &str = "asterism";
const APPLICATION: &str = "Asterism";
/// 与设计一致：macOS cache 使用 Bundle ID。
const BUNDLE_ID: &str = "dev.asterism.desktop";

#[derive(Clone, Debug)]
pub struct AppPaths {
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub config_dir: PathBuf,
}

impl AppPaths {
    pub fn detect() -> Self {
        if let Some(dirs) = ProjectDirs::from(QUALIFIER, ORGANIZATION, APPLICATION) {
            let cache_dir = if cfg!(target_os = "macos") {
                dirs.cache_dir()
                    .parent()
                    .map(|p| p.join(BUNDLE_ID))
                    .unwrap_or_else(|| dirs.cache_dir().to_path_buf())
            } else {
                dirs.cache_dir().to_path_buf()
            };
            return Self {
                data_dir: dirs.data_local_dir().to_path_buf(),
                cache_dir,
                config_dir: dirs.config_dir().to_path_buf(),
            };
        }
        let fallback = std::env::temp_dir().join("asterism");
        Self {
            data_dir: fallback.clone(),
            cache_dir: fallback.join("cache"),
            config_dir: fallback.join("config"),
        }
    }

    pub fn ensure(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.data_dir)?;
        std::fs::create_dir_all(self.cache_dir.join("items"))?;
        std::fs::create_dir_all(&self.config_dir)?;
        Ok(())
    }

    pub fn item_cache(&self, item_id: asterism_core::ContentId) -> PathBuf {
        self.cache_dir.join("items").join(item_id.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_is_non_empty() {
        let paths = AppPaths::detect();
        assert!(!paths.data_dir.as_os_str().is_empty());
        assert!(!paths.cache_dir.as_os_str().is_empty());
    }
}
