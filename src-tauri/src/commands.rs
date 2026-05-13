use serde::{Deserialize, Serialize};
use tauri::State;

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
        Ok(AppInfo {
            initialized: true,
            peer_id: runtime.my_id.clone(),
            username: profile.username,
            department: profile.department,
            listen_port: runtime.listen_port,
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
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

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
    chat.send_message(&peer, &content)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn send_file(
    state: State<'_, AppState>,
    peer_id: String,
    file_path: String,
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
    chat.send_file(&peer, &file_path)
        .await
        .map_err(|e| e.to_string())
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
            .args(["/C", "start", "", &path])
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

fn echo_files_dir() -> std::path::PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| "/tmp".to_string());
    std::path::PathBuf::from(home).join("Echo").join("files")
}
