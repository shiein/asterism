#![allow(clippy::too_many_arguments)]

mod actions;
mod capture_cmds;
mod commands;
mod overlay_cli;
mod runtime;
mod settings;
mod sync_engine;

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
        .setup(|app| {
            let state = DesktopState::start(app.handle().clone())?;
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_history,
            commands::set_favorite,
            commands::delete_item,
            commands::copy_item,
            commands::get_identity,
            commands::execute_action,
            commands::recovery_key,
            commands::capture_fullscreen,
            commands::capture_region,
            commands::get_sync_settings,
            commands::save_sync_settings,
            commands::connect_hub,
            commands::hub_pairing_code,
            commands::hub_devices,
            commands::import_recovery,
            commands::enable_autostart,
            commands::publish_pairing_avk,
            capture_cmds::list_windows,
            capture_cmds::capture_window,
            capture_cmds::annotation_source,
            capture_cmds::export_annotated,
            capture_cmds::record_gif,
            capture_cmds::record_video,
            capture_cmds::scroll_capture,
        ])
        .run(tauri::generate_context!())
        .expect("failed to start Asterism desktop");
}
