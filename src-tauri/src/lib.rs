mod chat;
mod commands;
mod contact_sync;
mod db;
mod discovery;
mod state;
mod tray;
mod windows_event_log;
pub mod updater;

use db::Database;
use crate::discovery::{Peer, PeerEntry};
use log::info;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tauri::api::dialog::{
    blocking::MessageDialogBuilder, MessageDialogButtons, MessageDialogKind,
};
use tauri::{CustomMenuItem, Manager, Menu, Submenu};
use tokio::sync::{mpsc, Mutex, RwLock};

use state::AppState;

const MENU_CHECK_UPDATE: &str = "check_update";

fn app_menu() -> Menu {
    Menu::os_default("Echo").add_submenu(Submenu::new(
        "帮助",
        Menu::new().add_item(CustomMenuItem::new(MENU_CHECK_UPDATE, "检查更新")),
    ))
}

fn startup_log_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("ECHO_DATA_DIR") {
        return PathBuf::from(dir);
    }

    #[cfg(windows)]
    {
        if let Ok(dir) = std::env::var("LOCALAPPDATA").or_else(|_| std::env::var("APPDATA")) {
            return PathBuf::from(dir).join("Echo");
        }
    }

    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".echo");
    }

    std::env::temp_dir().join("echo")
}

fn append_crash_log(log_dir: &Path, message: &str) {
    let timestamp = chrono::Local::now().to_rfc3339();
    let entry = format!("[{}] {}\n", timestamp, message);

    let _ = std::fs::create_dir_all(log_dir);
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_dir.join("crash.log"))
        .and_then(|mut file| {
            use std::io::Write;
            file.write_all(entry.as_bytes())
        });

    windows_event_log::write_error(&entry);
}

fn install_panic_logger(log_dir: PathBuf) {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let payload = panic_info
            .payload()
            .downcast_ref::<&str>()
            .map(|value| (*value).to_string())
            .or_else(|| {
                panic_info
                    .payload()
                    .downcast_ref::<String>()
                    .cloned()
            })
            .unwrap_or_else(|| "unknown panic payload".to_string());
        let location = panic_info
            .location()
            .map(|loc| format!("{}:{}:{}", loc.file(), loc.line(), loc.column()))
            .unwrap_or_else(|| "unknown location".to_string());

        append_crash_log(&log_dir, &format!("panic at {}: {}", location, payload));
        default_hook(panic_info);
    }));
}

fn init_file_logger(log_dir: &Path) {
    if let Err(error) = std::fs::create_dir_all(log_dir) {
        append_crash_log(log_dir, &format!("failed to create log dir: {}", error));
    }

    let logger = match flexi_logger::Logger::try_with_str("info") {
        Ok(logger) => logger,
        Err(error) => {
            append_crash_log(log_dir, &format!("failed to parse log level: {}", error));
            return;
        }
    };

    if let Err(error) = logger
        .log_to_file(
            flexi_logger::FileSpec::default()
                .directory(log_dir)
                .basename("echo"),
        )
        .rotate(
            flexi_logger::Criterion::Size(5 * 1024 * 1024),
            flexi_logger::Naming::Numbers,
            flexi_logger::Cleanup::KeepLogFiles(0),
        )
        .duplicate_to_stderr(flexi_logger::Duplicate::All)
        .format_for_files(flexi_logger::detailed_format)
        .format_for_stderr(flexi_logger::colored_default_format)
        .start()
    {
        append_crash_log(log_dir, &format!("failed to start logger: {}", error));
    }
}

#[cfg(windows)]
fn configure_windows_webview2_runtime() {
    const FIXED_RUNTIME_DIR: &str = "WebView2Runtime";
    const FIXED_RUNTIME_PREFIX: &str = "Microsoft.WebView2.FixedVersionRuntime";
    const WEBVIEW2_EXE: &str = "msedgewebview2.exe";

    if std::env::var_os("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER").is_some() {
        return;
    }

    let Ok(current_exe) = std::env::current_exe() else {
        return;
    };
    let Some(exe_dir) = current_exe.parent() else {
        return;
    };

    let direct = exe_dir.join(FIXED_RUNTIME_DIR);
    if direct.join(WEBVIEW2_EXE).is_file() {
        std::env::set_var("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER", &direct);
        info!("Using bundled WebView2 fixed runtime: {}", direct.display());
        return;
    }

    let Ok(entries) = std::fs::read_dir(exe_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.starts_with(FIXED_RUNTIME_PREFIX) && path.join(WEBVIEW2_EXE).is_file() {
            std::env::set_var("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER", &path);
            info!("Using bundled WebView2 fixed runtime: {}", path.display());
            return;
        }
    }
}

