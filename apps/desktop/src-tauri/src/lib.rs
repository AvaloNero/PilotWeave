mod account;
pub mod adapters;
mod commands;
pub mod decimal;
mod deployment;
pub mod domain;
pub mod error;
mod fingerprint;
pub mod github_auth;
pub mod github_billing;
pub mod github_billing_store;
mod installer;
mod native_process;
mod platform;
mod redact;
mod safe_file;
mod safe_io;
mod secrets;
mod setup;
mod state;
#[cfg(feature = "local-e2e")]
mod test_support;
mod transaction;
mod usage;
pub mod usage_db;
mod validation;
mod write_lock;

use account::LoginStore;
use commands::ManagedState;
use github_auth::GithubAuthorizationStore;
use redact::redact_text;
use state::StateStore;
use usage_db::UsageDb;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(feature = "local-e2e")]
    if let Err(error) = test_support::initialize() {
        eprintln!(
            "Isolated validation initialization failed: {}",
            redact::redact_text(&error.to_string())
        );
        std::process::exit(2);
    }
    #[cfg(not(feature = "local-e2e"))]
    if std::env::args()
        .skip(1)
        .any(|a| a.starts_with("--local-e2e"))
    {
        eprintln!("Validation arguments are not supported in this build");
        std::process::exit(2);
    }
    let context = tauri::generate_context!();
    #[cfg(feature = "local-e2e")]
    let context = {
        let mut context = context;
        context.config_mut().identifier = "dev.pilotweave.local-validation".into();
        // Build the test window in setup so its absolute WebView directory is native-owned.
        context.config_mut().app.windows.clear();
        context
    };
    let store = StateStore::open().unwrap_or_else(StateStore::unavailable);
    let login_store = LoginStore::open();
    let github_authorization = GithubAuthorizationStore::open();
    // The usage database is isolated from connection management: when it
    // cannot be opened the app still runs and reports the unavailable state.
    let (usage_db, usage_db_error) = match UsageDb::open().and_then(|db| {
        usage::importer::initialize(&db)?;
        usage::store::recover(&db)?;
        Ok(db)
    }) {
        Ok(db) => (Some(db), None),
        Err(error) => (None, Some(redact_text(&error.to_string()))),
    };
    tauri::Builder::default()
        .setup(|app| {
            #[cfg(feature = "local-e2e")]
            tauri::WebviewWindowBuilder::new(
                app,
                "main",
                tauri::WebviewUrl::App("index.html".into()),
            )
            .title("PilotWeave — ISOLATED VALIDATION")
            .inner_size(1180.0, 820.0)
            .data_directory(
                test_support::root()
                    .expect("validated context")
                    .join("webview"),
            )
            .build()?;
            #[cfg(not(feature = "local-e2e"))]
            app.handle()
                .plugin(tauri_plugin_log::Builder::new().build())?;
            Ok(())
        })
        .plugin(tauri_plugin_opener::init())
        .manage(ManagedState::new(
            store,
            login_store,
            github_authorization,
            usage_db,
            usage_db_error,
        ))
        .invoke_handler(tauri::generate_handler![
            commands::get_dashboard,
            commands::get_installation_status,
            commands::preview_install,
            commands::apply_install_plan,
            commands::get_account_status,
            commands::preview_login,
            commands::apply_login_plan,
            commands::get_github_authorization_status,
            commands::get_github_billing_overview,
            commands::refresh_github_billing,
            commands::authorize_github,
            commands::refresh_github_authorization,
            commands::clear_github_authorization,
            commands::upsert_connection,
            commands::delete_connection,
            commands::preview_deployment,
            commands::apply_deployment_plan,
            commands::preview_deployment_recovery,
            commands::apply_deployment_recovery,
            usage::commands::get_usage_overview,
            usage::commands::get_usage_sources,
            usage::commands::get_usage_runs,
            usage::commands::set_usage_source_enabled,
            usage::commands::clear_local_usage,
            usage::commands::sync_local_usage,
            usage::commands::cancel_usage_sync,
            usage::commands::get_price_catalog,
            usage::commands::refresh_price_catalog,
            usage::commands::get_official_runtime_usage,
            usage::commands::refresh_official_runtime_usage,
            usage::commands::get_github_billing,
            usage::commands::refresh_personal_github_usage,
            setup::get_setup_status,
            setup::select_setup_connection,
            setup::confirm_account_alignment,
            setup::confirm_manual_provider,
        ])
        .run(context)
        .expect("error while running PilotWeave");
}
