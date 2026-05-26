use log::{error, info};
use serde::{Deserialize, Serialize};
use tauri::{Manager, State};
use std::sync::Arc;

use crate::chat::{send_file_in_background_with_kind, WireMessage};
use crate::db::{ChatMessage, StoredPeer, UnreadCount, UserProfile};
use crate::discovery::Peer;
use crate::state::{AppState, RuntimeServices};

#[derive(Serialize)]
pub struct AppInfo {
    pub initialized: bool,
    pub peer_id: String,
    pub username: String,
    pub department: String,
    pub listen_port: u16,
    pub my_ip: String,
}

#[derive(Deserialize)]
pub struct SaveProfilePayload {
    pub username: String,
    pub department: String,
}

#[tauri::command]
pub async fn get_app_info(state: State<'_, AppState>) -> Result<AppInfo, String> {
    let profile = state.profile.lock().await.clone();
    let runtime = { state.runtime.read().await.clone() };

    if let (Some(profile), Some(runtime)) = (profile, runtime.as_ref()) {
        let my_ip = local_ip_address::local_ip()
            .map(|ip| ip.to_string())
            .unwrap_or_else(|_| "127.0.0.1".to_string());

        Ok(AppInfo {
            initialized: true,
            peer_id: runtime.my_id.clone(),
            username: profile.username,
            department: profile.department,
            listen_port: runtime.listen_port,
            my_ip,
        })
    } else {
        Ok(AppInfo {
            initialized: false,
            peer_id: String::new(),
            username: String::new(),
            department: String::new(),
            listen_port: std::env::var("ECHO_PORT")
                .ok()
                .and_then(|value| value.parse::<u16>().ok())
                .unwrap_or(9527),
            my_ip: String::new(),
        })
    }
}

#[tauri::command]
pub async fn get_departments(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let mut departments = state.db.get_departments().await.map_err(|e| e.to_string())?;

    if let Some(runtime) = { state.runtime.read().await.clone() }.as_ref() {
        let peers = runtime.discovery.read().await.get_peers();
        for peer in peers {
            let dep = peer.department.trim();
            if !dep.is_empty() && !departments.iter().any(|d| d == dep) {
                departments.push(dep.to_string());
            }
        }
    }

    departments.sort_by_key(|d| d.to_lowercase());
    departments.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    Ok(departments)
}

#[tauri::command]
pub async fn save_profile(
    state: State<'_, AppState>,
    payload: SaveProfilePayload,
) -> Result<(), String> {
    let username = payload.username.trim();
    let department = payload.department.trim();

    if username.is_empty() {
        return Err("用户名不能为空".to_string());
    }
    if department.is_empty() {
        return Err("部门不能为空".to_string());
    }

    let existing_peer_id = state
        .profile
        .lock()
        .await
        .as_ref()
        .map(|profile| profile.peer_id.clone())
        .filter(|peer_id| !peer_id.is_empty())
        .unwrap_or_default(); // will be set to IP:port by RuntimeServices::start()

    state
        .db
        .save_user_profile(&existing_peer_id, username, department)
        .await
        .map_err(|e| e.to_string())?;

    let profile = UserProfile {
        peer_id: existing_peer_id,
        username: username.to_string(),
        department: department.to_string(),
    };

    *state.profile.lock().await = Some(profile.clone());

    let runtime_opt = { state.runtime.read().await.clone() };
    if let Some(runtime) = runtime_opt.as_ref() {
        runtime
            .update_profile(username, department)
            .await
            .map_err(|e| e.to_string())?;

        // Notify peers we know about so their cached username/department updates.
        // This includes online + stored peers, deduped. Offline peers get the
        // change via the same pending_notifications queue used for group msgs.
        let listen_port = runtime.listen_port;
        let my_id = runtime.my_id.clone();
        let online_peers = runtime.discovery.read().await.get_peers();

        let stored = state.db.list_stored_peers().await.unwrap_or_default();
        let mut targets: std::collections::HashSet<String> = std::collections::HashSet::new();
        for p in &online_peers { targets.insert(p.id.clone()); }
        for sp in &stored { targets.insert(sp.peer_id.clone()); }
        targets.remove(&my_id);
        let target_ids: Vec<String> = targets.into_iter().collect();

        let payload = serde_json::json!({
            "username": username,
            "department": department,
        }).to_string();

        let empty_dir: Vec<crate::discovery::PeerEntry> = Vec::new();
        send_or_queue_notification(
            &state.db, &online_peers, &target_ids,
            &my_id, username, department, listen_port,
            &payload, "profile_updated", None, None, &empty_dir,
        ).await;
    } else {
        let listen_port = std::env::var("ECHO_PORT")
            .ok()
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(9527);

        let profile = state.profile.lock().await.clone().unwrap();
        let relay_tx = state.relay_tx.clone();
        let runtime = RuntimeServices::start(state.db.clone(), &profile, listen_port, relay_tx)
            .await
            .map_err(|e| e.to_string())?;
        *state.runtime.write().await = Some(Arc::new(runtime));
    }

    Ok(())
}

