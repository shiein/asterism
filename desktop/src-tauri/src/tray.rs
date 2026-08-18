use tauri::AppHandle;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

pub fn setup_tray(app: &AppHandle) -> anyhow::Result<()> {
    let toggle_item =
        MenuItem::with_id(app, "toggle_window", "显示/隐藏 Asterism", true, None::<&str>)?;
    let region_item = MenuItem::with_id(app, "capture_region", "选区截图", true, None::<&str>)?;
    let fullscreen_item =
        MenuItem::with_id(app, "capture_fullscreen", "全屏截图", true, None::<&str>)?;
    let record_gif_item = MenuItem::with_id(app, "record_gif", "GIF 录制", true, None::<&str>)?;
    let record_video_item = MenuItem::with_id(app, "record_video", "屏幕录制", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let quit_item = MenuItem::with_id(app, "quit", "退出 Asterism", true, None::<&str>)?;

    let menu = Menu::with_items(
        app,
        &[
            &toggle_item,
            &region_item,
            &fullscreen_item,
            &record_gif_item,
            &record_video_item,
            &sep,
            &quit_item,
        ],
    )?;

    let mut builder = TrayIconBuilder::with_id("main-tray")
        .menu(&menu)
        .tooltip("Asterism 剪贴板与效率中心")
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "toggle_window" => {
                crate::shortcuts::handle_shortcut_action(app, "toggle_window");
            }
            "capture_region" => {
                crate::shortcuts::handle_shortcut_action(app, "capture_region");
            }
            "capture_fullscreen" => {
                crate::shortcuts::handle_shortcut_action(app, "capture_fullscreen");
            }
            "record_gif" => {
                crate::shortcuts::handle_shortcut_action(app, "record_gif");
            }
            "record_video" => {
                crate::shortcuts::handle_shortcut_action(app, "record_video");
            }
            "quit" => {
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                crate::shortcuts::handle_shortcut_action(app, "toggle_window");
            }
        });

    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    } else {
        // 兜底 16x16 纯色图标，防止在没有 icon 的环境中构建失败
        let fallback_rgba = [10u8, 132, 255, 255].repeat(16 * 16);
        let icon = tauri::image::Image::new(&fallback_rgba, 16, 16);
        builder = builder.icon(icon);
    }

    builder.build(app)?;
    Ok(())
}
