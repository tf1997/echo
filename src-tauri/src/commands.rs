use log::{error, info};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    path::Path,
    sync::{Arc, OnceLock},
};
use tauri::{AppHandle, Manager, State};

use crate::chat::{
    base64_encode_into, base64_encoded_capacity, cancel_outgoing_file_transfer,
    clear_outgoing_file_transfer, emit_contact_message_updated, is_self_peer,
    pause_outgoing_file_transfer, register_outgoing_file_transfer, requires_message_ack,
    resume_outgoing_file_transfer, send_file_in_background_with_kind,
    serialize_file_wire_message_line, wait_for_message_ack, wait_for_outgoing_file_transfer,
    FileWireMessageLine, WireMessage, FILE_CHUNK_SIZE, FILE_SOCKET_BUFFER_SIZE,
    FILE_TRANSFER_CANCELLED_MESSAGE,
};
use crate::db::{ChatMessage, StoredPeer, UnreadCount, UserProfile};
use crate::discovery::Peer;
use crate::state::{AppState, RuntimeServices};

static PENDING_DELIVERY_LOCKS: OnceLock<tokio::sync::Mutex<HashSet<String>>> = OnceLock::new();

async fn try_begin_pending_delivery(peer_id: &str) -> bool {
    let locks = PENDING_DELIVERY_LOCKS.get_or_init(|| tokio::sync::Mutex::new(HashSet::new()));
    locks.lock().await.insert(peer_id.to_string())
}

async fn finish_pending_delivery(peer_id: &str) {
    if let Some(locks) = PENDING_DELIVERY_LOCKS.get() {
        locks.lock().await.remove(peer_id);
    }
}

#[derive(Serialize)]
pub struct AppInfo {
    pub initialized: bool,
    pub peer_id: String,
    pub node_id: String,
    pub username: String,
    pub department: String,
    pub software_version: String,
    pub mac_address: String,
    pub avatar_path: String,
    pub avatar_hash: String,
    pub avatar_updated_at: i64,
    pub listen_port: u16,
    pub my_ip: String,
}

#[derive(Deserialize)]
pub struct SaveProfilePayload {
    pub username: String,
    pub department: String,
    #[serde(default)]
    pub avatar_source_path: Option<String>,
    #[serde(default)]
    pub clear_avatar: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvatarInfo {
    pub avatar_path: String,
    pub avatar_hash: String,
    pub avatar_updated_at: i64,
}

const AVATAR_MAX_BYTES: u64 = 5 * 1024 * 1024;

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
            peer_id: runtime.my_id(),
            node_id: runtime.my_node_id.clone(),
            username: profile.username,
            department: profile.department,
            software_version: crate::profile_metadata::software_version(),
            mac_address: crate::profile_metadata::mac_address(),
            avatar_path: profile.avatar_path,
            avatar_hash: profile.avatar_hash,
            avatar_updated_at: profile.avatar_updated_at,
            listen_port: runtime.listen_port,
            my_ip,
        })
    } else {
        Ok(AppInfo {
            initialized: false,
            peer_id: String::new(),
            node_id: String::new(),
            username: String::new(),
            department: String::new(),
            software_version: crate::profile_metadata::software_version(),
            mac_address: crate::profile_metadata::mac_address(),
            avatar_path: String::new(),
            avatar_hash: String::new(),
            avatar_updated_at: 0,
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
    let mut departments = state
        .db
        .get_departments()
        .await
        .map_err(|e| e.to_string())?;

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
    app_handle: AppHandle,
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

    let avatar_update = if let Some(source_path) = payload
        .avatar_source_path
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty())
    {
        Some(prepare_avatar_info(source_path)?)
    } else if payload.clear_avatar {
        Some(AvatarInfo {
            avatar_path: String::new(),
            avatar_hash: String::new(),
            avatar_updated_at: now_millis(),
        })
    } else {
        None
    };

    let current_runtime_id = { state.runtime.read().await.clone() }
        .as_ref()
        .map(|runtime| runtime.my_id())
        .filter(|peer_id| !peer_id.is_empty());
    let existing_profile_id = state
        .profile
        .lock()
        .await
        .as_ref()
        .map(|profile| profile.peer_id.clone())
        .filter(|peer_id| !peer_id.is_empty());
    let profile_peer_id = current_runtime_id
        .or(existing_profile_id)
        .unwrap_or_default(); // will be set to IP:port by RuntimeServices::start()
    let existing_avatar = state
        .profile
        .lock()
        .await
        .as_ref()
        .map(|profile| AvatarInfo {
            avatar_path: profile.avatar_path.clone(),
            avatar_hash: profile.avatar_hash.clone(),
            avatar_updated_at: profile.avatar_updated_at,
        })
        .unwrap_or_else(|| AvatarInfo {
            avatar_path: String::new(),
            avatar_hash: String::new(),
            avatar_updated_at: 0,
        });

    state
        .db
        .save_user_profile(
            &profile_peer_id,
            username,
            department,
            &crate::profile_metadata::software_version(),
            &crate::profile_metadata::mac_address(),
        )
        .await
        .map_err(|e| e.to_string())?;

    if let Some(avatar) = avatar_update.as_ref() {
        state
            .db
            .update_user_avatar(
                &avatar.avatar_path,
                &avatar.avatar_hash,
                avatar.avatar_updated_at,
            )
            .await
            .map_err(|e| e.to_string())?;
    }

    let final_avatar = avatar_update.unwrap_or(existing_avatar);
    let node_id = state
        .db
        .ensure_user_node_id()
        .await
        .map_err(|e| e.to_string())?;
    let profile = UserProfile {
        peer_id: profile_peer_id,
        node_id,
        username: username.to_string(),
        department: department.to_string(),
        software_version: crate::profile_metadata::software_version(),
        mac_address: crate::profile_metadata::mac_address(),
        avatar_path: final_avatar.avatar_path,
        avatar_hash: final_avatar.avatar_hash,
        avatar_updated_at: final_avatar.avatar_updated_at,
    };

    *state.profile.lock().await = Some(profile.clone());

    let runtime_opt = { state.runtime.read().await.clone() };
    if let Some(runtime) = runtime_opt.as_ref() {
        runtime
            .update_profile(username, department)
            .await
            .map_err(|e| e.to_string())?;

        spawn_profile_updated_notification(&state, profile).await;
    } else {
        let listen_port = std::env::var("ECHO_PORT")
            .ok()
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(9527);

        let profile = state.profile.lock().await.clone().unwrap();
        let relay_tx = state.relay_tx.clone();
        let runtime = RuntimeServices::start(
            app_handle,
            state.db.clone(),
            &profile,
            listen_port,
            relay_tx,
        )
        .await
        .map_err(|e| e.to_string())?;
        *state.runtime.write().await = Some(Arc::new(runtime));
    }

    Ok(())
}

fn now_millis() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn normalized_client_msg_id(client_msg_id: Option<String>) -> Option<String> {
    client_msg_id
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn client_msg_id_or_new(client_msg_id: Option<String>) -> String {
    normalized_client_msg_id(client_msg_id)
        .unwrap_or_else(|| format!("server-{}", uuid::Uuid::new_v4()))
}

fn echo_home_dir() -> std::path::PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| "/tmp".to_string());
    std::path::PathBuf::from(home).join("Echo")
}

fn avatar_root_dir() -> std::path::PathBuf {
    echo_home_dir().join("avatars")
}

fn sanitize_peer_id(peer_id: &str) -> String {
    peer_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn avatar_extension(path: &str) -> Result<String, String> {
    let ext = Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" => Ok(ext),
        _ => Err("请选择 png、jpg、jpeg、gif 或 webp 图片".to_string()),
    }
}

fn image_bytes_match_extension(ext: &str, bytes: &[u8]) -> bool {
    match ext {
        "png" => bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]),
        "jpg" | "jpeg" => bytes.starts_with(&[0xFF, 0xD8, 0xFF]),
        "gif" => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
        "webp" => bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP",
        _ => false,
    }
}

fn hash_avatar_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    hex::encode(digest)
}

fn validate_avatar_bytes(ext: &str, bytes: &[u8]) -> Result<(), String> {
    if bytes.is_empty() {
        return Err("头像文件为空".to_string());
    }
    if bytes.len() as u64 > AVATAR_MAX_BYTES {
        return Err("头像不能超过 5MB".to_string());
    }
    if !image_bytes_match_extension(ext, bytes) {
        return Err("头像文件内容与扩展名不匹配".to_string());
    }
    Ok(())
}

fn write_avatar_file(
    owner_dir: std::path::PathBuf,
    avatar_hash: &str,
    ext: &str,
    bytes: &[u8],
) -> Result<String, String> {
    std::fs::create_dir_all(&owner_dir).map_err(|e| e.to_string())?;
    let dest = owner_dir.join(format!("{}.{}", avatar_hash, ext));
    std::fs::write(&dest, bytes).map_err(|e| e.to_string())?;
    Ok(dest.to_string_lossy().to_string())
}

fn prepare_avatar_info(source_path: &str) -> Result<AvatarInfo, String> {
    let ext = avatar_extension(source_path)?;
    let metadata = std::fs::metadata(source_path).map_err(|e| e.to_string())?;
    if !metadata.is_file() {
        return Err("请选择头像图片文件".to_string());
    }
    if metadata.len() > AVATAR_MAX_BYTES {
        return Err("头像不能超过 5MB".to_string());
    }
    let bytes = std::fs::read(source_path).map_err(|e| e.to_string())?;
    validate_avatar_bytes(&ext, &bytes)?;

    let avatar_hash = hash_avatar_bytes(&bytes);
    let avatar_path =
        write_avatar_file(avatar_root_dir().join("self"), &avatar_hash, &ext, &bytes)?;
    let avatar_updated_at = now_millis();

    Ok(AvatarInfo {
        avatar_path,
        avatar_hash,
        avatar_updated_at,
    })
}

async fn spawn_profile_updated_notification(state: &AppState, profile: UserProfile) {
    let runtime_opt = { state.runtime.read().await.clone() };
    let Some(runtime) = runtime_opt else {
        return;
    };
    let db = Arc::clone(&state.db);

    tauri::async_runtime::spawn(async move {
        notify_profile_updated_to_peers(db, runtime, profile).await;
    });
}

async fn notify_profile_updated_to_peers(
    db: Arc<crate::db::Database>,
    runtime: Arc<RuntimeServices>,
    profile: UserProfile,
) {
    let listen_port = runtime.listen_port;
    let my_id = runtime.my_id();
    let my_node_id = runtime.my_node_id.clone();
    let online_peers = runtime.discovery.read().await.get_peers();
    let stored = db.list_stored_peers().await.unwrap_or_default();
    let mut targets: std::collections::HashSet<String> = std::collections::HashSet::new();
    for p in &online_peers {
        targets.insert(p.id.clone());
    }
    for sp in &stored {
        targets.insert(sp.peer_id.clone());
    }
    targets.remove(&my_id);
    let target_ids: Vec<String> = targets.into_iter().collect();

    let payload = serde_json::json!({
        "username": profile.username,
        "department": profile.department,
        "software_version": profile.software_version,
        "mac_address": profile.mac_address,
        "avatar_hash": profile.avatar_hash,
        "avatar_updated_at": profile.avatar_updated_at,
    })
    .to_string();

    let empty_dir: Vec<crate::discovery::PeerEntry> = Vec::new();
    send_or_queue_notification(
        db.as_ref(),
        &online_peers,
        &target_ids,
        &my_id,
        &my_node_id,
        &profile.username,
        &profile.department,
        listen_port,
        &payload,
        "profile_updated",
        None,
        None,
        None,
        &empty_dir,
    )
    .await;
}

fn spawn_send_or_queue_notification(
    db: Arc<crate::db::Database>,
    online_peers: Vec<Peer>,
    target_peer_ids: Vec<String>,
    self_id: String,
    self_node_id: String,
    self_name: String,
    self_department: String,
    self_port: u16,
    content: String,
    kind: String,
    group_id: Option<String>,
    file_name: Option<String>,
    client_msg_id: Option<String>,
    known_peers: Vec<crate::discovery::PeerEntry>,
) {
    tauri::async_runtime::spawn(async move {
        send_or_queue_notification(
            db.as_ref(),
            &online_peers,
            &target_peer_ids,
            &self_id,
            &self_node_id,
            &self_name,
            &self_department,
            self_port,
            &content,
            &kind,
            group_id.as_deref(),
            file_name.as_deref(),
            client_msg_id.as_deref(),
            &known_peers,
        )
        .await;
    });
}

#[tauri::command]
pub async fn set_profile_avatar(
    state: State<'_, AppState>,
    source_path: String,
) -> Result<AvatarInfo, String> {
    let avatar = prepare_avatar_info(&source_path)?;

    state
        .db
        .update_user_avatar(
            &avatar.avatar_path,
            &avatar.avatar_hash,
            avatar.avatar_updated_at,
        )
        .await
        .map_err(|e| e.to_string())?;

    let updated_profile = {
        let mut guard = state.profile.lock().await;
        let Some(profile) = guard.as_mut() else {
            return Err("应用尚未初始化用户信息".to_string());
        };
        profile.avatar_path = avatar.avatar_path.clone();
        profile.avatar_hash = avatar.avatar_hash.clone();
        profile.avatar_updated_at = avatar.avatar_updated_at;
        profile.clone()
    };

    spawn_profile_updated_notification(&state, updated_profile).await;

    Ok(avatar)
}