#[cfg(not(windows))]
fn configure_windows_webview2_runtime() {}

fn is_webview2_startup_error(details: &str) -> bool {
    let details = details.to_ascii_lowercase();
    details.contains("webview2") || details.contains("createwebview")
}

fn startup_error_message(details: &str) -> String {
    if is_webview2_startup_error(details) {
        return format!(
            "Echo 启动失败：Windows WebView2 Runtime 不可用。\n\n\
WebView2Loader.dll 只是加载器；目标机器还需要安装 Microsoft Edge WebView2 Runtime，\
或使用包含 WebView2Runtime 固定运行时目录的 Echo 便携包/离线安装包。\n\n\
技术信息：{}",
            details
        );
    }

    format!("Echo 启动失败。\n\n技术信息：{}", details)
}

fn show_startup_error(message: &str) {
    let _ = MessageDialogBuilder::new("Echo 启动失败", message)
        .kind(MessageDialogKind::Error)
        .buttons(MessageDialogButtons::OkWithLabel("确定".to_string()))
        .show();
}

fn handle_startup_error(log_dir: &Path, error: tauri::Error) {
    let details = error.to_string();
    let message = startup_error_message(&details);
    append_crash_log(log_dir, &message);
    show_startup_error(&message);
    std::process::exit(1);
}

