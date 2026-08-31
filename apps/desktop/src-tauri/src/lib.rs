pub mod adapters;
mod commands;
pub mod domain;
pub mod error;
mod secrets;
mod state;

use commands::ManagedState;
use state::StateStore;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let store = StateStore::open().expect("failed to initialize PilotWeave state");
    tauri::Builder::default()
        .plugin(tauri_plugin_log::Builder::new().build())
        .plugin(tauri_plugin_opener::init())
        .manage(ManagedState::new(store))
        .invoke_handler(tauri::generate_handler![
            commands::get_dashboard,
            commands::upsert_connection,
            commands::delete_connection,
            commands::preview_deployment,
            commands::apply_deployment,
        ])
        .run(tauri::generate_context!())
        .expect("error while running PilotWeave");
}
