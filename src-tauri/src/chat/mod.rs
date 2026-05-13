use anyhow::{Context, Result};
use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

use crate::db::Database;
use crate::discovery::Peer;

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
}

impl ChatServer {
    pub fn new(
        listen_port: u16,
        my_id: String,
        my_name: String,
        my_department: String,
        db: Arc<Database>,
    ) -> Self {
        let (incoming_tx, _incoming_rx) = mpsc::unbounded_channel();
        Self {
            listen_port,
            my_id,
            my_name,
            my_department,
            db,
            incoming_tx,
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

        tauri::async_runtime::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, peer_addr)) => {
                        info!("Incoming connection from {}", peer_addr);
                        let db = Arc::clone(&db);
                        let tx = incoming_tx.clone();
                        tauri::async_runtime::spawn(async move {
                            if let Err(e) = Self::handle_incoming(stream, peer_addr, db, tx).await {
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
    ) -> Result<()> {
        let (reader, _writer) = stream.into_split();
        let mut lines = BufReader::new(reader).lines();

        while let Some(line) = lines.next_line().await? {
            match serde_json::from_str::<WireMessage>(&line) {
                Ok(msg) => {
                    info!("Received message from {}: {}", msg.sender_name, msg.content);

                    // Save to database
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

                    if let Err(e) = db
                        .save_message(
                            &msg.sender_id,
                            &msg.sender_name,
                            &msg.receiver_id,
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

                    // Forward to frontend
                    let incoming = IncomingMessage {
                        sender_id: msg.sender_id,
                        sender_name: msg.sender_name,
                        content: msg.content,
                        msg_type: msg.msg_type,
                        file_name: msg.file_name,
                        file_size: msg.file_size,
                        timestamp: chrono::Utc::now().to_rfc3339(),
                    };
                    let _ = incoming_tx.send(incoming);
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

    /// Send a file to a peer (reads file and sends as base64 in JSON).
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

        // Split into chunks if file is large
        const CHUNK_SIZE: usize = 64 * 1024; // 64KB chunks
        let total_chunks = (data.len() + CHUNK_SIZE - 1) / CHUNK_SIZE;

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
            };

            self.send_wire_message(peer, &msg).await?;
        }

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

        Ok(())
    }
}

/// Simple base64 encoding using standard library-style implementation.
fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::new();
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;

        result.push(CHARS[((triple >> 18) & 0x3F) as usize] as char);
        result.push(CHARS[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            result.push(CHARS[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
        if chunk.len() > 2 {
            result.push(CHARS[(triple & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
    }
    result
}