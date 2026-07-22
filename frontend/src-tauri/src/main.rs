// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let mode = std::env::args().nth(1);
    if mode.as_deref() == Some("--azookey-updater-integration-test") {
        match frontend_lib::run_updater_integration_test() {
            Ok(_) => return,
            Err(error) => {
                eprintln!("updater integration test failed: {error}");
                std::process::exit(1);
            }
        }
    }
    if mode.as_deref() == Some("--azookey-apply-update") {
        if let Err(error) = frontend_lib::run_updater_helper() {
            eprintln!("updater helper failed: {error}");
            std::process::exit(1);
        }
        return;
    }

    frontend_lib::run()
}
