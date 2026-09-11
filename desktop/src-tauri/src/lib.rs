//! Tauri application entry point.
//! Commands (invoke handlers) live in `commands.rs`; the profiles file
//! watcher and the file logger are started in `setup`.

pub mod commands;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        // Opens external links (e.g. the linux.do friend link on the
        // About page) in the system browser.
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            commands::list_profiles,
            commands::get_profile,
            commands::save_profile,
            commands::delete_profile,
            commands::test_connection,
            commands::fetch_models,
            commands::launch_profile,
            commands::login_profile,
            commands::list_sessions,
            commands::resume_session,
            commands::set_session_pinned,
            commands::set_session_title,
            commands::delete_session,
            commands::clear_unpinned_sessions,
            commands::get_status,
            commands::get_settings,
            commands::save_settings,
            commands::get_logs,
            commands::detect_terminals,
        ])
        .setup(|app| {
            // File logging for troubleshooting (the GUI's log viewer and
            // the CLI `logs` command read the same file).
            agent_switch_core::logging::init(
                &agent_switch_core::profile_store::config_root(),
            );
            commands::spawn_profile_watcher(app.handle().clone());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Agent Switch");
}
