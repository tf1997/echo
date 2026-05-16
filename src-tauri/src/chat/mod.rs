use anyhow::{Context, Result};
use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

use crate::db::Database;
use crate::discovery::{Peer, PeerEntry};
use tauri::Emitter;

/// Wire protocol message sent between peers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireMessage {
    pub sender_id: String,
    pub sender_name: String,
    pub sender_department: String,
    pub sender_port: u16,
    pub receiver_id: String,
    pub content: String,
    pub msg_type: String,   // "text", "file", "file_chunk", "file_end"
    pub file_name: Option<String>,
    pub file_size: Option<u64>,
    pub file_data: Option<Vec<u8>>, // base64 encoded in JSON
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub known_peers: Vec<PeerEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
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

pub struct ChatServer {
    listen_port: u16,
    my_id: String,
    my_name: String,
    my_department: String,
    db: Arc<Database>,
    incoming_tx: mpsc::UnboundedSender<IncomingMessage>,
    peers: Arc<std::sync::RwLock<std::collections::HashMap<String, Peer>>>,
}

impl ChatServer {
    pub fn new(
        listen_port: u16,
        my_id: String,
        my_name: String,
        my_department: String,
        db: Arc<Database>,
        peers: Arc<std::sync::RwLock<std::collections::HashMap<String, Peer>>>,
    ) -> Self {
        let (incoming_tx, _incoming_rx) = mpsc::unbounded_channel();
        Self {
            listen_port,
            my_id,
            my_name,
            my_department,
            db,
            incoming_tx,
            peers,
        }
    }

    pub fn my_id(&self) -> &str { &self.my_id }
    pub fn my_name(&self) -> &str { &self.my_name }
    pub fn my_department(&self) -> &str { &self.my_department }
    pub fn listen_port(&self) -> u16 { self.listen_port }
    pub fn db(&self) -> &Arc<Database> { &self.db }
    pub fn peers(&self) -> &Arc<std::sync::RwLock<std::collections::HashMap<String, Peer>>> { &self.peers }

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
        let my_id = self.my_id.clone();

