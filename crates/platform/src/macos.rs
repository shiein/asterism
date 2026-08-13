use objc2_app_kit::{NSRunningApplication, NSWorkspace};

use crate::ForegroundApp;

/// 结合 Pasteboard 变化时的前台应用推断来源。Best Effort。
pub fn foreground_app() -> ForegroundApp {
    // SAFETY: AppKit 查询当前前台应用；失败时返回空。
    let workspace = NSWorkspace::sharedWorkspace();
    let Some(app) = workspace.frontmostApplication() else {
        return ForegroundApp::default();
    };
    read_app(&app)
}

fn read_app(app: &NSRunningApplication) -> ForegroundApp {
    let name = app.localizedName().map(|s| s.to_string());
    let identifier = app.bundleIdentifier().map(|s| s.to_string());
    ForegroundApp { name, identifier }
}
