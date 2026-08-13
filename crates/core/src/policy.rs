use serde::{Deserialize, Serialize};

use crate::content::{ContentFlags, ContentKind};
use crate::error::{CoreError, Result};

/// Apple Universal Clipboard 并存策略。无法可靠识别变化来源。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UniversalClipboardMode {
    #[default]
    AutoBridge,
    ReceiveOnly,
    ManualSend,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppExclusion {
    /// 可执行名或 bundle id。来源识别为 Best Effort。
    pub pattern: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CapturePolicy {
    pub respect_sensitive_flags: bool,
    pub excluded_apps: Vec<AppExclusion>,
    pub uc_mode: UniversalClipboardMode,
}

impl Default for CapturePolicy {
    fn default() -> Self {
        Self {
            respect_sensitive_flags: true,
            excluded_apps: default_sensitive_apps(),
            uc_mode: UniversalClipboardMode::AutoBridge,
        }
    }
}

impl CapturePolicy {
    pub fn is_excluded_app(&self, source_app: Option<&str>) -> bool {
        let Some(app) = source_app else {
            return false;
        };
        let app_l = app.to_ascii_lowercase();
        self.excluded_apps.iter().any(|ex| {
            let pat = ex.pattern.to_ascii_lowercase();
            app_l == pat || app_l.contains(&pat)
        })
    }
}

fn default_sensitive_apps() -> Vec<AppExclusion> {
    ["1password", "keepassxc", "bitwarden", "lastpass"]
        .into_iter()
        .map(|p| AppExclusion { pattern: p.to_string() })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteLimits {
    pub max_file_bytes: u64,
    pub max_item_bytes: u64,
    pub max_file_count: u64,
}

impl Default for RemoteLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: 100 * 1024 * 1024,
            max_item_bytes: 500 * 1024 * 1024,
            max_file_count: 10_000,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemotePolicy {
    pub allow_text: bool,
    pub allow_image: bool,
    pub allow_file: bool,
    pub limits: RemoteLimits,
}

impl Default for RemotePolicy {
    fn default() -> Self {
        Self {
            allow_text: true,
            allow_image: true,
            allow_file: true,
            limits: RemoteLimits::default(),
        }
    }
}

impl RemotePolicy {
    pub fn allows_kind(&self, kind: ContentKind) -> bool {
        match kind {
            ContentKind::Text => self.allow_text,
            ContentKind::Image | ContentKind::Screenshot | ContentKind::Gif => self.allow_image,
            ContentKind::Files => self.allow_file,
            ContentKind::Video => self.allow_file,
            ContentKind::AiResult | ContentKind::OcrResult => false,
        }
    }

    pub fn check_preflight(
        &self,
        kind: ContentKind,
        file_count: u64,
        logical_size: u64,
    ) -> Result<()> {
        if !self.allows_kind(kind) {
            return Err(CoreError::PolicyRejected("kind disabled for remote"));
        }
        if file_count > self.limits.max_file_count {
            return Err(CoreError::PolicyRejected("file count exceeds remote limit"));
        }
        if logical_size > self.limits.max_item_bytes {
            return Err(CoreError::PolicyRejected("item size exceeds remote limit"));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SensitiveDecision {
    Allow,
    /// 识别到系统/密码管理器敏感标志。
    MarkedSensitive,
    /// 命中应用排除名单（Best Effort）。
    ExcludedApp,
}

impl SensitiveDecision {
    pub fn should_ignore(self) -> bool {
        !matches!(self, Self::Allow)
    }

    pub fn flags(self) -> ContentFlags {
        match self {
            Self::Allow => ContentFlags::empty(),
            Self::MarkedSensitive | Self::ExcludedApp => {
                ContentFlags::SENSITIVE | ContentFlags::LOCAL_ONLY
            }
        }
    }
}

/// 目录预检硬上限，防止百万文件拖死后台线程。与 RemoteLimits 独立。
pub const LOCAL_MAX_ENUMERATION_ENTRIES: u64 = 100_000;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_excludes_password_managers() {
        let p = CapturePolicy::default();
        assert!(p.is_excluded_app(Some("1Password")));
        assert!(p.is_excluded_app(Some("com.keepassxc.keepassxc")));
        assert!(!p.is_excluded_app(Some("Safari")));
        assert!(!p.is_excluded_app(None));
    }

    #[test]
    fn remote_preflight_rejects_oversize_before_read() {
        let policy = RemotePolicy::default();
        let err = policy.check_preflight(ContentKind::Files, 1, policy.limits.max_item_bytes + 1);
        assert!(err.is_err());
    }
}