#[tauri::command]
pub async fn clear_profile_avatar(state: State<'_, AppState>) -> Result<AvatarInfo, String> {
    let avatar_updated_at = now_millis();
    state
        .db
        .update_user_avatar("", "", avatar_updated_at)
        .await
        .map_err(|e| e.to_string())?;

    let updated_profile = {
        let mut guard = state.profile.lock().await;
        let Some(profile) = guard.as_mut() else {
            return Err("应用尚未初始化用户信息".to_string());
        };
        profile.avatar_path.clear();
        profile.avatar_hash.clear();
        profile.avatar_updated_at = avatar_updated_at;
        profile.clone()
    };

    spawn_profile_updated_notification(&state, updated_profile).await;

    Ok(AvatarInfo {
        avatar_path: String::new(),
        avatar_hash: String::new(),
        avatar_updated_at,
    })
}

#[derive(Deserialize)]
struct AvatarResponsePayload {
    #[serde(default)]
    avatar_hash: String,
    #[serde(default)]
    avatar_updated_at: i64,
    #[serde(default)]
    file_name: String,
    #[serde(default)]
    data: String,
}

fn decode_avatar_response_data(data: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|_| "头像数据解码失败".to_string())
}

#[tauri::command]
pub async fn request_peer_avatar(
    state: State<'_, AppState>,
    peer_id: String,
) -> Result<Option<StoredPeer>, String> {
    let runtime =
        { state.runtime.read().await.clone() }.ok_or_else(|| "应用尚未初始化".to_string())?;

    let online_peer = runtime.discovery.read().await.get_peer(&peer_id);
    let stored_peer = state
        .db
        .get_stored_peer(&peer_id)
        .await
        .map_err(|e| e.to_string())?;

    let peer = if let Some(peer) = online_peer {
        peer
    } else if let Some(stored) = stored_peer.clone() {
        let mut peer = Peer::new_with_avatar(
            stored.peer_id.clone(),
            stored.username.clone(),
            stored.department.clone(),
            stored.software_version.clone(),
            stored.mac_address.clone(),
            stored.avatar_path.clone(),
            stored.avatar_hash.clone(),
            stored.avatar_updated_at,
            stored
                .ip
                .parse()
                .map_err(|_| "无效的联系人 IP 地址".to_string())?,
            stored.port,
        );
        peer.node_id = stored.node_id.clone();
        peer
    } else {
        return Ok(None);
    };

    if peer.avatar_hash.is_empty() {
        return Ok(stored_peer);
    }

    let profile = state
        .profile
        .lock()
        .await
        .clone()
        .ok_or_else(|| "应用尚未初始化用户信息".to_string())?;

    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let stream = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        tokio::net::TcpStream::connect(peer.address()),
    )
    .await
    .map_err(|_| "请求头像超时".to_string())?
    .map_err(|e| e.to_string())?;
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    let request = WireMessage {
        sender_id: runtime.my_id(),
        sender_node_id: runtime.my_node_id.clone(),
        sender_name: profile.username.clone(),
        sender_department: profile.department.clone(),
        sender_software_version: crate::profile_metadata::software_version(),
        sender_mac_address: crate::profile_metadata::mac_address(),
        sender_port: runtime.listen_port,
        receiver_id: peer.id.clone(),
        receiver_node_id: peer.node_id.clone(),
        content: peer.avatar_hash.clone(),
        msg_type: "avatar_request".to_string(),
        file_name: None,
        file_size: None,
        file_data: None,
        file_kind: None,
        known_peers: Vec::new(),
        group_id: None,
        client_msg_id: None,
    };
    let json = serde_json::to_string(&request).map_err(|e| e.to_string())?;
    writer
        .write_all(json.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    writer.write_all(b"\n").await.map_err(|e| e.to_string())?;
    writer.flush().await.map_err(|e| e.to_string())?;

    let line = tokio::time::timeout(std::time::Duration::from_secs(5), lines.next_line())
        .await
        .map_err(|_| "请求头像超时".to_string())?
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "对方未返回头像".to_string())?;
    let msg: WireMessage = serde_json::from_str(&line).map_err(|e| e.to_string())?;
    if msg.msg_type != "avatar_response" {
        return Err("对方返回了无效头像响应".to_string());
    }
    let payload: AvatarResponsePayload =
        serde_json::from_str(&msg.content).map_err(|_| "头像响应格式错误".to_string())?;
    if payload.avatar_hash != peer.avatar_hash || payload.data.is_empty() {
        return Ok(stored_peer);
    }

    let bytes = decode_avatar_response_data(&payload.data)?;
    let calculated_hash = hash_avatar_bytes(&bytes);
    if calculated_hash != payload.avatar_hash {
        return Err("头像校验失败".to_string());
    }
    let ext = avatar_extension(&payload.file_name)?;
    validate_avatar_bytes(&ext, &bytes)?;

    let avatar_path = write_avatar_file(
        avatar_root_dir()
            .join("peers")
            .join(sanitize_peer_id(&peer.id)),
        &payload.avatar_hash,
        &ext,
        &bytes,
    )?;

    state
        .db
        .upsert_peer_with_node_id_avatar(
            &peer.id,
            &peer.node_id,
            &peer.username,
            &peer.department,
            &peer.software_version,
            &peer.mac_address,
            &avatar_path,
            &payload.avatar_hash,
            payload.avatar_updated_at,
            &peer.ip.to_string(),
            peer.port,
            true,
        )
        .await
        .map_err(|e| e.to_string())?;

    let mut updated_peer = Peer::new_with_avatar(
        peer.id.clone(),
        peer.username,
        peer.department,
        peer.software_version,
        peer.mac_address,
        avatar_path,
        payload.avatar_hash,
        payload.avatar_updated_at,
        peer.ip,
        peer.port,
    );
    updated_peer.node_id = peer.node_id.clone();
    runtime.discovery.read().await.register_peer(updated_peer);

    state
        .db
        .get_stored_peer(&peer.id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_stored_peers(state: State<'_, AppState>) -> Result<Vec<StoredPeer>, String> {
    state
        .db
        .list_stored_peers()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn refresh_peer_profile(
    state: State<'_, AppState>,
    peer_id: String,
    ip: String,
    port: u16,
) -> Result<Option<StoredPeer>, String> {
    let my_profile = state.profile.lock().await.clone();
    let (my_id, my_node_id, my_port) = {
        let runtime_opt = { state.runtime.read().await.clone() };
        match runtime_opt.as_ref() {
            Some(runtime) => (
                runtime.my_id(),
                runtime.my_node_id.clone(),
                runtime.listen_port,
            ),
            None => return Err("应用尚未初始化".to_string()),
        }
    };
    let my_name = my_profile
        .as_ref()
        .map(|profile| profile.username.as_str())
        .unwrap_or("");
    let my_department = my_profile
        .as_ref()
        .map(|profile| profile.department.as_str())
        .unwrap_or("");

    let addr = format!("{}:{}", ip, port);
    let Some(identity) =
        probe_identity(&addr, &my_id, &my_node_id, my_name, my_department, my_port).await
    else {
        return state
            .db
            .get_stored_peer(&peer_id)
            .await
            .map_err(|e| e.to_string());
    };

    let remote_port = if identity.port == 0 {
        port
    } else {
        identity.port
    };
    let remote_ip = if identity.ip.is_empty() {
        ip
    } else {
        identity.ip
    };
    let parsed_ip = remote_ip
        .parse::<std::net::IpAddr>()
        .map_err(|_| "无效的联系人 IP 地址".to_string())?;

    state
        .db
        .upsert_peer_with_node_id_avatar(
            &identity.peer_id,
            &identity.node_id,
            &identity.username,
            &identity.department,
            &identity.software_version,
            &identity.mac_address,
            "",
            &identity.avatar_hash,
            identity.avatar_updated_at,
            &remote_ip,
            remote_port,
            true,
        )
        .await
        .map_err(|e| e.to_string())?;

    if let Some(runtime) = { state.runtime.read().await.clone() }.as_ref() {
        let mut peer = crate::discovery::Peer::new_with_avatar(
            identity.peer_id.clone(),
            identity.username,
            identity.department,
            identity.software_version,
            identity.mac_address,
            String::new(),
            identity.avatar_hash,
            identity.avatar_updated_at,
            parsed_ip,
            remote_port,
        );
        peer.node_id = identity.node_id.clone();
        runtime.discovery.read().await.register_peer(peer);
    }

    state
        .db
        .get_stored_peer(&identity.peer_id)
        .await
        .map_err(|e| e.to_string())
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
    app_handle: tauri::AppHandle,
    state: State<'_, AppState>,
    peer_id: String,
    content: String,
    client_msg_id: Option<String>,
) -> Result<ChatMessage, String> {
    send_message_typed(
        app_handle,
        state,
        peer_id,
        content,
        "text".to_string(),
        client_msg_id,
    )
    .await
}

#[tauri::command]
pub async fn send_message_typed(
    app_handle: tauri::AppHandle,
    state: State<'_, AppState>,
    peer_id: String,
    content: String,
    msg_type: String,
    client_msg_id: Option<String>,
) -> Result<ChatMessage, String> {
    let client_msg_id = client_msg_id_or_new(client_msg_id);
    let runtime_opt = { state.runtime.read().await.clone() };
    let Some(runtime) = runtime_opt.as_ref() else {
        return Err("应用尚未初始化用户信息".to_string());
    };

    let discovery = runtime.discovery.read().await;
    let discovered_peer = discovery.get_peer(&peer_id);
    if msg_type == "nudge"
        && !discovered_peer
            .as_ref()
            .map(|peer| peer.online)
            .unwrap_or(false)
    {
        return Err("对方离线，不能发送抖一抖".to_string());
    }
    let peer = if let Some(peer) = discovered_peer {
        peer
    } else if let Some(stored_peer) = state
        .db
        .get_stored_peer(&peer_id)
        .await
        .map_err(|e| e.to_string())?
    {
        Peer::new_with_profile(
            stored_peer.peer_id,
            stored_peer.username,
            stored_peer.department,
            stored_peer.software_version,
            stored_peer.mac_address,
            stored_peer
                .ip
                .parse()
                .map_err(|_| "无效的联系人 IP 地址".to_string())?,
            stored_peer.port,
        )
    } else {
        return Err(format!("Peer {} not found", peer_id));
    };
    drop(discovery);

    let saved = {
        let chat = runtime.chat.lock().await;
        chat.send_message_typed(&peer, &content, &msg_type, Some(client_msg_id.as_str()))
            .await
            .map_err(|e| e.to_string())?
    };
    crate::chat::emit_contact_message_updated(&app_handle, &peer.id, saved.clone());
    Ok(saved)
}

#[tauri::command]
pub async fn send_file(
    app_handle: tauri::AppHandle,
    state: State<'_, AppState>,
    peer_id: String,
    file_path: String,
    file_name: Option<String>,
    client_msg_id: Option<String>,
) -> Result<ChatMessage, String> {
    send_file_with_kind(
        app_handle,
        state,
        peer_id,
        file_path,
        file_name,
        "file",
        client_msg_id,
    )
    .await
}

#[tauri::command]
pub async fn send_sticker(
    app_handle: tauri::AppHandle,
    state: State<'_, AppState>,
    peer_id: String,
    file_path: String,
    file_name: Option<String>,
    client_msg_id: Option<String>,
) -> Result<ChatMessage, String> {
    send_file_with_kind(
        app_handle,
        state,
        peer_id,
        file_path,
        file_name,
        "sticker",
        client_msg_id,
    )
    .await
}

#[tauri::command]
pub async fn pause_file_transfer(client_msg_id: String) -> Result<(), String> {
    if pause_outgoing_file_transfer(client_msg_id.trim()).await {
        Ok(())
    } else {
        Err("发送任务不存在或已完成".to_string())
    }
}

#[tauri::command]
pub async fn resume_file_transfer(client_msg_id: String) -> Result<(), String> {
    if resume_outgoing_file_transfer(client_msg_id.trim()).await {
        Ok(())
    } else {
        Err("发送任务不存在或已完成".to_string())
    }
}

#[tauri::command]
pub async fn cancel_file_transfer(client_msg_id: String) -> Result<(), String> {
    if cancel_outgoing_file_transfer(client_msg_id.trim()).await {
        Ok(())
    } else {
        Err("发送任务不存在或已完成".to_string())
    }
}

async fn send_file_with_kind(
    app_handle: tauri::AppHandle,
    state: State<'_, AppState>,
    peer_id: String,
    file_path: String,
    file_name_override: Option<String>,
    file_kind: &str,
    client_msg_id: Option<String>,
) -> Result<ChatMessage, String> {
    info!("send_file: start ({})", file_kind);
    let client_msg_id = Some(client_msg_id_or_new(client_msg_id));
    let t0 = std::time::Instant::now();
    let runtime_opt = { state.runtime.read().await.clone() };
    let Some(runtime) = runtime_opt.as_ref() else {
        return Err("应用尚未初始化用户信息".to_string());
    };
    let my_id = runtime.my_id();

    let discovery = runtime.discovery.read().await;
    let peer = if let Some(peer) = discovery.get_peer(&peer_id) {
        peer
    } else if let Some(stored_peer) = state
        .db
        .get_stored_peer(&peer_id)
        .await
        .map_err(|e| e.to_string())?
    {
        let last_seen = chrono::DateTime::parse_from_rfc3339(&stored_peer.last_seen_at)
            .map(|dt| dt.timestamp())
            .unwrap_or_default();
        {
            let mut peer = Peer::with_online_details(
                stored_peer.peer_id,
                stored_peer.username,
                stored_peer.department,
                stored_peer.software_version,
                stored_peer.mac_address,
                stored_peer
                    .ip
                    .parse()
                    .map_err(|_| "无效的联系人 IP 地址".to_string())?,
                stored_peer.port,
                stored_peer.is_online,
                last_seen,
            );
            peer.node_id = stored_peer.node_id;
            peer
        }
    } else {
        return Err(format!("Peer {} not found", peer_id));
    };
    drop(discovery);

    let peer_is_self = is_self_peer(&peer, &my_id, runtime.listen_port);
    if !peer_is_self && !peer.online {
        return Err("对方当前离线，文件未发送。请等待对方上线后重试。".to_string());
    }

    // Clone what we need and release the chat lock immediately
    let (my_node_id, my_name, my_department, listen_port, db, peers_arc) = {
        let chat = runtime.chat.lock().await;
        (
            chat.my_node_id().to_string(),
            chat.my_name().to_string(),
            chat.my_department().to_string(),
            chat.listen_port(),
            chat.db().clone(),
            chat.peers().clone(),
        )
    };
    let _ = runtime;

    let file_name = file_name_override
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            std::path::Path::new(&file_path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string()
        });

    if peer_is_self {
        let file_size = tokio::fs::metadata(&file_path)
            .await
            .map(|metadata| metadata.len() as i64)
            .map_err(|e| e.to_string())?;
        let msg_kind = if file_kind == "sticker" {
            "sticker"
        } else {
            "file"
        };
        let content = if msg_kind == "sticker" {
            "[表情]".to_string()
        } else {
            format!("📎 {}", file_name)
        };

        let _ = db.add_recent_contact(&peer.id).await;
        let mut saved = db
            .save_message_dedup_with_delivery(
                &my_id,
                &my_name,
                &peer.id,
                &content,
                msg_kind,
                Some(&file_path),
                Some(&file_name),
                Some(file_size),
                client_msg_id.as_deref(),
                Some(true),
            )
            .await
            .map_err(|e| e.to_string())?;
        if peer.id == my_id {
            let _ = db.mark_read(&peer.id, &my_id).await;
            saved.is_read = true;
        }
        emit_contact_message_updated(&app_handle, &peer.id, saved.clone());
        return Ok(saved);
    }

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
    let bg_client_msg_id = client_msg_id.clone();
    let event_client_msg_id = bg_client_msg_id.clone();
    register_outgoing_file_transfer(event_client_msg_id.as_deref()).await;
    tauri::async_runtime::spawn(async move {
        let clear_client_msg_id = event_client_msg_id.clone();
        match send_file_in_background_with_kind(
            &bg_path,
            &bg_name,
            &bg_peer,
            my_id,
            my_node_id,
            my_name,
            my_department,
            listen_port,
            db,
            peers_arc,
            handle,
            bg_client_msg_id,
            &bg_kind,
        )
        .await
        {
            Ok(msg) => info!("File sent: {}", msg.content),
            Err(e) => {
                error!("File send failed: {}", e);
                let _ = error_handle.emit_all(
                    "file-error",
                    serde_json::json!({
                        "fileName": bg_name,
                        "clientMsgId": event_client_msg_id.as_deref(),
                        "error": e.to_string(),
                    }),
                );
            }
        }
        clear_outgoing_file_transfer(clear_client_msg_id.as_deref()).await;
    });

    let msg_kind = if file_kind == "sticker" {
        "sticker"
    } else {
        "file"
    };
    let content = if msg_kind == "sticker" {
        "[表情]".to_string()
    } else {
        format!("📎 {}", file_name)
    };

    info!(
        "send_file: returning placeholder ({:?} total)",
        t0.elapsed()
    );
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
        client_msg_id,
        delivered: None,
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
    node_id: String,
    username: String,
    department: String,
    software_version: String,
    mac_address: String,
    avatar_hash: String,
    avatar_updated_at: i64,
    ip: String,
    port: u16,
}

async fn probe_identity(
    addr: &str,
    my_id: &str,
    my_node_id: &str,
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
        sender_node_id: my_node_id.to_string(),
        sender_name: my_name.to_string(),
        sender_department: my_department.to_string(),
        sender_software_version: crate::profile_metadata::software_version(),
        sender_mac_address: crate::profile_metadata::mac_address(),
        sender_port: my_port,
        receiver_id: String::new(),
        receiver_node_id: String::new(),
        content: String::new(),
        msg_type: "identity_probe".to_string(),
        file_name: None,
        file_size: None,
        file_data: None,
        file_kind: None,
        known_peers: Vec::new(),
        group_id: None,
        client_msg_id: None,
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

    let avatar_payload: serde_json::Value =
        serde_json::from_str(&msg.content).unwrap_or_else(|_| serde_json::Value::Null);
    let avatar_hash = avatar_payload
        .get("avatar_hash")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string();
    let avatar_updated_at = avatar_payload
        .get("avatar_updated_at")
        .and_then(|value| value.as_i64())
        .unwrap_or_default();

    Some(RemoteIdentity {
        peer_id: msg.sender_id,
        node_id: msg.sender_node_id,
        username: msg.sender_name,
        department: msg.sender_department,
        software_version: msg.sender_software_version,
        mac_address: msg.sender_mac_address,
        avatar_hash,
        avatar_updated_at,
        ip: peer_addr
            .map(|addr| addr.ip().to_string())
            .unwrap_or_else(|| {
                addr.rsplit_once(':')
                    .map(|(ip, _)| ip.to_string())
                    .unwrap_or_default()
            }),
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
    let (my_id, my_node_id, my_port) = {
        let runtime_opt = { state.runtime.read().await.clone() };
        match runtime_opt.as_ref() {
            Some(r) => (r.my_id(), r.my_node_id.clone(), r.listen_port),
            None => return Err("应用尚未初始化".to_string()),
        }
    };

    // Send our announce as a unicast UDP probe to the remote peer's discovery port.
    // Include our own known_peers so they also get our contacts (bidirectional relay).
    let our_known: Vec<serde_json::Value> = {
        let runtime_opt = { state.runtime.read().await.clone() };
        if let Some(runtime) = runtime_opt.as_ref() {
            runtime
                .discovery
                .read()
                .await
                .get_peers()
                .into_iter()
                .filter(|p| {
                    p.online
                        && crate::contact_filter::has_contact_identity(&p.username, &p.department)
                        && p.port != 0
                })
                .map(|p| {
                    serde_json::json!({
                        "id": p.id, "username": p.username, "department": p.department,
                        "software_version": p.software_version,
                        "mac_address": p.mac_address,
                        "avatar_hash": p.avatar_hash,
                        "avatar_updated_at": p.avatar_updated_at,
                        "ip": p.ip.to_string(), "port": p.port,
                    })
                })
                .collect()
        } else {
            vec![]
        }
    };

    let probe = serde_json::json!({
        "id": my_id,
        "node_id": my_node_id,
        "username": my_profile.as_ref().map(|p| p.username.as_str()).unwrap_or(""),
        "department": my_profile.as_ref().map(|p| p.department.as_str()).unwrap_or(""),
        "software_version": crate::profile_metadata::software_version(),
        "mac_address": crate::profile_metadata::mac_address(),
        "avatar_hash": my_profile.as_ref().map(|p| p.avatar_hash.as_str()).unwrap_or(""),
        "avatar_updated_at": my_profile.as_ref().map(|p| p.avatar_updated_at).unwrap_or_default(),
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

    if let Some(identity) =
        probe_identity(&addr, &my_id, &my_node_id, my_name, my_department, my_port).await
    {
        let remote_port = if identity.port == 0 {
            port
        } else {
            identity.port
        };
        let remote_ip = if identity.ip.is_empty() {
            ip.clone()
        } else {
            identity.ip.clone()
        };
        let remote_parsed_ip = remote_ip.parse::<std::net::IpAddr>().unwrap_or(parsed_ip);
        let mut peer = crate::discovery::Peer::new_with_avatar(
            identity.peer_id.clone(),
            identity.username.clone(),
            identity.department.clone(),
            identity.software_version.clone(),
            identity.mac_address.clone(),
            String::new(),
            identity.avatar_hash.clone(),
            identity.avatar_updated_at,
            remote_parsed_ip,
            remote_port,
        );
        peer.node_id = identity.node_id.clone();

        {
            let runtime_opt = { state.runtime.read().await.clone() };
            if let Some(runtime) = runtime_opt.as_ref() {
                let disc = runtime.discovery.read().await;
                disc.register_peer(peer);
            }
        }

        state
            .db
            .upsert_peer_with_node_id_avatar(
                &identity.peer_id,
                &identity.node_id,
                &identity.username,
                &identity.department,
                &identity.software_version,
                &identity.mac_address,
                "",
                &identity.avatar_hash,
                identity.avatar_updated_at,
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
                chat.exchange_contacts(&found_ip, remote_port, &found_id)
                    .await;
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
                runtime
                    .discovery
                    .read()
                    .await
                    .get_peers()
                    .into_iter()
                    .find(|p| p.ip == parsed_ip && p.port == port)
            } else {
                None
            }
        };

        if let Some(f) = found {
            existing = Some(f);
            break;
        }
        log::info!(
            "UDP probe attempt {} for {}:{} — no response yet",
            attempt + 1,
            ip,
            port
        );
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
                chat.exchange_contacts(&found_ip, found_port, &found_id)
                    .await;
            });
        }
        return Ok(DiscoverResult {
            online: true,
            message: format!(
                "已连接 {} ({}) @ {}:{}",
                found.username, found.department, found.ip, found.port
            ),
        });
    }

    // Fallback: no unicast response received, register manually
    let stored_peer = state.db.list_stored_peers().await.ok().and_then(|peers| {
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
    let display_version = stored_peer
        .as_ref()
        .map(|peer| peer.software_version.clone())
        .unwrap_or_default();
    let display_mac = stored_peer
        .as_ref()
        .map(|peer| peer.mac_address.clone())
        .unwrap_or_default();
    let peer = crate::discovery::Peer::new_with_profile(
        pid.clone(),
        display_name.clone(),
        display_department.clone(),
        display_version,
        display_mac,
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
    state: State<'_, AppState>,
    peer_id: String,
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

    let stored = state
        .db
        .get_stored_peer(&peer_id)
        .await
        .map_err(|e| e.to_string())?;
    let username = stored
        .as_ref()
        .map(|peer| peer.username.clone())
        .unwrap_or_default();
    let department = stored
        .as_ref()
        .map(|peer| peer.department.clone())
        .unwrap_or_default();
    let software_version = stored
        .as_ref()
        .map(|peer| peer.software_version.clone())
        .unwrap_or_default();
    let mac_address = stored
        .as_ref()
        .map(|peer| peer.mac_address.clone())
        .unwrap_or_default();

    if let Some(runtime) = { state.runtime.read().await.clone() }.as_ref() {
        if online {
            runtime.discovery.write().await.touch_peer(&peer_id);
        } else {
            runtime.discovery.write().await.set_online(&peer_id, false);
        }
    }

    let _ = state
        .db
        .upsert_peer_with_profile(
            &peer_id,
            &username,
            &department,
            &software_version,
            &mac_address,
            &ip,
            port,
            online,
        )
        .await;

    Ok(online)
}

#[tauri::command]
pub async fn get_conversation(
    state: State<'_, AppState>,
    peer_id: String,
    limit: Option<i64>,
) -> Result<Vec<ChatMessage>, String> {
    let runtime_opt = { state.runtime.read().await.clone() };
    let Some(runtime) = runtime_opt.as_ref() else {
        return Ok(vec![]);
    };

    state
        .db
        .get_conversation(&peer_id, &runtime.my_id(), limit)
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
        .get_unread_counts(&runtime.my_id())
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn update_tray_unread(
    app: AppHandle,
    items: Vec<crate::tray::TrayUnreadItem>,
) -> Result<(), String> {
    crate::tray::update_unread(&app, items).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn mark_read(state: State<'_, AppState>, peer_id: String) -> Result<(), String> {
    let runtime_opt = { state.runtime.read().await.clone() };
    let Some(runtime) = runtime_opt.as_ref() else {
        return Ok(());
    };

    state
        .db
        .mark_read(&peer_id, &runtime.my_id())
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
pub struct ScreenshotData {
    pub base64: String,
    pub mime: String,
    pub width: i32,
    pub height: i32,
    pub x: i32,
    pub y: i32,
}

#[tauri::command]
pub fn capture_screenshot() -> Result<ScreenshotData, String> {
    capture_screenshot_impl()
}

#[cfg(target_os = "windows")]
fn capture_screenshot_impl() -> Result<ScreenshotData, String> {
    use std::mem::{size_of, zeroed};
    use windows_sys::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC,
        GetDIBits, GetDeviceCaps, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
        CAPTUREBLT, DESKTOPHORZRES, DESKTOPVERTRES, DIB_RGB_COLORS, HGDIOBJ, SRCCOPY,
    };
    use windows_sys::Win32::UI::HiDpi::{
        SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT,
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
        SM_YVIRTUALSCREEN,
    };

    struct ThreadDpiAwarenessGuard(DPI_AWARENESS_CONTEXT);

    impl Drop for ThreadDpiAwarenessGuard {
        fn drop(&mut self) {
            if self.0 != 0 {
                unsafe {
                    SetThreadDpiAwarenessContext(self.0);
                }
            }
        }
    }

    let _dpi_awareness_guard = unsafe {
        ThreadDpiAwarenessGuard(SetThreadDpiAwarenessContext(
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        ))
    };

    unsafe {
        let screen_dc = GetDC(0);
        if screen_dc == 0 {
            return Err("无法获取屏幕设备上下文".to_string());
        }

        let x = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let y = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let mut width = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let mut height = GetSystemMetrics(SM_CYVIRTUALSCREEN);
        let desktop_width = GetDeviceCaps(screen_dc, DESKTOPHORZRES as i32);
        let desktop_height = GetDeviceCaps(screen_dc, DESKTOPVERTRES as i32);
        if x == 0 && y == 0 && desktop_width > width && desktop_height > height {
            width = desktop_width;
            height = desktop_height;
        }
        if width <= 0 || height <= 0 {
            ReleaseDC(0, screen_dc);
            return Err("无法获取屏幕尺寸".to_string());
        }

        let memory_dc = CreateCompatibleDC(screen_dc);
        if memory_dc == 0 {
            ReleaseDC(0, screen_dc);
            return Err("无法创建截图设备上下文".to_string());
        }

        let bitmap = CreateCompatibleBitmap(screen_dc, width, height);
        if bitmap == 0 {
            DeleteDC(memory_dc);
            ReleaseDC(0, screen_dc);
            return Err("无法创建截图位图".to_string());
        }

        let old_object = SelectObject(memory_dc, bitmap as HGDIOBJ);
        let copied = BitBlt(
            memory_dc,
            0,
            0,
            width,
            height,
            screen_dc,
            x,
            y,
            SRCCOPY | CAPTUREBLT,
        );
        if copied == 0 {
            if old_object != 0 {
                SelectObject(memory_dc, old_object);
            }
            DeleteObject(bitmap as HGDIOBJ);
            DeleteDC(memory_dc);
            ReleaseDC(0, screen_dc);
            return Err("无法复制屏幕图像".to_string());
        }

        let row_stride = ((width as usize * 24 + 31) / 32) * 4;
        let image_size = row_stride
            .checked_mul(height as usize)
            .ok_or_else(|| "截图尺寸过大".to_string())?;
        let mut pixels = vec![0u8; image_size];

        let mut bitmap_info: BITMAPINFO = zeroed();
        bitmap_info.bmiHeader = BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: height,
            biPlanes: 1,
            biBitCount: 24,
            biCompression: BI_RGB,
            biSizeImage: image_size as u32,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        };

        let got_bits = GetDIBits(
            memory_dc,
            bitmap,
            0,
            height as u32,
            pixels.as_mut_ptr().cast(),
            &mut bitmap_info,
            DIB_RGB_COLORS,
        );

        if old_object != 0 {
            SelectObject(memory_dc, old_object);
        }
        DeleteObject(bitmap as HGDIOBJ);
        DeleteDC(memory_dc);
        ReleaseDC(0, screen_dc);

        if got_bits == 0 {
            return Err("无法读取截图像素".to_string());
        }

        let rgb = dib_bgr_bottom_up_to_rgb(&pixels, width, height, row_stride)?;
        let png = encode_rgb_png_fast(width, height, &rgb)?;

        Ok(ScreenshotData {
            base64: base64_encode_std(&png),
            mime: "image/png".to_string(),
            width,
            height,
            x,
            y,
        })
    }
}

#[cfg(target_os = "windows")]
fn dib_bgr_bottom_up_to_rgb(
    pixels: &[u8],
    width: i32,
    height: i32,
    row_stride: usize,
) -> Result<Vec<u8>, String> {
    let width = usize::try_from(width).map_err(|_| "截图宽度无效".to_string())?;
    let height = usize::try_from(height).map_err(|_| "截图高度无效".to_string())?;
    let row_bytes = width
        .checked_mul(3)
        .ok_or_else(|| "截图行数据过大".to_string())?;
    let rgb_len = row_bytes
        .checked_mul(height)
        .ok_or_else(|| "截图数据过大".to_string())?;
    let mut rgb = vec![0u8; rgb_len];

    for dest_y in 0..height {
        let src_y = height - 1 - dest_y;
        let src_row = src_y
            .checked_mul(row_stride)
            .ok_or_else(|| "截图行偏移过大".to_string())?;
        let dest_row = dest_y
            .checked_mul(row_bytes)
            .ok_or_else(|| "截图行偏移过大".to_string())?;
        let src_end = src_row
            .checked_add(row_bytes)
            .ok_or_else(|| "截图行数据过大".to_string())?;
        if src_end > pixels.len() {
            return Err("截图像素数据不完整".to_string());
        }

        for x in 0..width {
            let src = src_row + x * 3;
            let dest = dest_row + x * 3;
            rgb[dest] = pixels[src + 2];
            rgb[dest + 1] = pixels[src + 1];
            rgb[dest + 2] = pixels[src];
        }
    }

    Ok(rgb)
}

#[cfg(target_os = "windows")]
fn encode_rgb_png_fast(width: i32, height: i32, rgb: &[u8]) -> Result<Vec<u8>, String> {
    let width = u32::try_from(width).map_err(|_| "截图宽度无效".to_string())?;
    let height = u32::try_from(height).map_err(|_| "截图高度无效".to_string())?;
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        encoder.set_filter(png::FilterType::Sub);
        let mut writer = encoder
            .write_header()
            .map_err(|e| format!("无法创建 PNG 截图：{e}"))?;
        writer
            .write_image_data(rgb)
            .map_err(|e| format!("无法写入 PNG 截图：{e}"))?;
    }
    Ok(bytes)
}

#[cfg(target_os = "macos")]
fn capture_screenshot_impl() -> Result<ScreenshotData, String> {
    let path = screenshot_temp_path("png");
    let output = std::process::Command::new("screencapture")
        .args(["-x", "-t", "png"])
        .arg(&path)
        .output()
        .map_err(|e| format!("无法启动 macOS 截图命令 screencapture：{e}"))?;

    if !output.status.success() {
        let _ = std::fs::remove_file(&path);
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let detail = if stderr.is_empty() {
            "请确认已在系统设置中授予 Echo 屏幕录制权限".to_string()
        } else {
            stderr
        };
        return Err(format!("macOS 截图失败：{detail}"));
    }

    read_png_screenshot_file(&path, 0, 0)
}

#[cfg(target_os = "linux")]
fn capture_screenshot_impl() -> Result<ScreenshotData, String> {
    struct ScreenshotCommand {
        program: &'static str,
        args_before_path: &'static [&'static str],
    }

    const COMMANDS: &[ScreenshotCommand] = &[
        ScreenshotCommand {
            program: "grim",
            args_before_path: &[],
        },
        ScreenshotCommand {
            program: "gnome-screenshot",
            args_before_path: &["-f"],
        },
        ScreenshotCommand {
            program: "spectacle",
            args_before_path: &["-b", "-n", "-o"],
        },
        ScreenshotCommand {
            program: "maim",
            args_before_path: &[],
        },
        ScreenshotCommand {
            program: "scrot",
            args_before_path: &[],
        },
        ScreenshotCommand {
            program: "import",
            args_before_path: &["-window", "root"],
        },
        ScreenshotCommand {
            program: "flameshot",
            args_before_path: &["full", "-p"],
        },
    ];

    let path = screenshot_temp_path("png");
    let mut failures = Vec::new();

    for command in COMMANDS {
        let _ = std::fs::remove_file(&path);
        let output = std::process::Command::new(command.program)
            .args(command.args_before_path)
            .arg(&path)
            .output();

        match output {
            Ok(output) if output.status.success() => match read_png_screenshot_file(&path, 0, 0) {
                Ok(screenshot) => return Ok(screenshot),
                Err(err) => failures.push(format!("{} 输出无效：{}", command.program, err)),
            },
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                if stderr.is_empty() {
                    failures.push(format!(
                        "{} 退出码 {:?}",
                        command.program,
                        output.status.code()
                    ));
                } else {
                    failures.push(format!("{}：{}", command.program, stderr));
                }
            }
            Err(err) => failures.push(format!("{}：{}", command.program, err)),
        }
    }

    let _ = std::fs::remove_file(&path);
    let session_type = std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "unknown".to_string());
    Err(format!(
        "Linux 截图失败。当前会话：{session_type}。请安装 grim、gnome-screenshot、spectacle、maim、scrot、ImageMagick import 或 flameshot 之一；Wayland 环境可能还需要桌面门户授权。尝试结果：{}",
        failures.join("；")
    ))
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn screenshot_temp_path(ext: &str) -> std::path::PathBuf {
    let timestamp = chrono::Utc::now().timestamp_millis();
    std::env::temp_dir().join(format!(
        "echo-screenshot-{}-{}.{}",
        std::process::id(),
        timestamp,
        ext
    ))
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn read_png_screenshot_file(path: &Path, x: i32, y: i32) -> Result<ScreenshotData, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("无法读取截图文件：{e}"))?;
    let _ = std::fs::remove_file(path);
    let (width, height) = png_dimensions(&bytes)?;
    Ok(ScreenshotData {
        base64: base64_encode_std(&bytes),
        mime: "image/png".to_string(),
        width,
        height,
        x,
        y,
    })
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn png_dimensions(bytes: &[u8]) -> Result<(i32, i32), String> {
    const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if bytes.len() < 24 || !bytes.starts_with(PNG_SIGNATURE) {
        return Err("截图文件不是有效 PNG".to_string());
    }

    let width = u32::from_be_bytes(
        bytes[16..20]
            .try_into()
            .map_err(|_| "无法读取 PNG 宽度".to_string())?,
    );
    let height = u32::from_be_bytes(
        bytes[20..24]
            .try_into()
            .map_err(|_| "无法读取 PNG 高度".to_string())?,
    );

    if width == 0 || height == 0 {
        return Err("截图尺寸无效".to_string());
    }

    let width = i32::try_from(width).map_err(|_| "截图宽度过大".to_string())?;
    let height = i32::try_from(height).map_err(|_| "截图高度过大".to_string())?;
    Ok((width, height))
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
fn capture_screenshot_impl() -> Result<ScreenshotData, String> {
    Err("当前平台暂不支持直接截取整个屏幕".to_string())
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
            .args([
                "/C",
                "start",
                "",
                &std::path::Path::new(&path).to_string_lossy().as_ref(),
            ])
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
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        std::process::Command::new("explorer")
            .creation_flags(CREATE_NO_WINDOW)
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

    let my_id = runtime.my_id();
    let rows = state
        .db
        .search_messages(&my_id, &query)
        .await
        .map_err(|e| e.to_string())?;

    let mut groups: std::collections::BTreeMap<String, SearchResult> =
        std::collections::BTreeMap::new();
    for row in rows {
        let peer_id = if row.sender_id == my_id {
            row.receiver_id.clone()
        } else {
            row.sender_id.clone()
        };
        let peer_name = if row.sender_id == my_id {
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
pub async fn search_conversation_messages(
    state: State<'_, AppState>,
    peer_id: String,
    query: String,
    limit: Option<i64>,
    filter: Option<String>,
    day_start: Option<String>,
    day_end: Option<String>,
) -> Result<Vec<ChatMessage>, String> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(vec![]);
    }

    let runtime_opt = { state.runtime.read().await.clone() };
    let Some(runtime) = runtime_opt.as_ref() else {
        return Ok(vec![]);
    };

    state
        .db
        .search_conversation_messages(
            &peer_id,
            &runtime.my_id(),
            query,
            limit,
            filter.as_deref(),
            day_start.as_deref(),
            day_end.as_deref(),
        )
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn search_group_messages(
    state: State<'_, AppState>,
    group_id: String,
    query: String,
    limit: Option<i64>,
    filter: Option<String>,
    day_start: Option<String>,
    day_end: Option<String>,
) -> Result<Vec<ChatMessage>, String> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(vec![]);
    }

    state
        .db
        .search_group_messages(
            &group_id,
            query,
            limit,
            filter.as_deref(),
            day_start.as_deref(),
            day_end.as_deref(),
        )
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_conversation_history(
    state: State<'_, AppState>,
    peer_id: String,
    before_id: Option<i64>,
    limit: Option<i64>,
    filter: Option<String>,
    day_start: Option<String>,
    day_end: Option<String>,
) -> Result<Vec<ChatMessage>, String> {
    let runtime_opt = { state.runtime.read().await.clone() };
    let Some(runtime) = runtime_opt.as_ref() else {
        return Ok(vec![]);
    };

    state
        .db
        .get_conversation_history(
            &peer_id,
            &runtime.my_id(),
            before_id,
            limit,
            filter.as_deref(),
            day_start.as_deref(),
            day_end.as_deref(),
        )
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_group_history(
    state: State<'_, AppState>,
    group_id: String,
    before_id: Option<i64>,
    limit: Option<i64>,
    filter: Option<String>,
    day_start: Option<String>,
    day_end: Option<String>,
) -> Result<Vec<ChatMessage>, String> {
    state
        .db
        .get_group_history(
            &group_id,
            before_id,
            limit,
            filter.as_deref(),
            day_start.as_deref(),
            day_end.as_deref(),
        )
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn delete_chat_messages(
    state: State<'_, AppState>,
    message_ids: Vec<i64>,
) -> Result<u64, String> {
    state
        .db
        .delete_messages(&message_ids)
        .await
        .map_err(|e| e.to_string())
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
        runtime
            .discovery
            .write()
            .await
            .update_scan_subnets(&subnets);
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
        Some("txt") | Some("md") | Some("rs") | Some("ts") | Some("js") | Some("json")
        | Some("html") | Some("css") => "text/plain",
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
                if matches!(
                    ext.to_lowercase().as_str(),
                    "png" | "jpg" | "jpeg" | "gif" | "webp"
                ) {
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
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    if !matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp") {
        return Err("invalid emoji file type".to_string());
    }
    std::fs::remove_file(path).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_recent_contacts(state: State<'_, AppState>) -> Result<Vec<StoredPeer>, String> {
    log::info!("list_recent_contacts COMMAND called");
    let result = state
        .db
        .list_recent_contacts()
        .await
        .map_err(|e| e.to_string())?;
    log::info!("list_recent_contacts: {} entries", result.len());
    Ok(result)
}

#[tauri::command]
pub async fn remove_recent_contact(
    state: State<'_, AppState>,
    peer_id: String,
) -> Result<(), String> {
    state
        .db
        .remove_recent_contact(&peer_id)
        .await
        .map_err(|e| e.to_string())
}

// ── Group commands ──

#[derive(Deserialize)]
pub struct CreateGroupPayload {
    pub name: String,
    pub members: Vec<String>,
}

#[tauri::command]
pub async fn create_group(
    state: State<'_, AppState>,
    payload: CreateGroupPayload,
) -> Result<crate::db::GroupInfo, String> {
    let gid = format!(
        "group-{}",
        uuid::Uuid::new_v4()
            .to_string()
            .split('-')
            .next()
            .unwrap_or("0")
    );
    let my_id = {
        let runtime_opt = { state.runtime.read().await.clone() };
        runtime_opt.as_ref().map(|r| r.my_id()).unwrap_or_default()
    };
    let mut all_members = payload.members.clone();
    if !all_members.iter().any(|m| m == &my_id) {
        all_members.push(my_id.clone());
    }
    state
        .db
        .create_group(&gid, &payload.name, &my_id, &all_members)
        .await
        .map_err(|e| e.to_string())?;
    let members = state
        .db
        .get_group_members(&gid)
        .await
        .map_err(|error| error.to_string())?;
    let all_members: Vec<String> = members
        .iter()
        .map(|member| member.peer_id.clone())
        .collect();

    let (my_node_id, listen_port, online_peers) = {
        let runtime_opt = { state.runtime.read().await.clone() };
        match runtime_opt.as_ref() {
            Some(r) => (
                r.my_node_id.clone(),
                r.listen_port,
                r.discovery.read().await.get_peers(),
            ),
            None => (String::new(), 9527, vec![]),
        }
    };
    let (my_name, my_department) = {
        let p = state.profile.lock().await;
        p.as_ref()
            .map(|p| (p.username.clone(), p.department.clone()))
            .unwrap_or_default()
    };
    let directory = build_member_directory(
        &state.db,
        &online_peers,
        &all_members,
        &my_id,
        &my_name,
        &my_department,
        listen_port,
    )
    .await;
    let content =
        serde_json::json!({"name": payload.name, "member_ids": all_members.clone()}).to_string();
    spawn_send_or_queue_notification(
        state.db.clone(),
        online_peers,
        all_members,
        my_id.clone(),
        my_node_id,
        my_name,
        my_department,
        listen_port,
        content,
        "group_created".to_string(),
        Some(gid.clone()),
        None,
        None,
        directory,
    );

    Ok(crate::db::GroupInfo {
        group_id: gid,
        name: payload.name,
        creator_id: my_id,
        created_at: String::new(),
        members,
        last_message: None,
        last_message_at: None,
        last_message_sender: None,
        unread_count: 0,
    })
}

#[tauri::command]
pub async fn list_groups(state: State<'_, AppState>) -> Result<Vec<crate::db::GroupInfo>, String> {
    let my_id = {
        let runtime_opt = { state.runtime.read().await.clone() };
        runtime_opt.as_ref().map(|r| r.my_id()).unwrap_or_default()
    };
    let mut groups = state
        .db
        .list_groups(&my_id)
        .await
        .map_err(|e| e.to_string())?;
    for g in &mut groups {
        g.members = state
            .db
            .get_group_members(&g.group_id)
            .await
            .unwrap_or_default();
    }
    Ok(groups)
}

#[tauri::command]
pub async fn send_group_message(
    app_handle: tauri::AppHandle,
    state: State<'_, AppState>,
    group_id: String,
    content: String,
    client_msg_id: Option<String>,
) -> Result<ChatMessage, String> {
    send_group_message_typed(
        app_handle,
        state,
        group_id,
        content,
        "text".to_string(),
        client_msg_id,
    )
    .await
}

#[tauri::command]
pub async fn send_group_message_typed(
    app_handle: tauri::AppHandle,
    state: State<'_, AppState>,
    group_id: String,
    content: String,
    msg_type: String,
    client_msg_id: Option<String>,
) -> Result<ChatMessage, String> {
    let client_msg_id = client_msg_id_or_new(client_msg_id);
    let (my_id, my_node_id, my_name, my_department, listen_port, members) = {
        let runtime_opt = { state.runtime.read().await.clone() };
        let r = runtime_opt.as_ref().ok_or("未初始化")?;
        let prof = state.profile.lock().await;
        let my_name = prof
            .as_ref()
            .map(|p| p.username.clone())
            .unwrap_or_default();
        let my_dept = prof
            .as_ref()
            .map(|p| p.department.clone())
            .unwrap_or_default();
        let members = state
            .db
            .get_group_members(&group_id)
            .await
            .map_err(|e| e.to_string())?;
        (
            r.my_id(),
            r.my_node_id.clone(),
            my_name,
            my_dept,
            r.listen_port,
            members,
        )
    };

    let online_peers = {
        let runtime_opt = { state.runtime.read().await.clone() };
        match runtime_opt.as_ref() {
            Some(r) => r.discovery.read().await.get_peers(),
            None => vec![],
        }
    };

    // Persist first so a slow/offline member can never make the sender's own
    // message disappear. Receiver-side client_msg_id dedup keeps background
    // retries safe when an ACK is lost.
    let msg = state
        .db
        .save_group_message_dedup(
            &group_id,
            &my_id,
            &my_name,
            &content,
            &msg_type,
            None,
            None,
            None,
            true,
            Some(client_msg_id.as_str()),
        )
        .await
        .map_err(|e| e.to_string())?;

    crate::chat::emit_group_message_updated(&app_handle, &group_id, msg.clone());

    let target_ids: Vec<String> = members.iter().map(|m| m.peer_id.clone()).collect();
    spawn_send_or_queue_notification(
        state.db.clone(),
        online_peers,
        target_ids,
        my_id,
        my_node_id,
        my_name,
        my_department,
        listen_port,
        content,
        msg_type,
        Some(group_id),
        None,
        Some(client_msg_id),
        Vec::new(),
    );

    Ok(msg)
}

#[tauri::command]
pub async fn get_group_messages(
    state: State<'_, AppState>,
    group_id: String,
    limit: Option<i64>,
) -> Result<Vec<ChatMessage>, String> {
    state
        .db
        .get_group_messages(&group_id, limit)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn rename_group(
    state: State<'_, AppState>,
    group_id: String,
    new_name: String,
) -> Result<(), String> {
    state
        .db
        .rename_group(&group_id, &new_name)
        .await
        .map_err(|e| e.to_string())?;

    let members = state
        .db
        .get_group_members(&group_id)
        .await
        .map_err(|e| e.to_string())?;
    let (my_id, my_node_id, my_name, my_department, listen_port, online_peers) = {
        let runtime_opt = { state.runtime.read().await.clone() };
        let r = runtime_opt.as_ref().ok_or("未初始化")?;
        let prof = state.profile.lock().await;
        let my_name = prof
            .as_ref()
            .map(|p| p.username.clone())
            .unwrap_or_default();
        let my_dept = prof
            .as_ref()
            .map(|p| p.department.clone())
            .unwrap_or_default();
        let peers = r.discovery.read().await.get_peers();
        (
            r.my_id(),
            r.my_node_id.clone(),
            my_name,
            my_dept,
            r.listen_port,
            peers,
        )
    };

    let target_ids: Vec<String> = members.iter().map(|m| m.peer_id.clone()).collect();
    let empty_dir: Vec<crate::discovery::PeerEntry> = Vec::new();
    spawn_send_or_queue_notification(
        state.db.clone(),
        online_peers,
        target_ids,
        my_id,
        my_node_id,
        my_name,
        my_department,
        listen_port,
        format!("群名已修改为「{}」", new_name),
        "group_renamed".to_string(),
        Some(group_id),
        Some(new_name),
        None,
        empty_dir,
    );
    Ok(())
}

#[tauri::command]
pub async fn invite_to_group(
    state: State<'_, AppState>,
    group_id: String,
    members: Vec<String>,
) -> Result<(), String> {
    let runtime = { state.runtime.read().await.clone() }.ok_or("未初始化")?;
    let my_id = runtime.my_id();
    state
        .db
        .add_group_members(&group_id, &members)
        .await
        .map_err(|e| e.to_string())?;

    let groups = state
        .db
        .list_groups(&my_id)
        .await
        .map_err(|e| e.to_string())?;
    let group = groups
        .iter()
        .find(|g| g.group_id == group_id)
        .ok_or("群组不存在")?
        .clone();
    let member_records = state
        .db
        .get_group_members(&group_id)
        .await
        .map_err(|e| e.to_string())?;
    let all_member_ids: Vec<String> = member_records.iter().map(|m| m.peer_id.clone()).collect();

    let online_peers = runtime.discovery.read().await.get_peers();
    let my_node_id = runtime.my_node_id.clone();
    let listen_port = runtime.listen_port;
    let (my_name, my_department) = {
        let p = state.profile.lock().await;
        p.as_ref()
            .map(|p| (p.username.clone(), p.department.clone()))
            .unwrap_or_default()
    };
    let directory = build_member_directory(
        &state.db,
        &online_peers,
        &all_member_ids,
        &my_id,
        &my_name,
        &my_department,
        listen_port,
    )
    .await;
    let content =
        serde_json::json!({"name": group.name, "member_ids": all_member_ids.clone()}).to_string();
    spawn_send_or_queue_notification(
        state.db.clone(),
        online_peers,
        all_member_ids,
        my_id,
        my_node_id,
        my_name,
        my_department,
        listen_port,
        content,
        "group_created".to_string(),
        Some(group_id),
        None,
        None,
        directory,
    );
    Ok(())
}

#[tauri::command]
pub async fn leave_group(state: State<'_, AppState>, group_id: String) -> Result<(), String> {
    let (my_id, my_node_id, listen_port, online_peers, members) = {
        let runtime_opt = { state.runtime.read().await.clone() };
        let r = runtime_opt.as_ref().ok_or("未初始化")?;
        let members = state
            .db
            .get_group_members(&group_id)
            .await
            .map_err(|e| e.to_string())?;
        let online = r.discovery.read().await.get_peers();
        (
            r.my_id(),
            r.my_node_id.clone(),
            r.listen_port,
            online,
            members,
        )
    };

    let groups = state
        .db
        .list_groups(&my_id)
        .await
        .map_err(|e| e.to_string())?;
    if let Some(g) = groups.iter().find(|g| g.group_id == group_id) {
        if g.creator_id == my_id {
            return Err("群主不可退群，请使用解散群".to_string());
        }
    }
    let (my_name, my_department) = {
        let p = state.profile.lock().await;
        p.as_ref()
            .map(|p| (p.username.clone(), p.department.clone()))
            .unwrap_or_default()
    };

    let target_ids: Vec<String> = members.iter().map(|m| m.peer_id.clone()).collect();
    let empty_dir: Vec<crate::discovery::PeerEntry> = Vec::new();
    spawn_send_or_queue_notification(
        state.db.clone(),
        online_peers,
        target_ids,
        my_id.clone(),
        my_node_id,
        my_name,
        my_department,
        listen_port,
        String::new(),
        "group_member_left".to_string(),
        Some(group_id.clone()),
        None,
        None,
        empty_dir,
    );

    state
        .db
        .remove_group_member(&group_id, &my_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn dissolve_group(state: State<'_, AppState>, group_id: String) -> Result<(), String> {
    let members = state
        .db
        .get_group_members(&group_id)
        .await
        .map_err(|e| e.to_string())?;
    let (my_id, my_node_id, listen_port, online_peers) = {
        let runtime_opt = { state.runtime.read().await.clone() };
        let r = runtime_opt.as_ref().ok_or("未初始化")?;
        let peers = r.discovery.read().await.get_peers();
        (r.my_id(), r.my_node_id.clone(), r.listen_port, peers)
    };
    let (my_name, my_department) = {
        let p = state.profile.lock().await;
        p.as_ref()
            .map(|p| (p.username.clone(), p.department.clone()))
            .unwrap_or_default()
    };

    state
        .db
        .dissolve_group(&group_id)
        .await
        .map_err(|e| e.to_string())?;

    let target_ids: Vec<String> = members.iter().map(|m| m.peer_id.clone()).collect();
    let empty_dir: Vec<crate::discovery::PeerEntry> = Vec::new();
    spawn_send_or_queue_notification(
        state.db.clone(),
        online_peers,
        target_ids,
        my_id,
        my_node_id,
        my_name,
        my_department,
        listen_port,
        "群组已解散".to_string(),
        "group_dissolved".to_string(),
        Some(group_id),
        None,
        None,
        empty_dir,
    );
    Ok(())
}

#[tauri::command]
pub async fn send_group_file(
    app_handle: tauri::AppHandle,
    state: State<'_, AppState>,
    group_id: String,
    file_path: String,
    file_name: Option<String>,
    client_msg_id: Option<String>,
) -> Result<ChatMessage, String> {
    send_group_file_with_kind(
        app_handle,
        state,
        group_id,
        file_path,
        file_name,
        "file",
        client_msg_id,
    )
    .await
}

#[tauri::command]
pub async fn send_group_sticker(
    app_handle: tauri::AppHandle,
    state: State<'_, AppState>,
    group_id: String,
    file_path: String,
    file_name: Option<String>,
    client_msg_id: Option<String>,
) -> Result<ChatMessage, String> {
    send_group_file_with_kind(
        app_handle,
        state,
        group_id,
        file_path,
        file_name,
        "sticker",
        client_msg_id,
    )
    .await
}

async fn send_group_file_with_kind(
    app_handle: tauri::AppHandle,
    state: State<'_, AppState>,
    group_id: String,
    file_path: String,
    file_name_override: Option<String>,
    file_kind: &str,
    client_msg_id: Option<String>,
) -> Result<ChatMessage, String> {
    use chrono::Utc;
    let client_msg_id = Some(client_msg_id_or_new(client_msg_id));
    let (my_id, my_name, my_department, listen_port, db, members) = {
        let runtime_opt = { state.runtime.read().await.clone() };
        let r = runtime_opt.as_ref().ok_or("未初始化")?;
        let my_name = state
            .profile
            .lock()
            .await
            .as_ref()
            .map(|p| p.username.clone())
            .unwrap_or_default();
        let my_department = state
            .profile
            .lock()
            .await
            .as_ref()
            .map(|p| p.department.clone())
            .unwrap_or_default();
        let members = state
            .db
            .get_group_members(&group_id)
            .await
            .map_err(|e| e.to_string())?;
        (
            r.my_id(),
            my_name,
            my_department,
            r.listen_port,
            state.db.clone(),
            members,
        )
    };

    let file_name = file_name_override
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            std::path::Path::new(&file_path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string()
        });
    let file_size = tokio::fs::metadata(&file_path)
        .await
        .map(|m| m.len() as i64)
        .map_err(|e| e.to_string())?;
    let pending_cache: Arc<tokio::sync::Mutex<Option<String>>> =
        Arc::new(tokio::sync::Mutex::new(None));
    let msg_kind = if file_kind == "sticker" {
        "sticker"
    } else {
        "file"
    };
    let content = if msg_kind == "sticker" {
        "[表情]".to_string()
    } else {
        format!("📎 {}", file_name)
    };

    register_outgoing_file_transfer(client_msg_id.as_deref()).await;

    let online_peers = {
        let runtime_opt = { state.runtime.read().await.clone() };
        match runtime_opt.as_ref() {
            Some(r) => r.discovery.read().await.get_peers(),
            None => vec![],
        }
    };

    let bg_handle = app_handle.clone();
    let bg_error_handle = app_handle.clone();
    let bg_db = db.clone();
    let bg_members = members;
    let bg_online_peers = online_peers;
    let bg_my_id = my_id.clone();
    let bg_my_name = my_name.clone();
    let bg_my_department = my_department.clone();
    let bg_group_id = group_id.clone();
    let bg_file_path = file_path.clone();
    let bg_file_name = file_name.clone();
    let bg_msg_kind = msg_kind.to_string();
    let bg_content = content.clone();
    let bg_client_msg_id = client_msg_id.clone();
    let bg_pending_cache = pending_cache.clone();

    tauri::async_runtime::spawn(async move {
        let mut had_delivery_failure = false;
        for member in bg_members {
            if member.peer_id == bg_my_id {
                continue;
            }
            if let Err(e) = wait_for_outgoing_file_transfer(bg_client_msg_id.as_deref()).await {
                clear_outgoing_file_transfer(bg_client_msg_id.as_deref()).await;
                let _ = bg_error_handle.emit_all(
                    "file-error",
                    serde_json::json!({
                        "fileName": bg_file_name.as_str(),
                        "clientMsgId": bg_client_msg_id.as_deref(),
                        "error": e.to_string(),
                    }),
                );
                return;
            }

            let target_id = member.peer_id.clone();
            let target_name = member.username.clone();
            let target_department = member.department.clone();
            let target_software_version =
                resolve_peer_software_version(&target_id, bg_db.as_ref(), &bg_online_peers).await;
            let resolved_addr =
                resolve_peer_addr(&target_id, bg_db.as_ref(), &bg_online_peers).await;

            if let Some((ip, port)) = resolved_addr {
                let mut peer = crate::discovery::Peer::new(
                    target_id.clone(),
                    target_name,
                    target_department,
                    ip,
                    port,
                );
                peer.software_version = target_software_version;
                match send_group_file_to_peer_with_progress(
                    &bg_file_path,
                    &bg_file_name,
                    file_size,
                    &peer,
                    &bg_my_id,
                    &bg_my_name,
                    &bg_my_department,
                    listen_port,
                    &bg_group_id,
                    &bg_msg_kind,
                    bg_client_msg_id.as_deref(),
                    &bg_handle,
                )
                .await
                {
                    Ok(_) => log::info!("Group file sent to {}", peer.id),
                    Err(e) if e == FILE_TRANSFER_CANCELLED_MESSAGE => {
                        clear_outgoing_file_transfer(bg_client_msg_id.as_deref()).await;
                        let _ = bg_error_handle.emit_all(
                            "file-error",
                            serde_json::json!({
                                "fileName": bg_file_name.as_str(),
                                "clientMsgId": bg_client_msg_id.as_deref(),
                                "error": e,
                            }),
                        );
                        return;
                    }
                    Err(e) => {
                        had_delivery_failure = true;
                        log::error!("Group file send failed to {}: {}", peer.id, e);
                        let _ = bg_error_handle.emit_all(
                            "file-error",
                            serde_json::json!({
                                "fileName": bg_file_name.as_str(),
                                "clientMsgId": bg_client_msg_id.as_deref(),
                                "error": e.as_str(),
                            }),
                        );
                        if let Err(queue_err) = queue_group_file_for_peer(
                            bg_db.as_ref(),
                            &bg_file_path,
                            &bg_file_name,
                            file_size,
                            bg_pending_cache.clone(),
                            &peer.id,
                            &bg_my_id,
                            &bg_my_name,
                            &bg_my_department,
                            listen_port,
                            &bg_group_id,
                            &bg_msg_kind,
                            bg_client_msg_id.as_deref(),
                        )
                        .await
                        {
                            log::error!(
                                "Failed to queue group file for {}: {}",
                                peer.id,
                                queue_err
                            );
                            let _ = bg_error_handle.emit_all(
                                "file-error",
                                serde_json::json!({
                                    "fileName": bg_file_name.as_str(),
                                    "clientMsgId": bg_client_msg_id.as_deref(),
                                    "error": queue_err,
                                }),
                            );
                        }
                    }
                }
            } else {
                if let Err(e) = queue_group_file_for_peer(
                    bg_db.as_ref(),
                    &bg_file_path,
                    &bg_file_name,
                    file_size,
                    bg_pending_cache.clone(),
                    &target_id,
                    &bg_my_id,
                    &bg_my_name,
                    &bg_my_department,
                    listen_port,
                    &bg_group_id,
                    &bg_msg_kind,
                    bg_client_msg_id.as_deref(),
                )
                .await
                {
                    had_delivery_failure = true;
                    log::error!("Failed to queue group file for {}: {}", target_id, e);
                    let _ = bg_error_handle.emit_all(
                        "file-error",
                        serde_json::json!({
                            "fileName": bg_file_name.as_str(),
                            "clientMsgId": bg_client_msg_id.as_deref(),
                            "error": e,
                        }),
                    );
                } else {
                    log::info!("Queued group file for offline member {}", target_id);
                }
            }
        }

        if let Err(e) = wait_for_outgoing_file_transfer(bg_client_msg_id.as_deref()).await {
            clear_outgoing_file_transfer(bg_client_msg_id.as_deref()).await;
            let _ = bg_error_handle.emit_all(
                "file-error",
                serde_json::json!({
                    "fileName": bg_file_name.as_str(),
                    "clientMsgId": bg_client_msg_id.as_deref(),
                    "error": e.to_string(),
                }),
            );
            return;
        }

        let saved = match bg_db
            .save_group_message_dedup(
                &bg_group_id,
                &bg_my_id,
                &bg_my_name,
                &bg_content,
                &bg_msg_kind,
                Some(&bg_file_path),
                Some(&bg_file_name),
                Some(file_size),
                true,
                bg_client_msg_id.as_deref(),
            )
            .await
        {
            Ok(saved) => saved,
            Err(error) => {
                clear_outgoing_file_transfer(bg_client_msg_id.as_deref()).await;
                let _ = bg_error_handle.emit_all(
                    "file-error",
                    serde_json::json!({
                        "fileName": bg_file_name.as_str(),
                        "clientMsgId": bg_client_msg_id.as_deref(),
                        "error": error.to_string(),
                    }),
                );
                return;
            }
        };
        crate::chat::emit_group_message_updated(&bg_handle, &bg_group_id, saved);

        if !had_delivery_failure {
            let _ = bg_handle.emit_all(
                "file-progress",
                serde_json::json!({
                    "fileName": bg_file_name.as_str(),
                    "clientMsgId": bg_client_msg_id.as_deref(),
                    "sent": file_size,
                    "total": file_size,
                    "speed": 0,
                }),
            );
        }
        clear_outgoing_file_transfer(bg_client_msg_id.as_deref()).await;
    });

    Ok(ChatMessage {
        id: 0,
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
        client_msg_id,
        delivered: None,
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
    client_msg_id: Option<&str>,
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
        client_msg_id: client_msg_id.unwrap_or_default().to_string(),
    };

    let _ = app_handle.emit_all(
        "file-progress",
        serde_json::json!({
            "fileName": file_name,
            "clientMsgId": client_msg_id,
            "sent": 0,
            "total": file_size,
            "speed": 0,
        }),
    );

    send_group_file_payloads_over_tcp_controlled(
        &peer.address(),
        &transfer,
        client_msg_id,
        &peer.software_version,
    )
    .await?;

    let _ = app_handle.emit_all(
        "file-progress",
        serde_json::json!({
            "fileName": file_name,
            "clientMsgId": client_msg_id,
            "sent": file_size,
            "total": file_size,
            "speed": 0,
        }),
    );

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
    client_msg_id: Option<&str>,
) -> Result<(), String> {
    let cached_file_path =
        get_or_create_pending_cache(file_path, file_name, &pending_cache).await?;
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
        client_msg_id,
    )
    .await
    .map_err(|e| e.to_string())
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
    tokio::fs::create_dir_all(&pending_dir)
        .await
        .map_err(|e| e.to_string())?;

    let safe_name = std::path::Path::new(file_name)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file");
    let dest = pending_dir.join(format!(
        "{}_{}",
        chrono::Utc::now().timestamp_millis(),
        safe_name
    ));
    tokio::fs::copy(file_path, &dest)
        .await
        .map_err(|e| e.to_string())?;
    Ok(dest.to_string_lossy().to_string())
}

#[tauri::command]
pub async fn get_group_unread_counts(
    state: State<'_, AppState>,
) -> Result<Vec<crate::db::GroupUnread>, String> {
    let runtime_opt = { state.runtime.read().await.clone() };
    let Some(runtime) = runtime_opt.as_ref() else {
        return Ok(vec![]);
    };
    state
        .db
        .get_group_unread_counts(&runtime.my_id())
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn mark_group_read(state: State<'_, AppState>, group_id: String) -> Result<(), String> {
    let runtime_opt = { state.runtime.read().await.clone() };
    let Some(runtime) = runtime_opt.as_ref() else {
        return Ok(());
    };
    state
        .db
        .mark_group_read(&group_id, &runtime.my_id())
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn deliver_pending(state: State<'_, AppState>, peer_id: String) -> Result<(), String> {
    let pending = state
        .db
        .get_pending_for_peer(&peer_id)
        .await
        .map_err(|e| e.to_string())?;
    if pending.is_empty() {
        return Ok(());
    }

    let (_my_id, listen_port) = {
        let runtime_opt = { state.runtime.read().await.clone() };
        let r = runtime_opt.as_ref().ok_or("未初始化")?;
        (r.my_id(), r.listen_port)
    };

    let mut delivered_ids = Vec::new();
    for p in &pending {
        let stored_opt = state.db.get_stored_peer(&peer_id).await.unwrap_or_default();
        let ip = stored_opt
            .as_ref()
            .and_then(|sp| sp.ip.parse::<std::net::IpAddr>().ok());
        let port = stored_opt.as_ref().map(|sp| sp.port).unwrap_or(0);
        if let Some(ip) = ip {
            if !ip.is_unspecified() && port != 0 {
                let addr = format!("{}:{}", ip, port);
                let wm = crate::chat::WireMessage {
                    sender_id: p.sender_id.clone(),
                    sender_node_id: String::new(),
                    sender_name: p.sender_name.clone(),
                    sender_department: String::new(),
                    sender_software_version: crate::profile_metadata::software_version(),
                    sender_mac_address: crate::profile_metadata::mac_address(),
                    sender_port: listen_port,
                    receiver_id: peer_id.clone(),
                    receiver_node_id: String::new(),
                    content: p.content.clone(),
                    msg_type: p.msg_type.clone(),
                    file_name: None,
                    file_size: None,
                    file_data: None,
                    file_kind: None,
                    known_peers: Vec::new(),
                    group_id: Some(p.group_id.clone()),
                    client_msg_id: None,
                };
                let json = serde_json::to_string(&wm).unwrap_or_default();
                if let Ok(mut stream) = tokio::time::timeout(
                    std::time::Duration::from_secs(3),
                    tokio::net::TcpStream::connect(&addr),
                )
                .await
                .map_err(|_| "")?
                {
                    use tokio::io::AsyncWriteExt;
                    if stream.write_all(json.as_bytes()).await.is_ok() {
                        delivered_ids.push(p.id);
                    }
                }
            }
        }
    }
    if !delivered_ids.is_empty() {
        state
            .db
            .delete_pending_msgs(&delivered_ids)
            .await
            .map_err(|e| e.to_string())?;
        log::info!(
            "Delivered {} pending messages to {}",
            delivered_ids.len(),
            peer_id
        );
    }
    Ok(())
}

/// Deliver pending notifications (any kind) to a peer that just came back online.
pub async fn deliver_pending_to_peer(
    db: &crate::db::Database,
    peer_id_str: &str,
    app_handle: &AppHandle,
) {
    if !try_begin_pending_delivery(peer_id_str).await {
        log::debug!("Skipping concurrent pending delivery for {}", peer_id_str);
        return;
    }
    deliver_pending_to_peer_inner(db, peer_id_str, app_handle).await;
    finish_pending_delivery(peer_id_str).await;
}

async fn deliver_pending_to_peer_inner(
    db: &crate::db::Database,
    peer_id_str: &str,
    app_handle: &AppHandle,
) {
    // Resolve peer address.
    let stored = match db.get_stored_peer(peer_id_str).await {
        Ok(Some(p)) => p,
        _ => return,
    };
    let ip: std::net::IpAddr = match stored.ip.parse() {
        Ok(ip) => ip,
        Err(_) => return,
    };
    if ip.is_unspecified() || stored.port == 0 {
        return;
    }
    let addr = format!("{}:{}", ip, stored.port);

    // 1) Generic pending_notifications (preferred — payload is a full WireMessage).
    //    Drain not just this peer_id's queue but any queued under historical
    //    endpoints of the same node (aliases). After an IP change a message may
    //    have been queued under the old peer_id, which no longer resolves to an
    //    address on its own — redirect those to the current endpoint here instead
    //    of leaving them stranded in a dead queue.
    let mut queue_owner_ids = vec![peer_id_str.to_string()];
    if let Ok(aliases) = db.identity_aliases(peer_id_str).await {
        for alias in aliases {
            if alias != peer_id_str && !queue_owner_ids.contains(&alias) {
                queue_owner_ids.push(alias);
            }
        }
    }
    for owner_id in &queue_owner_ids {
        let notifs = db
            .get_pending_notifications(owner_id)
            .await
            .unwrap_or_default();
        if notifs.is_empty() {
            continue;
        }
        let delivered_notif_ids =
            deliver_pending_payloads_over_tcp(&addr, &notifs, &stored.software_version).await;
        if !delivered_notif_ids.is_empty() {
            let delivered_ids: HashSet<i64> = delivered_notif_ids.iter().copied().collect();
            let mut removable_ids = Vec::with_capacity(delivered_notif_ids.len());
            for (id, _kind, payload) in &notifs {
                if !delivered_ids.contains(id) {
                    continue;
                }

                let mut delivery_state_updated = true;
                if let Ok(wire_message) = serde_json::from_str::<WireMessage>(payload) {
                    if wire_message.group_id.is_none() {
                        if let Some(client_msg_id) = wire_message
                            .client_msg_id
                            .as_deref()
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                        {
                            let delivery_state = if requires_message_ack(
                                Some(client_msg_id),
                                &stored.software_version,
                            ) {
                                Some(true)
                            } else {
                                None
                            };
                            match db
                                .update_private_message_delivery(
                                    &wire_message.sender_id,
                                    client_msg_id,
                                    delivery_state,
                                )
                                .await
                            {
                                Ok(Some(saved)) => {
                                    emit_contact_message_updated(app_handle, peer_id_str, saved)
                                }
                                Ok(None) => {}
                                Err(error) => {
                                    delivery_state_updated = false;
                                    log::error!(
                                        "Failed to update delivery state for {}: {}",
                                        client_msg_id,
                                        error
                                    );
                                }
                            }
                        }
                    }
                }

                if delivery_state_updated {
                    removable_ids.push(*id);
                }
            }

            if !removable_ids.is_empty() {
                let _ = db.delete_pending_notifications(&removable_ids).await;
            }
            log::info!(
                "Delivered {} pending notifs to {} (queued under {}, removed {})",
                delivered_notif_ids.len(),
                peer_id_str,
                owner_id,
                removable_ids.len()
            );
        }
    }

    // 2) Legacy pending_group_messages — still drained for backward-compat with
    // any data queued by previous app versions.
    deliver_pending_file_transfers_to_peer(db, peer_id_str, &stored, ip).await;

    let pending = db
        .get_pending_for_peer(peer_id_str)
        .await
        .unwrap_or_default();
    if pending.is_empty() {
        return;
    }
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
        if deliver_over_tcp(&addr, &json, None, &stored.software_version).await {
            delivered.push(p.id);
        }
    }
    if !delivered.is_empty() {
        let _ = db.delete_pending_msgs(&delivered).await;
        log::info!(
            "Delivered {} legacy pending msgs to {}",
            delivered.len(),
            peer_id_str
        );
    }
}

async fn deliver_pending_file_transfers_to_peer(
    db: &crate::db::Database,
    peer_id: &str,
    stored: &StoredPeer,
    ip: std::net::IpAddr,
) {
    let pending_files = db
        .get_pending_file_transfers(peer_id)
        .await
        .unwrap_or_default();
    if pending_files.is_empty() {
        return;
    }
    let addr = format!("{}:{}", ip, stored.port);

    for transfer in pending_files {
        if !std::path::Path::new(&transfer.file_path).exists() {
            log::error!(
                "Pending group file missing for {}: {}",
                peer_id,
                transfer.file_path
            );
            let _ = db.delete_pending_file_transfer(transfer.id).await;
            continue;
        }

        let ok =
            send_group_file_payloads_over_tcp(&addr, &transfer, &stored.software_version).await;
        if !ok {
            log::error!(
                "Failed to deliver pending group file {} to {}",
                transfer.file_name,
                peer_id
            );
            break;
        }

        let file_path = transfer.file_path.clone();
        let _ = db.delete_pending_file_transfer(transfer.id).await;
        if db
            .count_pending_file_transfers_by_path(&file_path)
            .await
            .unwrap_or(1)
            == 0
        {
            let _ = tokio::fs::remove_file(&file_path).await;
        }
        log::info!(
            "Delivered pending group file {} to {}",
            transfer.file_name,
            peer_id
        );
    }
}

async fn send_group_file_payloads_over_tcp(
    addr: &str,
    transfer: &crate::db::PendingFileTransfer,
    peer_software_version: &str,
) -> bool {
    send_group_file_payloads_over_tcp_controlled(addr, transfer, None, peer_software_version)
        .await
        .is_ok()
}

async fn send_group_file_payloads_over_tcp_controlled(
    addr: &str,
    transfer: &crate::db::PendingFileTransfer,
    client_msg_id: Option<&str>,
    peer_software_version: &str,
) -> Result<(), String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let stored_client_msg_id = transfer.client_msg_id.trim();
    let wire_client_msg_id = client_msg_id.or_else(|| {
        if stored_client_msg_id.is_empty() {
            None
        } else {
            Some(stored_client_msg_id)
        }
    });

    wait_for_outgoing_file_transfer(client_msg_id)
        .await
        .map_err(|e| e.to_string())?;

    let declared_file_size =
        u64::try_from(transfer.file_size).map_err(|_| "文件大小无效，无法发送".to_string())?;
    let actual_file_size = tokio::fs::metadata(&transfer.file_path)
        .await
        .map_err(|error| error.to_string())?
        .len();
    if actual_file_size != declared_file_size {
        return Err(format!(
            "源文件大小已变化（当前 {} 字节，预期 {} 字节）",
            actual_file_size, declared_file_size
        ));
    }
    let mut file = match tokio::fs::File::open(&transfer.file_path).await {
        Ok(file) => file,
        Err(error) => return Err(error.to_string()),
    };
    let stream = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        tokio::net::TcpStream::connect(addr),
    )
    .await;
    let Ok(Ok(mut stream)) = stream else {
        return Err(format!("Failed to connect to {}", addr));
    };
    let _ = stream.set_nodelay(true);
    let socket = socket2::SockRef::from(&stream);
    let _ = socket.set_send_buffer_size(FILE_SOCKET_BUFFER_SIZE);
    let _ = socket.set_recv_buffer_size(FILE_SOCKET_BUFFER_SIZE);

    let mut buf = vec![0u8; FILE_CHUNK_SIZE];
    let mut content_buf = String::with_capacity(base64_encoded_capacity(FILE_CHUNK_SIZE));
    let mut payload = Vec::with_capacity(content_buf.capacity() + 1024);
    let sender_software_version = crate::profile_metadata::software_version();
    let sender_mac_address = crate::profile_metadata::mac_address();
    let mut sent_bytes: u64 = 0;
    if declared_file_size == 0 {
        wait_for_outgoing_file_transfer(client_msg_id)
            .await
            .map_err(|e| e.to_string())?;
        base64_encode_into(&[], &mut content_buf);
        if let Err(error) = serialize_file_wire_message_line(
            FileWireMessageLine {
                sender_id: &transfer.sender_id,
                sender_node_id: "",
                sender_name: &transfer.sender_name,
                sender_department: &transfer.sender_department,
                sender_software_version: &sender_software_version,
                sender_mac_address: &sender_mac_address,
                sender_port: transfer.sender_port,
                receiver_id: &transfer.peer_id,
                receiver_node_id: "",
                content: &content_buf,
                msg_type: "file_end",
                file_name: &transfer.file_name,
                file_size: declared_file_size,
                file_kind: &transfer.file_kind,
                known_peers: &[],
                group_id: Some(&transfer.group_id),
                client_msg_id: wire_client_msg_id,
            },
            &mut payload,
        ) {
            return Err(format!("Failed to serialize group file chunk: {}", error));
        }
        if stream.write_all(&payload).await.is_err() {
            return Err(format!("Failed to write to {}", addr));
        }
    } else {
        while sent_bytes < declared_file_size {
            wait_for_outgoing_file_transfer(client_msg_id)
                .await
                .map_err(|e| e.to_string())?;

            let remaining = (declared_file_size - sent_bytes).min(buf.len() as u64) as usize;
            let n = match file.read(&mut buf[..remaining]).await {
                Ok(0) => return Err("源文件在发送期间被截断，文件未完整发送".to_string()),
                Ok(n) => n,
                Err(error) => return Err(error.to_string()),
            };
            sent_bytes = sent_bytes
                .checked_add(n as u64)
                .ok_or_else(|| "文件发送字节计数溢出".to_string())?;
            let is_last = sent_bytes == declared_file_size;
            let msg_type = if is_last { "file_end" } else { "file_chunk" };
            base64_encode_into(&buf[..n], &mut content_buf);
            if let Err(error) = serialize_file_wire_message_line(
                FileWireMessageLine {
                    sender_id: &transfer.sender_id,
                    sender_node_id: "",
                    sender_name: &transfer.sender_name,
                    sender_department: &transfer.sender_department,
                    sender_software_version: &sender_software_version,
                    sender_mac_address: &sender_mac_address,
                    sender_port: transfer.sender_port,
                    receiver_id: &transfer.peer_id,
                    receiver_node_id: "",
                    content: &content_buf,
                    msg_type,
                    file_name: &transfer.file_name,
                    file_size: declared_file_size,
                    file_kind: &transfer.file_kind,
                    known_peers: &[],
                    group_id: Some(&transfer.group_id),
                    client_msg_id: wire_client_msg_id,
                },
                &mut payload,
            ) {
                return Err(format!("Failed to serialize group file chunk: {}", error));
            }
            if stream.write_all(&payload).await.is_err() {
                return Err(format!("Failed to write to {}", addr));
            }
            if is_last {
                break;
            }
        }
    }

    stream.flush().await.map_err(|e| e.to_string())?;
    if requires_message_ack(wire_client_msg_id, peer_software_version)
        && !wait_for_message_ack(&mut stream, wire_client_msg_id).await
    {
        return Err("对方未确认文件完整接收，文件将保留并稍后重试".to_string());
    }
    Ok(())
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
pub(crate) fn build_notification_json(
    sender_id: &str,
    sender_node_id: &str,
    sender_name: &str,
    sender_department: &str,
    sender_port: u16,
    receiver_id: &str,
    receiver_node_id: &str,
    content: &str,
    msg_type: &str,
    group_id: Option<&str>,
    file_name: Option<&str>,
    client_msg_id: Option<&str>,
    known_peers: &[crate::discovery::PeerEntry],
) -> String {
    serde_json::json!({
        "sender_id": sender_id,
        "sender_node_id": sender_node_id,
        "sender_name": sender_name,
        "sender_department": sender_department,
        "sender_software_version": crate::profile_metadata::software_version(),
        "sender_mac_address": crate::profile_metadata::mac_address(),
        "sender_port": sender_port,
        "receiver_id": receiver_id,
        "receiver_node_id": receiver_node_id,
        "content": content,
        "msg_type": msg_type,
        "group_id": group_id,
        "file_name": file_name,
        "file_size": null,
        "file_data": null,
        "client_msg_id": client_msg_id,
        "known_peers": known_peers,
    })
    .to_string()
}

/// Try a TCP delivery with a 2s connect timeout. ACK-capable peers must confirm
/// messages carrying a client id; legacy peers retain write-complete semantics.
async fn deliver_over_tcp(
    addr: &str,
    json: &str,
    client_msg_id: Option<&str>,
    peer_software_version: &str,
) -> bool {
    match tokio::time::timeout(
        std::time::Duration::from_secs(2),
        tokio::net::TcpStream::connect(addr),
    )
    .await
    {
        Ok(Ok(mut stream)) => {
            use tokio::io::AsyncWriteExt;
            if stream.write_all(json.as_bytes()).await.is_err()
                || stream.write_all(b"\n").await.is_err()
                || stream.flush().await.is_err()
            {
                return false;
            }
            if !requires_message_ack(client_msg_id, peer_software_version) {
                return true;
            }
            if wait_for_message_ack(&mut stream, client_msg_id).await {
                log::debug!("Delivery ack received from {}", addr);
                true
            } else {
                log::warn!("Required delivery ack missing from {}", addr);
                false
            }
        }
        _ => false,
    }
}

async fn deliver_pending_payloads_over_tcp(
    addr: &str,
    payloads: &[(i64, String, String)],
    peer_software_version: &str,
) -> Vec<i64> {
    if payloads.is_empty() {
        return Vec::new();
    }

    let mut delivered = Vec::new();
    let stream = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        tokio::net::TcpStream::connect(addr),
    )
    .await;

    let Ok(Ok(mut stream)) = stream else {
        return delivered;
    };

    use tokio::io::AsyncWriteExt;
    for (id, _kind, payload) in payloads {
        if stream.write_all(payload.as_bytes()).await.is_err()
            || stream.write_all(b"\n").await.is_err()
            || stream.flush().await.is_err()
        {
            break;
        }
        let expected_client_msg_id = serde_json::from_str::<serde_json::Value>(payload)
            .ok()
            .and_then(|value| {
                value
                    .get("client_msg_id")
                    .and_then(|id| id.as_str())
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
                    .map(str::to_string)
            });
        if requires_message_ack(expected_client_msg_id.as_deref(), peer_software_version) {
            if !wait_for_message_ack(&mut stream, expected_client_msg_id.as_deref()).await {
                log::warn!(
                    "Required pending delivery ack missing from {} for {:?}",
                    addr,
                    expected_client_msg_id
                );
                break;
            }
            log::debug!(
                "Pending delivery ack received for {}",
                expected_client_msg_id.as_deref().unwrap_or_default()
            );
        }
        delivered.push(*id);
    }
    if stream.flush().await.is_err() {
        return Vec::new();
    }

    delivered
}

fn stored_peer_recently_online(peer: &StoredPeer) -> bool {
    const ONLINE_GRACE_SECS: i64 = 15;

    if !peer.is_online {
        return false;
    }

    let Ok(last_seen) = chrono::DateTime::parse_from_rfc3339(&peer.last_seen_at) else {
        return false;
    };

    chrono::Utc::now().timestamp() - last_seen.timestamp() <= ONLINE_GRACE_SECS
}

/// Generic "fan-out a notification to a set of peers, queue if offline".
///
/// `kind` is the logical type used for offline queueing (matches the wire
/// `msg_type`). The receiver's TCP handler dispatches by `msg_type` inside the
/// payload, so we don't need separate tables per kind.
///
/// Returns the count of (delivered, queued, failed_to_queue).
pub async fn send_or_queue_notification(
    db: &crate::db::Database,
    online_peers: &[Peer],
    target_peer_ids: &[String],
    self_id: &str,
    self_node_id: &str,
    self_name: &str,
    self_department: &str,
    self_port: u16,
    content: &str,
    kind: &str,
    group_id: Option<&str>,
    file_name: Option<&str>,
    client_msg_id: Option<&str>,
    known_peers: &[crate::discovery::PeerEntry],
) -> (usize, usize, usize) {
    let outcomes = futures::future::join_all(
        target_peer_ids
            .iter()
            .filter(|peer_id| peer_id.as_str() != self_id)
            .map(|peer_id| async move {
                let receiver_node_id = resolve_peer_node_id(peer_id, db, online_peers).await;
                let json = build_notification_json(
                    self_id,
                    self_node_id,
                    self_name,
                    self_department,
                    self_port,
                    peer_id,
                    &receiver_node_id,
                    content,
                    kind,
                    group_id,
                    file_name,
                    client_msg_id,
                    known_peers,
                );

                let peer_software_version =
                    resolve_peer_software_version(peer_id, db, online_peers).await;
                let delivered = match resolve_peer_addr(peer_id, db, online_peers).await {
                    Some((ip, port)) => {
                        let addr = format!("{}:{}", ip, port);
                        deliver_over_tcp(&addr, &json, client_msg_id, &peer_software_version).await
                    }
                    None => false,
                };

                if delivered {
                    return (1usize, 0usize, 0usize);
                }

                match db.queue_pending_notification(peer_id, kind, &json).await {
                    Ok(()) => (0, 1, 0),
                    Err(error) => {
                        log::error!(
                            "Failed to queue notification '{}' for {}: {}",
                            kind,
                            peer_id,
                            error
                        );
                        (0, 0, 1)
                    }
                }
            }),
    )
    .await;

    let (delivered, queued, failed) = outcomes.into_iter().fold(
        (0usize, 0usize, 0usize),
        |(delivered, queued, failed), (next_delivered, next_queued, next_failed)| {
            (
                delivered + next_delivered,
                queued + next_queued,
                failed + next_failed,
            )
        },
    );

    if queued > 0 || failed > 0 {
        log::info!(
            "Notification '{}' delivered={} queued={} failed={}",
            kind,
            delivered,
            queued,
            failed
        );
    }
    (delivered, queued, failed)
}

async fn resolve_peer_node_id(
    peer_id: &str,
    db: &crate::db::Database,
    online_peers: &[Peer],
) -> String {
    if let Some(peer) = online_peers.iter().find(|peer| peer.id == peer_id) {
        if !peer.node_id.trim().is_empty() {
            return peer.node_id.clone();
        }
    }
    db.get_stored_peer(peer_id)
        .await
        .ok()
        .flatten()
        .map(|peer| peer.node_id)
        .unwrap_or_default()
}

async fn resolve_peer_software_version(
    peer_id: &str,
    db: &crate::db::Database,
    online_peers: &[Peer],
) -> String {
    if let Some(peer) = online_peers.iter().find(|peer| peer.id == peer_id) {
        if !peer.software_version.trim().is_empty() {
            return peer.software_version.clone();
        }
    }
    db.get_stored_peer(peer_id)
        .await
        .ok()
        .flatten()
        .map(|peer| peer.software_version)
        .unwrap_or_default()
}

async fn resolve_peer_addr(
    peer_id: &str,
    db: &crate::db::Database,
    online_peers: &[Peer],
) -> Option<(std::net::IpAddr, u16)> {
    if let Some(p) = online_peers.iter().find(|p| p.id == peer_id) {
        if p.online && !p.ip.is_unspecified() && p.port != 0 {
            return Some((p.ip, p.port));
        }
        return None;
    }
    if let Ok(Some(sp)) = db.get_stored_peer(peer_id).await {
        if !stored_peer_recently_online(&sp) {
            return None;
        }
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
    let self_avatar = db.get_user_profile().await.ok().flatten();
    let mut out = Vec::with_capacity(member_ids.len());
    for id in member_ids {
        if id == self_id {
            out.push(crate::discovery::PeerEntry {
                id: self_id.to_string(),
                node_id: self_avatar
                    .as_ref()
                    .map(|profile| profile.node_id.clone())
                    .unwrap_or_default(),
                username: self_name.to_string(),
                department: self_department.to_string(),
                software_version: crate::profile_metadata::software_version(),
                mac_address: crate::profile_metadata::mac_address(),
                avatar_hash: self_avatar
                    .as_ref()
                    .map(|profile| profile.avatar_hash.clone())
                    .unwrap_or_default(),
                avatar_updated_at: self_avatar
                    .as_ref()
                    .map(|profile| profile.avatar_updated_at)
                    .unwrap_or_default(),
                ip: my_ip.clone(),
                port: self_port,
            });
            continue;
        }
        if let Some(p) = online_peers.iter().find(|p| p.id == *id) {
            out.push(crate::discovery::PeerEntry {
                id: p.id.clone(),
                node_id: p.node_id.clone(),
                username: p.username.clone(),
                department: p.department.clone(),
                software_version: p.software_version.clone(),
                mac_address: p.mac_address.clone(),
                avatar_hash: p.avatar_hash.clone(),
                avatar_updated_at: p.avatar_updated_at,
                ip: p.ip.to_string(),
                port: p.port,
            });
            continue;
        }
        if let Ok(Some(sp)) = db.get_stored_peer(id).await {
            out.push(crate::discovery::PeerEntry {
                id: sp.peer_id,
                node_id: sp.node_id,
                username: sp.username,
                department: sp.department,
                software_version: sp.software_version,
                mac_address: sp.mac_address,
                avatar_hash: sp.avatar_hash,
                avatar_updated_at: sp.avatar_updated_at,
                ip: sp.ip,
                port: sp.port,
            });
        } else {
            // Member we have no info about — still ship the id so receivers know they exist
            out.push(crate::discovery::PeerEntry {
                id: id.clone(),
                node_id: String::new(),
                username: String::new(),
                department: String::new(),
                software_version: String::new(),
                mac_address: String::new(),
                avatar_hash: String::new(),
                avatar_updated_at: 0,
                ip: String::new(),
                port: 0,
            });
        }
    }
    out
}

#[cfg(test)]
mod pending_delivery_tests {
    use super::{
        deliver_over_tcp, deliver_pending_payloads_over_tcp, finish_pending_delivery,
        send_group_file_payloads_over_tcp_controlled, send_or_queue_notification,
        try_begin_pending_delivery,
    };
    use crate::{
        db::{Database, PendingFileTransfer},
        discovery::Peer,
    };
    use std::sync::Arc;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::sync::Barrier;

    fn message_payload(client_msg_id: &str) -> String {
        serde_json::json!({
            "sender_id": "sender",
            "sender_name": "Sender",
            "sender_department": "",
            "sender_port": 9527,
            "receiver_id": "receiver",
            "content": "hello",
            "msg_type": "text",
            "file_name": null,
            "file_size": null,
            "file_data": null,
            "client_msg_id": client_msg_id
        })
        .to_string()
    }

    fn ack_payload(client_msg_id: &str) -> Vec<u8> {
        format!(
            "{}\n",
            serde_json::json!({
                "sender_id": "receiver",
                "sender_name": "Receiver",
                "sender_department": "",
                "sender_port": 9527,
                "receiver_id": "sender",
                "content": "",
                "msg_type": "message_ack",
                "file_name": null,
                "file_size": null,
                "file_data": null,
                "client_msg_id": client_msg_id
            })
        )
        .into_bytes()
    }

    #[tokio::test]
    async fn pending_delivery_is_single_flight_per_peer() {
        let peer = "single-flight-test-peer";
        let other = "single-flight-test-other";
        assert!(try_begin_pending_delivery(peer).await);
        assert!(!try_begin_pending_delivery(peer).await);
        assert!(try_begin_pending_delivery(other).await);

        finish_pending_delivery(peer).await;
        finish_pending_delivery(other).await;
        assert!(try_begin_pending_delivery(peer).await);
        finish_pending_delivery(peer).await;
    }

    #[tokio::test]
    async fn ack_capable_delivery_rejects_wrong_ack() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            reader
                .get_mut()
                .write_all(&ack_payload("another-message"))
                .await
                .unwrap();
        });

        assert!(
            !deliver_over_tcp(
                &addr,
                &message_payload("message-1"),
                Some("message-1"),
                "0.2.0"
            )
            .await
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn legacy_delivery_succeeds_without_ack() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
        });

        assert!(
            deliver_over_tcp(
                &addr,
                &message_payload("message-1"),
                Some("message-1"),
                "0.1.0"
            )
            .await
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn pending_delivery_only_marks_matching_acks_as_delivered() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut reader = BufReader::new(stream);
            for ack_id in ["message-1", "wrong-message"] {
                let mut line = String::new();
                reader.read_line(&mut line).await.unwrap();
                reader
                    .get_mut()
                    .write_all(&ack_payload(ack_id))
                    .await
                    .unwrap();
            }
        });
        let payloads = vec![
            (1, "text".to_string(), message_payload("message-1")),
            (2, "text".to_string(), message_payload("message-2")),
        ];

        let delivered = deliver_pending_payloads_over_tcp(&addr, &payloads, "0.2.0").await;

        assert_eq!(delivered, vec![1]);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn group_file_delivery_waits_for_matching_ack() {
        let file_path =
            std::env::temp_dir().join(format!("echo-task10-file-{}.txt", uuid::Uuid::new_v4()));
        tokio::fs::write(&file_path, b"verified file bytes")
            .await
            .unwrap();
        let transfer = PendingFileTransfer {
            id: 1,
            group_id: "group-1".to_string(),
            peer_id: "receiver".to_string(),
            sender_id: "sender".to_string(),
            sender_name: "Sender".to_string(),
            sender_department: "研发部".to_string(),
            sender_port: 9527,
            file_path: file_path.to_string_lossy().to_string(),
            file_name: "verified.txt".to_string(),
            file_size: 19,
            file_kind: "file".to_string(),
            client_msg_id: "file-message-1".to_string(),
        };

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            let payload: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
            assert_eq!(payload["msg_type"], "file_end");
            assert_eq!(payload["file_size"], 19);
            reader
                .get_mut()
                .write_all(&ack_payload("file-message-1"))
                .await
                .unwrap();
        });

        let result = send_group_file_payloads_over_tcp_controlled(
            &addr,
            &transfer,
            Some("file-message-1"),
            "0.2.0",
        )
        .await;

        assert!(result.is_ok());
        server.await.unwrap();
        let _ = tokio::fs::remove_file(file_path).await;
    }

    #[tokio::test]
    async fn notification_fanout_connects_to_members_concurrently() {
        let barrier = Arc::new(Barrier::new(2));
        let mut peers = Vec::new();
        let mut servers = Vec::new();

        for index in 0..2 {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let peer_id = format!("fanout-peer-{index}");
            peers.push(Peer::new_with_profile(
                peer_id,
                format!("Peer {index}"),
                String::new(),
                "0.2.0".to_string(),
                String::new(),
                addr.ip(),
                addr.port(),
            ));

            let server_barrier = Arc::clone(&barrier);
            servers.push(tokio::spawn(async move {
                let (stream, _) = listener.accept().await.unwrap();
                let mut reader = BufReader::new(stream);
                let mut line = String::new();
                reader.read_line(&mut line).await.unwrap();
                let payload: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
                let client_msg_id = payload["client_msg_id"].as_str().unwrap().to_string();

                // Neither server ACKs until both connections have delivered their
                // payload. A serial fanout would deadlock here until its ACK timeout.
                server_barrier.wait().await;
                reader
                    .get_mut()
                    .write_all(&ack_payload(&client_msg_id))
                    .await
                    .unwrap();
            }));
        }

        let db_path =
            std::env::temp_dir().join(format!("echo-task13-fanout-{}.db", uuid::Uuid::new_v4()));
        let db = Database::new(db_path.to_str().unwrap()).await.unwrap();
        let target_ids: Vec<String> = peers.iter().map(|peer| peer.id.clone()).collect();

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            send_or_queue_notification(
                &db,
                &peers,
                &target_ids,
                "sender",
                "sender-node",
                "Sender",
                "研发部",
                9527,
                "hello group",
                "text",
                Some("group-1"),
                None,
                Some("fanout-message"),
                &[],
            ),
        )
        .await
        .expect("fanout should not wait for members serially");

        assert_eq!(result, (2, 0, 0));
        for server in servers {
            server.await.unwrap();
        }

        drop(db);
        let _ = std::fs::remove_file(db_path);
    }
}
