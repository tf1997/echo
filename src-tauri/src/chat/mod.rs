use anyhow::{Context, Result};
use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

use crate::contact_sync;
use crate::db::Database;
use crate::discovery::{Peer, PeerEntry};
use tauri::Manager;

/// Wire protocol message sent between peers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireMessage {
    pub sender_id: String,
    pub sender_name: String,
    pub sender_department: String,
    pub sender_port: u16,
    pub receiver_id: String,
    pub content: String,
    pub msg_type: String,   // "text", "file", "sticker", "file_chunk", "file_end"
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
        let my_name = self.my_name.clone();
        let my_department = self.my_department.clone();
        let my_port = self.listen_port;

        tauri::async_runtime::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, peer_addr)) => {
                        info!("Incoming connection from {}", peer_addr);
                        let db = Arc::clone(&db);
                        let tx = incoming_tx.clone();
                        let peers = Arc::clone(&peers);
                        let my_id = my_id.clone();
                        let my_name = my_name.clone();
                        let my_department = my_department.clone();
                        tauri::async_runtime::spawn(async move {
                            if let Err(e) = Self::handle_incoming(stream, peer_addr, db, tx, peers, my_id, my_name, my_department, my_port).await {
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
        my_name: String,
        my_department: String,
        my_port: u16,
    ) -> Result<()> {
        let (reader, _writer) = stream.into_split();
        let mut lines = BufReader::new(reader).lines();

        let mut file_buffer: Vec<u8> = Vec::new();
        let mut file_sender_id: Option<String> = None;
        let mut file_sender_name: Option<String> = None;
        let mut file_group_id: Option<String> = None;
        let mut file_kind: String = "file".to_string();

        while let Some(line) = lines.next_line().await? {
            match serde_json::from_str::<WireMessage>(&line) {
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
                            my_port,
                            my_ip,
                            &msg.sender_id,
                            msg.sender_port,
                            &peer_addr.ip().to_string(),
                            &msg.content,
                        ).await;
                        continue;
                    }

                    if msg_type == "contact_sync_res" {
                        contact_sync::handle_contact_sync_res(
                            &db,
                            &peers,
                            &my_id,
                            &msg.content,
                        ).await;
                        continue;
                    }

                    // Mark sender as recent contact
                    let _ = db.add_recent_contact(&msg.sender_id).await;

                    for entry in &msg.known_peers {
                        if entry.id == my_id || entry.ip.is_empty() || entry.port == 0 { continue; }
                        if entry.ip.parse::<std::net::IpAddr>().is_err() { continue; }
                        let _ = db.upsert_peer(
                            &entry.id, &entry.username, &entry.department,
                            &entry.ip, entry.port, false,
                        ).await;
                    }

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
                            // Idempotent — INSERT OR IGNORE under the hood; safe to call repeatedly
                            let _ = db.create_group(gid, &group_name, &msg.sender_id, &all_members).await;
                            // Always add members (covers re-broadcast after invite_to_group)
                            let _ = db.add_group_members(gid, &all_members).await;
                            info!("Synced group {} ({} members)", group_name, all_members.len());
                        } else if msg.msg_type == "group_renamed" {
                            // file_name carries the actual new name; content is the display message.
                            // For offline-queued messages, fall back to parsing content.
                            let new_name = msg.file_name.as_deref().unwrap_or_else(|| {
                                msg.content.trim_start_matches("群名已修改为「").trim_end_matches('」')
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
                        info!("Peer {} profile updated → {} ({})",
                            msg.sender_id, msg.sender_name, msg.sender_department);
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

                        // Persist known_peers to DB (out of the sync RwLock so we can await).
                        // Skips entries with missing ip/port — those carry id-only and we have nothing to store.
                        for entry in &msg.known_peers {
                            if entry.id == my_id || entry.ip.is_empty() || entry.port == 0 { continue; }
                            if entry.ip.parse::<std::net::IpAddr>().is_err() { continue; }
                            let _ = db.upsert_peer(
                                &entry.id, &entry.username, &entry.department,
                                &entry.ip, entry.port, false,
                            ).await;
                        }
                    }

                    match msg_type {
                        "file_chunk" => {
                            if file_buffer.is_empty() {
                                file_sender_id = Some(msg.sender_id.clone());
                                file_sender_name = Some(msg.sender_name.clone());
                                file_group_id = msg.group_id.clone();
                                file_kind = msg.file_kind.as_deref().unwrap_or("file").to_string();
                            }
                            let decoded = base64_decode(&msg.content)
                                .unwrap_or_default();
                            file_buffer.extend_from_slice(&decoded);
                        }
                        "file_end" => {
                            if file_sender_id.is_none() {
                                file_sender_id = Some(msg.sender_id.clone());
                                file_sender_name = Some(msg.sender_name.clone());
                                file_group_id = msg.group_id.clone();
                            }
                            file_kind = msg.file_kind.as_deref().unwrap_or(&file_kind).to_string();
                            let decoded = base64_decode(&msg.content)
                                .unwrap_or_default();
                            file_buffer.extend_from_slice(&decoded);

                            let file_name_display = msg.file_name.as_deref().unwrap_or("unknown");
                            let saved_path = save_received_file(&file_buffer, file_name_display)?;

                            let sender_id = file_sender_id.as_deref().unwrap_or(&msg.sender_id);
                            let sender_name = file_sender_name.as_deref().unwrap_or(&msg.sender_name);
                            let msg_kind = if file_kind == "sticker" { "sticker" } else { "file" };
                            let display_content = if msg_kind == "sticker" {
                                "[表情]".to_string()
                            } else {
                                format!("📎 {}", file_name_display)
                            };
                            if let Some(ref gid) = file_group_id {
                                // Group file message — is_read=false so unread fires
                                if let Err(e) = db.save_group_message(
                                    gid, sender_id, sender_name, &display_content, msg_kind,
                                    Some(&saved_path), Some(file_name_display),
                                    msg.file_size.map(|s| s as i64), false,
                                ).await {
                                    error!("Failed to save incoming group file message: {}", e);
                                }
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
                                    )
                                    .await
                                {
                                    error!("Failed to save incoming file message: {}", e);
                                }
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

                            file_buffer.clear();
                            file_sender_id = None;
                            file_sender_name = None;
                            file_group_id = None;
                            file_kind = "file".to_string();
                        }
                        "group_created" | "group_dissolved" | "group_member_left" | "profile_updated" => {
                            // System notifications — already handled above, don't save as message
                        }
                        _ => {
                            // Text or other message types
                            info!("Received message from {}: {}", msg.sender_name, msg.content);

                            if let Some(ref gid) = msg.group_id {
                                // Group message — save with group_id; is_read=false so unread counts work
                                let _ = db.save_group_message(gid, &msg.sender_id, &msg.sender_name, &msg.content, &msg.msg_type, None, None, None, false).await;
                                // Auto-join if needed (only when group truly missing — typical when we missed group_created)
                                let my_groups = db.list_groups(&my_id).await.unwrap_or_default();
                                if !my_groups.iter().any(|g| g.group_id == *gid) {
                                    let _ = db.create_group(gid, "(未命名群组)", &msg.sender_id, &[msg.sender_id.clone(), my_id.clone()]).await;
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
        self.send_message_typed(peer, content, "text").await
    }

    pub async fn send_message_typed(&self, peer: &Peer, content: &str, msg_type: &str) -> Result<crate::db::ChatMessage> {
        let msg = WireMessage {
            sender_id: self.my_id.clone(),
            sender_name: self.my_name.clone(),
            sender_department: self.my_department.clone(),
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

        self.send_wire_message(peer, &msg).await?;
        let _ = self.db.add_recent_contact(&peer.id).await;
        let saved = self.db
            .save_message(&self.my_id, &self.my_name, &peer.id, content, msg_type, None, None, None)
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
                file_kind: Some("file".to_string()),
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
            let _ = app_handle.emit_all("file-progress", serde_json::json!({
                "fileName": file_name,
                "sent": sent,
                "total": file_size,
            }));
        }
        stream.flush().await?;

        // Emit complete
        let _ = app_handle.emit_all("file-progress", serde_json::json!({
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
            self.listen_port,
            my_ip,
            target_ip,
            target_port,
            target_id,
        ).await;
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
    send_file_in_background_with_kind(
        file_path, file_name, peer, my_id, my_name, my_department,
        listen_port, db, peers, app_handle, "file",
    ).await
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
    file_kind: &str,
) -> Result<crate::db::ChatMessage> {
    send_file_in_background_inner(
        file_path, file_name, peer, my_id, my_name, my_department,
        listen_port, db, peers, app_handle, None, file_kind,
    ).await
}

/// Like `send_file_in_background` but tags each chunk with a group_id and skips per-peer DB save
/// (the caller persists a single outgoing message before the fanout).
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
) -> Result<()> {
    send_file_in_background_inner(
        file_path, file_name, peer, my_id, my_name, my_department,
        listen_port, db, peers, app_handle, group_id, "file",
    ).await.map(|_| ())
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
    file_kind: &str,
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
    let _ = app_handle.emit_all("file-progress", serde_json::json!({
        "fileName": file_name, "sent": 0, "total": file_size, "speed": 0,
    }));

    let mut buf = vec![0u8; CHUNK_SIZE];
    let mut i: u64 = 0;
    let start_time = std::time::Instant::now();

    loop {
        let n = file.read(&mut buf).await
            .with_context(|| format!("Failed to read file chunk {}", i))?;
        if n == 0 { break; }

        let is_last = n < CHUNK_SIZE || (file_size as usize) <= ((i as usize + 1) * CHUNK_SIZE);
        let msg = WireMessage {
            sender_id: my_id.clone(), sender_name: my_name.clone(),
            sender_department: my_department.clone(), sender_port: listen_port,
            receiver_id: peer.id.clone(),
            content: base64_encode(&buf[..n]),
            msg_type: if is_last { "file_end".to_string() } else { "file_chunk".to_string() },
            file_name: Some(file_name.to_string()), file_size: Some(file_size),
            file_data: None,
            file_kind: Some(file_kind.to_string()),
            known_peers: if i == 0 { peers_list.clone() } else { Vec::new() },
            group_id: group_id.clone(),
        };

        let json = serde_json::to_string(&msg).context("Failed to serialize message")?;
        stream.write_all(json.as_bytes()).await?;
        stream.write_all(b"\n").await?;
        i += 1;

        let sent = std::cmp::min((i as usize) * CHUNK_SIZE, file_size as usize) as u64;
        let elapsed = start_time.elapsed().as_secs_f64().max(0.1);
        let speed = (sent as f64 / elapsed) as u64; // bytes/sec
        let _ = app_handle.emit_all("file-progress", serde_json::json!({
            "fileName": file_name, "sent": sent, "total": file_size, "speed": speed,
        }));
    }
    stream.flush().await?;

    let elapsed = start_time.elapsed().as_secs_f64().max(0.1);
    let speed = (file_size as f64 / elapsed) as u64;
    let _ = app_handle.emit_all("file-progress", serde_json::json!({
        "fileName": file_name, "sent": file_size, "total": file_size, "speed": speed,
    }));

    info!("File send complete: {} ({} bytes, {} chunks)", file_name, file_size, i);

    // For 1:1 chat we save the outgoing message + bump recent contacts here.
    // For group chats the caller already persisted the outgoing message before fanout,
    // so we just return a synthetic ChatMessage placeholder.
    if group_id.is_none() {
        // Mark as recent contact
        let _ = db.add_recent_contact(&peer.id).await;

        let msg_kind = if file_kind == "sticker" { "sticker" } else { "file" };
        let content = if msg_kind == "sticker" {
            "[表情]".to_string()
        } else {
            format!("📎 {}", file_name)
        };
        let saved = db.save_message(&my_id, &my_name, &peer.id,
            &content, msg_kind,
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
    } else {
        // Update last_seen for group case too
        {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
            if let Ok(mut map) = peers.write() {
                if let Some(existing) = map.values_mut().find(|p| p.ip == peer.ip && p.port == peer.port) {
                    existing.online = true;
                    existing.last_seen = now;
                }
            }
        }
        Ok(crate::db::ChatMessage {
            id: 0,
            sender_id: my_id, sender_name: my_name, receiver_id: peer.id.clone(),
            content: format!("📎 {}", file_name), msg_type: "file".to_string(),
            file_path: Some(file_path.to_string()), file_name: Some(file_name.to_string()),
            file_size: Some(file_size as i64),
            timestamp: chrono::Utc::now().to_rfc3339(),
            is_read: true,
        })
    }
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
