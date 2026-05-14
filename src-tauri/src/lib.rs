mod chat;
mod commands;
mod db;
mod discovery;
mod state;

use db::Database;
use log::info;
use crate::discovery::Peer;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;
use tauri::{Emitter, Manager};
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
                db: db.clone(),
                profile: Mutex::new(profile),
                runtime: Mutex::new(runtime_services),
            });

            // Mark all stored peers as offline at startup
            let db_for_offline = db.clone();
            tauri::async_runtime::spawn(async move {
                let _ = db_for_offline.mark_all_peers_offline().await;
            });

            // Heartbeat-based online status sync, every 5 seconds
            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                // Allow time for initial discovery before first check
                tokio::time::sleep(Duration::from_secs(10)).await;

                loop {
                    if let Some(state) = app_handle.try_state::<AppState>() {
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs() as i64;

                        // Re-read peers fresh each cycle to avoid stale last_seen
                        let snapshot: Vec<(String, String, String, IpAddr, u16, bool, i64)> =
                            if let Some(runtime) = state.runtime.lock().await.as_ref() {
                                runtime.discovery.lock().await.get_peers()
                                    .into_iter()
                                    .map(|p| (p.id, p.username, p.department, p.ip, p.port, p.online, p.last_seen))
                                    .collect()
                            } else {
                                vec![]
                            };

                        for (id, username, department, ip, port, current_online, last_seen) in &snapshot {
                            let should_be_online = (now - last_seen) < 15;

                            if *current_online != should_be_online {
                                // Update in-memory status
                                if let Some(runtime) = state.runtime.lock().await.as_ref() {
                                    runtime.discovery.lock().await.set_online(id, should_be_online);
                                }

                                // Persist to DB
                                let _ = state
                                    .db
                                    .upsert_peer(
                                        id,
                                        username,
                                        department,
                                        &ip.to_string(),
                                        *port,
                                        should_be_online,
                                    )
                                    .await;

                                // Emit to frontend
                                let updated = Peer::with_online(
                                    id.clone(),
                                    username.clone(),
                                    department.clone(),
                                    *ip,
                                    *port,
                                    should_be_online,
                                    *last_seen,
                                );
                                let _ = app_handle.emit("peer-discovered", &updated);
                            }
                        }
                    }
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
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
            commands::get_unread_counts,
            commands::save_temp_file,
            commands::read_file_base64,
            commands::open_file,
            commands::open_folder,
            commands::search_messages,
            commands::check_peer_online,
            commands::discover_by_ip,
            commands::get_scan_subnets,
            commands::set_scan_subnets,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Echo");
}
