use crate::ForegroundApp;

/// Clipboard Owner / 前台进程推断。Best Effort，不能识别应用内部模式。
pub fn foreground_app() -> ForegroundApp {
    ForegroundApp::default()
}
