use asterism_core::policy::{CapturePolicy, SensitiveDecision};

use crate::capture::CapturedClipboard;

pub const MACOS_CONCEALED: &str = "org.nspasteboard.ConcealedType";
pub const WIN_EXCLUDE_MONITOR: &str = "ExcludeClipboardContentFromMonitorProcessing";
pub const WIN_NO_HISTORY: &str = "CanIncludeInClipboardHistory";
pub const WIN_NO_CLOUD: &str = "CanUploadToCloudClipboard";

pub fn decide(captured: &CapturedClipboard, policy: &CapturePolicy) -> SensitiveDecision {
    if policy.respect_sensitive_flags
        && (captured.sensitive || has_sensitive_format(&captured.formats))
    {
        return SensitiveDecision::MarkedSensitive;
    }
    let app = captured.source_app.as_deref();
    if policy.is_excluded_app(app) {
        return SensitiveDecision::ExcludedApp;
    }
    SensitiveDecision::Allow
}

fn has_sensitive_format(formats: &[String]) -> bool {
    formats.iter().any(|f| {
        let l = f.to_ascii_lowercase();
        l.contains(&MACOS_CONCEALED.to_ascii_lowercase())
            || l.contains(&WIN_EXCLUDE_MONITOR.to_ascii_lowercase())
            || l.eq_ignore_ascii_case(WIN_NO_HISTORY)
            || l.eq_ignore_ascii_case(WIN_NO_CLOUD)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concealed_type_is_sensitive() {
        let captured = CapturedClipboard {
            change_token: 1,
            source_app: Some("Safari".into()),
            formats: vec![MACOS_CONCEALED.into()],
            text: Some("x".into()),
            image: None,
            files: vec![],
            sensitive: false,
        };
        assert_eq!(
            decide(&captured, &CapturePolicy::default()),
            SensitiveDecision::MarkedSensitive
        );
    }
}
