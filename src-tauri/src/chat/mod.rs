use anyhow::{Context, Result};
use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Mutex};

use crate::contact_sync;
use crate::db::Database;
use crate::discovery::{Peer, PeerEntry};
use tauri::{AppHandle, Manager};

pub const FILE_CHUNK_SIZE: usize = 2 * 1024 * 1024;
const FILE_LINE_BUFFER_SIZE: usize = 4 * 1024 * 1024;
const FILE_RECEIVE_BUFFER_SIZE: usize = 16 * 1024 * 1024;
pub const FILE_SOCKET_BUFFER_SIZE: usize = 16 * 1024 * 1024;
const FILE_PROGRESS_INTERVAL: std::time::Duration = std::time::Duration::from_millis(200);
const EVENT_CONVERSATION_UPDATED: &str = "conversation-updated";
pub const FILE_TRANSFER_CANCELLED_MESSAGE: &str = "发送已取消";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileTransferControlState {
    Running,
    Paused,
    Cancelled,
}

static FILE_TRANSFER_CONTROLS: OnceLock<Mutex<HashMap<String, FileTransferControlState>>> =
    OnceLock::new();

fn file_transfer_controls() -> &'static Mutex<HashMap<String, FileTransferControlState>> {
    FILE_TRANSFER_CONTROLS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn normalized_client_msg_id(client_msg_id: Option<&str>) -> Option<String> {
    client_msg_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

pub async fn register_outgoing_file_transfer(client_msg_id: Option<&str>) {
    let Some(client_msg_id) = normalized_client_msg_id(client_msg_id) else {
        return;
    };
    file_transfer_controls()
        .lock()
        .await
        .insert(client_msg_id, FileTransferControlState::Running);
}

pub async fn pause_outgoing_file_transfer(client_msg_id: &str) -> bool {
    let mut controls = file_transfer_controls().lock().await;
    if let Some(state) = controls.get_mut(client_msg_id) {
        if *state != FileTransferControlState::Cancelled {
            *state = FileTransferControlState::Paused;
        }
        return true;
    }
    false
}

pub async fn resume_outgoing_file_transfer(client_msg_id: &str) -> bool {
    let mut controls = file_transfer_controls().lock().await;
    if let Some(state) = controls.get_mut(client_msg_id) {
        if *state != FileTransferControlState::Cancelled {
            *state = FileTransferControlState::Running;
        }
        return true;
    }
    false
}

pub async fn cancel_outgoing_file_transfer(client_msg_id: &str) -> bool {
    let mut controls = file_transfer_controls().lock().await;
    if let Some(state) = controls.get_mut(client_msg_id) {
        *state = FileTransferControlState::Cancelled;
        return true;
    }
    false
}

pub async fn clear_outgoing_file_transfer(client_msg_id: Option<&str>) {
    let Some(client_msg_id) = normalized_client_msg_id(client_msg_id) else {
        return;
    };
    file_transfer_controls().lock().await.remove(&client_msg_id);
}

pub async fn wait_for_outgoing_file_transfer(client_msg_id: Option<&str>) -> Result<()> {
    let Some(client_msg_id) = normalized_client_msg_id(client_msg_id) else {
        return Ok(());
    };

    loop {
        let state = {
            file_transfer_controls()
                .lock()
                .await
                .get(&client_msg_id)
                .copied()
        };
        match state {
            Some(FileTransferControlState::Paused) => {
                tokio::time::sleep(std::time::Duration::from_millis(160)).await;
            }
            Some(FileTransferControlState::Cancelled) => {
                return Err(anyhow::anyhow!(FILE_TRANSFER_CANCELLED_MESSAGE));
            }
            _ => return Ok(()),
        }
    }
}

/// Wire protocol message sent between peers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireMessage {
    pub sender_id: String,
    pub sender_name: String,
    pub sender_department: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sender_software_version: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sender_mac_address: String,
    pub sender_port: u16,
    pub receiver_id: String,
    pub content: String,
    pub msg_type: String, // "text", "file", "sticker", "file_chunk", "file_end"
    pub file_name: Option<String>,
    pub file_size: Option<u64>,
    pub file_data: Option<Vec<u8>>, // base64 encoded in JSON
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub known_peers: Vec<PeerEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
}

#[derive(Serialize)]
struct WireMessageRef<'a> {
    sender_id: &'a str,
    sender_name: &'a str,
    sender_department: &'a str,
    #[serde(skip_serializing_if = "is_empty_str")]
    sender_software_version: &'a str,
    #[serde(skip_serializing_if = "is_empty_str")]
    sender_mac_address: &'a str,
    sender_port: u16,
    receiver_id: &'a str,
    content: &'a str,
    msg_type: &'a str,
    file_name: Option<&'a str>,
    file_size: Option<u64>,
    file_data: Option<&'a [u8]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    file_kind: Option<&'a str>,
    #[serde(skip_serializing_if = "is_empty_peer_entries")]
    known_peers: &'a [PeerEntry],
    #[serde(skip_serializing_if = "Option::is_none")]
    group_id: Option<&'a str>,
}

pub struct FileWireMessageLine<'a> {
    pub sender_id: &'a str,
    pub sender_name: &'a str,
    pub sender_department: &'a str,
    pub sender_software_version: &'a str,
    pub sender_mac_address: &'a str,
    pub sender_port: u16,
    pub receiver_id: &'a str,
    pub content: &'a str,
    pub msg_type: &'a str,
    pub file_name: &'a str,
    pub file_size: u64,
    pub file_kind: &'a str,
    pub known_peers: &'a [PeerEntry],
    pub group_id: Option<&'a str>,
}

struct IncomingFileState {
    file: BufWriter<tokio::fs::File>,
    path: String,
    file_name: String,
    sender_id: String,
    sender_name: String,
    group_id: Option<String>,
    kind: String,
}

async fn create_received_file(filename: &str) -> Result<(tokio::fs::File, String)> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| "/tmp".to_string());
    let files_dir = std::path::PathBuf::from(home).join("Echo").join("files");
    tokio::fs::create_dir_all(&files_dir).await?;

    let timestamp = chrono::Utc::now().timestamp_millis();
    let file_path = files_dir.join(format!("{}_{}", timestamp, filename));
    let file = tokio::fs::File::create(&file_path).await?;
    Ok((file, file_path.to_string_lossy().to_string()))
}