#[tauri::command]
pub async fn list_stored_peers(state: State<'_, AppState>) -> Result<Vec<StoredPeer>, String> {
    state.db.list_stored_peers().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_peers(state: State<'_, AppState>) -> Result<Vec<Peer>, String> {
    let runtime_opt = { state.runtime.read().await.clone() };
    let Some(runtime) = runtime_opt.as_ref() else {
        return Ok(vec![]);
    };
    let discovery = runtime.discovery.read().await;
    Ok(discovery.get_peers())
}

#[tauri::command]
pub async fn send_message(
    state: State<'_, AppState>,
    peer_id: String,
    content: String,
) -> Result<ChatMessage, String> {
    send_message_typed(state, peer_id, content, "text".to_string()).await
}

#[tauri::command]
pub async fn send_message_typed(
    state: State<'_, AppState>,
    peer_id: String,
    content: String,
    msg_type: String,
) -> Result<ChatMessage, String> {
    let runtime_opt = { state.runtime.read().await.clone() };
    let Some(runtime) = runtime_opt.as_ref() else {
        return Err("应用尚未初始化用户信息".to_string());
    };

    let discovery = runtime.discovery.read().await;
    let peer = if let Some(peer) = discovery.get_peer(&peer_id) {
        peer
    } else if let Some(stored_peer) = state
        .db
        .get_stored_peer(&peer_id)
        .await
        .map_err(|e| e.to_string())?
    {
        Peer::new(
            stored_peer.peer_id,
            stored_peer.username,
            stored_peer.department,
            stored_peer.ip.parse().map_err(|_| "无效的联系人 IP 地址".to_string())?,
            stored_peer.port,
        )
    } else {
        return Err(format!("Peer {} not found", peer_id));
    };
    drop(discovery);

    let chat = runtime.chat.lock().await;
    chat.send_message_typed(&peer, &content, &msg_type)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn send_file(
    app_handle: tauri::AppHandle,
    state: State<'_, AppState>,
    peer_id: String,
    file_path: String,
) -> Result<ChatMessage, String> {
    send_file_with_kind(app_handle, state, peer_id, file_path, "file").await
}

#[tauri::command]
pub async fn send_sticker(
    app_handle: tauri::AppHandle,
    state: State<'_, AppState>,
    peer_id: String,
    file_path: String,
) -> Result<ChatMessage, String> {
    send_file_with_kind(app_handle, state, peer_id, file_path, "sticker").await
}

async fn send_file_with_kind(
    app_handle: tauri::AppHandle,
    state: State<'_, AppState>,
    peer_id: String,
    file_path: String,
    file_kind: &str,
) -> Result<ChatMessage, String> {
    info!("send_file: start ({})", file_kind);
    let t0 = std::time::Instant::now();
    let runtime_opt = { state.runtime.read().await.clone() };
    let Some(runtime) = runtime_opt.as_ref() else {
        return Err("应用尚未初始化用户信息".to_string());
    };

    let discovery = runtime.discovery.read().await;
    let peer = if let Some(peer) = discovery.get_peer(&peer_id) {
        peer
    } else if let Some(stored_peer) = state
        .db
        .get_stored_peer(&peer_id)
        .await
        .map_err(|e| e.to_string())?
    {
        Peer::new(
            stored_peer.peer_id,
            stored_peer.username,
            stored_peer.department,
            stored_peer.ip.parse().map_err(|_| "无效的联系人 IP 地址".to_string())?,
            stored_peer.port,
        )
    } else {
        return Err(format!("Peer {} not found", peer_id));
    };
    drop(discovery);

    // Clone what we need and release the chat lock immediately
    let (my_id, my_name, my_department, listen_port, db, peers_arc) = {
        let chat = runtime.chat.lock().await;
        (chat.my_id().to_string(), chat.my_name().to_string(), chat.my_department().to_string(), chat.listen_port(), chat.db().clone(), chat.peers().clone())
    };
    let _ = runtime;

    let file_name = std::path::Path::new(&file_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    // Clone for placeholder (before moving into background task)
    let placeholder_my_id = my_id.clone();
    let placeholder_my_name = my_name.clone();
    let placeholder_peer_id = peer.id.clone();

    // Clone for background task
    let bg_path = file_path.clone();
    let bg_name = file_name.clone();
    let bg_peer = peer.clone();
    let handle = app_handle.clone();
    let error_handle = app_handle.clone();
    let bg_kind = file_kind.to_string();
    tauri::async_runtime::spawn(async move {
        match send_file_in_background_with_kind(&bg_path, &bg_name, &bg_peer, my_id, my_name, my_department, listen_port, db, peers_arc, handle, &bg_kind).await {
            Ok(msg) => info!("File sent: {}", msg.content),
            Err(e) => {
                error!("File send failed: {}", e);
                let _ = error_handle.emit_all("file-error", serde_json::json!({
                    "fileName": bg_name,
                    "error": e.to_string(),
                }));
            }
        }
    });

    let msg_kind = if file_kind == "sticker" { "sticker" } else { "file" };
    let content = if msg_kind == "sticker" {
        "[表情]".to_string()
    } else {
        format!("📎 {}", file_name)
    };

    info!("send_file: returning placeholder ({:?} total)", t0.elapsed());
    use chrono::Utc;
    Ok(ChatMessage {
        id: 0,
        sender_id: placeholder_my_id,
        sender_name: placeholder_my_name,
        receiver_id: placeholder_peer_id,
        content,
        msg_type: msg_kind.to_string(),
        file_name: Some(file_name),
        file_path: Some(file_path),
        file_size: None,
        timestamp: Utc::now().to_rfc3339(),
        is_read: true,
    })
}

#[derive(Serialize)]
pub struct DiscoverResult {
    pub online: bool,
    pub message: String,
}

#[derive(Clone)]
struct RemoteIdentity {
    peer_id: String,
    username: String,
    department: String,
    ip: String,
    port: u16,
}

async fn probe_identity(
    addr: &str,
    my_id: &str,
    my_name: &str,
    my_department: &str,
    my_port: u16,
) -> Option<RemoteIdentity> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let stream = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        tokio::net::TcpStream::connect(addr),
    )
    .await
    .ok()?
    .ok()?;
    let peer_addr = stream.peer_addr().ok();
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    let probe = WireMessage {
        sender_id: my_id.to_string(),
        sender_name: my_name.to_string(),
        sender_department: my_department.to_string(),
        sender_port: my_port,
        receiver_id: String::new(),
        content: String::new(),
        msg_type: "identity_probe".to_string(),
        file_name: None,
        file_size: None,
        file_data: None,
        file_kind: None,
        known_peers: Vec::new(),
        group_id: None,
    };

    let json = serde_json::to_string(&probe).ok()?;
    writer.write_all(json.as_bytes()).await.ok()?;
    writer.write_all(b"\n").await.ok()?;
    writer.flush().await.ok()?;

    let line = tokio::time::timeout(std::time::Duration::from_secs(2), lines.next_line())
        .await
        .ok()?
        .ok()??;
    let msg: WireMessage = serde_json::from_str(&line).ok()?;
    if msg.msg_type != "identity_response" || msg.sender_id.trim().is_empty() {
        return None;
    }

    Some(RemoteIdentity {
        peer_id: msg.sender_id,
        username: msg.sender_name,
        department: msg.sender_department,
        ip: peer_addr
            .map(|addr| addr.ip().to_string())
            .unwrap_or_else(|| addr.rsplit_once(':').map(|(ip, _)| ip.to_string()).unwrap_or_default()),
        port: msg.sender_port,
    })
}

#[tauri::command]
pub async fn discover_by_ip(
    state: State<'_, AppState>,
    ip: String,
    port: u16,
) -> Result<DiscoverResult, String> {
    let addr = format!("{}:{}", ip, port);

    // Try TCP connect
    let online = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        tokio::net::TcpStream::connect(&addr),
    )
    .await
    .map(|r| r.is_ok())
    .unwrap_or(false);

    if !online {
        return Ok(DiscoverResult {
            online: false,
            message: format!("无法连接到 {}:{}", ip, port),
        });
    }

    // Read my profile and runtime info
    let my_profile = state.profile.lock().await.clone();
    let (my_id, my_port) = {
        let runtime_opt = { state.runtime.read().await.clone() };
        match runtime_opt.as_ref() {
            Some(r) => (r.my_id.clone(), r.listen_port),
            None => return Err("应用尚未初始化".to_string()),
        }
    };

    // Send our announce as a unicast UDP probe to the remote peer's discovery port.
    // Include our own known_peers so they also get our contacts (bidirectional relay).
    let our_known: Vec<serde_json::Value> = {
        let runtime_opt = { state.runtime.read().await.clone() };
        if let Some(runtime) = runtime_opt.as_ref() {
            runtime.discovery.read().await.get_peers().into_iter()
                .filter(|p| p.online)
                .map(|p| serde_json::json!({
                    "id": p.id, "username": p.username, "department": p.department,
                    "ip": p.ip.to_string(), "port": p.port,
                }))
                .collect()
        } else { vec![] }
    };

    let probe = serde_json::json!({
        "id": my_id,
        "username": my_profile.as_ref().map(|p| p.username.as_str()).unwrap_or(""),
        "department": my_profile.as_ref().map(|p| p.department.as_str()).unwrap_or(""),
        "ip": "",
        "port": my_port,
        "known_peers": our_known,
    });

    let probe_bytes = serde_json::to_vec(&probe).unwrap_or_default();
    let target = format!("{}:{}", ip, port + 2);

    // Send UDP probe up to 3 times, waiting 1s between each
    let parsed_ip: std::net::IpAddr = ip.parse().map_err(|e| format!("无效 IP: {}", e))?;
    let mut existing = None;

    let my_name = my_profile
        .as_ref()
        .map(|p| p.username.as_str())
        .unwrap_or("");
    let my_department = my_profile
        .as_ref()
        .map(|p| p.department.as_str())
        .unwrap_or("");

    if let Some(identity) = probe_identity(&addr, &my_id, my_name, my_department, my_port).await {
        let remote_port = if identity.port == 0 { port } else { identity.port };
        let remote_ip = if identity.ip.is_empty() {
            ip.clone()
        } else {
            identity.ip.clone()
        };
        let remote_parsed_ip = remote_ip
            .parse::<std::net::IpAddr>()
            .unwrap_or(parsed_ip);
        let peer = crate::discovery::Peer::new(
            identity.peer_id.clone(),
            identity.username.clone(),
            identity.department.clone(),
            remote_parsed_ip,
            remote_port,
        );

        {
            let runtime_opt = { state.runtime.read().await.clone() };
            if let Some(runtime) = runtime_opt.as_ref() {
                let disc = runtime.discovery.read().await;
                disc.register_peer(peer);
            }
        }

        state
            .db
            .upsert_peer(
                &identity.peer_id,
                &identity.username,
                &identity.department,
                &remote_ip,
                remote_port,
                true,
            )
            .await
            .map_err(|e| e.to_string())?;
        let _ = state.db.add_recent_contact(&identity.peer_id).await;

        let runtime_arc = { state.runtime.read().await.clone() };
        if let Some(runtime) = runtime_arc {
            let found_ip = remote_ip.clone();
            let found_id = identity.peer_id.clone();
            tauri::async_runtime::spawn(async move {
                let chat = runtime.chat.lock().await;
                chat.exchange_contacts(&found_ip, remote_port, &found_id).await;
            });
        }

        return Ok(DiscoverResult {
            online: true,
            message: format!(
                "已连接 {} ({}) @ {}:{}",
                identity.username, identity.department, remote_ip, remote_port
            ),
        });
    }

    for attempt in 0..3 {
        if let Ok(sock) = std::net::UdpSocket::bind("0.0.0.0:0") {
            let _ = sock.set_broadcast(true);
            let _ = sock.send_to(&probe_bytes, &target);
        }

        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        // Check if the peer was auto-registered from the unicast response
        let found = {
            let runtime_opt = { state.runtime.read().await.clone() };
            if let Some(runtime) = runtime_opt.as_ref() {
                runtime.discovery.read().await
                    .get_peers().into_iter()
                    .find(|p| p.ip == parsed_ip && p.port == port)
            } else { None }
        };

        if let Some(f) = found {
            existing = Some(f);
            break;
        }
        log::info!("UDP probe attempt {} for {}:{} — no response yet", attempt + 1, ip, port);
    }

    if let Some(found) = existing {
        // Mechanism 1: ice-breaking — exchange contact summaries in background
        let runtime_arc = { state.runtime.read().await.clone() };
        if let Some(runtime) = runtime_arc {
            let found_ip = found.ip.to_string();
            let found_port = found.port;
            let found_id = found.id.clone();
            tauri::async_runtime::spawn(async move {
                let chat = runtime.chat.lock().await;
                chat.exchange_contacts(&found_ip, found_port, &found_id).await;
            });
        }
        return Ok(DiscoverResult {
            online: true,
            message: format!("已连接 {} ({}) @ {}:{}", found.username, found.department, found.ip, found.port),
        });
    }

    // Fallback: no unicast response received, register manually
    let stored_peer = state
        .db
        .list_stored_peers()
        .await
        .ok()
        .and_then(|peers| {
            peers
                .into_iter()
                .find(|peer| peer.ip == ip && peer.port == port)
        });
    let pid = stored_peer
        .as_ref()
        .map(|peer| peer.peer_id.clone())
        .unwrap_or_else(|| format!("{}:{}", ip, port));
    let display_name = stored_peer
        .as_ref()
        .map(|peer| peer.username.clone())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "手动添加".to_string());
    let display_department = stored_peer
        .as_ref()
        .map(|peer| peer.department.clone())
        .unwrap_or_default();
    let peer = crate::discovery::Peer::new(
        pid.clone(),
        display_name.clone(),
        display_department.clone(),
        parsed_ip,
        port,
    );

    {
        let runtime_opt = { state.runtime.read().await.clone() };
        if let Some(runtime) = runtime_opt.as_ref() {
            let disc = runtime.discovery.read().await;
            // Don't duplicate: check by IP:port first
            let already = disc
                .get_peers()
                .into_iter()
                .any(|p| p.ip == parsed_ip && p.port == port);
            if !already {
                disc.register_peer(peer.clone());
            }
        }
    }
    // Save to DB only when this is a new temporary peer. If this endpoint is
    // already known, keep the stored identity instead of overwriting it.
    if stored_peer.is_none() {
        let _ = state
            .db
            .upsert_peer(&pid, "手动添加", "", &ip, port, true)
            .await;
    } else {
        let _ = state.db.add_recent_contact(&pid).await;
    }

    // Mechanism 1: ice-breaking — exchange contact summaries in background
    {
        let runtime_arc = { state.runtime.read().await.clone() };
        if let Some(runtime) = runtime_arc {
            let ip_copy = ip.clone();
            let pid_copy = pid.clone();
            tauri::async_runtime::spawn(async move {
                let chat = runtime.chat.lock().await;
                chat.exchange_contacts(&ip_copy, port, &pid_copy).await;
            });
        }
    }

    Ok(DiscoverResult {
        online: true,
        message: format!("已添加 {}:{}（未获取到对方信息）", ip, port),
    })
}

