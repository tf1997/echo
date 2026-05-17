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
        .plugin(tauri_plugin_dialog::init())
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

            // Load stored peers from DB into DiscoveryService memory on startup
            let db_for_load = db.clone();
            let app_handle_for_load = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let stored = db_for_load.list_stored_peers().await.unwrap_or_default();
                if stored.is_empty() {
                    return;
                }
                // Wait until state is available
                tokio::time::sleep(Duration::from_secs(1)).await;
                if let Some(state) = app_handle_for_load.try_state::<AppState>() {
                    if let Some(runtime) = state.runtime.lock().await.as_ref() {
                        let disc = runtime.discovery.lock().await;
                        for sp in &stored {
                            if let Ok(ip) = sp.ip.parse::<IpAddr>() {
                                let peer = Peer::new(
                                    sp.peer_id.clone(),
                                    sp.username.clone(),
                                    sp.department.clone(),
                                    ip,
                                    sp.port,
                                );
                                disc.register_peer(peer);
                            }
                        }
                        info!("Loaded {} stored peer(s) into memory", stored.len());
                    }
                }
            });

            // Health check: TCP connect refreshes last_seen, timeout marks offline
            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                use tokio::net::TcpStream;

                loop {
                    if let Some(state) = app_handle.try_state::<AppState>() {
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs() as i64;

                        let snapshot: Vec<(String, String, String, IpAddr, u16)> =
                            if let Some(runtime) = state.runtime.lock().await.as_ref() {
                                runtime.discovery.lock().await.get_peers()
                                    .into_iter()
                                    .map(|p| (p.id, p.username, p.department, p.ip, p.port))
                                    .collect()
                            } else {
                                vec![]
                            };

                        log::info!("HealthCheck cycle: {} peer(s)", snapshot.len());

                        for (id, username, department, ip, port) in &snapshot {
                            let addr = format!("{}:{}", ip, port);
                            let tcp_ok = tokio::time::timeout(
                                Duration::from_secs(2),
                                TcpStream::connect(&addr),
                            )
                            .await
                            .map(|r| r.is_ok())
                            .unwrap_or(false);

                            if tcp_ok {
                                // TCP success → peer is alive, refresh last_seen
                                if let Some(runtime) = state.runtime.lock().await.as_ref() {
                                    runtime.discovery.lock().await.touch_peer(id);
                                }
                                let _ = state.db.upsert_peer(id, username, department, &ip.to_string(), *port, true).await;
                                // Deliver pending group messages
                                let db = state.db.clone();
                                let pid = id.clone();
                                tauri::async_runtime::spawn(async move {
                                    crate::commands::deliver_pending_to_peer(&db, &pid).await;
                                });
                                log::debug!("HealthCheck: {} TCP OK → online", id);
                            } else {
                                // TCP fail → check if last_seen is too old
                                let should_offline = if let Some(runtime) = state.runtime.lock().await.as_ref() {
                                    let disc = runtime.discovery.lock().await;
                                    disc.get_peer(id)
                                        .map(|p| p.online && (now - p.last_seen) >= 15)
                                        .unwrap_or(false)
                                } else {
                                    false
                                };

                                if should_offline {
                                    if let Some(runtime) = state.runtime.lock().await.as_ref() {
                                        runtime.discovery.lock().await.set_online(id, false);
                                    }
                                    let _ = state.db.upsert_peer(id, username, department, &ip.to_string(), *port, false).await;
                                    let updated = Peer::with_online(id.clone(), username.clone(), department.clone(), *ip, *port, false, now);
                                    let _ = app_handle.emit("peer-discovered", &updated);
                                    log::info!("HealthCheck: {} → OFFLINE (tcp failed, age>15s)", username);
                                }
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
            commands::discover_by_ip,
            commands::get_scan_subnets,
            commands::set_scan_subnets,
            commands::list_emoji_files,
            commands::add_emoji_file,
            commands::list_recent_contacts,
            commands::remove_recent_contact,
            commands::create_group,
            commands::list_groups,
            commands::send_group_message,
            commands::send_group_file,
            commands::get_group_messages,
            commands::rename_group,
            commands::leave_group,
            commands::invite_to_group,
            commands::dissolve_group,
            commands::get_group_unread_counts,
            commands::mark_group_read,
            commands::deliver_pending,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Echo");
}
