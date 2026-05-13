use serde::{Deserialize, Serialize};
use tauri::State;

use crate::db::{ChatMessage, StoredPeer, UserProfile};
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
