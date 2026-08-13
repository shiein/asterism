mod commands;
mod runtime;

use runtime::DesktopState;
use tauri::Manager;
use tracing_subscriber::EnvFilter;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
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
        ])
        .run(tauri::generate_context!())
        .expect("failed to start Asterism desktop");
}
