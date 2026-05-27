use std::collections::{HashMap, HashSet};

use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

use crate::chat::WireMessage;
use crate::db::Database;
use crate::discovery::Peer;

// ── Data structures ──────────────────────────────────────────────────

/// Lightweight peer summary for contact sync exchange.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContactSummaryEntry {
    pub peer_id: String,
    pub username: String,
    pub department: String,
    pub ip: String,
    pub port: u16,
    /// `last_seen_at` unix timestamp — used as version for delta comparison.
    pub version: i64,
}

/// Response payload: our full summary list plus full details for peers
/// the requester is missing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContactSyncResponse {
    pub summaries: Vec<ContactSummaryEntry>,
    pub missing_details: Vec<ContactSummaryEntry>,
}

// ── Builders ─────────────────────────────────────────────────────────

/// Build a merged contact summary list from DB + in-memory peers + self.
pub async fn build_summaries(
    db: &Database,
    peers_map: &std::sync::RwLock<HashMap<String, Peer>>,
    my_id: &str,
    my_name: &str,
    my_department: &str,
    my_port: u16,
    my_ip: &str,
) -> Vec<ContactSummaryEntry> {
    let stored = db.list_stored_peers().await.unwrap_or_default();
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();

    // Self first
    seen.insert(my_id.to_string());
    out.push(ContactSummaryEntry {
        peer_id: my_id.to_string(),
        username: my_name.to_string(),
        department: my_department.to_string(),
        ip: my_ip.to_string(),
        port: my_port,
        version: 0,
    });

    // From DB
    for sp in &stored {
        if sp.peer_id != my_id && seen.insert(sp.peer_id.clone()) {
            out.push(ContactSummaryEntry {
                peer_id: sp.peer_id.clone(),
                username: sp.username.clone(),
                department: sp.department.clone(),
                ip: sp.ip.clone(),
                port: sp.port,
                version: sp.last_seen_at.parse::<i64>().unwrap_or(0),
            });
        }
    }

    // From memory (peers not yet persisted)
    if let Ok(map) = peers_map.read() {
        for (id, p) in map.iter() {
            if id != my_id && seen.insert(id.clone()) {
                out.push(ContactSummaryEntry {
                    peer_id: p.id.clone(),
                    username: p.username.clone(),
                    department: p.department.clone(),
                    ip: p.ip.to_string(),
                    port: p.port,
                    version: p.last_seen,
                });
            }
        }
    }
    out
}

// ── Helpers ──────────────────────────────────────────────────────────

/// Send a `WireMessage` JSON line over a one-shot TCP connection.
async fn deliver_response(addr: &str, msg: &WireMessage) -> bool {
    let json = match serde_json::to_string(msg) {
        Ok(j) => j,
        Err(e) => {
            error!("contact_sync: serialize response: {}", e);
            return false;
        }
    };
    match TcpStream::connect(addr).await {
        Ok(mut stream) => {
            if let Err(e) = stream.write_all(json.as_bytes()).await {
                error!("contact_sync: write to {}: {}", addr, e);
                return false;
            }
            if let Err(e) = stream.write_all(b"\n").await {
                error!("contact_sync: write newline: {}", e);
                return false;
            }
            let _ = stream.flush().await;
            true
        }
        Err(e) => {
            warn!("contact_sync: connect to {}: {}", addr, e);
            false
        }
    }
}

/// Merge a `ContactSummaryEntry` into the local peers map (memory only).
fn merge_into_memory(
    entry: &ContactSummaryEntry,
    my_id: &str,
    peers_map: &std::sync::RwLock<HashMap<String, Peer>>,
) {
    if entry.peer_id == my_id || entry.ip.is_empty() || entry.port == 0 {
        return;
    }
    if let Ok(ip) = entry.ip.parse::<std::net::IpAddr>() {
        if let Ok(mut map) = peers_map.write() {
            if !map.contains_key(&entry.peer_id) {
                map.insert(
                    entry.peer_id.clone(),
                    Peer::with_online(
                        entry.peer_id.clone(),
                        entry.username.clone(),
                        entry.department.clone(),
                        ip,
                        entry.port,
                        false,
                        entry.version,
                    ),
                );
            }
        }
    }
}

// ── Message handlers ─────────────────────────────────────────────────