pub fn run() {
    // ── File logger with size-based truncation ─────────────────────────
    let log_dir = startup_log_dir();
    install_panic_logger(log_dir.clone());
    init_file_logger(&log_dir);
    configure_windows_webview2_runtime();

    let result = tauri::Builder::default()
        .menu(app_menu())
        .system_tray(tray::system_tray(MENU_CHECK_UPDATE))
        .on_menu_event(|event| {
            if event.menu_item_id() == MENU_CHECK_UPDATE {
                updater::spawn_manual_update_check(event.window().app_handle().clone());
            }
        })
        .on_system_tray_event(|app, event| {
            tray::handle_tray_event(app, event, MENU_CHECK_UPDATE);
        })
        .on_window_event(|event| {
            if event.window().label() != "main" {
                return;
            }
            if let tauri::WindowEvent::Focused(focused) = event.event() {
                tray::note_window_focused(&event.window().app_handle(), *focused);
            }
        })
        .setup(move |app| {
            let listen_port = std::env::var("ECHO_PORT")
                .ok()
                .and_then(|value| value.parse::<u16>().ok())
                .unwrap_or(9527);

            let app_data_dir = std::env::var("ECHO_DATA_DIR")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| {
                    app.path_resolver()
                        .app_data_dir()
                        .unwrap_or_else(|| startup_log_dir())
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

            // Channel to bridge UDP-discovered relay peers → async DB contact sync
            let (relay_tx, mut relay_rx) = mpsc::unbounded_channel::<Vec<PeerEntry>>();

            let runtime_services = if let Some(profile) = profile.as_ref() {
                let runtime = tauri::async_runtime::block_on(async {
                    state::RuntimeServices::start(Arc::clone(&db), profile, listen_port, Some(relay_tx.clone()))
                        .await
                        .expect("Failed to start runtime services")
                });
                info!("Runtime started with saved profile: {}", profile.username);
                Some(Arc::new(runtime))
            } else {
                info!("No saved profile found, waiting for first-time setup.");
                None
            };

            app.manage(AppState {
                db: db.clone(),
                profile: Mutex::new(profile),
                runtime: RwLock::new(runtime_services),
                relay_tx: Some(relay_tx),
            });

            updater::spawn_background_update_check(app.handle().clone());

            // ── UDP relay → contact sync processor ───────────────────
            // Receives PeerEntry batches forwarded from the UDP discovery
            // listener and persists them to peers + recent_contacts.
            let processor_db = db.clone();
            tauri::async_runtime::spawn(async move {
                while let Some(batch) = relay_rx.recv().await {
                    for entry in &batch {
                        if entry.ip.is_empty() || entry.port == 0 {
                            continue;
                        }
                        if entry.ip.parse::<IpAddr>().is_err() {
                            continue;
                        }
                        let _ = processor_db
                            .upsert_peer(
                                &entry.id,
                                &entry.username,
                                &entry.department,
                                &entry.ip,
                                entry.port,
                                false,
                            )
                            .await;
                        let _ = processor_db.add_recent_contact(&entry.id).await;
                    }
                    info!(
                        "RelaySync: persisted {} relayed peer(s) to contacts",
                        batch.len()
                    );
                }
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
                    // clone runtime handle quickly to avoid holding AppState lock
                    let runtime_opt = { state.runtime.read().await.clone() };
                    if let Some(runtime) = runtime_opt.as_ref() {
                        let disc = runtime.discovery.write().await;
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

            // Health check: concurrent TCP checks with proper lock handling
            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                use tokio::net::TcpStream;
                use std::net::SocketAddr;

                loop {
                    if let Some(state) = app_handle.try_state::<AppState>() {
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs() as i64;

                        // Snapshot peers, then release all locks
                        let snapshot: Vec<(String, String, String, IpAddr, u16)> = {
                            let runtime_opt = { state.runtime.read().await.clone() };
                            if let Some(runtime) = runtime_opt.as_ref() {
                                runtime.discovery.read().await.get_peers()
                                    .into_iter()
                                    .map(|p| (p.id, p.username, p.department, p.ip, p.port))
                                    .collect()
                            } else {
                                vec![]
                            }
                        };

                        if !snapshot.is_empty() {
                            log::info!("HealthCheck cycle: {} peer(s)", snapshot.len());
                        }

                        // Concurrent TCP detection using JoinSet to prevent blocking
                        let mut tasks = tokio::task::JoinSet::new();

                        for (id, username, department, ip, port) in snapshot {
                            tasks.spawn(async move {
                                // Support both IPv4 and IPv6 with SocketAddr
                                let addr = SocketAddr::new(ip, port);
                                let tcp_ok = tokio::time::timeout(
                                    Duration::from_secs(2),
                                    TcpStream::connect(&addr),
                                )
                                .await
                                .map(|r| r.is_ok())
                                .unwrap_or(false);

                                (id, username, department, ip, port, tcp_ok)
                            });
                        }

                        // Process concurrent check results
                        while let Some(res) = tasks.join_next().await {
                            if let Ok((id, username, department, ip, port, tcp_ok)) = res {
                                if tcp_ok {
                                    // TCP success → peer is alive, refresh last_seen
                                    if let Some(runtime) = { state.runtime.read().await.clone() }.as_ref() {
                                        runtime.discovery.write().await.touch_peer(&id);
                                    }
                                    let _ = state.db.upsert_peer(&id, &username, &department, &ip.to_string(), port, true).await;
                                    let db = state.db.clone();
                                    let pid = id.clone();
                                    tauri::async_runtime::spawn(async move {
                                        crate::commands::deliver_pending_to_peer(&db, &pid).await;
                                    });
                                    log::debug!("HealthCheck: {} TCP OK → deliver pending", username);
                                } else {
                                    // TCP fail → check if last_seen is too old
                                    let should_offline = if let Some(runtime) = { state.runtime.read().await.clone() }.as_ref() {
                                        let disc = runtime.discovery.read().await;
                                        disc.get_peer(&id)
                                            .map(|p| p.online && (now - p.last_seen) >= 15)
                                            .unwrap_or(false)
                                    } else {
                                        false
                                    };

                                    if should_offline {
                                        if let Some(runtime) = { state.runtime.read().await.clone() }.as_ref() {
                                            runtime.discovery.write().await.set_online(&id, false);
                                        }
                                        let _ = state.db.upsert_peer(&id, &username, &department, &ip.to_string(), port, false).await;
                                        let updated = Peer::with_online(id.clone(), username.clone(), department.clone(), ip, port, false, now);
                                        let _ = app_handle.emit_all("peer-discovered", &updated);
                                        log::info!("HealthCheck: {} → OFFLINE (tcp failed, age>15s)", username);
                                    }
                                }
                            }
                        }
                    }
                    tokio::time::sleep(Duration::from_secs(8)).await;
                }
            });

            // ── Mechanism 2: Anti-entropy contact sync ──────────────────
            // Every 5–8 min (with jitter), randomly pick 2-3 online + 1 offline
            // peer and exchange full contact summaries for delta reconciliation.
            let ae_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                use rand::seq::SliceRandom;
                use rand::Rng;

                loop {
                    // Jittered interval: 300–480 s
                    let delay_secs = {
                        let mut rng = rand::thread_rng();
                        rng.gen_range(300..=480)
                    };
                    tokio::time::sleep(Duration::from_secs(delay_secs)).await;

                    let Some(state) = ae_handle.try_state::<AppState>() else {
                        continue;
                    };

                    // Snapshot online + offline peers (read lock, quick)
                    let (online_peers, offline_peers): (Vec<Peer>, Vec<Peer>) = {
                        let runtime_opt = { state.runtime.read().await.clone() };
                        let Some(runtime) = runtime_opt.as_ref() else { continue };
                        let all = runtime.discovery.read().await.get_peers();
                        all.into_iter().partition(|p| p.online)
                    };

                    if online_peers.is_empty() && offline_peers.is_empty() {
                        continue;
                    }

                    // Pick 2–3 online + 1 offline. ThreadRng is !Send;
                    // scope it so it drops before the first .await below.
                    let selected: Vec<Peer> = {
                        let mut rng = rand::thread_rng();
                        let online_limit = rng.gen_range(2..=3).min(online_peers.len());
                        let mut sel: Vec<Peer> = online_peers
                            .choose_multiple(&mut rng, online_limit)
                            .cloned()
                            .collect();
                        if let Some(p) = offline_peers.choose(&mut rng) {
                            sel.push(p.clone());
                        }
                        sel
                    };

                    if selected.is_empty() {
                        continue;
                    }

                    // Extract sync params once; release locks before I/O
                    let (db, peers, my_id, my_name, my_department, my_port, my_ip) = {
                        let runtime_opt = { state.runtime.read().await.clone() };
                        let Some(runtime) = runtime_opt.as_ref() else { continue };
                        let chat = runtime.chat.lock().await;
                        let my_ip = chat
                            .my_id()
                            .rsplitn(2, ':')
                            .nth(1)
                            .unwrap_or("127.0.0.1")
                            .to_string();
                        (
                            chat.db().clone(),
                            chat.peers().clone(),
                            chat.my_id().to_string(),
                            chat.my_name().to_string(),
                            chat.my_department().to_string(),
                            chat.listen_port(),
                            my_ip,
                        )
                    };

                    info!(
                        "AntiEntropy: exchanging with {} peer(s) ({} online, {} offline)",
                        selected.len(),
                        selected.iter().filter(|p| p.online).count(),
                        selected.iter().filter(|p| !p.online).count(),
                    );

                    for p in &selected {
                        contact_sync::exchange_with_peer(
                            &db,
                            &peers,
                            &my_id,
                            &my_name,
                            &my_department,
                            my_port,
                            &my_ip,
                            &p.ip.to_string(),
                            p.port,
                            &p.id,
                        )
                        .await;
                    }
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
            commands::send_message_typed,
            commands::send_file,
            commands::send_sticker,
            commands::get_conversation,
            commands::mark_read,
            commands::get_unread_counts,
            commands::update_tray_unread,
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
            commands::delete_emoji_file,
            commands::list_recent_contacts,
            commands::remove_recent_contact,
            commands::create_group,
            commands::list_groups,
            commands::send_group_message,
            commands::send_group_message_typed,
            commands::send_group_file,
            commands::send_group_sticker,
            commands::get_group_messages,
            commands::rename_group,
            commands::leave_group,
            commands::invite_to_group,
            commands::dissolve_group,
            commands::get_group_unread_counts,
            commands::mark_group_read,
            commands::deliver_pending,
            updater::check_for_updates_command,
            updater::download_update_command,
        ])
        .run(tauri::generate_context!());

    if let Err(error) = result {
        handle_startup_error(&log_dir, error);
    }
}
