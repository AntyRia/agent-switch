//! Tauri application entry point.
//! Commands (invoke handlers) live in `commands.rs`; the profiles file
//! watcher is started in `setup`.

pub mod commands;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            commands::list_profiles,
            commands::get_profile,
            commands::save_profile,
            commands::delete_profile,
            commands::test_connection,
            commands::fetch_models,
            commands::launch_profile,
            commands::list_sessions,
            commands::resume_session,
            commands::get_status,
        ])
        .setup(|app| {
            commands::spawn_profile_watcher(app.handle().clone());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Agent Switch");
}
