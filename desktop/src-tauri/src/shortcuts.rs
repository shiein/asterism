use crate::settings::ShortcutSettings;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

static CAPTURE_ACTIVE: AtomicBool = AtomicBool::new(false);

pub fn setup_shortcuts(app: &AppHandle) -> anyhow::Result<()> {
    let state = app.state::<crate::runtime::DesktopState>();
    let shortcuts = state.app_settings.read().shortcuts.clone();
    if let Err(err) = register_shortcuts(app, &shortcuts) {
        tracing::warn!(error = %err, "failed to register global shortcuts on startup");
    }
    Ok(())
}

pub fn register_shortcuts(app: &AppHandle, shortcuts: &ShortcutSettings) -> Result<(), String> {
    // 1. 先验证所有快捷键格式，避免部分成功部分失败导致状态不一致
    let parse_or_none = |s: &str| -> Result<Option<Shortcut>, String> {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }
        Shortcut::from_str(trimmed)
            .map(Some)
            .map_err(|err| format!("无效快捷键 '{trimmed}': {err}"))
    };

    let items = [
        (parse_or_none(&shortcuts.toggle_window)?, "toggle_window"),
        (parse_or_none(&shortcuts.capture_region)?, "capture_region"),
        (parse_or_none(&shortcuts.capture_fullscreen)?, "capture_fullscreen"),
        (parse_or_none(&shortcuts.record_gif)?, "record_gif"),
        (parse_or_none(&shortcuts.record_video)?, "record_video"),
    ];

    let global_shortcut = app.global_shortcut();
    let _ = global_shortcut.unregister_all();

    for (parsed, action_id) in items {
        if let Some(shortcut) = parsed {
            let app_handle = app.clone();
            global_shortcut
                .on_shortcut(shortcut, move |_app, _shortcut, event| {
                    if event.state() == ShortcutState::Pressed {
                        handle_shortcut_action(&app_handle, action_id);
                    }
                })
                .map_err(|err| format!("注册快捷键失败: {err}"))?;
        }
    }

    Ok(())
}

pub fn handle_shortcut_action(app: &AppHandle, action_id: &str) {
    match action_id {
        "toggle_window" => {
            if let Some(window) = app.get_webview_window("main") {
                if window.is_visible().unwrap_or(false) && window.is_focused().unwrap_or(false) {
                    let _ = window.hide();
                } else {
                    let _ = window.show();
                    let _ = window.unminimize();
                    let _ = window.set_focus();
                }
            }
        }
        "capture_region" => {
            if CAPTURE_ACTIVE.swap(true, Ordering::SeqCst) {
                return;
            }
            let app_clone = app.clone();
            tauri::async_runtime::spawn(async move {
                let state = app_clone.state::<crate::runtime::DesktopState>();
                if let Err(err) = crate::commands::capture_region(app_clone.clone(), state).await {
                    tracing::error!(error = %err, "shortcut capture_region failed");
                    let _ = app_clone.emit("capture:error", err.to_string());
                }
                CAPTURE_ACTIVE.store(false, Ordering::SeqCst);
            });
        }
        "capture_fullscreen" => {
            if CAPTURE_ACTIVE.swap(true, Ordering::SeqCst) {
                return;
            }
            let app_clone = app.clone();
            tauri::async_runtime::spawn(async move {
                let state = app_clone.state::<crate::runtime::DesktopState>();
                if let Err(err) =
                    crate::commands::capture_fullscreen(app_clone.clone(), state).await
                {
                    tracing::error!(error = %err, "shortcut capture_fullscreen failed");
                    let _ = app_clone.emit("capture:error", err.to_string());
                }
                CAPTURE_ACTIVE.store(false, Ordering::SeqCst);
            });
        }
        "record_gif" => {
            let app_clone = app.clone();
            tauri::async_runtime::spawn(async move {
                let state = app_clone.state::<crate::runtime::DesktopState>();
                if let Err(err) =
                    crate::capture_cmds::record_gif(app_clone.clone(), state, 15).await
                {
                    tracing::error!(error = %err, "shortcut record_gif failed");
                    let _ = app_clone.emit("capture:error", err.to_string());
                }
            });
        }
        "record_video" => {
            let app_clone = app.clone();
            tauri::async_runtime::spawn(async move {
                let state = app_clone.state::<crate::runtime::DesktopState>();
                if let Err(err) =
                    crate::capture_cmds::record_video(app_clone.clone(), state, 30, None).await
                {
                    tracing::error!(error = %err, "shortcut record_video failed");
                    let _ = app_clone.emit("capture:error", err.to_string());
                }
            });
        }
        _ => {}
    }
}