async fn start_incoming_file(msg: &WireMessage) -> Result<IncomingFileState> {
    let file_name = msg
        .file_name
        .clone()
        .unwrap_or_else(|| "unknown".to_string());
    let (file, path) = create_received_file(&file_name).await?;
    Ok(IncomingFileState {
        file: BufWriter::with_capacity(FILE_RECEIVE_BUFFER_SIZE, file),
        path,
        file_name,
        sender_id: msg.sender_id.clone(),
        sender_name: msg.sender_name.clone(),
        group_id: msg.group_id.clone(),
        kind: msg.file_kind.as_deref().unwrap_or("file").to_string(),
    })
}

fn file_send_connection_error(error: std::io::Error, peer: &Peer) -> anyhow::Error {
    use std::io::ErrorKind;

    match error.kind() {
        ErrorKind::BrokenPipe
        | ErrorKind::ConnectionAborted
        | ErrorKind::ConnectionReset
        | ErrorKind::NotConnected => {
            anyhow::anyhow!("对方当前离线或连接已断开，文件未发送。请等待对方上线后重试。")
        }
        _ => anyhow::anyhow!("发送文件到 {} 失败: {}", peer.address(), error),
    }
}

fn emit_file_progress(
    app_handle: &AppHandle,
    file_name: &str,
    client_msg_id: Option<&str>,
    sent: u64,
    total: u64,
    speed: u64,
) {
    let _ = app_handle.emit_all("file-progress", serde_json::json!({
        "fileName": file_name, "clientMsgId": client_msg_id, "sent": sent, "total": total, "speed": speed,
    }));
}

fn is_empty_str(value: &&str) -> bool {
    value.is_empty()
}

fn is_empty_peer_entries(value: &&[PeerEntry]) -> bool {
    value.is_empty()
}

fn serialize_wire_message_line(msg: &WireMessage) -> Result<Vec<u8>> {
    let mut payload = serde_json::to_vec(msg).context("Failed to serialize message")?;
    payload.push(b'\n');
    Ok(payload)
}

pub fn base64_encoded_capacity(input_len: usize) -> usize {
    base64::encoded_len(input_len, true).unwrap_or_else(|| {
        input_len
            .saturating_mul(4)
            .saturating_div(3)
            .saturating_add(4)
    })
}

pub fn base64_encode_into(data: &[u8], output: &mut String) {
    use base64::Engine;
    output.clear();
    let expected_len = base64_encoded_capacity(data.len());
    if output.capacity() < expected_len {
        output.reserve(expected_len - output.capacity());
    }
    base64::engine::general_purpose::STANDARD.encode_string(data, output);
}

fn base64_decode_into(input: &str, output: &mut Vec<u8>) -> Result<()> {
    use base64::Engine;
    output.clear();
    base64::engine::general_purpose::STANDARD
        .decode_vec(input, output)
        .with_context(|| "Failed to decode base64")
}

pub fn serialize_file_wire_message_line(
    msg: FileWireMessageLine<'_>,
    payload: &mut Vec<u8>,
) -> Result<()> {
    payload.clear();
    let wire_msg = WireMessageRef {
        sender_id: msg.sender_id,
        sender_name: msg.sender_name,
        sender_department: msg.sender_department,
        sender_software_version: msg.sender_software_version,
        sender_mac_address: msg.sender_mac_address,
        sender_port: msg.sender_port,
        receiver_id: msg.receiver_id,
        content: msg.content,
        msg_type: msg.msg_type,
        file_name: Some(msg.file_name),
        file_size: Some(msg.file_size),
        file_data: None,
        file_kind: Some(msg.file_kind),
        known_peers: msg.known_peers,
        group_id: msg.group_id,
    };
    serde_json::to_writer(&mut *payload, &wire_msg).context("Failed to serialize message")?;
    payload.push(b'\n');
    Ok(())
}

fn tune_tcp_stream_socket(stream: &TcpStream, label: &str) {
    if let Err(error) = stream.set_nodelay(true) {
        warn!("Failed to enable TCP_NODELAY for {}: {}", label, error);
    }
    let socket = socket2::SockRef::from(stream);
    if let Err(error) = socket.set_send_buffer_size(FILE_SOCKET_BUFFER_SIZE) {
        warn!("Failed to set TCP send buffer for {}: {}", label, error);
    }
    if let Err(error) = socket.set_recv_buffer_size(FILE_SOCKET_BUFFER_SIZE) {
        warn!("Failed to set TCP recv buffer for {}: {}", label, error);
    }
}

fn tune_file_tcp_stream(stream: &TcpStream, peer: &Peer) {
    let label = peer.address();
    tune_tcp_stream_socket(stream, &label);
}

/// Events forwarded to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncomingMessage {
    pub sender_id: String,
    pub sender_name: String,
    pub content: String,
    pub msg_type: String,
    pub file_name: Option<String>,
    pub file_size: Option<u64>,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize)]
struct ConversationUpdated {
    kind: String,
    peer_id: Option<String>,
    group_id: Option<String>,
}

pub struct ChatServer {
    app_handle: AppHandle,
    listen_port: u16,
    my_id: String,
    my_name: String,
    my_department: String,
    my_software_version: String,
    my_mac_address: String,
    db: Arc<Database>,
    incoming_tx: mpsc::UnboundedSender<IncomingMessage>,
    peers: Arc<std::sync::RwLock<std::collections::HashMap<String, Peer>>>,
}

impl ChatServer {
    pub fn new(
        app_handle: AppHandle,
        listen_port: u16,
        my_id: String,
        my_name: String,
        my_department: String,
        my_software_version: String,
        my_mac_address: String,
        db: Arc<Database>,
        peers: Arc<std::sync::RwLock<std::collections::HashMap<String, Peer>>>,
    ) -> Self {
        let (incoming_tx, _incoming_rx) = mpsc::unbounded_channel();
        Self {
            app_handle,
            listen_port,
            my_id,
            my_name,
            my_department,
            my_software_version,
            my_mac_address,
            db,
            incoming_tx,
            peers,
        }
    }

