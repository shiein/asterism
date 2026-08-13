#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if std::env::args().any(|arg| arg == "--overlay-select") {
        std::process::exit(asterism_desktop_lib::run_overlay_select());
    }
    asterism_desktop_lib::run();
}
