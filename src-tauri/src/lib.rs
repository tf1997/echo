mod chat;
mod commands;
mod db;
mod discovery;
mod state;

use db::Database;
use log::info;
use crate::discovery::Peer;
use std::sync::Arc;
use std::time::Duration;
use tauri::{Emitter, Manager};
use tokio::net::TcpStream;
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

            // Continuous TCP health check + discovery sync, every 8 seconds
            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    if let Some(state) = app_handle.try_state::<AppState>() {
                        let peers = if let Some(runtime) = state.runtime.lock().await.as_ref() {
                            runtime.discovery.lock().await.get_peers()
                        } else {
                            vec![]
                        };

                        // Parallel TCP health checks
                        let checks: Vec<_> = peers.iter().map(|peer| {
                            let addr = format!("{}:{}", peer.ip, peer.port);
                            let peer_id = peer.id.clone();
                            async move {
                                let online = tokio::time::timeout(
                                    Duration::from_secs(2),
                                    TcpStream::connect(&addr),
                                )
                                .await
                                .map(|r| r.is_ok())
                                .unwrap_or(false);
                                (peer_id, online, addr)
                            }
                        }).collect();

                        let results = futures::future::join_all(checks).await;

                        if let Some(runtime) = state.runtime.lock().await.as_ref() {
                            for (peer_id, online, _addr) in &results {
                                runtime.discovery.lock().await.set_online(peer_id, *online);
                            }
                        }

                        for (peer_id, online, _) in &results {
                            if let Some(peer) = peers.iter().find(|p| &p.id == peer_id) {
                                let _ = state
                                    .db
                                    .upsert_peer(
                                        &peer.id,
                                        &peer.username,
                                        &peer.department,
                                        &peer.ip.to_string(),
                                        peer.port,
                                        *online,
                                    )
                                    .await;
                                let updated = Peer::with_online(
                                    peer.id.clone(),
                                    peer.username.clone(),
                                    peer.department.clone(),
                                    peer.ip,
                                    peer.port,
                                    *online,
                                    peer.last_seen,
                                );
                                let _ = app_handle.emit("peer-discovered", &updated);
                            }
                        }
                    }
                    tokio::time::sleep(Duration::from_secs(8)).await;
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running Echo");
}