#[tauri::command]
pub async fn check_peer_online(
    ip: String,
    port: u16,
) -> Result<bool, String> {
    let addr = format!("{}:{}", ip, port);
    let online = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        tokio::net::TcpStream::connect(&addr),
    )
    .await
    .map(|r| r.is_ok())
    .unwrap_or(false);
    Ok(online)
}

#[tauri::command]
pub async fn get_conversation(
    state: State<'_, AppState>,
    peer_id: String,
) -> Result<Vec<ChatMessage>, String> {
    let runtime_opt = { state.runtime.read().await.clone() };
    let Some(runtime) = runtime_opt.as_ref() else {
        return Ok(vec![]);
    };

    state
        .db
        .get_conversation(&peer_id, &runtime.my_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_unread_counts(state: State<'_, AppState>) -> Result<Vec<UnreadCount>, String> {
    let runtime_opt = { state.runtime.read().await.clone() };
    let Some(runtime) = runtime_opt.as_ref() else {
        return Ok(vec![]);
    };

    state
        .db
        .get_unread_counts(&runtime.my_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn mark_read(
    state: State<'_, AppState>,
    peer_id: String,
) -> Result<(), String> {
    let runtime_opt = { state.runtime.read().await.clone() };
    let Some(runtime) = runtime_opt.as_ref() else {
        return Ok(());
    };

    state
        .db
        .mark_read(&peer_id, &runtime.my_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn save_temp_file(data: Vec<u8>, filename: String) -> Result<String, String> {
    let files_dir = echo_files_dir();
    std::fs::create_dir_all(&files_dir).map_err(|e| e.to_string())?;

    let timestamp = chrono::Utc::now().timestamp_millis();
    let file_path = files_dir.join(format!("{}_{}", timestamp, filename));

    std::fs::write(&file_path, &data).map_err(|e| e.to_string())?;

    Ok(file_path.to_string_lossy().to_string())
}

#[derive(Serialize)]
pub struct FileData {
    pub base64: String,
    pub mime: String,
}

#[tauri::command]
pub fn read_file_base64(file_path: String) -> Result<FileData, String> {
    let bytes = std::fs::read(&file_path).map_err(|e| e.to_string())?;

    let mime = path_mime(&file_path).to_string();

    Ok(FileData {
        base64: base64_encode_std(&bytes),
        mime,
    })
}

#[tauri::command]
pub fn open_file(path: String) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(&path)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(&path)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", &std::path::Path::new(&path).to_string_lossy().as_ref()])
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub fn open_folder(path: String) -> Result<(), String> {
    let parent = std::path::Path::new(&path)
        .parent()
        .unwrap_or(std::path::Path::new(&path));

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(parent)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(parent)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(parent)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[derive(Serialize)]
pub struct SearchResult {
    pub peer_id: String,
    pub peer_name: String,
    pub messages: Vec<SearchHit>,
}

#[derive(Serialize)]
pub struct SearchHit {
    pub id: i64,
    pub sender_id: String,
    pub sender_name: String,
    pub receiver_id: String,
    pub content: String,
    pub msg_type: String,
    pub file_name: Option<String>,
    pub file_path: Option<String>,
    pub timestamp: String,
}

#[tauri::command]
pub async fn search_messages(
    state: State<'_, AppState>,
    query: String,
) -> Result<Vec<SearchResult>, String> {
    let runtime_opt = { state.runtime.read().await.clone() };
    let Some(runtime) = runtime_opt.as_ref() else {
        return Ok(vec![]);
    };

    let rows = state
        .db
        .search_messages(&runtime.my_id, &query)
        .await
        .map_err(|e| e.to_string())?;

    let mut groups: std::collections::BTreeMap<String, SearchResult> = std::collections::BTreeMap::new();
    for row in rows {
        let peer_id = if row.sender_id == runtime.my_id {
            row.receiver_id.clone()
        } else {
            row.sender_id.clone()
        };
        let peer_name = if row.sender_id == runtime.my_id {
            "我发往".to_string()
        } else {
            row.sender_name.clone()
        };

        groups
            .entry(peer_id.clone())
            .or_insert_with(|| SearchResult {
                peer_id: peer_id.clone(),
                peer_name,
                messages: vec![],
            })
            .messages
            .push(SearchHit {
                id: row.id,
                sender_id: row.sender_id,
                sender_name: row.sender_name,
                receiver_id: row.receiver_id,
                content: row.content,
                msg_type: row.msg_type,
                file_name: row.file_name,
                file_path: row.file_path,
                timestamp: row.timestamp,
            });
    }

    Ok(groups.into_values().collect())
}

#[tauri::command]
pub async fn get_scan_subnets(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    // Return from in-memory config (more up-to-date), fallback to DB
    if let Some(runtime) = { state.runtime.read().await.clone() }.as_ref() {
        let disc = runtime.discovery.read().await;
        let subnets = disc.get_scan_subnets();
        if !subnets.is_empty() {
            return Ok(subnets);
        }
    }
    state.db.get_scan_subnets().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn set_scan_subnets(
    state: State<'_, AppState>,
    subnets: Vec<String>,
) -> Result<(), String> {
    let joined = subnets.join(",");
    state
        .db
        .save_scan_subnets(&joined)
        .await
        .map_err(|e| e.to_string())?;

    if let Some(runtime) = { state.runtime.read().await.clone() }.as_ref() {
        runtime.discovery.write().await.update_scan_subnets(&subnets);
    }

    Ok(())
}

fn path_mime(path: &str) -> &'static str {
    match std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("bmp") => "image/bmp",
        Some("svg") => "image/svg+xml",
        Some("pdf") => "application/pdf",
        Some("zip") => "application/zip",
        Some("txt") | Some("md") | Some("rs") | Some("ts") | Some("js") | Some("json") | Some("html") | Some("css") => "text/plain",
        _ => "application/octet-stream",
    }
}

fn base64_encode_std(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

#[tauri::command]
pub fn list_emoji_files() -> Result<Vec<String>, String> {
    let dir = emoji_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut files: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(&dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.is_file() {
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if matches!(ext.to_lowercase().as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp") {
                    files.push(path.to_string_lossy().to_string());
                }
            }
        }
    }
    Ok(files)
}

#[tauri::command]
pub fn add_emoji_file(source_path: String) -> Result<String, String> {
    let dir = emoji_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let name = std::path::Path::new(&source_path)
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("invalid filename")?;
    let dest = dir.join(name);
    std::fs::copy(&source_path, &dest).map_err(|e| e.to_string())?;
    Ok(dest.to_string_lossy().to_string())
}

#[tauri::command]
pub fn delete_emoji_file(file_path: String) -> Result<(), String> {
    let dir = emoji_dir();
    let dir = std::fs::canonicalize(&dir).map_err(|e| e.to_string())?;
    let path = std::fs::canonicalize(&file_path).map_err(|e| e.to_string())?;
    if !path.starts_with(&dir) {
        return Err("invalid emoji path".to_string());
    }
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    if !matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp") {
        return Err("invalid emoji file type".to_string());
    }
    std::fs::remove_file(path).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_recent_contacts(state: State<'_, AppState>) -> Result<Vec<StoredPeer>, String> {
    log::info!("list_recent_contacts COMMAND called");
    let result = state.db.list_recent_contacts().await.map_err(|e| e.to_string())?;
    log::info!("list_recent_contacts: {} entries", result.len());
    Ok(result)
}

#[tauri::command]
pub async fn remove_recent_contact(state: State<'_, AppState>, peer_id: String) -> Result<(), String> {
    state.db.remove_recent_contact(&peer_id).await.map_err(|e| e.to_string())
}

// ── Group commands ──

#[derive(Deserialize)]
pub struct CreateGroupPayload {
    pub name: String,
    pub members: Vec<String>,
}

#[tauri::command]
pub async fn create_group(
    state: State<'_, AppState>, payload: CreateGroupPayload,
) -> Result<crate::db::GroupInfo, String> {
    let gid = format!("group-{}", uuid::Uuid::new_v4().to_string().split('-').next().unwrap_or("0"));
    let my_id = {
        let runtime_opt = { state.runtime.read().await.clone() };
        runtime_opt.as_ref().map(|r| r.my_id.clone()).unwrap_or_default()
    };
    let mut all_members = payload.members.clone();
    if !all_members.iter().any(|m| m == &my_id) {
        all_members.push(my_id.clone());
    }
    state.db.create_group(&gid, &payload.name, &my_id, &all_members).await.map_err(|e| e.to_string())?;
    let members = state.db.get_group_members(&gid).await.unwrap_or_default();

    let (listen_port, online_peers) = {
        let runtime_opt = { state.runtime.read().await.clone() };
        match runtime_opt.as_ref() {
            Some(r) => (r.listen_port, r.discovery.read().await.get_peers()),
            None => (9527, vec![]),
        }
    };
    let (my_name, my_department) = {
        let p = state.profile.lock().await;
        p.as_ref().map(|p| (p.username.clone(), p.department.clone()))
            .unwrap_or_default()
    };
    let directory = build_member_directory(
        &state.db, &online_peers, &all_members,
        &my_id, &my_name, &my_department, listen_port,
    ).await;
    let content = serde_json::json!({"name": payload.name, "member_ids": all_members}).to_string();

    send_or_queue_notification(
        &state.db, &online_peers, &all_members,
        &my_id, &my_name, &my_department, listen_port,
        &content, "group_created", Some(&gid), None, &directory,
    ).await;

    Ok(crate::db::GroupInfo {
        group_id: gid, name: payload.name, creator_id: my_id,
        created_at: String::new(), members,
        last_message: None, last_message_at: None, last_message_sender: None, unread_count: 0,
    })
}

#[tauri::command]
pub async fn list_groups(state: State<'_, AppState>) -> Result<Vec<crate::db::GroupInfo>, String> {
    let my_id = {
        let runtime_opt = { state.runtime.read().await.clone() };
        runtime_opt.as_ref().map(|r| r.my_id.clone()).unwrap_or_default()
    };
    let mut groups = state.db.list_groups(&my_id).await.map_err(|e| e.to_string())?;
    for g in &mut groups {
        g.members = state.db.get_group_members(&g.group_id).await.unwrap_or_default();
    }
    Ok(groups)
}

#[tauri::command]
pub async fn send_group_message(
    state: State<'_, AppState>, group_id: String, content: String,
) -> Result<ChatMessage, String> {
    send_group_message_typed(state, group_id, content, "text".to_string()).await
}

#[tauri::command]
pub async fn send_group_message_typed(
    state: State<'_, AppState>, group_id: String, content: String, msg_type: String,
) -> Result<ChatMessage, String> {
    let (my_id, my_name, my_department, listen_port, members) = {
        let runtime_opt = { state.runtime.read().await.clone() };
        let r = runtime_opt.as_ref().ok_or("未初始化")?;
        let prof = state.profile.lock().await;
        let my_name = prof.as_ref().map(|p| p.username.clone()).unwrap_or_default();
        let my_dept = prof.as_ref().map(|p| p.department.clone()).unwrap_or_default();
        let members = state.db.get_group_members(&group_id).await.map_err(|e| e.to_string())?;
        (r.my_id.clone(), my_name, my_dept, r.listen_port, members)
    };

    let msg = state.db.save_group_message(&group_id, &my_id, &my_name, &content, &msg_type, None, None, None, true).await.map_err(|e| e.to_string())?;

    let online_peers = {
        let runtime_opt = { state.runtime.read().await.clone() };
        match runtime_opt.as_ref() {
            Some(r) => r.discovery.read().await.get_peers(),
            None => vec![],
        }
    };

    let target_ids: Vec<String> = members.iter().map(|m| m.peer_id.clone()).collect();
    let empty_dir: Vec<crate::discovery::PeerEntry> = Vec::new();
    send_or_queue_notification(
        &state.db, &online_peers, &target_ids,
        &my_id, &my_name, &my_department, listen_port,
        &content, &msg_type, Some(&group_id), None, &empty_dir,
    ).await;

    Ok(msg)
}

#[tauri::command]
pub async fn get_group_messages(state: State<'_, AppState>, group_id: String) -> Result<Vec<ChatMessage>, String> {
    state.db.get_group_messages(&group_id).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn rename_group(state: State<'_, AppState>, group_id: String, new_name: String) -> Result<(), String> {
    state.db.rename_group(&group_id, &new_name).await.map_err(|e| e.to_string())?;

    let members = state.db.get_group_members(&group_id).await.map_err(|e| e.to_string())?;
    let (my_id, my_name, my_department, listen_port, online_peers) = {
        let runtime_opt = { state.runtime.read().await.clone() };
        let r = runtime_opt.as_ref().ok_or("未初始化")?;
        let prof = state.profile.lock().await;
        let my_name = prof.as_ref().map(|p| p.username.clone()).unwrap_or_default();
        let my_dept = prof.as_ref().map(|p| p.department.clone()).unwrap_or_default();
        let peers = r.discovery.read().await.get_peers();
        (r.my_id.clone(), my_name, my_dept, r.listen_port, peers)
    };

    let target_ids: Vec<String> = members.iter().map(|m| m.peer_id.clone()).collect();
    let empty_dir: Vec<crate::discovery::PeerEntry> = Vec::new();
    send_or_queue_notification(
        &state.db, &online_peers, &target_ids,
        &my_id, &my_name, &my_department, listen_port,
        &format!("群名已修改为「{}」", new_name),
        "group_renamed", Some(&group_id), Some(&new_name), &empty_dir,
    ).await;
    Ok(())
}

#[tauri::command]
pub async fn invite_to_group(state: State<'_, AppState>, group_id: String, members: Vec<String>) -> Result<(), String> {
    state.db.add_group_members(&group_id, &members).await.map_err(|e| e.to_string())?;

    let groups = {
        let my_id_opt = { state.runtime.read().await.clone() }.as_ref().map(|r| r.my_id.clone());
        let my_id = my_id_opt.unwrap_or_default();
        state.db.list_groups(&my_id).await.map_err(|e| e.to_string())?
    };
    let group = groups.iter().find(|g| g.group_id == group_id).ok_or("群组不存在")?.clone();
    let member_records = state.db.get_group_members(&group_id).await.map_err(|e| e.to_string())?;
    let all_member_ids: Vec<String> = member_records.iter().map(|m| m.peer_id.clone()).collect();

    let (my_id, listen_port, online_peers) = {
        let runtime_opt = { state.runtime.read().await.clone() };
        let r = runtime_opt.as_ref().ok_or("未初始化")?;
        let peers = r.discovery.read().await.get_peers();
        (r.my_id.clone(), r.listen_port, peers)
    };
    let (my_name, my_department) = {
        let p = state.profile.lock().await;
        p.as_ref().map(|p| (p.username.clone(), p.department.clone())).unwrap_or_default()
    };
    let directory = build_member_directory(
        &state.db, &online_peers, &all_member_ids,
        &my_id, &my_name, &my_department, listen_port,
    ).await;
    let content = serde_json::json!({"name": group.name, "member_ids": all_member_ids}).to_string();

    send_or_queue_notification(
        &state.db, &online_peers, &all_member_ids,
        &my_id, &my_name, &my_department, listen_port,
        &content, "group_created", Some(&group_id), None, &directory,
    ).await;
    Ok(())
}

#[tauri::command]
pub async fn leave_group(state: State<'_, AppState>, group_id: String) -> Result<(), String> {
    let (my_id, listen_port, online_peers, members) = {
        let runtime_opt = { state.runtime.read().await.clone() };
        let r = runtime_opt.as_ref().ok_or("未初始化")?;
        let members = state.db.get_group_members(&group_id).await.map_err(|e| e.to_string())?;
        let online = r.discovery.read().await.get_peers();
        (r.my_id.clone(), r.listen_port, online, members)
    };

    let groups = state.db.list_groups(&my_id).await.map_err(|e| e.to_string())?;
    if let Some(g) = groups.iter().find(|g| g.group_id == group_id) {
        if g.creator_id == my_id {
            return Err("群主不可退群，请使用解散群".to_string());
        }
    }
    let (my_name, my_department) = {
        let p = state.profile.lock().await;
        p.as_ref().map(|p| (p.username.clone(), p.department.clone())).unwrap_or_default()
    };

    let target_ids: Vec<String> = members.iter().map(|m| m.peer_id.clone()).collect();
    let empty_dir: Vec<crate::discovery::PeerEntry> = Vec::new();
    send_or_queue_notification(
        &state.db, &online_peers, &target_ids,
        &my_id, &my_name, &my_department, listen_port,
        "", "group_member_left", Some(&group_id), None, &empty_dir,
    ).await;

    state.db.remove_group_member(&group_id, &my_id).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn dissolve_group(state: State<'_, AppState>, group_id: String) -> Result<(), String> {
    let members = state.db.get_group_members(&group_id).await.map_err(|e| e.to_string())?;
    let (my_id, listen_port, online_peers) = {
        let runtime_opt = { state.runtime.read().await.clone() };
        let r = runtime_opt.as_ref().ok_or("未初始化")?;
        let peers = r.discovery.read().await.get_peers();
        (r.my_id.clone(), r.listen_port, peers)
    };
    let (my_name, my_department) = {
        let p = state.profile.lock().await;
        p.as_ref().map(|p| (p.username.clone(), p.department.clone())).unwrap_or_default()
    };

    state.db.dissolve_group(&group_id).await.map_err(|e| e.to_string())?;

    let target_ids: Vec<String> = members.iter().map(|m| m.peer_id.clone()).collect();
    let empty_dir: Vec<crate::discovery::PeerEntry> = Vec::new();
    send_or_queue_notification(
        &state.db, &online_peers, &target_ids,
        &my_id, &my_name, &my_department, listen_port,
        "群组已解散", "group_dissolved", Some(&group_id), None, &empty_dir,
    ).await;
    Ok(())
}

#[tauri::command]
pub async fn send_group_file(
    app_handle: tauri::AppHandle,
    state: State<'_, AppState>,
    group_id: String,
    file_path: String,
) -> Result<ChatMessage, String> {
    send_group_file_with_kind(app_handle, state, group_id, file_path, "file").await
}

#[tauri::command]
pub async fn send_group_sticker(
    app_handle: tauri::AppHandle,
    state: State<'_, AppState>,
    group_id: String,
    file_path: String,
) -> Result<ChatMessage, String> {
    send_group_file_with_kind(app_handle, state, group_id, file_path, "sticker").await
}

async fn send_group_file_with_kind(
    app_handle: tauri::AppHandle,
    state: State<'_, AppState>,
    group_id: String,
    file_path: String,
    file_kind: &str,
) -> Result<ChatMessage, String> {
    use chrono::Utc;
    let (my_id, my_name, my_department, listen_port, db, members) = {
        let runtime_opt = { state.runtime.read().await.clone() };
        let r = runtime_opt.as_ref().ok_or("未初始化")?;
        let my_name = state.profile.lock().await.as_ref().map(|p| p.username.clone()).unwrap_or_default();
        let my_department = state.profile.lock().await.as_ref().map(|p| p.department.clone()).unwrap_or_default();
        let members = state.db.get_group_members(&group_id).await.map_err(|e| e.to_string())?;
        (r.my_id.clone(), my_name, my_department, r.listen_port, state.db.clone(), members)
    };

    let file_name = std::path::Path::new(&file_path)
        .file_name().and_then(|n| n.to_str()).unwrap_or("unknown").to_string();
    let file_size = tokio::fs::metadata(&file_path).await
        .map(|m| m.len() as i64).map_err(|e| e.to_string())?;
    let pending_cache: Arc<tokio::sync::Mutex<Option<String>>> = Arc::new(tokio::sync::Mutex::new(None));
    let msg_kind = if file_kind == "sticker" { "sticker" } else { "file" };
    let content = if msg_kind == "sticker" {
        "[表情]".to_string()
    } else {
        format!("📎 {}", file_name)
    };

    // Save outgoing file message to the group conversation (read=true for sender)
    let saved = db.save_group_message(
        &group_id, &my_id, &my_name,
        &content, msg_kind,
        Some(&file_path), Some(&file_name), Some(file_size), true,
    ).await.map_err(|e| e.to_string())?;

    // Get online peer snapshot once
    let online_peers = {
        let runtime_opt = { state.runtime.read().await.clone() };
        match runtime_opt.as_ref() {
            Some(r) => r.discovery.read().await.get_peers(),
            None => vec![],
        }
    };

    for member in members {
        if member.peer_id == my_id { continue; }
        let bg_path = file_path.clone();
        let bg_name = file_name.clone();
        let bg_my_id = my_id.clone();
        let bg_my_name = my_name.clone();
        let bg_my_dep = my_department.clone();
        let bg_db = db.clone();
        let bg_handle = app_handle.clone();
        let bg_error_handle = app_handle.clone();
        let bg_gid = group_id.clone();
        let bg_pending_cache = pending_cache.clone();
        let bg_kind = msg_kind.to_string();
        let mut target_id = member.peer_id.clone();
        let target_name = member.username.clone();
        let target_department = member.department.clone();
        let mut resolved_addr = resolve_peer_addr(&target_id, &db, &online_peers).await;
        if resolved_addr.is_none() && !target_name.is_empty() {
            if let Ok(Some(latest_peer)) = db.find_peer_by_identity(&target_name, &target_department).await {
                if latest_peer.peer_id != target_id {
                    log::info!(
                        "Group file target {} resolved by identity to latest peer_id {}",
                        target_id,
                        latest_peer.peer_id
                    );
                    target_id = latest_peer.peer_id.clone();
                    resolved_addr = resolve_peer_addr(&target_id, &db, &online_peers).await;
                }
            }
        }
        if let Some((ip, port)) = resolved_addr {
            let peer = crate::discovery::Peer::new(
                target_id.clone(),
                target_name,
                target_department,
                ip, port,
            );
            match send_group_file_to_peer_with_progress(
                &bg_path, &bg_name, file_size, &peer,
                &bg_my_id, &bg_my_name, &bg_my_dep, listen_port,
                &bg_gid, &bg_kind, &bg_handle,
            ).await {
                Ok(_) => log::info!("Group file sent to {}", peer.id),
                Err(e) => {
                    log::error!("Group file send failed to {}: {}", peer.id, e);
                    if let Err(queue_err) = queue_group_file_for_peer(
                        &bg_db, &bg_path, &bg_name, file_size, bg_pending_cache,
                        &peer.id, &bg_my_id, &bg_my_name, &bg_my_dep,
                        listen_port, &bg_gid, &bg_kind,
                    ).await {
                        log::error!("Failed to queue group file for {}: {}", peer.id, queue_err);
                        let _ = bg_error_handle.emit_all("file-error", serde_json::json!({
                            "fileName": bg_name,
                            "error": queue_err,
                        }));
                    }
                }
            }
        } else {
            if let Err(e) = queue_group_file_for_peer(
                &bg_db, &bg_path, &bg_name, file_size, bg_pending_cache,
                &target_id, &bg_my_id, &bg_my_name, &bg_my_dep,
                listen_port, &bg_gid, &bg_kind,
            ).await {
                log::error!("Failed to queue group file for {}: {}", target_id, e);
                let _ = bg_error_handle.emit_all("file-error", serde_json::json!({
                    "fileName": bg_name,
                    "error": e,
                }));
            } else {
                log::info!("Queued group file for offline member {}", target_id);
            }
        }
    }

    let _ = app_handle.emit_all("file-progress", serde_json::json!({
        "fileName": file_name,
        "sent": file_size,
        "total": file_size,
        "speed": 0,
    }));

    Ok(ChatMessage {
        id: saved.id,
        sender_id: my_id,
        sender_name: my_name,
        receiver_id: String::new(),
        content,
        msg_type: msg_kind.to_string(),
        file_name: Some(file_name),
        file_path: Some(file_path),
        file_size: Some(file_size),
        timestamp: Utc::now().to_rfc3339(),
        is_read: true,
    })
}

async fn send_group_file_to_peer_with_progress(
    file_path: &str,
    file_name: &str,
    file_size: i64,
    peer: &Peer,
    my_id: &str,
    my_name: &str,
    my_department: &str,
    listen_port: u16,
    group_id: &str,
    file_kind: &str,
    app_handle: &tauri::AppHandle,
) -> Result<(), String> {
    let transfer = crate::db::PendingFileTransfer {
        id: 0,
        group_id: group_id.to_string(),
        peer_id: peer.id.clone(),
        sender_id: my_id.to_string(),
        sender_name: my_name.to_string(),
        sender_department: my_department.to_string(),
        sender_port: listen_port,
        file_path: file_path.to_string(),
        file_name: file_name.to_string(),
        file_size,
        file_kind: file_kind.to_string(),
    };

    let _ = app_handle.emit_all("file-progress", serde_json::json!({
        "fileName": file_name,
        "sent": 0,
        "total": file_size,
        "speed": 0,
    }));

    send_group_file_payloads_over_tcp(&peer.address(), &transfer)
        .await
        .then_some(())
        .ok_or_else(|| format!("Failed to send file to {}", peer.address()))?;

    let _ = app_handle.emit_all("file-progress", serde_json::json!({
        "fileName": file_name,
        "sent": file_size,
        "total": file_size,
        "speed": 0,
    }));

    Ok(())
}

async fn queue_group_file_for_peer(
    db: &crate::db::Database,
    file_path: &str,
    file_name: &str,
    file_size: i64,
    pending_cache: Arc<tokio::sync::Mutex<Option<String>>>,
    peer_id: &str,
    my_id: &str,
    my_name: &str,
    my_department: &str,
    listen_port: u16,
    group_id: &str,
    file_kind: &str,
) -> Result<(), String> {
    let cached_file_path = get_or_create_pending_cache(file_path, file_name, &pending_cache).await?;
    db.queue_pending_file_transfer(
        group_id,
        peer_id,
        my_id,
        my_name,
        my_department,
        listen_port,
        &cached_file_path,
        file_name,
        file_size,
        file_kind,
    ).await.map_err(|e| e.to_string())
}

async fn get_or_create_pending_cache(
    file_path: &str,
    file_name: &str,
    pending_cache: &Arc<tokio::sync::Mutex<Option<String>>>,
) -> Result<String, String> {
    let mut guard = pending_cache.lock().await;
    if let Some(path) = guard.as_ref() {
        return Ok(path.clone());
    }

    let copied = copy_file_to_pending_cache(file_path, file_name).await?;
    *guard = Some(copied.clone());
    Ok(copied)
}

async fn copy_file_to_pending_cache(file_path: &str, file_name: &str) -> Result<String, String> {
    let pending_dir = echo_files_dir().join("pending");
    tokio::fs::create_dir_all(&pending_dir).await.map_err(|e| e.to_string())?;

    let safe_name = std::path::Path::new(file_name)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file");
    let dest = pending_dir.join(format!("{}_{}", chrono::Utc::now().timestamp_millis(), safe_name));
    tokio::fs::copy(file_path, &dest).await.map_err(|e| e.to_string())?;
    Ok(dest.to_string_lossy().to_string())
}

#[tauri::command]
pub async fn get_group_unread_counts(state: State<'_, AppState>) -> Result<Vec<crate::db::GroupUnread>, String> {
    let runtime_opt = { state.runtime.read().await.clone() };
    let Some(runtime) = runtime_opt.as_ref() else {
        return Ok(vec![]);
    };
    state.db.get_group_unread_counts(&runtime.my_id).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn mark_group_read(state: State<'_, AppState>, group_id: String) -> Result<(), String> {
    let runtime_opt = { state.runtime.read().await.clone() };
    let Some(runtime) = runtime_opt.as_ref() else {
        return Ok(());
    };
    state.db.mark_group_read(&group_id, &runtime.my_id).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn deliver_pending(state: State<'_, AppState>, peer_id: String) -> Result<(), String> {
    let pending = state.db.get_pending_for_peer(&peer_id).await.map_err(|e| e.to_string())?;
    if pending.is_empty() { return Ok(()); }

    let (_my_id, listen_port) = {
        let runtime_opt = { state.runtime.read().await.clone() };
        let r = runtime_opt.as_ref().ok_or("未初始化")?;
        (r.my_id.clone(), r.listen_port)
    };

    let mut delivered_ids = Vec::new();
    for p in &pending {
        let stored_opt = state.db.get_stored_peer(&peer_id).await.unwrap_or_default();
        let ip = stored_opt.as_ref().and_then(|sp| sp.ip.parse::<std::net::IpAddr>().ok());
        let port = stored_opt.as_ref().map(|sp| sp.port).unwrap_or(0);
        if let Some(ip) = ip {
            if !ip.is_unspecified() && port != 0 {
                let addr = format!("{}:{}", ip, port);
                let wm = crate::chat::WireMessage {
                    sender_id: p.sender_id.clone(), sender_name: p.sender_name.clone(),
                    sender_department: String::new(), sender_port: listen_port,
                    receiver_id: peer_id.clone(), content: p.content.clone(),
                    msg_type: p.msg_type.clone(), file_name: None, file_size: None, file_data: None,
                    file_kind: None,
                    known_peers: Vec::new(), group_id: Some(p.group_id.clone()),
                };
                let json = serde_json::to_string(&wm).unwrap_or_default();
                if let Ok(mut stream) = tokio::time::timeout(std::time::Duration::from_secs(3), tokio::net::TcpStream::connect(&addr)).await.map_err(|_| "")? {
                    use tokio::io::AsyncWriteExt;
                    if stream.write_all(json.as_bytes()).await.is_ok() {
                        delivered_ids.push(p.id);
                    }
                }
            }
        }
    }
    if !delivered_ids.is_empty() {
        state.db.delete_pending_msgs(&delivered_ids).await.map_err(|e| e.to_string())?;
        log::info!("Delivered {} pending messages to {}", delivered_ids.len(), peer_id);
    }
    Ok(())
}

/// Deliver pending notifications (any kind) to a peer that just came back online.
pub async fn deliver_pending_to_peer(db: &crate::db::Database, peer_id_str: &str) {
    // Resolve peer address.
    let stored = match db.get_stored_peer(peer_id_str).await {
        Ok(Some(p)) => p,
        _ => return,
    };
    let ip: std::net::IpAddr = match stored.ip.parse() {
        Ok(ip) => ip,
        Err(_) => return,
    };
    if ip.is_unspecified() || stored.port == 0 { return; }
    let addr = format!("{}:{}", ip, stored.port);

    // 1) Generic pending_notifications (preferred — payload is a full WireMessage).
    let notifs = db.get_pending_notifications(peer_id_str).await.unwrap_or_default();
    let delivered_notif_ids = deliver_pending_payloads_over_tcp(&addr, &notifs).await;
    if !delivered_notif_ids.is_empty() {
        let _ = db.delete_pending_notifications(&delivered_notif_ids).await;
        log::info!("Delivered {} pending notifs to {}", delivered_notif_ids.len(), peer_id_str);
    }

    // 2) Legacy pending_group_messages — still drained for backward-compat with
    // any data queued by previous app versions.
    deliver_pending_file_transfers_to_peer(db, peer_id_str, &stored, ip).await;

    let pending = db.get_pending_for_peer(peer_id_str).await.unwrap_or_default();
    if pending.is_empty() { return; }
    let mut delivered = Vec::new();
    for p in &pending {
        let wm = serde_json::json!({
            "sender_id": p.sender_id, "sender_name": p.sender_name,
            "sender_department": "", "sender_port": stored.port,
            "receiver_id": peer_id_str, "content": p.content,
            "msg_type": p.msg_type, "group_id": p.group_id,
            "known_peers": [], "file_name": null, "file_size": null, "file_data": null,
        });
        let json = serde_json::to_string(&wm).unwrap_or_default();
        if deliver_over_tcp(&addr, &json).await {
            delivered.push(p.id);
        }
    }
    if !delivered.is_empty() {
        let _ = db.delete_pending_msgs(&delivered).await;
        log::info!("Delivered {} legacy pending msgs to {}", delivered.len(), peer_id_str);
    }
}

async fn deliver_pending_file_transfers_to_peer(
    db: &crate::db::Database,
    peer_id: &str,
    stored: &StoredPeer,
    ip: std::net::IpAddr,
) {
    let pending_files = db.get_pending_file_transfers(peer_id).await.unwrap_or_default();
    if pending_files.is_empty() {
        return;
    }
    let addr = format!("{}:{}", ip, stored.port);

    for transfer in pending_files {
        if !std::path::Path::new(&transfer.file_path).exists() {
            log::error!("Pending group file missing for {}: {}", peer_id, transfer.file_path);
            let _ = db.delete_pending_file_transfer(transfer.id).await;
            continue;
        }

        let ok = send_group_file_payloads_over_tcp(&addr, &transfer).await;
        if !ok {
            log::error!("Failed to deliver pending group file {} to {}", transfer.file_name, peer_id);
            break;
        }

        let file_path = transfer.file_path.clone();
        let _ = db.delete_pending_file_transfer(transfer.id).await;
        if db.count_pending_file_transfers_by_path(&file_path).await.unwrap_or(1) == 0 {
            let _ = tokio::fs::remove_file(&file_path).await;
        }
        log::info!("Delivered pending group file {} to {}", transfer.file_name, peer_id);
    }
}

async fn send_group_file_payloads_over_tcp(
    addr: &str,
    transfer: &crate::db::PendingFileTransfer,
) -> bool {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const CHUNK_SIZE: usize = 48 * 1024;

    let mut file = match tokio::fs::File::open(&transfer.file_path).await {
        Ok(file) => file,
        Err(_) => return false,
    };
    let stream = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        tokio::net::TcpStream::connect(addr),
    ).await;
    let Ok(Ok(mut stream)) = stream else {
        return false;
    };

    let mut buf = vec![0u8; CHUNK_SIZE];
    let mut chunk_index: usize = 0;
    loop {
        let n = match file.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => return false,
        };
        let is_last = n < CHUNK_SIZE
            || (transfer.file_size as usize) <= ((chunk_index + 1) * CHUNK_SIZE);
        let msg = crate::chat::WireMessage {
            sender_id: transfer.sender_id.clone(),
            sender_name: transfer.sender_name.clone(),
            sender_department: transfer.sender_department.clone(),
            sender_port: transfer.sender_port,
            receiver_id: transfer.peer_id.clone(),
            content: base64_encode_std(&buf[..n]),
            msg_type: if is_last { "file_end".to_string() } else { "file_chunk".to_string() },
            file_name: Some(transfer.file_name.clone()),
            file_size: Some(transfer.file_size as u64),
            file_data: None,
            file_kind: Some(transfer.file_kind.clone()),
            known_peers: Vec::new(),
            group_id: Some(transfer.group_id.clone()),
        };
        let Ok(json) = serde_json::to_string(&msg) else {
            return false;
        };
        if stream.write_all(json.as_bytes()).await.is_err()
            || stream.write_all(b"\n").await.is_err()
        {
            return false;
        }
        chunk_index += 1;
    }

    stream.flush().await.is_ok()
}

fn emoji_dir() -> std::path::PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| "/tmp".to_string());
    std::path::PathBuf::from(home).join("Echo").join("emojis")
}

fn echo_files_dir() -> std::path::PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| "/tmp".to_string());
    std::path::PathBuf::from(home).join("Echo").join("files")
}

/// Build a WireMessage JSON for a notification.
fn build_notification_json(
    sender_id: &str, sender_name: &str, sender_department: &str, sender_port: u16,
    receiver_id: &str, content: &str, msg_type: &str,
    group_id: Option<&str>, file_name: Option<&str>,
    known_peers: &[crate::discovery::PeerEntry],
) -> String {
    serde_json::json!({
        "sender_id": sender_id,
        "sender_name": sender_name,
        "sender_department": sender_department,
        "sender_port": sender_port,
        "receiver_id": receiver_id,
        "content": content,
        "msg_type": msg_type,
        "group_id": group_id,
        "file_name": file_name,
        "file_size": null,
        "file_data": null,
        "known_peers": known_peers,
    }).to_string()
}

/// Try a TCP delivery with a 2s timeout. Returns true if both header bytes
/// and the trailing newline were written successfully.
async fn deliver_over_tcp(addr: &str, json: &str) -> bool {
    match tokio::time::timeout(
        std::time::Duration::from_secs(2),
        tokio::net::TcpStream::connect(addr),
    ).await {
        Ok(Ok(mut stream)) => {
            use tokio::io::AsyncWriteExt;
            stream.write_all(json.as_bytes()).await.is_ok()
                && stream.write_all(b"\n").await.is_ok()
        }
        _ => false,
    }
}

async fn deliver_pending_payloads_over_tcp(
    addr: &str,
    payloads: &[(i64, String, String)],
) -> Vec<i64> {
    if payloads.is_empty() {
        return Vec::new();
    }

    let mut delivered = Vec::new();
    let stream = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        tokio::net::TcpStream::connect(addr),
    ).await;

    let Ok(Ok(mut stream)) = stream else {
        return delivered;
    };

    use tokio::io::AsyncWriteExt;
    for (id, _kind, payload) in payloads {
        if stream.write_all(payload.as_bytes()).await.is_err()
            || stream.write_all(b"\n").await.is_err()
        {
            break;
        }
        delivered.push(*id);
    }
    if stream.flush().await.is_err() {
        return Vec::new();
    }

    delivered
}

/// Generic "fan-out a notification to a set of peers, queue if offline".
///
/// `kind` is the logical type used for offline queueing (matches the wire
/// `msg_type`). The receiver's TCP handler dispatches by `msg_type` inside the
/// payload, so we don't need separate tables per kind.
///
/// Returns the count of (delivered, queued).
pub async fn send_or_queue_notification(
    db: &crate::db::Database,
    online_peers: &[Peer],
    target_peer_ids: &[String],
    self_id: &str,
    self_name: &str,
    self_department: &str,
    self_port: u16,
    content: &str,
    kind: &str,
    group_id: Option<&str>,
    file_name: Option<&str>,
    known_peers: &[crate::discovery::PeerEntry],
) -> (usize, usize) {
    let mut delivered = 0usize;
    let mut queued = 0usize;

    for pid in target_peer_ids {
        if pid == self_id { continue; }

        let json = build_notification_json(
            self_id, self_name, self_department, self_port,
            pid, content, kind, group_id, file_name, known_peers,
        );

        let ok = match resolve_peer_addr(pid, db, online_peers).await {
            Some((ip, port)) => {
                let addr = format!("{}:{}", ip, port);
                deliver_over_tcp(&addr, &json).await
            }
            None => false,
        };

        if ok {
            delivered += 1;
        } else {
            let _ = db.queue_pending_notification(pid, kind, &json).await;
            queued += 1;
        }
    }

    if queued > 0 {
        log::info!("Notification '{}' delivered={} queued={}", kind, delivered, queued);
    }
    (delivered, queued)
}

async fn resolve_peer_addr(
    peer_id: &str,
    db: &crate::db::Database,
    online_peers: &[Peer],
) -> Option<(std::net::IpAddr, u16)> {
    if let Some(p) = online_peers.iter().find(|p| p.id == peer_id) {
        if !p.ip.is_unspecified() && p.port != 0 {
            return Some((p.ip, p.port));
        }
    }
    if let Ok(Some(sp)) = db.get_stored_peer(peer_id).await {
        if let Ok(ip) = sp.ip.parse::<std::net::IpAddr>() {
            if !ip.is_unspecified() && sp.port != 0 {
                return Some((ip, sp.port));
            }
        }
    }
    None
}

/// Build a full PeerEntry list for the given member ids, including ourselves.
/// Used so receivers of `group_created` can populate their local `peers` table
/// with usernames/departments even for members they've never directly contacted.
async fn build_member_directory(
    db: &crate::db::Database,
    online_peers: &[Peer],
    member_ids: &[String],
    self_id: &str,
    self_name: &str,
    self_department: &str,
    self_port: u16,
) -> Vec<crate::discovery::PeerEntry> {
    let my_ip = local_ip_address::local_ip()
        .map(|ip| ip.to_string())
        .unwrap_or_default();
    let mut out = Vec::with_capacity(member_ids.len());
    for id in member_ids {
        if id == self_id {
            out.push(crate::discovery::PeerEntry {
                id: self_id.to_string(),
                username: self_name.to_string(),
                department: self_department.to_string(),
                ip: my_ip.clone(),
                port: self_port,
            });
            continue;
        }
        if let Some(p) = online_peers.iter().find(|p| p.id == *id) {
            out.push(crate::discovery::PeerEntry {
                id: p.id.clone(),
                username: p.username.clone(),
                department: p.department.clone(),
                ip: p.ip.to_string(),
                port: p.port,
            });
            continue;
        }
        if let Ok(Some(sp)) = db.get_stored_peer(id).await {
            out.push(crate::discovery::PeerEntry {
                id: sp.peer_id,
                username: sp.username,
                department: sp.department,
                ip: sp.ip,
                port: sp.port,
            });
        } else {
            // Member we have no info about — still ship the id so receivers know they exist
            out.push(crate::discovery::PeerEntry {
                id: id.clone(),
                username: String::new(),
                department: String::new(),
                ip: String::new(),
                port: 0,
            });
        }
    }
    out
}
