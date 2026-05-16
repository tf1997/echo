use anyhow::{Context, Result};
use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

use crate::db::Database;
use crate::discovery::{Peer, PeerEntry};

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
                        _ => {
                            // Text or other message types
                            info!("Received message from {}: {}", msg.sender_name, msg.content);

                            if let Err(e) = db
                                .save_message(
                                    &msg.sender_id,
                                    &msg.sender_name,
                                    &my_id,
                                    &msg.content,
                                    &msg.msg_type,
                                    msg.file_name.as_deref(),
                                    None,
                                    msg.file_size.map(|s| s as i64),
                                )
                                .await
                            {
                                error!("Failed to save incoming message: {}", e);
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
        };

        self.send_wire_message(peer, &msg).await?;

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

    /// Send a file to a peer (reads file and sends as base64 in JSON over single connection).
    pub async fn send_file(&self, peer: &Peer, file_path: &str) -> Result<crate::db::ChatMessage> {
        use tokio::fs;

        let data = fs::read(file_path)
            .await
            .with_context(|| format!("Failed to read file: {}", file_path))?;

        let file_name = std::path::Path::new(file_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        let file_size = data.len() as u64;

        const CHUNK_SIZE: usize = 64 * 1024; // 64KB chunks
        let total_chunks = (data.len() + CHUNK_SIZE - 1) / CHUNK_SIZE;

        // Open a single TCP connection for all chunks
        let mut stream = TcpStream::connect(peer.address())
            .await
            .with_context(|| format!("Failed to connect to peer {}", peer.address()))?;

        for i in 0..total_chunks {
            let start = i * CHUNK_SIZE;
            let end = std::cmp::min(start + CHUNK_SIZE, data.len());
            let chunk = &data[start..end];

            let msg = WireMessage {
                sender_id: self.my_id.clone(),
                sender_name: self.my_name.clone(),
                sender_department: self.my_department.clone(),
                sender_port: self.listen_port,
                receiver_id: peer.id.clone(),
                content: base64_encode(chunk),
                msg_type: if i == total_chunks - 1 {
                    "file_end".to_string()
                } else {
                    "file_chunk".to_string()
                },
                file_name: Some(file_name.clone()),
                file_size: Some(file_size),
                file_data: None,
                known_peers: self.build_known_peers(),
            };

            let json = serde_json::to_string(&msg).context("Failed to serialize message")?;
            stream.write_all(json.as_bytes()).await?;
            stream.write_all(b"\n").await?;
        }
        stream.flush().await?;

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