/// Handle an incoming `contact_summary` message.
///
/// 1. Merge sender's summary into local DB + memory.
/// 2. Build our own summary, compute delta (what they're missing).
/// 3. Send `contact_sync_res` back to the sender.
///
/// Returns `true` if the message was recognized and processed.
pub async fn handle_contact_summary(
    db: &Database,
    peers_map: &std::sync::RwLock<HashMap<String, Peer>>,
    my_id: &str,
    my_name: &str,
    my_department: &str,
    my_port: u16,
    my_ip: &str,
    sender_id: &str,
    sender_port: u16,
    sender_ip: &str,
    content: &str,
) -> bool {
    let entries: Vec<ContactSummaryEntry> = match serde_json::from_str(content) {
        Ok(v) => v,
        Err(e) => {
            warn!("contact_sync: bad contact_summary from {}: {}", sender_id, e);
            return false;
        }
    };

    info!(
        "contact_sync: received summary from {} ({} entries)",
        sender_id,
        entries.len()
    );

    // 1. Merge sender's peers into our DB + memory
    let our_stored = db.list_stored_peers().await.unwrap_or_default();
    let our_ids: HashSet<String> = our_stored.iter().map(|p| p.peer_id.clone()).collect();
    let mut new_count = 0u32;

    for entry in &entries {
        if entry.peer_id == my_id || our_ids.contains(&entry.peer_id) {
            continue;
        }
        if entry.ip.is_empty() || entry.port == 0 {
            continue;
        }
        if entry.ip.parse::<std::net::IpAddr>().is_err() {
            continue;
        }
        // Persist
        let _ = db
            .upsert_peer(
                &entry.peer_id,
                &entry.username,
                &entry.department,
                &entry.ip,
                entry.port,
                false,
            )
            .await;
        let _ = db.add_recent_contact(&entry.peer_id).await;
        merge_into_memory(entry, my_id, peers_map);
        new_count += 1;
    }
    if new_count > 0 {
        info!("contact_sync: added {} new peer(s) from summary", new_count);
    }

    // 2. Build our summary
    let our_summaries = build_summaries(db, peers_map, my_id, my_name, my_department, my_port, my_ip).await;

    // 3. Compute what they're missing
    let our_set: HashSet<&str> = our_summaries.iter().map(|s| s.peer_id.as_str()).collect();
    let their_set: HashSet<&str> = entries.iter().map(|s| s.peer_id.as_str()).collect();
    let missing_ids: HashSet<&str> = our_set.difference(&their_set).copied().collect();

    let missing_details: Vec<ContactSummaryEntry> = our_summaries
        .iter()
        .filter(|s| missing_ids.contains(s.peer_id.as_str()))
        .cloned()
        .collect();

    // 4. Send response back
    let response = ContactSyncResponse {
        summaries: our_summaries,
        missing_details,
    };
    let response_content =
        serde_json::to_string(&response).unwrap_or_else(|_| "{}".to_string());

    let response_msg = WireMessage {
        sender_id: my_id.to_string(),
        sender_name: my_name.to_string(),
        sender_department: my_department.to_string(),
        sender_port: my_port,
        receiver_id: sender_id.to_string(),
        content: response_content,
        msg_type: "contact_sync_res".to_string(),
        file_name: None,
        file_size: None,
        file_data: None,
        file_kind: None,
        known_peers: Vec::new(),
        group_id: None,
    };

    let target_addr = format!("{}:{}", sender_ip, sender_port);
    deliver_response(&target_addr, &response_msg).await;

    true
}

/// Handle an incoming `contact_sync_res` message.
///
/// Merge the received summaries and missing-details into local DB + memory.
///
/// Returns `true` if the message was recognized and processed.
pub async fn handle_contact_sync_res(
    db: &Database,
    peers_map: &std::sync::RwLock<HashMap<String, Peer>>,
    my_id: &str,
    content: &str,
) -> bool {
    let response: ContactSyncResponse = match serde_json::from_str(content) {
        Ok(v) => v,
        Err(e) => {
            warn!("contact_sync: bad contact_sync_res: {}", e);
            return false;
        }
    };

    let mut added = 0u32;

    // Merge summaries
    for entry in &response.summaries {
        if entry.peer_id == my_id {
            continue;
        }
        if entry.ip.is_empty() || entry.port == 0 {
            continue;
        }
        if entry.ip.parse::<std::net::IpAddr>().is_err() {
            continue;
        }
        let _ = db.add_recent_contact(&entry.peer_id).await;
        let _ = db
            .upsert_peer(
                &entry.peer_id,
                &entry.username,
                &entry.department,
                &entry.ip,
                entry.port,
                false,
            )
            .await;
        merge_into_memory(entry, my_id, peers_map);
        added += 1;
    }

    // Merge missing_details
    for entry in &response.missing_details {
        if entry.peer_id == my_id {
            continue;
        }
        if entry.ip.is_empty() || entry.port == 0 {
            continue;
        }
        if entry.ip.parse::<std::net::IpAddr>().is_err() {
            continue;
        }
        let _ = db.add_recent_contact(&entry.peer_id).await;
        let _ = db
            .upsert_peer(
                &entry.peer_id,
                &entry.username,
                &entry.department,
                &entry.ip,
                entry.port,
                false,
            )
            .await;
        merge_into_memory(entry, my_id, peers_map);
        added += 1;
    }

    if added > 0 {
        info!("contact_sync: merged {} peer(s) from sync response", added);
    }
    true
}

// ── Outbound (initiator side) ────────────────────────────────────────

/// Initiate a contact-summary exchange with a peer.
///
/// Sends our full summary to `target_ip:target_port` via TCP.
/// The response arrives asynchronously through the normal TCP listener
/// (`handle_contact_sync_res`).
pub async fn exchange_with_peer(
    db: &Database,
    peers_map: &std::sync::RwLock<HashMap<String, Peer>>,
    my_id: &str,
    my_name: &str,
    my_department: &str,
    my_port: u16,
    my_ip: &str,
    target_ip: &str,
    target_port: u16,
    target_id: &str,
) {
    let summaries = build_summaries(db, peers_map, my_id, my_name, my_department, my_port, my_ip).await;
    let content = serde_json::to_string(&summaries).unwrap_or_else(|_| "[]".to_string());

    let msg = WireMessage {
        sender_id: my_id.to_string(),
        sender_name: my_name.to_string(),
        sender_department: my_department.to_string(),
        sender_port: my_port,
        receiver_id: target_id.to_string(),
        content,
        msg_type: "contact_summary".to_string(),
        file_name: None,
        file_size: None,
        file_data: None,
        file_kind: None,
        known_peers: Vec::new(),
        group_id: None,
    };

    let addr = format!("{}:{}", target_ip, target_port);
    info!(
        "contact_sync: exchanging summaries with {} @ {}",
        target_id, addr
    );
    deliver_response(&addr, &msg).await;
}