        tauri::async_runtime::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, peer_addr)) => {
                        info!("Incoming connection from {}", peer_addr);
                        let db = Arc::clone(&db);
                        let tx = incoming_tx.clone();
                        let peers = Arc::clone(&peers);
                        let my_id = my_id.clone();
                        tauri::async_runtime::spawn(async move {
                            if let Err(e) = Self::handle_incoming(stream, peer_addr, db, tx, peers, my_id).await {
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
        db: Arc<Database>,
        incoming_tx: mpsc::UnboundedSender<IncomingMessage>,
        peers: Arc<std::sync::RwLock<std::collections::HashMap<String, Peer>>>,
        my_id: String,
    ) -> Result<()> {
        let (reader, _writer) = stream.into_split();
        let mut lines = BufReader::new(reader).lines();

        let mut file_buffer: Vec<u8> = Vec::new();
        let mut file_sender_id: Option<String> = None;
        let mut file_sender_name: Option<String> = None;

        while let Some(line) = lines.next_line().await? {
            match serde_json::from_str::<WireMessage>(&line) {
                Ok(msg) => {
                    let msg_type = msg.msg_type.as_str();

                    // Mark sender as recent contact
                    let _ = db.add_recent_contact(&msg.sender_id).await;

                    // Auto-join/discover group for system messages
                    if let Some(ref gid) = msg.group_id {
                        if msg.msg_type == "group_created" {
                            // Parse member list from content JSON: {"name":"...","member_ids":[...]}
                            let all_members: Vec<String> = serde_json::from_str::<serde_json::Value>(&msg.content).ok()
                                .and_then(|v| v["member_ids"].as_array().cloned())
                                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
                                .unwrap_or_else(|| vec![msg.sender_id.clone(), my_id.clone()]);
                            let group_name = serde_json::from_str::<serde_json::Value>(&msg.content).ok()
                                .and_then(|v| v["name"].as_str().map(|s| s.to_string()))
                                .unwrap_or_else(|| msg.content.clone());
                            let _ = db.create_group(gid, &group_name, &msg.sender_id, &all_members).await;
                            info!("Joined new group {} ({} members)", group_name, all_members.len());
                        } else if msg.msg_type == "group_dissolved" {
                            let _ = db.remove_group_member(gid, &my_id).await;
                            info!("Group {} dissolved — removed", gid);
                        }
                    }

                    // Register the sender as a peer in DB
                    if let Err(e) = db
                        .upsert_peer(
                            &msg.sender_id,
                            &msg.sender_name,
                            &msg.sender_department,
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
                        let new_peer = Peer::new(
                            pid.clone(),
                            msg.sender_name.clone(),
                            msg.sender_department.clone(),
                            remote_ip,
                            msg.sender_port,
                        );
                        if let Ok(mut map) = peers.write() {
                            let is_new = !map.contains_key(&pid);
                            let already = map.values().any(|p| p.ip == remote_ip && p.port == msg.sender_port);
                            if is_new && !already {
                                map.insert(pid.clone(), new_peer.clone());
                                info!("Auto-registered sender as peer: {}", new_peer);
                            } else if already {
                                if let Some(existing) = map.values_mut().find(|p| p.ip == remote_ip && p.port == msg.sender_port) {
                                    existing.username = msg.sender_name.clone();
                                    existing.department = msg.sender_department.clone();
                                    existing.online = true;
                                }
                            }

                            // Process sender's known_peers (bidirectional relay via chat)
                            for entry in &msg.known_peers {
                                if entry.id != my_id && !map.contains_key(&entry.id) {
                                    if let Ok(entry_ip) = entry.ip.parse::<std::net::IpAddr>() {
                                        let relay = Peer::new(
                                            entry.id.clone(), entry.username.clone(),
                                            entry.department.clone(), entry_ip, entry.port,
                                        );
                                        map.insert(entry.id.clone(), relay.clone());
                                        info!("Chat relay: discovered {} via {}", entry.username, msg.sender_name);
                                    }
                                }
                            }
                        }
                    }

                    match msg_type {
                        "file_chunk" => {
                            if file_buffer.is_empty() {
                                file_sender_id = Some(msg.sender_id.clone());
                                file_sender_name = Some(msg.sender_name.clone());
                            }
                            let decoded = base64_decode(&msg.content)
                                .unwrap_or_default();
                            file_buffer.extend_from_slice(&decoded);
                        }
                        "file_end" => {
                            if file_sender_id.is_none() {
                                file_sender_id = Some(msg.sender_id.clone());
                                file_sender_name = Some(msg.sender_name.clone());
                            }
                            let decoded = base64_decode(&msg.content)
                                .unwrap_or_default();
                            file_buffer.extend_from_slice(&decoded);

                            let file_name_display = msg.file_name.as_deref().unwrap_or("unknown");
                            let saved_path = save_received_file(&file_buffer, file_name_display)?;

                            let sender_id = file_sender_id.as_deref().unwrap_or(&msg.sender_id);
                            let sender_name = file_sender_name.as_deref().unwrap_or(&msg.sender_name);
                            let receiver_id = &my_id;
                            if let Err(e) = db
                                .save_message(
                                    sender_id,
                                    sender_name,
                                    receiver_id,
                                    &format!("📎 {}", file_name_display),
                                    "file",
                                    Some(&saved_path),
                                    Some(file_name_display),
                                    msg.file_size.map(|s| s as i64),
                                )
                                .await
                            {
                                error!("Failed to save incoming file message: {}", e);
                            }

                            let _ = incoming_tx.send(IncomingMessage {
                                sender_id: sender_id.to_string(),
                                sender_name: sender_name.to_string(),
                                content: format!("📎 {}", file_name_display),
                                msg_type: "file".to_string(),
                                file_name: Some(file_name_display.to_string()),
                                file_size: msg.file_size,
                                timestamp: chrono::Utc::now().to_rfc3339(),
                            });

                            file_buffer.clear();
                            file_sender_id = None;
                            file_sender_name = None;
                        }
                        "group_created" | "group_dissolved" => {
                            // System notifications — already handled above, don't save as message
                        }
                        _ => {
                            // Text or other message types
                            info!("Received message from {}: {}", msg.sender_name, msg.content);

                            if let Some(ref gid) = msg.group_id {
                                // Group message — save with group_id
                                let _ = db.save_group_message(gid, &msg.sender_id, &msg.sender_name, &msg.content, &msg.msg_type, None, None, None).await;
                                // Auto-join if needed
                                let my_groups = db.list_groups(&my_id).await.unwrap_or_default();
                                if !my_groups.iter().any(|g| g.group_id == *gid) {
                                    let _ = db.add_group_members(gid, &[my_id.clone()]).await;
                                    info!("Auto-joined group {} from message", gid);
                                }
                            } else {
                                // Private message
                                let _ = db.save_message(&msg.sender_id, &msg.sender_name, &my_id, &msg.content, &msg.msg_type, msg.file_name.as_deref(), None, msg.file_size.map(|s| s as i64)).await;
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

        Ok(())
    }

    /// Send a text message to a peer.
    pub async fn send_message(&self, peer: &Peer, content: &str) -> Result<crate::db::ChatMessage> {
        let msg = WireMessage {
            sender_id: self.my_id.clone(),
            sender_name: self.my_name.clone(),
            sender_department: self.my_department.clone(),
            sender_port: self.listen_port,
            receiver_id: peer.id.clone(),
            content: content.to_string(),
            msg_type: "text".to_string(),
            file_name: None,
            file_size: None,
            file_data: None,
            known_peers: self.build_known_peers(),
            group_id: None,
        };

        self.send_wire_message(peer, &msg).await?;

        // Mark as recent contact
        let _ = self.db.add_recent_contact(&peer.id).await;

        // Save outgoing message to DB
        let saved = self.db
            .save_message(
                &self.my_id,
                &self.my_name,
                &peer.id,
                content,
                "text",
                None,
                None,
                None,
            )
            .await?;

        Ok(saved)
    }

    /// Send a file to a peer (streams the file, emits progress events).
    pub async fn send_file(&self, peer: &Peer, file_path: &str, file_name: &str, app_handle: tauri::AppHandle) -> Result<crate::db::ChatMessage> {
        use tokio::fs::File;
        use tokio::io::AsyncReadExt;

        let metadata = tokio::fs::metadata(file_path)
            .await
            .with_context(|| format!("Failed to read metadata: {}", file_path))?;
        let file_size = metadata.len();

        let file_name = file_name.to_string();

        // 48KB raw → 64KB base64 → fits in ~66KB JSON line (safe for BufReader)
        const CHUNK_SIZE: usize = 48 * 1024;
        let total_chunks = ((file_size as usize + CHUNK_SIZE - 1) / CHUNK_SIZE) as u64;

        let mut file = File::open(file_path)
            .await
            .with_context(|| format!("Failed to open file: {}", file_path))?;

        let mut stream = TcpStream::connect(peer.address())
            .await
            .with_context(|| format!("Failed to connect to peer {}", peer.address()))?;

        let peers_list = self.build_known_peers();
        let mut buf = vec![0u8; CHUNK_SIZE];
        let mut i: u64 = 0;

        loop {
            let n = file.read(&mut buf).await
                .with_context(|| format!("Failed to read file chunk {}", i))?;
            if n == 0 {
                break;
            }
            let is_last = n < CHUNK_SIZE || (file_size as usize) <= ((i as usize + 1) * CHUNK_SIZE);

            let msg = WireMessage {
                sender_id: self.my_id.clone(),
                sender_name: self.my_name.clone(),
                sender_department: self.my_department.clone(),
                sender_port: self.listen_port,
                receiver_id: peer.id.clone(),
                content: base64_encode(&buf[..n]),
                msg_type: if is_last { "file_end".to_string() } else { "file_chunk".to_string() },
                file_name: Some(file_name.clone()),
                file_size: Some(file_size),
                file_data: None,
                // Only include known_peers in first chunk (relay info, not needed per chunk)
                known_peers: if i == 0 { peers_list.clone() } else { Vec::new() },
                group_id: None,
            };

            let json = serde_json::to_string(&msg).context("Failed to serialize message")?;
            stream.write_all(json.as_bytes()).await?;
            stream.write_all(b"\n").await?;
            i += 1;

            // Emit progress
            let sent = std::cmp::min((i as usize) * CHUNK_SIZE, file_size as usize) as u64;
            let _ = app_handle.emit("file-progress", serde_json::json!({
                "fileName": file_name,
                "sent": sent,
                "total": file_size,
            }));
        }
        stream.flush().await?;

        // Emit complete
        let _ = app_handle.emit("file-progress", serde_json::json!({
            "fileName": file_name,
            "sent": file_size,
            "total": file_size,
        }));

        info!("File send complete: {} ({} bytes, {} chunks)", file_name, file_size, i);

        self.bump_last_seen(peer);

        // Save outgoing file message to DB
        let saved = self.db
            .save_message(
                &self.my_id,
                &self.my_name,
                &peer.id,
                &format!("📎 {}", file_name),
                "file",
                Some(file_path),
                Some(&file_name),
                Some(file_size as i64),
            )
            .await?;

        Ok(saved)
    }

    async fn send_wire_message(&self, peer: &Peer, msg: &WireMessage) -> Result<()> {
        let json = serde_json::to_string(msg).context("Failed to serialize message")?;
        let mut stream = TcpStream::connect(peer.address())
            .await
            .with_context(|| format!("Failed to connect to peer {}", peer.address()))?;

        stream
            .write_all(json.as_bytes())
            .await
            .context("Failed to write message")?;
        stream.write_all(b"\n").await?;
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
                    ip: p.ip.to_string(),
                    port: p.port,
                })
                .collect()
        } else {
            Vec::new()
        }
    }

    fn bump_last_seen(&self, peer: &Peer) {
        if let Ok(mut map) = self.peers.write() {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;

            if let Some(existing) = map.values_mut().find(|p| p.ip == peer.ip && p.port == peer.port)
            {
                existing.online = true;
                existing.last_seen = now;
                if !peer.username.is_empty() && peer.username != "手动添加" {
                    existing.username = peer.username.clone();
                }
                if !peer.department.is_empty() {
                    existing.department = peer.department.clone();
                }
                info!("bump_last_seen: {} updated (online, last_seen={})", existing.id, now);
            } else {
                // Peer not in discovery map yet — insert it so UI picks it up
                let mut p = peer.clone();
                p.online = true;
                p.last_seen = now;
                map.insert(p.id.clone(), p.clone());
                info!("bump_last_seen: inserted new peer {} into map ({} total)", p.id, map.len());
            }
        }
    }
}

fn base64_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

fn base64_decode(input: &str) -> Result<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(input)
        .with_context(|| "Failed to decode base64")
}

/// Send a file in a background task (doesn't hold any long-lived locks).
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
) -> Result<crate::db::ChatMessage> {
    use tokio::fs::File;
    use tokio::io::AsyncReadExt;

    let metadata = tokio::fs::metadata(file_path).await
        .with_context(|| format!("Failed to read metadata: {}", file_path))?;
    let file_size = metadata.len();

    const CHUNK_SIZE: usize = 48 * 1024;

    let mut file = File::open(file_path).await
        .with_context(|| format!("Failed to open file: {}", file_path))?;

    let mut stream = TcpStream::connect(peer.address()).await
        .with_context(|| format!("Failed to connect to peer {}", peer.address()))?;

    // Build known_peers once
    let peers_list: Vec<PeerEntry> = if let Ok(map) = peers.read() {
        map.values().filter(|p| p.online).map(|p| PeerEntry {
            id: p.id.clone(), username: p.username.clone(),
            department: p.department.clone(), ip: p.ip.to_string(), port: p.port,
        }).collect()
    } else { Vec::new() };

    // Emit start event immediately so UI shows progress bar
    let _ = app_handle.emit("file-progress", serde_json::json!({
        "fileName": file_name, "sent": 0, "total": file_size, "speed": 0,
    }));

    let mut buf = vec![0u8; CHUNK_SIZE];
    let mut i: u64 = 0;
    let start_time = std::time::Instant::now();

    loop {
        let n = file.read(&mut buf).await
            .with_context(|| format!("Failed to read file chunk {}", i))?;
        if n == 0 { break; }

        let is_last = n < CHUNK_SIZE;
        let msg = WireMessage {
            sender_id: my_id.clone(), sender_name: my_name.clone(),
            sender_department: my_department.clone(), sender_port: listen_port,
            receiver_id: peer.id.clone(),
            content: base64_encode(&buf[..n]),
            msg_type: if is_last { "file_end".to_string() } else { "file_chunk".to_string() },
            file_name: Some(file_name.to_string()), file_size: Some(file_size),
            file_data: None,
            known_peers: if i == 0 { peers_list.clone() } else { Vec::new() },
            group_id: None,
        };

        let json = serde_json::to_string(&msg).context("Failed to serialize message")?;
        stream.write_all(json.as_bytes()).await?;
        stream.write_all(b"\n").await?;
        i += 1;

        let sent = std::cmp::min((i as usize) * CHUNK_SIZE, file_size as usize) as u64;
        let elapsed = start_time.elapsed().as_secs_f64().max(0.1);
        let speed = (sent as f64 / elapsed) as u64; // bytes/sec
        let _ = app_handle.emit("file-progress", serde_json::json!({
            "fileName": file_name, "sent": sent, "total": file_size, "speed": speed,
        }));
    }
    stream.flush().await?;

    let elapsed = start_time.elapsed().as_secs_f64().max(0.1);
    let speed = (file_size as f64 / elapsed) as u64;
    let _ = app_handle.emit("file-progress", serde_json::json!({
        "fileName": file_name, "sent": file_size, "total": file_size, "speed": speed,
    }));

    info!("File send complete: {} ({} bytes, {} chunks)", file_name, file_size, i);

    // Mark as recent contact
    let _ = db.add_recent_contact(&peer.id).await;

    // Save to DB
    let saved = db.save_message(&my_id, &my_name, &peer.id,
        &format!("📎 {}", file_name), "file",
        Some(file_path), Some(file_name), Some(file_size as i64),
    ).await?;

    // Update peer last_seen
    {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
        if let Ok(mut map) = peers.write() {
            if let Some(existing) = map.values_mut().find(|p| p.ip == peer.ip && p.port == peer.port) {
                existing.online = true;
                existing.last_seen = now;
            }
        }
    }

    Ok(saved)
}

fn save_received_file(data: &[u8], filename: &str) -> Result<String> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| "/tmp".to_string());
    let files_dir = std::path::PathBuf::from(home).join("Echo").join("files");
    std::fs::create_dir_all(&files_dir)?;

    let timestamp = chrono::Utc::now().timestamp_millis();
    let file_path = files_dir.join(format!("{}_{}", timestamp, filename));

    std::fs::write(&file_path, data)?;
    Ok(file_path.to_string_lossy().to_string())
}