mod chat;
mod commands;
mod db;
mod discovery;
mod state;

use db::Database;
use log::info;
use std::sync::Arc;
use tauri::Manager;
use tokio::sync::Mutex;

use state::AppState;

pub fn run() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .setup(move |app| {
            let listen_port = std::env::var("ECHO_PORT")
                .ok()
                .and_then(|value| value.parse::<u16>().ok())
                .unwrap_or(9527);

            let app_data_dir = std::env::var("ECHO_DATA_DIR")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| {
                    app.path()
                        .app_data_dir()
                        .expect("Failed to get app data dir")
                });
            std::fs::create_dir_all(&app_data_dir).ok();

            let db_path = app_data_dir.join("echo.db");
            let db_path_str = db_path.to_string_lossy().to_string();

            let db = Arc::new(tauri::async_runtime::block_on(async {
                Database::new(&db_path_str)
                    .await
                    .expect("Failed to initialize database")
            }));

            let profile = tauri::async_runtime::block_on(async {
                db.get_user_profile()
                    .await
                    .expect("Failed to load user profile")
            });

            let runtime_services = if let Some(profile) = profile.as_ref() {
                let runtime = tauri::async_runtime::block_on(async {
                    state::RuntimeServices::start(Arc::clone(&db), profile, listen_port)
                        .await
                        .expect("Failed to start runtime services")
                });
                info!("Runtime started with saved profile: {}", profile.username);
                Some(runtime)
            } else {
                info!("No saved profile found, waiting for first-time setup.");
                None
            };

            app.manage(AppState {
                db,
                profile: Mutex::new(profile),
                runtime: Mutex::new(runtime_services),
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_app_info,
            commands::get_departments,
            commands::save_profile,
            commands::list_stored_peers,
            commands::get_peers,
            commands::send_message,
            commands::send_file,
            commands::get_conversation,
            commands::mark_read,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Echo");
}
