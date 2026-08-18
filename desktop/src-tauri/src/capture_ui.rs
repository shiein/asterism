use std::time::{Duration, SystemTime, UNIX_EPOCH};

use asterism_capture::{MonitorInfo, Selection};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::commands::CmdError;

const MAIN_WINDOW_LABEL: &str = "main";
const TOOLBAR_WINDOW_LABEL: &str = "capture-toolbar";
const TOOLBAR_WIDTH: f64 = 300.0;
const TOOLBAR_HEIGHT: f64 = 52.0;
const TOOLBAR_GAP: f64 = 12.0;

pub fn apply_capture_exclusion(window: &WebviewWindow) {
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{
            SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE,
        };
        if let Ok(hwnd) = window.hwnd() {
            unsafe {
                let _ = SetWindowDisplayAffinity(HWND(hwnd.0 as _), WDA_EXCLUDEFROMCAPTURE);
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        use objc2::msg_send;
        use objc2::runtime::AnyObject;
        if let Ok(ns_window) = window.ns_window() {
            unsafe {
                let win = ns_window as *mut AnyObject;
                if !win.is_null() {
                    const NS_WINDOW_ANIMATION_BEHAVIOR_NONE: isize = 2;
                    let _: () =
                        msg_send![win, setAnimationBehavior: NS_WINDOW_ANIMATION_BEHAVIOR_NONE];
                }
            }
        }
    }
}

pub struct HiddenMainWindow {
    window: WebviewWindow,
    restore: bool,
}

impl HiddenMainWindow {
    pub fn hide(app: &AppHandle) -> Result<Self, CmdError> {
        let window = app
            .get_webview_window(MAIN_WINDOW_LABEL)
            .ok_or_else(|| CmdError::Any("main window unavailable".into()))?;
        apply_capture_exclusion(&window);
        let restore = window.is_visible().map_err(|err| CmdError::Any(err.to_string()))?;
        if restore {
            window.hide().map_err(|err| CmdError::Any(err.to_string()))?;
        }
        Ok(Self { window, restore })
    }

    pub async fn wait_until_not_captured(&self) {
        if self.restore {
            // macOS 禁用过渡动画，Windows 启用 WDA_EXCLUDEFROMCAPTURE，
            // 仅需等待合成器单帧缓冲区刷洗（~15ms）即可完全消除残影与延迟。
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
    }
}

impl Drop for HiddenMainWindow {
    fn drop(&mut self) {
        if self.restore {
            if let Err(err) = self.window.show() {
                tracing::warn!(error = %err, "failed to restore main window after capture");
                return;
            }
            if let Err(err) = self.window.set_focus() {
                tracing::warn!(error = %err, "failed to focus main window after capture");
            }
        }
    }
}

pub struct RecordingToolbar {
    window: WebviewWindow,
}

impl RecordingToolbar {
    pub fn show(
        app: &AppHandle,
        monitor: &MonitorInfo,
        selection: &Selection,
        mode: &str,
        countdown: Duration,
    ) -> Result<(Self, std::time::Instant), CmdError> {
        if app.get_webview_window(TOOLBAR_WINDOW_LABEL).is_some() {
            return Err(CmdError::Any("recording toolbar is already open".into()));
        }
        let starts_at = std::time::Instant::now() + countdown;
        let starts_at_epoch_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .saturating_add(countdown)
            .as_millis();
        let (x, y) = toolbar_position(monitor, selection);
        let url = format!("index.html?captureToolbar=1&mode={mode}&startsAt={starts_at_epoch_ms}");
        let window =
            WebviewWindowBuilder::new(app, TOOLBAR_WINDOW_LABEL, WebviewUrl::App(url.into()))
                .title("Asterism Recording")
                .inner_size(TOOLBAR_WIDTH, TOOLBAR_HEIGHT)
                .position(x, y)
                .decorations(false)
                .resizable(false)
                .maximizable(false)
                .minimizable(false)
                .always_on_top(true)
                .visible_on_all_workspaces(true)
                .skip_taskbar(true)
                .content_protected(true)
                .shadow(true)
                .focused(true)
                .accept_first_mouse(true)
                .prevent_overflow()
                // 与前端 --hud-surface 同色，避免窗口底色与内容边缘不一致。
                .background_color(tauri::utils::config::Color(28, 28, 30, 255))
                .build()
                .map_err(|err| CmdError::Any(err.to_string()))?;
        Ok((Self { window }, starts_at))
    }
}

impl Drop for RecordingToolbar {
    fn drop(&mut self) {
        if let Err(err) = self.window.close() {
            tracing::warn!(error = %err, "failed to close recording toolbar");
        }
    }
}

fn toolbar_position(monitor: &MonitorInfo, selection: &Selection) -> (f64, f64) {
    let scale = monitor.scale_factor.max(1.0);
    let monitor_left = monitor.origin_logical.0;
    let monitor_top = monitor.origin_logical.1;
    let monitor_width = f64::from(monitor.capture_size.0) / scale;
    let monitor_height = f64::from(monitor.capture_size.1) / scale;
    let selection_left = monitor_left + selection.x / scale;
    let selection_top = monitor_top + selection.y / scale;
    let selection_width = selection.width / scale;
    let selection_height = selection.height / scale;

    let max_x = (monitor_left + monitor_width - TOOLBAR_WIDTH).max(monitor_left);
    let x = (selection_left + selection_width - TOOLBAR_WIDTH).clamp(monitor_left, max_x);
    let below = selection_top + selection_height + TOOLBAR_GAP;
    let above = selection_top - TOOLBAR_HEIGHT - TOOLBAR_GAP;
    let max_y = (monitor_top + monitor_height - TOOLBAR_HEIGHT).max(monitor_top);
    let y = if below <= max_y {
        below
    } else if above >= monitor_top {
        above
    } else {
        max_y
    };
    (x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor() -> MonitorInfo {
        MonitorInfo {
            id: 1,
            name: "test".into(),
            origin_physical: (0, 0),
            origin_logical: (0.0, 0.0),
            scale_factor: 2.0,
            capture_size: (2880, 1800),
        }
    }

    #[test]
    fn toolbar_prefers_space_below_selection() {
        let pos = toolbar_position(
            &monitor(),
            &Selection { x: 200.0, y: 100.0, width: 1000.0, height: 500.0 },
        );
        assert_eq!(pos, (300.0, 312.0));
    }

    #[test]
    fn toolbar_stays_inside_monitor_for_fullscreen_selection() {
        let (x, y) = toolbar_position(
            &monitor(),
            &Selection { x: 0.0, y: 0.0, width: 2880.0, height: 1800.0 },
        );
        assert!((0.0..=(1440.0 - TOOLBAR_WIDTH)).contains(&x));
        assert!((0.0..=(900.0 - TOOLBAR_HEIGHT)).contains(&y));
    }
}
