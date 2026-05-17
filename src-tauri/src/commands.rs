use log::{error, info};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::chat::send_file_in_background;
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
    let runtime = state.runtime.lock().await;

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

    if let Some(runtime) = state.runtime.lock().await.as_ref() {
        let peers = runtime.discovery.lock().await.get_peers();
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

    let runtime_guard = state.runtime.lock().await;
    if let Some(runtime) = runtime_guard.as_ref() {
        runtime
            .update_profile(username, department)
            .await
            .map_err(|e| e.to_string())?;
    } else {
        drop(runtime_guard);

        let listen_port = std::env::var("ECHO_PORT")
            .ok()
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(9527);

        let profile = state.profile.lock().await.clone().unwrap();
        let runtime = RuntimeServices::start(state.db.clone(), &profile, listen_port)
            .await
            .map_err(|e| e.to_string())?;
        *state.runtime.lock().await = Some(runtime);
    }

    Ok(())
}

#[tauri::command]
pub async fn list_stored_peers(state: State<'_, AppState>) -> Result<Vec<StoredPeer>, String> {
    state.db.list_stored_peers().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_peers(state: State<'_, AppState>) -> Result<Vec<Peer>, String> {
    let runtime = state.runtime.lock().await;
    let Some(runtime) = runtime.as_ref() else {
        return Ok(vec![]);
    };
    let discovery = runtime.discovery.lock().await;
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
    let runtime = state.runtime.lock().await;
    let Some(runtime) = runtime.as_ref() else {
        return Err("应用尚未初始化用户信息".to_string());
    };

    let discovery = runtime.discovery.lock().await;
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
    info!("send_file: start");
    let t0 = std::time::Instant::now();
    let runtime = state.runtime.lock().await;
    let Some(runtime) = runtime.as_ref() else {
        return Err("应用尚未初始化用户信息".to_string());
    };

    let discovery = runtime.discovery.lock().await;
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
    drop(runtime);

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
    tauri::async_runtime::spawn(async move {
        match send_file_in_background(&bg_path, &bg_name, &bg_peer, my_id, my_name, my_department, listen_port, db, peers_arc, handle).await {
            Ok(msg) => info!("File sent: {}", msg.content),
            Err(e) => error!("File send failed: {}", e),
        }
    });

    info!("send_file: returning placeholder ({:?} total)", t0.elapsed());
    use chrono::Utc;
    Ok(ChatMessage {
        id: 0,
        sender_id: placeholder_my_id,
        sender_name: placeholder_my_name,
        receiver_id: placeholder_peer_id,
        content: format!("📎 {}", file_name),
        msg_type: "file".to_string(),
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
        let runtime = state.runtime.lock().await;
        match runtime.as_ref() {
            Some(r) => (r.my_id.clone(), r.listen_port),
            None => return Err("应用尚未初始化".to_string()),
        }
    };

    // Send our announce as a unicast UDP probe to the remote peer's discovery port.
    // Include our own known_peers so they also get our contacts (bidirectional relay).
    let our_known: Vec<serde_json::Value> = {
        let runtime = state.runtime.lock().await;
        if let Some(runtime) = runtime.as_ref() {
            runtime.discovery.lock().await.get_peers().into_iter()
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

    for attempt in 0..3 {
        if let Ok(sock) = std::net::UdpSocket::bind("0.0.0.0:0") {
            let _ = sock.set_broadcast(true);
            let _ = sock.send_to(&probe_bytes, &target);
        }

        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        // Check if the peer was auto-registered from the unicast response
        let found = {
            let runtime = state.runtime.lock().await;
            if let Some(runtime) = runtime.as_ref() {
                runtime.discovery.lock().await
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
        return Ok(DiscoverResult {
            online: true,
            message: format!("已连接 {} ({}) @ {}:{}", found.username, found.department, found.ip, found.port),
        });
    }

    // Fallback: no unicast response received, register manually
    let pid = format!("{}:{}", ip, port);
    let peer = crate::discovery::Peer::new(
        pid.clone(),
        "手动添加".to_string(),
        String::new(),
        parsed_ip,
        port,
    );

    {
        let runtime = state.runtime.lock().await;
        if let Some(runtime) = runtime.as_ref() {
            let disc = runtime.discovery.lock().await;
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
    // Save to DB
    let _ = state
        .db
        .upsert_peer(&pid, "手动添加", "", &ip, port, true)
        .await;

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
    let runtime = state.runtime.lock().await;
    let Some(runtime) = runtime.as_ref() else {
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
    let runtime = state.runtime.lock().await;
    let Some(runtime) = runtime.as_ref() else {
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
    let runtime = state.runtime.lock().await;
    let Some(runtime) = runtime.as_ref() else {
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
    let runtime = state.runtime.lock().await;
    let Some(runtime) = runtime.as_ref() else {
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
    if let Some(runtime) = state.runtime.lock().await.as_ref() {
        let disc = runtime.discovery.lock().await;
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

    if let Some(runtime) = state.runtime.lock().await.as_ref() {
        runtime.discovery.lock().await.update_scan_subnets(&subnets);
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
        let runtime = state.runtime.lock().await;
        runtime.as_ref().map(|r| r.my_id.clone()).unwrap_or_default()
    };
    let mut all_members = payload.members.clone();
    if !all_members.iter().any(|m| m == &my_id) {
        all_members.push(my_id.clone());
    }
    state.db.create_group(&gid, &payload.name, &my_id, &all_members).await.map_err(|e| e.to_string())?;
    let members = state.db.get_group_members(&gid).await.unwrap_or_default();

    // Notify online members about the new group via TCP
    let (listen_port, online_peers) = {
        let runtime = state.runtime.lock().await;
        match runtime.as_ref() {
            Some(r) => (r.listen_port, r.discovery.lock().await.get_peers()),
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
    let timestamp = chrono::Utc::now().to_rfc3339();
    for m in &all_members {
        if m == &my_id { continue; }
        let Some((ip, port)) = resolve_peer_addr(m, &state.db, &online_peers).await else { continue; };

        let notify = serde_json::json!({
            "sender_id": my_id, "sender_name": my_name, "sender_department": my_department, "sender_port": listen_port,
            "receiver_id": m, "content": serde_json::json!({
                "name": payload.name, "member_ids": all_members,
            }).to_string(),
            "msg_type": "group_created", "group_id": gid, "known_peers": directory,
            "file_name": null, "file_size": null, "file_data": null,
        });
        let json = serde_json::to_string(&notify).unwrap_or_default();
        let addr = format!("{}:{}", ip, port);
        let delivered = match tokio::time::timeout(std::time::Duration::from_secs(2), tokio::net::TcpStream::connect(&addr)).await {
            Ok(Ok(mut stream)) => {
                use tokio::io::AsyncWriteExt;
                stream.write_all(json.as_bytes()).await.is_ok() && stream.write_all(b"\n").await.is_ok()
            }
            _ => false,
        };
        if !delivered {
            let content = serde_json::json!({"name": payload.name, "member_ids": all_members}).to_string();
            let _ = state.db.store_pending_group_msg(&gid, m, &my_id, &my_name, &content, "group_created", &timestamp).await;
        }
    }

    Ok(crate::db::GroupInfo {
        group_id: gid, name: payload.name, creator_id: my_id,
        created_at: String::new(), members,
        last_message: None, last_message_at: None, last_message_sender: None, unread_count: 0,
    })
}

#[tauri::command]
pub async fn list_groups(state: State<'_, AppState>) -> Result<Vec<crate::db::GroupInfo>, String> {
    let my_id = {
        let runtime = state.runtime.lock().await;
        runtime.as_ref().map(|r| r.my_id.clone()).unwrap_or_default()
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
    let (my_id, my_name, listen_port, members) = {
        let runtime = state.runtime.lock().await;
        let r = runtime.as_ref().ok_or("未初始化")?;
        let my_name = state.profile.lock().await.as_ref().map(|p| p.username.clone()).unwrap_or_default();
        let members = state.db.get_group_members(&group_id).await.map_err(|e| e.to_string())?;
        (r.my_id.clone(), my_name, r.listen_port, members)
    };

    let timestamp = chrono::Utc::now().to_rfc3339();
    let msg = state.db.save_group_message(&group_id, &my_id, &my_name, &content, &msg_type, None, None, None, true).await.map_err(|e| e.to_string())?;

    let online_peers = {
        let runtime = state.runtime.lock().await;
        match runtime.as_ref() {
            Some(r) => r.discovery.lock().await.get_peers(),
            None => vec![],
        }
    };

    for member in &members {
        if member.peer_id == my_id { continue; }
        let Some((ip, port)) = resolve_peer_addr(&member.peer_id, &state.db, &online_peers).await else { continue; };

        let mut delivered = false;
        let addr = format!("{}:{}", ip, port);
        let wm = crate::chat::WireMessage {
            sender_id: my_id.clone(), sender_name: my_name.clone(),
            sender_department: String::new(), sender_port: listen_port,
            receiver_id: member.peer_id.clone(), content: content.clone(),
            msg_type: msg_type.clone(), file_name: None, file_size: None, file_data: None,
            known_peers: Vec::new(), group_id: Some(group_id.clone()),
        };
        let json = serde_json::to_string(&wm).map_err(|e| e.to_string())?;
        match tokio::time::timeout(std::time::Duration::from_secs(3), tokio::net::TcpStream::connect(&addr)).await {
            Ok(Ok(mut stream)) => {
                use tokio::io::AsyncWriteExt;
                if stream.write_all(json.as_bytes()).await.is_ok() && stream.write_all(b"\n").await.is_ok() {
                    delivered = true;
                }
            }
            _ => {}
        }
        if !delivered {
            let _ = state.db.store_pending_group_msg(&group_id, &member.peer_id, &my_id, &my_name, &content, &msg_type, &timestamp).await;
        }
    }
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
        let runtime = state.runtime.lock().await;
        let r = runtime.as_ref().ok_or("未初始化")?;
        let my_name = state.profile.lock().await.as_ref().map(|p| p.username.clone()).unwrap_or_default();
        let my_dept = state.profile.lock().await.as_ref().map(|p| p.department.clone()).unwrap_or_default();
        let peers = r.discovery.lock().await.get_peers();
        (r.my_id.clone(), my_name, my_dept, r.listen_port, peers)
    };
    let timestamp = chrono::Utc::now().to_rfc3339();

    for m in &members {
        if m.peer_id == my_id { continue; }
        let Some((ip, port)) = resolve_peer_addr(&m.peer_id, &state.db, &online_peers).await else { continue; };
        let wm = crate::chat::WireMessage {
            sender_id: my_id.clone(), sender_name: my_name.clone(),
            sender_department: my_department.clone(), sender_port: listen_port,
            receiver_id: m.peer_id.clone(),
            content: format!("群名已修改为「{}」", new_name),
            msg_type: "group_renamed".to_string(),
            file_name: Some(new_name.clone()),
            file_size: None, file_data: None,
            known_peers: Vec::new(), group_id: Some(group_id.clone()),
        };
        let json = serde_json::to_string(&wm).unwrap_or_default();
        let addr = format!("{}:{}", ip, port);
        let delivered = match tokio::time::timeout(std::time::Duration::from_secs(2), tokio::net::TcpStream::connect(&addr)).await {
            Ok(Ok(mut stream)) => {
                use tokio::io::AsyncWriteExt;
                stream.write_all(json.as_bytes()).await.is_ok() && stream.write_all(b"\n").await.is_ok()
            }
            _ => false,
        };
        if !delivered {
            let _ = state.db.store_pending_group_msg(&group_id, &m.peer_id, &my_id, &my_name, &format!("群名已修改为「{}」", new_name), "group_renamed", &timestamp).await;
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn invite_to_group(state: State<'_, AppState>, group_id: String, members: Vec<String>) -> Result<(), String> {
    // 1) Persist new members locally
    state.db.add_group_members(&group_id, &members).await.map_err(|e| e.to_string())?;

    // 2) Fetch group info + full member list for the broadcast payload
    let groups = {
        let my_id_opt = state.runtime.lock().await.as_ref().map(|r| r.my_id.clone());
        let my_id = my_id_opt.unwrap_or_default();
        state.db.list_groups(&my_id).await.map_err(|e| e.to_string())?
    };
    let group = groups.iter().find(|g| g.group_id == group_id).ok_or("群组不存在")?.clone();
    let member_records = state.db.get_group_members(&group_id).await.map_err(|e| e.to_string())?;
    let all_member_ids: Vec<String> = member_records.iter().map(|m| m.peer_id.clone()).collect();

    let (my_id, listen_port, online_peers) = {
        let runtime = state.runtime.lock().await;
        let r = runtime.as_ref().ok_or("未初始化")?;
        let my_id = r.my_id.clone();
        let listen_port = r.listen_port;
        let peers = r.discovery.lock().await.get_peers();
        (my_id, listen_port, peers)
    };
    let (my_name, my_department) = {
        let p = state.profile.lock().await;
        p.as_ref().map(|p| (p.username.clone(), p.department.clone()))
            .unwrap_or_default()
    };
    let directory = build_member_directory(
        &state.db, &online_peers, &all_member_ids,
        &my_id, &my_name, &my_department, listen_port,
    ).await;
    let timestamp = chrono::Utc::now().to_rfc3339();

    // 3) Broadcast group_created to ALL current members (new + existing) so member tables converge
    for mid in &all_member_ids {
        if mid == &my_id { continue; }
        let Some((ip, port)) = resolve_peer_addr(mid, &state.db, &online_peers).await else { continue; };
        let content = serde_json::json!({"name": group.name, "member_ids": all_member_ids}).to_string();
        let notify = serde_json::json!({
            "sender_id": my_id, "sender_name": my_name, "sender_department": my_department, "sender_port": listen_port,
            "receiver_id": mid, "content": content,
            "msg_type": "group_created", "group_id": group_id, "known_peers": directory,
            "file_name": null, "file_size": null, "file_data": null,
        });
        let json = serde_json::to_string(&notify).unwrap_or_default();
        let addr = format!("{}:{}", ip, port);
        let delivered = match tokio::time::timeout(std::time::Duration::from_secs(2), tokio::net::TcpStream::connect(&addr)).await {
            Ok(Ok(mut stream)) => {
                use tokio::io::AsyncWriteExt;
                stream.write_all(json.as_bytes()).await.is_ok() && stream.write_all(b"\n").await.is_ok()
            }
            _ => false,
        };
        if !delivered {
            let _ = state.db.store_pending_group_msg(&group_id, mid, &my_id, &my_name, &content, "group_created", &timestamp).await;
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn leave_group(state: State<'_, AppState>, group_id: String) -> Result<(), String> {
    let (my_id, listen_port, online_peers, members) = {
        let runtime = state.runtime.lock().await;
        let r = runtime.as_ref().ok_or("未初始化")?;
        let members = state.db.get_group_members(&group_id).await.map_err(|e| e.to_string())?;
        let online = r.discovery.lock().await.get_peers();
        (r.my_id.clone(), r.listen_port, online, members)
    };

    // Group creator cannot leave — must dissolve instead
    let groups = state.db.list_groups(&my_id).await.map_err(|e| e.to_string())?;
    if let Some(g) = groups.iter().find(|g| g.group_id == group_id) {
        if g.creator_id == my_id {
            return Err("群主不可退群，请使用解散群".to_string());
        }
    }
    let (my_name, my_department) = {
        let p = state.profile.lock().await;
        p.as_ref().map(|p| (p.username.clone(), p.department.clone()))
            .unwrap_or_default()
    };

    // Notify remaining members BEFORE removing ourselves
    for m in &members {
        if m.peer_id == my_id { continue; }
        let Some((ip, port)) = resolve_peer_addr(&m.peer_id, &state.db, &online_peers).await else { continue; };
        let notify = serde_json::json!({
            "sender_id": my_id, "sender_name": my_name, "sender_department": my_department, "sender_port": listen_port,
            "receiver_id": m.peer_id, "content": "",
            "msg_type": "group_member_left", "group_id": group_id, "known_peers": [],
            "file_name": null, "file_size": null, "file_data": null,
        });
        let json = serde_json::to_string(&notify).unwrap_or_default();
        let addr = format!("{}:{}", ip, port);
        if let Ok(Ok(mut stream)) = tokio::time::timeout(std::time::Duration::from_secs(2), tokio::net::TcpStream::connect(&addr)).await {
            use tokio::io::AsyncWriteExt;
            let _ = stream.write_all(json.as_bytes()).await;
            let _ = stream.write_all(b"\n").await;
        }
    }

    state.db.remove_group_member(&group_id, &my_id).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn dissolve_group(state: State<'_, AppState>, group_id: String) -> Result<(), String> {
    let members = state.db.get_group_members(&group_id).await.map_err(|e| e.to_string())?;
    let (my_id, listen_port, online_peers) = {
        let runtime = state.runtime.lock().await;
        let r = runtime.as_ref().ok_or("未初始化")?;
        let online = r.discovery.lock().await.get_peers();
        (r.my_id.clone(), r.listen_port, online)
    };
    let (my_name, my_department) = {
        let p = state.profile.lock().await;
        p.as_ref().map(|p| (p.username.clone(), p.department.clone()))
            .unwrap_or_default()
    };

    state.db.dissolve_group(&group_id).await.map_err(|e| e.to_string())?;

    // Notify members via TCP
    for m in &members {
        if m.peer_id == my_id { continue; }
        let Some((ip, port)) = resolve_peer_addr(&m.peer_id, &state.db, &online_peers).await else { continue; };
        let wm = serde_json::json!({
            "sender_id": my_id, "sender_name": my_name, "sender_department": my_department, "sender_port": listen_port,
            "receiver_id": m.peer_id, "content": "群组已解散", "msg_type": "group_dissolved",
            "group_id": group_id, "known_peers": [], "file_name": null, "file_size": null, "file_data": null,
        });
        let addr = format!("{}:{}", ip, port);
        if let Ok(Ok(mut stream)) = tokio::time::timeout(std::time::Duration::from_secs(2), tokio::net::TcpStream::connect(&addr)).await {
            use tokio::io::AsyncWriteExt;
            let json = serde_json::to_string(&wm).unwrap_or_default();
            let _ = stream.write_all(json.as_bytes()).await;
            let _ = stream.write_all(b"\n").await;
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn send_group_file(
    app_handle: tauri::AppHandle,
    state: State<'_, AppState>,
    group_id: String,
    file_path: String,
) -> Result<ChatMessage, String> {
    use chrono::Utc;
    let (my_id, my_name, my_department, listen_port, db, peers_arc, members) = {
        let runtime = state.runtime.lock().await;
        let r = runtime.as_ref().ok_or("未初始化")?;
        let my_name = state.profile.lock().await.as_ref().map(|p| p.username.clone()).unwrap_or_default();
        let my_department = state.profile.lock().await.as_ref().map(|p| p.department.clone()).unwrap_or_default();
        let chat = r.chat.lock().await;
        let members = state.db.get_group_members(&group_id).await.map_err(|e| e.to_string())?;
        (r.my_id.clone(), my_name, my_department, r.listen_port, chat.db().clone(), chat.peers().clone(), members)
    };

    let file_name = std::path::Path::new(&file_path)
        .file_name().and_then(|n| n.to_str()).unwrap_or("unknown").to_string();
    let file_size = tokio::fs::metadata(&file_path).await
        .map(|m| m.len() as i64).map_err(|e| e.to_string())?;

    // Save outgoing file message to the group conversation (read=true for sender)
    let saved = db.save_group_message(
        &group_id, &my_id, &my_name,
        &format!("📎 {}", file_name), "file",
        Some(&file_path), Some(&file_name), Some(file_size), true,
    ).await.map_err(|e| e.to_string())?;

    // Get online peer snapshot once
    let online_peers = {
        let runtime = state.runtime.lock().await;
        match runtime.as_ref() {
            Some(r) => r.discovery.lock().await.get_peers(),
            None => vec![],
        }
    };

    for member in members {
        if member.peer_id == my_id { continue; }
        let Some((ip, port)) = resolve_peer_addr(&member.peer_id, &db, &online_peers).await else { continue; };
        let peer = crate::discovery::Peer::new(
            member.peer_id.clone(),
            member.username.clone(),
            member.department.clone(),
            ip, port,
        );
        let bg_path = file_path.clone();
        let bg_name = file_name.clone();
        let bg_my_id = my_id.clone();
        let bg_my_name = my_name.clone();
        let bg_my_dep = my_department.clone();
        let bg_db = db.clone();
        let bg_peers = peers_arc.clone();
        let bg_handle = app_handle.clone();
        let bg_gid = group_id.clone();
        tauri::async_runtime::spawn(async move {
            match crate::chat::send_file_in_background_grouped(
                &bg_path, &bg_name, &peer,
                bg_my_id, bg_my_name, bg_my_dep, listen_port,
                bg_db, bg_peers, bg_handle, Some(bg_gid),
            ).await {
                Ok(_) => log::info!("Group file sent to {}", peer.id),
                Err(e) => log::error!("Group file send failed to {}: {}", peer.id, e),
            }
        });
    }

    Ok(ChatMessage {
        id: saved.id,
        sender_id: my_id,
        sender_name: my_name,
        receiver_id: String::new(),
        content: format!("📎 {}", file_name),
        msg_type: "file".to_string(),
        file_name: Some(file_name),
        file_path: Some(file_path),
        file_size: Some(file_size),
        timestamp: Utc::now().to_rfc3339(),
        is_read: true,
    })
}

#[tauri::command]
pub async fn get_group_unread_counts(state: State<'_, AppState>) -> Result<Vec<crate::db::GroupUnread>, String> {
    let runtime = state.runtime.lock().await;
    let Some(runtime) = runtime.as_ref() else {
        return Ok(vec![]);
    };
    state.db.get_group_unread_counts(&runtime.my_id).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn mark_group_read(state: State<'_, AppState>, group_id: String) -> Result<(), String> {
    let runtime = state.runtime.lock().await;
    let Some(runtime) = runtime.as_ref() else {
        return Ok(());
    };
    state.db.mark_group_read(&group_id, &runtime.my_id).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn deliver_pending(state: State<'_, AppState>, peer_id: String) -> Result<(), String> {
    let pending = state.db.get_pending_for_peer(&peer_id).await.map_err(|e| e.to_string())?;
    if pending.is_empty() { return Ok(()); }

    let (_my_id, listen_port) = {
        let runtime = state.runtime.lock().await;
        let r = runtime.as_ref().ok_or("未初始化")?;
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

/// Deliver pending group messages to a peer (can be called from anywhere with DB access).
pub async fn deliver_pending_to_peer(db: &crate::db::Database, peer_id_str: &str) {
    let pending = match db.get_pending_for_peer(peer_id_str).await {
        Ok(p) => p,
        Err(_) => return,
    };
    if pending.is_empty() { return; }
    let peer_opt = match db.get_stored_peer(peer_id_str).await {
        Ok(p) => p,
        Err(_) => return,
    };
    let stored = match peer_opt {
        Some(p) => p,
        None => return,
    };
    let ip: std::net::IpAddr = match stored.ip.parse() {
        Ok(ip) => ip,
        Err(_) => return,
    };
    if ip.is_unspecified() || stored.port == 0 { return; }

    let port = stored.port;
    let addr = format!("{}:{}", ip, port);
    let mut delivered = Vec::new();
    for p in &pending {
        let wm = serde_json::json!({
            "sender_id": p.sender_id, "sender_name": p.sender_name,
            "sender_department": "", "sender_port": port,
            "receiver_id": peer_id_str, "content": p.content,
            "msg_type": p.msg_type, "group_id": p.group_id,
            "known_peers": [], "file_name": null, "file_size": null, "file_data": null,
        });
        let json = serde_json::to_string(&wm).unwrap_or_default();
        match tokio::time::timeout(
            std::time::Duration::from_secs(3), tokio::net::TcpStream::connect(&addr)
        ).await {
            Ok(Ok(mut stream)) => {
                use tokio::io::AsyncWriteExt;
                if stream.write_all(json.as_bytes()).await.is_ok() && stream.write_all(b"\n").await.is_ok() {
                    delivered.push(p.id);
                }
            }
            _ => {}
        }
    }
    if !delivered.is_empty() {
        let _ = db.delete_pending_msgs(&delivered).await;
        log::info!("Delivered {} pending msgs to {}", delivered.len(), peer_id_str);
    }
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
