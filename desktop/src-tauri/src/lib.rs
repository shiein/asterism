#![allow(clippy::too_many_arguments)]

mod actions;
mod capture_cmds;
mod capture_ui;
mod commands;
mod host;
mod overlay_cli;
mod plugins;
mod runtime;
mod settings;
mod shortcuts;
mod sync_engine;
mod tray;

use runtime::DesktopState;
use tauri::Manager;
use tracing_subscriber::EnvFilter;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run_overlay_select() -> i32 {
    overlay_cli::run_overlay_select()
}

pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .compact()
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .setup(|app| {
            if let Some(main_window) = app.get_webview_window("main") {
                capture_ui::apply_capture_exclusion(&main_window);
            }
            let state = DesktopState::start(app.handle().clone())?;
            app.manage(state);

            if let Err(err) = shortcuts::setup_shortcuts(app.handle()) {
                tracing::warn!(error = %err, "failed to setup shortcuts");
            }
            if let Err(err) = tray::setup_tray(app.handle()) {
                tracing::warn!(error = %err, "failed to setup system tray");
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event
                && window.label() == "main"
            {
                let app = window.app_handle();
                if let Some(state) = app.try_state::<DesktopState>()
                    && state.app_settings.read().close_to_tray
                {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_history,
            commands::set_favorite,
            commands::delete_item,
            commands::copy_item,
            commands::get_identity,
            commands::capture_permission_status,
            commands::open_screen_capture_settings,
            commands::execute_action,
            commands::list_actions,
            commands::recovery_key,
            commands::copy_recovery_key,
            commands::capture_fullscreen,
            commands::capture_region,
            commands::get_app_settings,
            commands::save_app_settings,
            commands::reset_shortcuts,
            commands::get_sync_settings,
            commands::save_sync_settings,
            commands::connect_hub,
            commands::hub_pairing_code,
            commands::hub_devices,
            commands::import_recovery,
            commands::enable_autostart,
            commands::publish_pairing_avk,
            commands::get_lan_peers,
            commands::trust_lan_peer,
            commands::untrust_lan_peer,
            commands::get_local_cert_fingerprint,
            commands::test_webdav,
            capture_cmds::list_windows,
            capture_cmds::capture_window,
            capture_cmds::preview_image,
            capture_cmds::annotation_source,
            capture_cmds::export_annotated,
            capture_cmds::record_gif,
            capture_cmds::record_video,
            capture_cmds::stop_recording,
            capture_cmds::scroll_capture,
        ])
        .run(tauri::generate_context!())
        .expect("failed to start Asterism desktop");
}
