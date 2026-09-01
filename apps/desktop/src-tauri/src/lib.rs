pub mod adapters;
mod commands;
pub mod decimal;
mod deployment;
pub mod domain;
pub mod error;
mod installer;
mod redact;
mod secrets;
mod state;
pub mod usage_db;

use commands::ManagedState;
use redact::redact_text;
use state::StateStore;
use usage_db::UsageDb;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let store = StateStore::open().expect("failed to initialize PilotWeave state");
    // The usage database is isolated from connection management: when it
    // cannot be opened the app still runs and reports the unavailable state.
    let (usage_db, usage_db_error) = match UsageDb::open() {
        Ok(db) => (Some(db), None),
        Err(error) => (None, Some(redact_text(&error.to_string()))),
    };
    tauri::Builder::default()
        .plugin(tauri_plugin_log::Builder::new().build())
        .plugin(tauri_plugin_opener::init())
        .manage(ManagedState::new(store, usage_db, usage_db_error))
        .invoke_handler(tauri::generate_handler![
            commands::get_dashboard,
            commands::get_installation_status,
            commands::preview_install,
            commands::apply_install_plan,
            commands::upsert_connection,
            commands::delete_connection,
            commands::preview_deployment,
            commands::apply_deployment,
            commands::apply_deployment_plan,
        ])
        .run(tauri::generate_context!())
        .expect("error while running PilotWeave");
}