    pub fn my_id(&self) -> &str {
        &self.my_id
    }
    pub fn my_name(&self) -> &str {
        &self.my_name
    }
    pub fn my_department(&self) -> &str {
        &self.my_department
    }
    pub fn my_software_version(&self) -> &str {
        &self.my_software_version
    }
    pub fn my_mac_address(&self) -> &str {
        &self.my_mac_address
    }
    pub fn listen_port(&self) -> u16 {
        self.listen_port
    }
    pub fn db(&self) -> &Arc<Database> {
        &self.db
    }
    pub fn peers(&self) -> &Arc<std::sync::RwLock<std::collections::HashMap<String, Peer>>> {
        &self.peers
    }

    pub fn update_identity(&mut self, username: &str, department: &str) {
        self.my_name = username.to_string();
        self.my_department = department.to_string();
    }

    /// Start listening for incoming TCP connections.
    pub async fn start(&self) -> Result<()> {
        let addr = format!("0.0.0.0:{}", self.listen_port);
        let listener = TcpListener::bind(&addr)
            .await
            .with_context(|| format!("Failed to bind TCP listener on {}", addr))?;

        info!("Chat server listening on {}", addr);

        let db = Arc::clone(&self.db);
        let incoming_tx = self.incoming_tx.clone();
        let peers = Arc::clone(&self.peers);
        let app_handle = self.app_handle.clone();
        let my_id = self.my_id.clone();
        let my_name = self.my_name.clone();
        let my_department = self.my_department.clone();
        let my_software_version = self.my_software_version.clone();
        let my_mac_address = self.my_mac_address.clone();
        let my_port = self.listen_port;

        tauri::async_runtime::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, peer_addr)) => {
                        info!("Incoming connection from {}", peer_addr);
                        let db = Arc::clone(&db);
                        let tx = incoming_tx.clone();
                        let peers = Arc::clone(&peers);
                        let app_handle = app_handle.clone();
                        let my_id = my_id.clone();
                        let my_name = my_name.clone();
                        let my_department = my_department.clone();
                        let my_software_version = my_software_version.clone();
                        let my_mac_address = my_mac_address.clone();
                        tauri::async_runtime::spawn(async move {
                            if let Err(e) = Self::handle_incoming(
                                stream,
                                peer_addr,
                                app_handle,
                                db,
                                tx,
                                peers,
                                my_id,
                                my_name,
                                my_department,
                                my_software_version,
                                my_mac_address,
                                my_port,
                            )
                            .await
                            {
                                error!("Error handling connection: {}", e);
                            }
                        });
                    }
                    Err(e) => {
                        error!("Failed to accept connection: {}", e);
                    }
                }
            }
        });

        Ok(())
    }

    async fn handle_incoming(
        stream: TcpStream,
        peer_addr: std::net::SocketAddr,
        app_handle: AppHandle,
        db: Arc<Database>,
        incoming_tx: mpsc::UnboundedSender<IncomingMessage>,
        peers: Arc<std::sync::RwLock<std::collections::HashMap<String, Peer>>>,
        my_id: String,
        my_name: String,
        my_department: String,
        my_software_version: String,
        my_mac_address: String,
        my_port: u16,
    ) -> Result<()> {
        tune_tcp_stream_socket(&stream, &peer_addr.to_string());
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::with_capacity(FILE_LINE_BUFFER_SIZE, reader);

        let mut incoming_file: Option<IncomingFileState> = None;
        let mut line = String::with_capacity(base64_encoded_capacity(FILE_CHUNK_SIZE) + 1024);
        let mut incoming_decode_buf = Vec::with_capacity(FILE_CHUNK_SIZE);

        loop {
            line.clear();
            let bytes_read = reader.read_line(&mut line).await?;
            if bytes_read == 0 {
                break;
            }
            let line_for_parse = line.trim_end_matches('\n').trim_end_matches('\r');

            match serde_json::from_str::<WireMessage>(line_for_parse) {
                Ok(msg) => {
                    let msg_type = msg.msg_type.as_str();

                    // ── Contact sync delegation ──────────────────────────
                    if msg_type == "contact_summary" {
                        let my_ip = my_id.rsplitn(2, ':').nth(1).unwrap_or("127.0.0.1");
                        contact_sync::handle_contact_summary(
                            &db,
                            &peers,
                            &my_id,
                            &my_name,
                            &my_department,
                            &my_software_version,
                            &my_mac_address,
                            my_port,
                            my_ip,
                            &msg.sender_id,
                            msg.sender_port,
                            &peer_addr.ip().to_string(),
                            &msg.content,
                        )
                        .await;
                        continue;
                    }

                    if msg_type == "contact_sync_res" {
                        contact_sync::handle_contact_sync_res(&db, &peers, &my_id, &msg.content)
                            .await;
                        continue;
                    }

                    if msg_type == "identity_probe" {
                        let response = WireMessage {
                            sender_id: my_id.clone(),
                            sender_name: my_name.clone(),
                            sender_department: my_department.clone(),
                            sender_software_version: my_software_version.clone(),
                            sender_mac_address: my_mac_address.clone(),
                            sender_port: my_port,
                            receiver_id: msg.sender_id.clone(),
                            content: String::new(),
                            msg_type: "identity_response".to_string(),
                            file_name: None,
                            file_size: None,
                            file_data: None,
                            file_kind: None,
                            known_peers: Vec::new(),
                            group_id: None,
                        };
                        let json = serde_json::to_string(&response)
                            .context("Failed to serialize identity response")?;
                        writer.write_all(json.as_bytes()).await?;
                        writer.write_all(b"\n").await?;
                        writer.flush().await?;
                        continue;
                    }

                    // Mark sender as recent contact
                    let _ = db.add_recent_contact(&msg.sender_id).await;

                    for entry in &msg.known_peers {
                        if entry.id == my_id || entry.ip.is_empty() || entry.port == 0 {
                            continue;
                        }
                        if entry.ip.parse::<std::net::IpAddr>().is_err() {
                            continue;
                        }
                        let _ = db
                            .upsert_peer_with_profile(
                                &entry.id,
                                &entry.username,
                                &entry.department,
                                &entry.software_version,
                                &entry.mac_address,
                                &entry.ip,
                                entry.port,
                                false,
                            )
                            .await;
                    }

                    // Auto-join/discover group for system messages
                    if let Some(ref gid) = msg.group_id {
                        if msg.msg_type == "group_created" {
                            // Parse member list from content JSON: {"name":"...","member_ids":[...]}
                            let all_members: Vec<String> =
                                serde_json::from_str::<serde_json::Value>(&msg.content)
                                    .ok()
                                    .and_then(|v| v["member_ids"].as_array().cloned())
                                    .map(|arr| {
                                        arr.iter()
                                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                                            .collect()
                                    })
                                    .unwrap_or_else(|| vec![msg.sender_id.clone(), my_id.clone()]);
                            let group_name =
                                serde_json::from_str::<serde_json::Value>(&msg.content)
                                    .ok()
                                    .and_then(|v| v["name"].as_str().map(|s| s.to_string()))
                                    .unwrap_or_else(|| msg.content.clone());
                            // Idempotent — INSERT OR IGNORE under the hood; safe to call repeatedly
                            let _ = db
                                .create_group(gid, &group_name, &msg.sender_id, &all_members)
                                .await;
                            // Always add members (covers re-broadcast after invite_to_group)
                            let _ = db.add_group_members(gid, &all_members).await;
                            info!(
                                "Synced group {} ({} members)",
                                group_name,
                                all_members.len()
                            );
                        } else if msg.msg_type == "group_renamed" {
                            // file_name carries the actual new name; content is the display message.
                            // For offline-queued messages, fall back to parsing content.
                            let new_name = msg.file_name.as_deref().unwrap_or_else(|| {
                                msg.content
                                    .trim_start_matches("群名已修改为「")
                                    .trim_end_matches('」')
                            });
                            let _ = db.rename_group(gid, new_name).await;
                            info!("Group {} renamed to {}", gid, new_name);
                        } else if msg.msg_type == "group_dissolved" {
                            let _ = db.remove_group_member(gid, &my_id).await;
                            info!("Group {} dissolved — removed", gid);
                        } else if msg.msg_type == "group_member_left" {
                            // sender_id is the leaving member's peer_id
                            let _ = db.remove_group_member(gid, &msg.sender_id).await;
                            info!("Member {} left group {}", msg.sender_id, gid);
                        } else if msg.sender_id != my_id {
                            let _ = db.add_group_members(gid, &[msg.sender_id.clone()]).await;
                        }
                    }

                    // Handle profile_updated (no group_id, just updates the peer record).
                    // Receiver-side merge happens via the upsert_peer call below using
                    // sender_name/sender_department from the wire message — so we just
                    // need to skip persisting it as a chat message.
                    if msg.msg_type == "profile_updated" {
                        info!(
                            "Peer {} profile updated → {} ({})",
                            msg.sender_id, msg.sender_name, msg.sender_department
                        );
                    }

                    // Register the sender as a peer in DB
                    if let Err(e) = db
                        .upsert_peer_with_profile(
                            &msg.sender_id,
                            &msg.sender_name,
                            &msg.sender_department,
                            &msg.sender_software_version,
                            &msg.sender_mac_address,
                            &peer_addr.ip().to_string(),
                            msg.sender_port,
                            true,
                        )
                        .await
                    {
                        error!("Failed to upsert sender peer: {}", e);
                    }

                    // Also register in the in-memory peers map so UI picks it up immediately.
                    {
                        let remote_ip = match peer_addr.ip() {
                            std::net::IpAddr::V4(ip) => std::net::IpAddr::V4(ip),
                            std::net::IpAddr::V6(ip) => std::net::IpAddr::V6(ip),
                        };
                        let pid = format!("{}:{}", peer_addr.ip(), msg.sender_port);
                        let new_peer = Peer::new_with_profile(
                            pid.clone(),
                            msg.sender_name.clone(),
                            msg.sender_department.clone(),
                            msg.sender_software_version.clone(),
                            msg.sender_mac_address.clone(),
                            remote_ip,
                            msg.sender_port,
                        );
                        if let Ok(mut map) = peers.write() {
                            let is_new = !map.contains_key(&pid);
                            let already = map
                                .values()
                                .any(|p| p.ip == remote_ip && p.port == msg.sender_port);
                            if is_new && !already {
                                map.insert(pid.clone(), new_peer.clone());
                                info!("Auto-registered sender as peer: {}", new_peer);
                            } else if already {
                                if let Some(existing) = map
                                    .values_mut()
                                    .find(|p| p.ip == remote_ip && p.port == msg.sender_port)
                                {
                                    existing.username = msg.sender_name.clone();
                                    existing.department = msg.sender_department.clone();
                                    if !msg.sender_software_version.is_empty() {
                                        existing.software_version =
                                            msg.sender_software_version.clone();
                                    }
                                    if !msg.sender_mac_address.is_empty() {
                                        existing.mac_address = msg.sender_mac_address.clone();
                                    }
                                    existing.online = true;
                                }
                            }

                            // Process sender's known_peers (bidirectional relay via chat)
                            for entry in &msg.known_peers {
                                if entry.id != my_id && !map.contains_key(&entry.id) {
                                    if let Ok(entry_ip) = entry.ip.parse::<std::net::IpAddr>() {
                                        let relay = Peer::with_online_details(
                                            entry.id.clone(),
                                            entry.username.clone(),
                                            entry.department.clone(),
                                            entry.software_version.clone(),
                                            entry.mac_address.clone(),
                                            entry_ip,
                                            entry.port,
                                            false,
                                            0,
                                        );
                                        map.insert(entry.id.clone(), relay.clone());
                                        info!(
                                            "Chat relay: discovered {} via {}",
                                            entry.username, msg.sender_name
                                        );
                                    }
                                }
                            }
                        }

                        // Persist known_peers to DB (out of the sync RwLock so we can await).
                        // Skips entries with missing ip/port — those carry id-only and we have nothing to store.
                        for entry in &msg.known_peers {
                            if entry.id == my_id || entry.ip.is_empty() || entry.port == 0 {
                                continue;
                            }
                            if entry.ip.parse::<std::net::IpAddr>().is_err() {
                                continue;
                            }
                            let _ = db
                                .upsert_peer_with_profile(
                                    &entry.id,
                                    &entry.username,
                                    &entry.department,
                                    &entry.software_version,
                                    &entry.mac_address,
                                    &entry.ip,
                                    entry.port,
                                    false,
                                )
                                .await;
                        }
                    }

                    match msg_type {
                        "file_chunk" => {
                            if incoming_file.is_none() {
                                incoming_file = Some(start_incoming_file(&msg).await?);
                            }
                            let file_state = incoming_file
                                .as_mut()
                                .expect("incoming_file just initialized");
                            file_state.kind = msg
                                .file_kind
                                .as_deref()
                                .unwrap_or(&file_state.kind)
                                .to_string();
                            if let Err(error) =
                                base64_decode_into(&msg.content, &mut incoming_decode_buf)
                            {
                                warn!("Failed to decode incoming file chunk: {}", error);
                                incoming_decode_buf.clear();
                            }
                            file_state
                                .file
                                .write_all(&incoming_decode_buf)
                                .await
                                .with_context(|| {
                                    format!(
                                        "Failed to write incoming file chunk: {}",
                                        file_state.file_name
                                    )
                                })?;
                        }
                        "file_end" => {
                            if incoming_file.is_none() {
                                incoming_file = Some(start_incoming_file(&msg).await?);
                            }
                            let mut file_state = incoming_file
                                .take()
                                .expect("incoming_file just initialized");
                            file_state.kind = msg
                                .file_kind
                                .as_deref()
                                .unwrap_or(&file_state.kind)
                                .to_string();
                            if let Err(error) =
                                base64_decode_into(&msg.content, &mut incoming_decode_buf)
                            {
                                warn!("Failed to decode incoming file end: {}", error);
                                incoming_decode_buf.clear();
                            }
                            file_state
                                .file
                                .write_all(&incoming_decode_buf)
                                .await
                                .with_context(|| {
                                    format!(
                                        "Failed to write incoming file end: {}",
                                        file_state.file_name
                                    )
                                })?;
                            file_state.file.flush().await.with_context(|| {
                                format!("Failed to flush incoming file: {}", file_state.file_name)
                            })?;

                            let file_name_display =
                                msg.file_name.as_deref().unwrap_or(&file_state.file_name);
                            let saved_path = file_state.path.clone();

                            let sender_id = file_state.sender_id.as_str();
                            let sender_name = file_state.sender_name.as_str();
                            let msg_kind = if file_state.kind == "sticker" {
                                "sticker"
                            } else {
                                "file"
                            };
                            let display_content = if msg_kind == "sticker" {
                                "[表情]".to_string()
                            } else {
                                format!("📎 {}", file_name_display)
                            };
                            if let Some(ref gid) = file_state.group_id {
                                // Group file message — is_read=false so unread fires
                                if let Err(e) = db
                                    .save_group_message(
                                        gid,
                                        sender_id,
                                        sender_name,
                                        &display_content,
                                        msg_kind,
                                        Some(&saved_path),
                                        Some(file_name_display),
                                        msg.file_size.map(|s| s as i64),
                                        false,
                                        None,
                                    )
                                    .await
                                {
                                    error!("Failed to save incoming group file message: {}", e);
                                }
                                emit_group_updated(&app_handle, gid);
                            } else {
                                let receiver_id = &my_id;
                                if let Err(e) = db
                                    .save_message(
                                        sender_id,
                                        sender_name,
                                        receiver_id,
                                        &display_content,
                                        msg_kind,
                                        Some(&saved_path),
                                        Some(file_name_display),
                                        msg.file_size.map(|s| s as i64),
                                        None, // client_msg_id - incoming messages don't have it
                                    )
                                    .await
                                {
                                    error!("Failed to save incoming file message: {}", e);
                                }
                                emit_contact_updated(&app_handle, sender_id);
                            }

                            let _ = incoming_tx.send(IncomingMessage {
                                sender_id: sender_id.to_string(),
                                sender_name: sender_name.to_string(),
                                content: display_content,
                                msg_type: msg_kind.to_string(),
                                file_name: Some(file_name_display.to_string()),
                                file_size: msg.file_size,
                                timestamp: chrono::Utc::now().to_rfc3339(),
                            });
                        }
                        "group_created" | "group_dissolved" | "group_member_left"
                        | "profile_updated" => {
                            // System notifications — already handled above, don't save as message
                            if let Some(ref gid) = msg.group_id {
                                emit_group_updated(&app_handle, gid);
                            } else {
                                emit_contact_updated(&app_handle, &msg.sender_id);
                            }
                        }
                        _ => {
                            // Text or other message types
                            info!("Received message from {}: {}", msg.sender_name, msg.content);

                            if let Some(ref gid) = msg.group_id {
                                // Group message — save with group_id; is_read=false so unread counts work
                                let _ = db
                                    .save_group_message(
                                        gid,
                                        &msg.sender_id,
                                        &msg.sender_name,
                                        &msg.content,
                                        &msg.msg_type,
                                        None,
                                        None,
                                        None,
                                        false,
                                        None,
                                    )
                                    .await;
                                // Auto-join if needed (only when group truly missing — typical when we missed group_created)
                                let my_groups = db.list_groups(&my_id).await.unwrap_or_default();
                                if !my_groups.iter().any(|g| g.group_id == *gid) {
                                    let _ = db
                                        .create_group(
                                            gid,
                                            "(未命名群组)",
                                            &msg.sender_id,
                                            &[msg.sender_id.clone(), my_id.clone()],
                                        )
                                        .await;
                                    info!("Auto-joined group {} from message", gid);
                                }
                                emit_group_updated(&app_handle, gid);
                            } else {
                                // Private message
                                let _ = db
                                    .save_message(
                                        &msg.sender_id,
                                        &msg.sender_name,
                                        &my_id,
                                        &msg.content,
                                        &msg.msg_type,
                                        msg.file_name.as_deref(),
                                        None,
                                        msg.file_size.map(|s| s as i64),
                                        None,
                                    )
                                    .await;
                                emit_contact_updated(&app_handle, &msg.sender_id);
                            }

                            let _ = incoming_tx.send(IncomingMessage {
                                sender_id: msg.sender_id,
                                sender_name: msg.sender_name,
                                content: msg.content,
                                msg_type: msg.msg_type,
                                file_name: msg.file_name,
                                file_size: msg.file_size,
                                timestamp: chrono::Utc::now().to_rfc3339(),
                            });
                        }
                    }
                }
                Err(e) => {
                    warn!("Failed to parse incoming message: {}", e);
                }
            }
        }

        if let Some(file_state) = incoming_file.take() {
            let path = file_state.path.clone();
            drop(file_state);
            if let Err(error) = tokio::fs::remove_file(&path).await {
                warn!(
                    "Failed to remove incomplete incoming file {}: {}",
                    path, error
                );
            }
        }

        Ok(())
    }

    /// Compatibility wrapper for callers that only need plain text.
    #[allow(dead_code)]
    pub async fn send_message(&self, peer: &Peer, content: &str) -> Result<crate::db::ChatMessage> {
        self.send_message_typed(peer, content, "text", None).await
    }

    pub async fn send_message_typed(
        &self,
        peer: &Peer,
        content: &str,
        msg_type: &str,
        client_msg_id: Option<&str>,
    ) -> Result<crate::db::ChatMessage> {
        let msg = WireMessage {
            sender_id: self.my_id.clone(),
            sender_name: self.my_name.clone(),
            sender_department: self.my_department.clone(),
            sender_software_version: self.my_software_version.clone(),
            sender_mac_address: self.my_mac_address.clone(),
            sender_port: self.listen_port,
            receiver_id: peer.id.clone(),
            content: content.to_string(),
            msg_type: msg_type.to_string(),
            file_name: None,
            file_size: None,
            file_data: None,
            file_kind: None,
            known_peers: self.build_known_peers(),
            group_id: None,
        };

        let delivered = match self.send_wire_message(peer, &msg).await {
            Ok(()) => true,
            Err(err) => {
                warn!(
                    "Direct message delivery to {} failed, queueing for later: {}",
                    peer.id, err
                );
                let json =
                    serde_json::to_string(&msg).context("Failed to serialize queued message")?;
                self.db
                    .queue_pending_notification(&peer.id, msg_type, &json)
                    .await
                    .context("Failed to queue offline message")?;
                false
            }
        };
        let _ = self.db.add_recent_contact(&peer.id).await;
        if !delivered {
            let _ = self
                .db
                .upsert_peer_with_profile(
                    &peer.id,
                    &peer.username,
                    &peer.department,
                    &peer.software_version,
                    &peer.mac_address,
                    &peer.ip.to_string(),
                    peer.port,
                    false,
                )
                .await;
        }
        let saved = self
            .db
            .save_message(
                &self.my_id,
                &self.my_name,
                &peer.id,
                content,
                msg_type,
                None,
                None,
                None,
                client_msg_id,
            )
            .await?;
        Ok(saved)
    }

    /// Legacy streaming sender kept as a reference path for the newer background sender.
    #[allow(dead_code)]
    pub async fn send_file(
        &self,
        peer: &Peer,
        file_path: &str,
        file_name: &str,
        app_handle: tauri::AppHandle,
    ) -> Result<crate::db::ChatMessage> {
        use tokio::fs::File;
        use tokio::io::AsyncReadExt;

        let metadata = tokio::fs::metadata(file_path)
            .await
            .with_context(|| format!("Failed to read metadata: {}", file_path))?;
        let file_size = metadata.len();

        let file_name = file_name.to_string();

        let mut file = File::open(file_path)
            .await
            .with_context(|| format!("Failed to open file: {}", file_path))?;

        let mut stream = TcpStream::connect(peer.address())
            .await
            .with_context(|| format!("Failed to connect to peer {}", peer.address()))?;
        tune_file_tcp_stream(&stream, peer);

        let peers_list = self.build_known_peers();
        let mut buf = vec![0u8; FILE_CHUNK_SIZE];
        let mut content_buf = String::with_capacity(base64_encoded_capacity(FILE_CHUNK_SIZE));
        let mut payload = Vec::with_capacity(content_buf.capacity() + 1024);
        let mut i: u64 = 0;
        let start_time = std::time::Instant::now();
        let mut last_progress_emit = start_time;

        loop {
            let n = file
                .read(&mut buf)
                .await
                .with_context(|| format!("Failed to read file chunk {}", i))?;
            if n == 0 {
                break;
            }
            let is_last =
                n < FILE_CHUNK_SIZE || (file_size as usize) <= ((i as usize + 1) * FILE_CHUNK_SIZE);
            let msg_type = if is_last { "file_end" } else { "file_chunk" };
            let known_peers: &[PeerEntry] = if i == 0 { peers_list.as_slice() } else { &[] };
            base64_encode_into(&buf[..n], &mut content_buf);

            serialize_file_wire_message_line(
                FileWireMessageLine {
                    sender_id: &self.my_id,
                    sender_name: &self.my_name,
                    sender_department: &self.my_department,
                    sender_software_version: &self.my_software_version,
                    sender_mac_address: &self.my_mac_address,
                    sender_port: self.listen_port,
                    receiver_id: &peer.id,
                    content: &content_buf,
                    msg_type,
                    file_name: &file_name,
                    file_size,
                    file_kind: "file",
                    known_peers,
                    group_id: None,
                },
                &mut payload,
            )?;

            stream.write_all(&payload).await?;
            i += 1;

            let sent = std::cmp::min((i as usize) * FILE_CHUNK_SIZE, file_size as usize) as u64;
            if last_progress_emit.elapsed() >= FILE_PROGRESS_INTERVAL {
                let elapsed = start_time.elapsed().as_secs_f64().max(0.1);
                let speed = (sent as f64 / elapsed) as u64;
                emit_file_progress(&app_handle, &file_name, None, sent, file_size, speed);
                last_progress_emit = std::time::Instant::now();
            }
        }
        stream.flush().await?;

        let elapsed = start_time.elapsed().as_secs_f64().max(0.1);
        let speed = (file_size as f64 / elapsed) as u64;
        emit_file_progress(&app_handle, &file_name, None, file_size, file_size, speed);

        info!(
            "File send complete: {} ({} bytes, {} chunks)",
            file_name, file_size, i
        );

        self.bump_last_seen(peer);

        // Save outgoing file message to DB
        let saved = self
            .db
            .save_message(
                &self.my_id,
                &self.my_name,
                &peer.id,
                &format!("📎 {}", file_name),
                "file",
                Some(file_path),
                Some(&file_name),
                Some(file_size as i64),
                None, // client_msg_id not used in this legacy path
            )
            .await?;

        Ok(saved)
    }

    async fn send_wire_message(&self, peer: &Peer, msg: &WireMessage) -> Result<()> {
        let mut stream = TcpStream::connect(peer.address())
            .await
            .with_context(|| format!("Failed to connect to peer {}", peer.address()))?;
        tune_file_tcp_stream(&stream, peer);

        let payload = serialize_wire_message_line(msg)?;
        stream
            .write_all(&payload)
            .await
            .context("Failed to write message")?;
        stream.flush().await?;

        // TCP success = peer is definitely online
        self.bump_last_seen(peer);

        Ok(())
    }

    fn build_known_peers(&self) -> Vec<PeerEntry> {
        if let Ok(map) = self.peers.read() {
            map.values()
                .filter(|p| p.online)
                .map(|p| PeerEntry {
                    id: p.id.clone(),
                    username: p.username.clone(),
                    department: p.department.clone(),
                    software_version: p.software_version.clone(),
                    mac_address: p.mac_address.clone(),
                    ip: p.ip.to_string(),
                    port: p.port,
                })
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Initiate a contact-summary exchange with a peer.
    /// Sends our full contact summary; the response arrives asynchronously
    /// through the normal TCP listener via `contact_sync_res`.
    pub async fn exchange_contacts(&self, target_ip: &str, target_port: u16, target_id: &str) {
        let my_ip = self.my_id.rsplitn(2, ':').nth(1).unwrap_or("127.0.0.1");
        contact_sync::exchange_with_peer(
            &self.db,
            &self.peers,
            &self.my_id,
            &self.my_name,
            &self.my_department,
            &self.my_software_version,
            &self.my_mac_address,
            self.listen_port,
            my_ip,
            target_ip,
            target_port,
            target_id,
        )
        .await;
    }

    fn bump_last_seen(&self, peer: &Peer) {
        if let Ok(mut map) = self.peers.write() {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;

            if let Some(existing) = map
                .values_mut()
                .find(|p| p.ip == peer.ip && p.port == peer.port)
            {
                existing.online = true;
                existing.last_seen = now;
                if !peer.username.is_empty() && peer.username != "手动添加" {
                    existing.username = peer.username.clone();
                }
                if !peer.department.is_empty() {
                    existing.department = peer.department.clone();
                }
                if !peer.software_version.is_empty() {
                    existing.software_version = peer.software_version.clone();
                }
                if !peer.mac_address.is_empty() {
                    existing.mac_address = peer.mac_address.clone();
                }
                info!(
                    "bump_last_seen: {} updated (online, last_seen={})",
                    existing.id, now
                );
            } else {
                // Peer not in discovery map yet — insert it so UI picks it up
                let mut p = peer.clone();
                p.online = true;
                p.last_seen = now;
                map.insert(p.id.clone(), p.clone());
                info!(
                    "bump_last_seen: inserted new peer {} into map ({} total)",
                    p.id,
                    map.len()
                );
            }
        }
    }
}

fn emit_contact_updated(app_handle: &AppHandle, peer_id: &str) {
    let _ = app_handle.emit_all(
        EVENT_CONVERSATION_UPDATED,
        ConversationUpdated {
            kind: "contact".to_string(),
            peer_id: Some(peer_id.to_string()),
            group_id: None,
        },
    );
}

fn emit_group_updated(app_handle: &AppHandle, group_id: &str) {
    let _ = app_handle.emit_all(
        EVENT_CONVERSATION_UPDATED,
        ConversationUpdated {
            kind: "group".to_string(),
            peer_id: None,
            group_id: Some(group_id.to_string()),
        },
    );
}

/// Compatibility wrapper for default file transfers; commands use the typed variant directly.
#[allow(dead_code)]
pub async fn send_file_in_background(
    file_path: &str,
    file_name: &str,
    peer: &Peer,
    my_id: String,
    my_name: String,
    my_department: String,
    listen_port: u16,
    db: Arc<Database>,
    peers: Arc<std::sync::RwLock<std::collections::HashMap<String, Peer>>>,
    app_handle: tauri::AppHandle,
    client_msg_id: Option<String>,
) -> Result<crate::db::ChatMessage> {
    send_file_in_background_with_kind(
        file_path,
        file_name,
        peer,
        my_id,
        my_name,
        my_department,
        listen_port,
        db,
        peers,
        app_handle,
        client_msg_id,
        "file",
    )
    .await
}

pub async fn send_file_in_background_with_kind(
    file_path: &str,
    file_name: &str,
    peer: &Peer,
    my_id: String,
    my_name: String,
    my_department: String,
    listen_port: u16,
    db: Arc<Database>,
    peers: Arc<std::sync::RwLock<std::collections::HashMap<String, Peer>>>,
    app_handle: tauri::AppHandle,
    client_msg_id: Option<String>,
    file_kind: &str,
) -> Result<crate::db::ChatMessage> {
    send_file_in_background_inner(
        file_path,
        file_name,
        peer,
        my_id,
        my_name,
        my_department,
        listen_port,
        db,
        peers,
        app_handle,
        None,
        client_msg_id,
        file_kind,
    )
    .await
}

/// Compatibility wrapper for grouped file transfers using the default `file` kind.
#[allow(dead_code)]
pub async fn send_file_in_background_grouped(
    file_path: &str,
    file_name: &str,
    peer: &Peer,
    my_id: String,
    my_name: String,
    my_department: String,
    listen_port: u16,
    db: Arc<Database>,
    peers: Arc<std::sync::RwLock<std::collections::HashMap<String, Peer>>>,
    app_handle: tauri::AppHandle,
    group_id: Option<String>,
    client_msg_id: Option<String>,
) -> Result<()> {
    send_file_in_background_inner(
        file_path,
        file_name,
        peer,
        my_id,
        my_name,
        my_department,
        listen_port,
        db,
        peers,
        app_handle,
        group_id,
        client_msg_id,
        "file",
    )
    .await
    .map(|_| ())
}

async fn send_file_in_background_inner(
    file_path: &str,
    file_name: &str,
    peer: &Peer,
    my_id: String,
    my_name: String,
    my_department: String,
    listen_port: u16,
    db: Arc<Database>,
    peers: Arc<std::sync::RwLock<std::collections::HashMap<String, Peer>>>,
    app_handle: tauri::AppHandle,
    group_id: Option<String>,
    client_msg_id: Option<String>,
    file_kind: &str,
) -> Result<crate::db::ChatMessage> {
    use tokio::fs::File;
    use tokio::io::AsyncReadExt;

    let metadata = tokio::fs::metadata(file_path)
        .await
        .with_context(|| format!("Failed to read metadata: {}", file_path))?;
    let file_size = metadata.len();

    let mut file = File::open(file_path)
        .await
        .with_context(|| format!("Failed to open file: {}", file_path))?;

    wait_for_outgoing_file_transfer(client_msg_id.as_deref()).await?;

    let mut stream = TcpStream::connect(peer.address())
        .await
        .with_context(|| format!("Failed to connect to peer {}", peer.address()))?;
    tune_file_tcp_stream(&stream, peer);

    // Build known_peers once
    let peers_list: Vec<PeerEntry> = if let Ok(map) = peers.read() {
        map.values()
            .filter(|p| p.online)
            .map(|p| PeerEntry {
                id: p.id.clone(),
                username: p.username.clone(),
                department: p.department.clone(),
                software_version: p.software_version.clone(),
                mac_address: p.mac_address.clone(),
                ip: p.ip.to_string(),
                port: p.port,
            })
            .collect()
    } else {
        Vec::new()
    };

    // Emit start event immediately so UI shows progress bar
    emit_file_progress(
        &app_handle,
        file_name,
        client_msg_id.as_deref(),
        0,
        file_size,
        0,
    );

    let mut buf = vec![0u8; FILE_CHUNK_SIZE];
    let mut content_buf = String::with_capacity(base64_encoded_capacity(FILE_CHUNK_SIZE));
    let mut payload = Vec::with_capacity(content_buf.capacity() + 1024);
    let sender_software_version = crate::profile_metadata::software_version();
    let sender_mac_address = crate::profile_metadata::mac_address();
    let mut i: u64 = 0;
    let start_time = std::time::Instant::now();
    let mut last_progress_emit = start_time;

    loop {
        wait_for_outgoing_file_transfer(client_msg_id.as_deref()).await?;

        let n = file
            .read(&mut buf)
            .await
            .with_context(|| format!("Failed to read file chunk {}", i))?;
        if n == 0 {
            break;
        }

        let is_last =
            n < FILE_CHUNK_SIZE || (file_size as usize) <= ((i as usize + 1) * FILE_CHUNK_SIZE);
        let msg_type = if is_last { "file_end" } else { "file_chunk" };
        let known_peers: &[PeerEntry] = if i == 0 { peers_list.as_slice() } else { &[] };
        base64_encode_into(&buf[..n], &mut content_buf);

        serialize_file_wire_message_line(
            FileWireMessageLine {
                sender_id: &my_id,
                sender_name: &my_name,
                sender_department: &my_department,
                sender_software_version: &sender_software_version,
                sender_mac_address: &sender_mac_address,
                sender_port: listen_port,
                receiver_id: &peer.id,
                content: &content_buf,
                msg_type,
                file_name,
                file_size,
                file_kind,
                known_peers,
                group_id: group_id.as_deref(),
            },
            &mut payload,
        )?;
        stream
            .write_all(&payload)
            .await
            .map_err(|error| file_send_connection_error(error, peer))?;
        i += 1;

        let sent = std::cmp::min((i as usize) * FILE_CHUNK_SIZE, file_size as usize) as u64;
        if last_progress_emit.elapsed() >= FILE_PROGRESS_INTERVAL {
            let elapsed = start_time.elapsed().as_secs_f64().max(0.1);
            let speed = (sent as f64 / elapsed) as u64;
            emit_file_progress(
                &app_handle,
                file_name,
                client_msg_id.as_deref(),
                sent,
                file_size,
                speed,
            );
            last_progress_emit = std::time::Instant::now();
        }
    }
    stream
        .flush()
        .await
        .map_err(|error| file_send_connection_error(error, peer))?;

    let elapsed = start_time.elapsed().as_secs_f64().max(0.1);
    let speed = (file_size as f64 / elapsed) as u64;
    emit_file_progress(
        &app_handle,
        file_name,
        client_msg_id.as_deref(),
        file_size,
        file_size,
        speed,
    );

    info!(
        "File send complete: {} ({} bytes, {} chunks)",
        file_name, file_size, i
    );

    // For 1:1 chat we save the outgoing message + bump recent contacts here.
    // For group chats the caller already persisted the outgoing message before fanout,
    // so we just return a synthetic ChatMessage placeholder.
    if group_id.is_none() {
        // Mark as recent contact
        let _ = db.add_recent_contact(&peer.id).await;

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
        let saved = db
            .save_message(
                &my_id,
                &my_name,
                &peer.id,
                &content,
                msg_kind,
                Some(file_path),
                Some(file_name),
                Some(file_size as i64),
                client_msg_id.as_deref(),
            )
            .await?;

        // Update peer last_seen
        {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            if let Ok(mut map) = peers.write() {
                if let Some(existing) = map
                    .values_mut()
                    .find(|p| p.ip == peer.ip && p.port == peer.port)
                {
                    existing.online = true;
                    existing.last_seen = now;
                }
            }
        }

        Ok(saved)
    } else {
        // Update last_seen for group case too
        {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            if let Ok(mut map) = peers.write() {
                if let Some(existing) = map
                    .values_mut()
                    .find(|p| p.ip == peer.ip && p.port == peer.port)
                {
                    existing.online = true;
                    existing.last_seen = now;
                }
            }
        }
        Ok(crate::db::ChatMessage {
            id: 0,
            sender_id: my_id,
            sender_name: my_name,
            receiver_id: peer.id.clone(),
            content: format!("📎 {}", file_name),
            msg_type: "file".to_string(),
            file_path: Some(file_path.to_string()),
            file_name: Some(file_name.to_string()),
            file_size: Some(file_size as i64),
            timestamp: chrono::Utc::now().to_rfc3339(),
            is_read: true,
            client_msg_id,
        })
    }
}
