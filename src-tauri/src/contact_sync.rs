use std::collections::{HashMap, HashSet};

use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

use crate::chat::WireMessage;
use crate::contact_filter;
use crate::db::Database;
use crate::discovery::Peer;

// ── Data structures ──────────────────────────────────────────────────

/// Lightweight peer summary for contact sync exchange.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContactSummaryEntry {
    pub peer_id: String,
    #[serde(default)]
    pub node_id: String,
    pub username: String,
    pub department: String,
    #[serde(default)]
    pub software_version: String,
    #[serde(default)]
    pub mac_address: String,
    #[serde(default)]
    pub avatar_hash: String,
    #[serde(default)]
    pub avatar_updated_at: i64,
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
    my_software_version: &str,
    my_mac_address: &str,
    my_port: u16,
    my_ip: &str,
) -> Vec<ContactSummaryEntry> {
    let stored = db.list_stored_peers().await.unwrap_or_default();
    let my_profile = db.get_user_profile().await.ok().flatten();
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();

    // Self first
    seen.insert(my_id.to_string());
    out.push(ContactSummaryEntry {
        peer_id: my_id.to_string(),
        node_id: my_profile
            .as_ref()
            .map(|profile| profile.node_id.clone())
            .unwrap_or_default(),
        username: my_name.to_string(),
        department: my_department.to_string(),
        software_version: my_software_version.to_string(),
        mac_address: my_mac_address.to_string(),
        avatar_hash: my_profile
            .as_ref()
            .map(|profile| profile.avatar_hash.clone())
            .unwrap_or_default(),
        avatar_updated_at: my_profile
            .as_ref()
            .map(|profile| profile.avatar_updated_at)
            .unwrap_or_default(),
        ip: my_ip.to_string(),
        port: my_port,
        version: 0,
    });

    // From DB
    for sp in &stored {
        if sp.peer_id != my_id
            && contact_filter::is_syncable_contact(
                &sp.peer_id,
                &sp.username,
                &sp.department,
                &sp.ip,
                sp.port,
            )
            && seen.insert(sp.peer_id.clone())
        {
            out.push(ContactSummaryEntry {
                peer_id: sp.peer_id.clone(),
                node_id: sp.node_id.clone(),
                username: sp.username.clone(),
                department: sp.department.clone(),
                software_version: sp.software_version.clone(),
                mac_address: sp.mac_address.clone(),
                avatar_hash: sp.avatar_hash.clone(),
                avatar_updated_at: sp.avatar_updated_at,
                ip: sp.ip.clone(),
                port: sp.port,
                version: sp.last_seen_at.parse::<i64>().unwrap_or(0),
            });
        }
    }

    // From memory (peers not yet persisted)
    if let Ok(map) = peers_map.read() {
        for (id, p) in map.iter() {
            let ip = p.ip.to_string();
            if id != my_id
                && contact_filter::is_syncable_contact(
                    &p.id,
                    &p.username,
                    &p.department,
                    &ip,
                    p.port,
                )
                && seen.insert(id.clone())
            {
                out.push(ContactSummaryEntry {
                    peer_id: p.id.clone(),
                    node_id: p.node_id.clone(),
                    username: p.username.clone(),
                    department: p.department.clone(),
                    software_version: p.software_version.clone(),
                    mac_address: p.mac_address.clone(),
                    avatar_hash: p.avatar_hash.clone(),
                    avatar_updated_at: p.avatar_updated_at,
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

fn is_valid_remote_entry(entry: &ContactSummaryEntry, my_id: &str) -> bool {
    entry.peer_id != my_id
        && contact_filter::is_syncable_contact(
            &entry.peer_id,
            &entry.username,
            &entry.department,
            &entry.ip,
            entry.port,
        )
}

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
    if !is_valid_remote_entry(entry, my_id) {
        return;
    }
    if let Ok(ip) = entry.ip.parse::<std::net::IpAddr>() {
        if let Ok(mut map) = peers_map.write() {
            if let Some(existing) = map.get_mut(&entry.peer_id) {
                if !entry.username.is_empty() {
                    existing.username = entry.username.clone();
                }
                if !entry.department.is_empty() {
                    existing.department = entry.department.clone();
                }
                if !entry.software_version.is_empty() {
                    existing.software_version = entry.software_version.clone();
                }
                if !entry.mac_address.is_empty() {
                    existing.mac_address = entry.mac_address.clone();
                }
                if entry.avatar_updated_at > existing.avatar_updated_at {
                    existing.avatar_path.clear();
                    existing.avatar_hash = entry.avatar_hash.clone();
                    existing.avatar_updated_at = entry.avatar_updated_at;
                } else if existing.avatar_hash.is_empty() && !entry.avatar_hash.is_empty() {
                    existing.avatar_hash = entry.avatar_hash.clone();
                    existing.avatar_updated_at = entry.avatar_updated_at;
                }
                existing.ip = ip;
                existing.port = entry.port;
                if entry.version > 0 {
                    existing.last_seen = entry.version;
                }
            } else {
                map.insert(entry.peer_id.clone(), {
                    let mut peer = Peer::with_online_avatar(
                        entry.peer_id.clone(),
                        entry.username.clone(),
                        entry.department.clone(),
                        entry.software_version.clone(),
                        entry.mac_address.clone(),
                        String::new(),
                        entry.avatar_hash.clone(),
                        entry.avatar_updated_at,
                        ip,
                        entry.port,
                        false,
                        entry.version,
                    );
                    peer.node_id.clear();
                    peer
                });
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
    my_node_id: &str,
    my_name: &str,
    my_department: &str,
    my_software_version: &str,
    my_mac_address: &str,
    my_port: u16,
    my_ip: &str,
    sender_id: &str,
    sender_node_id: &str,
    sender_port: u16,
    sender_ip: &str,
    content: &str,
) -> bool {
    let entries: Vec<ContactSummaryEntry> = match serde_json::from_str(content) {
        Ok(v) => v,
        Err(e) => {
            warn!(
                "contact_sync: bad contact_summary from {}: {}",
                sender_id, e
            );
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
        if !is_valid_remote_entry(entry, my_id) {
            continue;
        }
        // Persist
        let _ = db
            .upsert_peer_with_node_id_avatar(
                &entry.peer_id,
                "",
                &entry.username,
                &entry.department,
                &entry.software_version,
                &entry.mac_address,
                "",
                &entry.avatar_hash,
                entry.avatar_updated_at,
                &entry.ip,
                entry.port,
                false,
            )
            .await;
        let _ = db.add_recent_contact(&entry.peer_id).await;
        merge_into_memory(entry, my_id, peers_map);
        if !our_ids.contains(&entry.peer_id) {
            new_count += 1;
        }
    }
    if new_count > 0 {
        info!("contact_sync: added {} new peer(s) from summary", new_count);
    }

    // 2. Build our summary
    let our_summaries = build_summaries(
        db,
        peers_map,
        my_id,
        my_name,
        my_department,
        my_software_version,
        my_mac_address,
        my_port,
        my_ip,
    )
    .await;

    // 3. Compute what they're missing
    let our_set: HashSet<&str> = our_summaries.iter().map(|s| s.peer_id.as_str()).collect();
    let their_set: HashSet<&str> = entries
        .iter()
        .filter(|s| {
            !s.peer_id.trim().is_empty()
                && contact_filter::has_contact_identity(&s.username, &s.department)
        })
        .map(|s| s.peer_id.as_str())
        .collect();
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
    let response_content = serde_json::to_string(&response).unwrap_or_else(|_| "{}".to_string());

    let response_msg = WireMessage {
        sender_id: my_id.to_string(),
        sender_node_id: my_node_id.to_string(),
        sender_name: my_name.to_string(),
        sender_department: my_department.to_string(),
        sender_software_version: my_software_version.to_string(),
        sender_mac_address: my_mac_address.to_string(),
        sender_port: my_port,
        receiver_id: sender_id.to_string(),
        receiver_node_id: sender_node_id.to_string(),
        content: response_content,
        msg_type: "contact_sync_res".to_string(),
        file_name: None,
        file_size: None,
        file_data: None,
        file_kind: None,
        known_peers: Vec::new(),
        group_id: None,
        client_msg_id: None,
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
        if !is_valid_remote_entry(entry, my_id) {
            continue;
        }
        let _ = db.add_recent_contact(&entry.peer_id).await;
        let _ = db
            .upsert_peer_with_node_id_avatar(
                &entry.peer_id,
                "",
                &entry.username,
                &entry.department,
                &entry.software_version,
                &entry.mac_address,
                "",
                &entry.avatar_hash,
                entry.avatar_updated_at,
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
        if !is_valid_remote_entry(entry, my_id) {
            continue;
        }
        let _ = db.add_recent_contact(&entry.peer_id).await;
        let _ = db
            .upsert_peer_with_node_id_avatar(
                &entry.peer_id,
                "",
                &entry.username,
                &entry.department,
                &entry.software_version,
                &entry.mac_address,
                "",
                &entry.avatar_hash,
                entry.avatar_updated_at,
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
    my_node_id: &str,
    my_name: &str,
    my_department: &str,
    my_software_version: &str,
    my_mac_address: &str,
    my_port: u16,
    my_ip: &str,
    target_ip: &str,
    target_port: u16,
    target_id: &str,
    target_node_id: &str,
) {
    let summaries = build_summaries(
        db,
        peers_map,
        my_id,
        my_name,
        my_department,
        my_software_version,
        my_mac_address,
        my_port,
        my_ip,
    )
    .await;
    let content = serde_json::to_string(&summaries).unwrap_or_else(|_| "[]".to_string());

    let msg = WireMessage {
        sender_id: my_id.to_string(),
        sender_node_id: my_node_id.to_string(),
        sender_name: my_name.to_string(),
        sender_department: my_department.to_string(),
        sender_software_version: my_software_version.to_string(),
        sender_mac_address: my_mac_address.to_string(),
        sender_port: my_port,
        receiver_id: target_id.to_string(),
        receiver_node_id: target_node_id.to_string(),
        content,
        msg_type: "contact_summary".to_string(),
        file_name: None,
        file_size: None,
        file_data: None,
        file_kind: None,
        known_peers: Vec::new(),
        group_id: None,
        client_msg_id: None,
    };

    let addr = format!("{}:{}", target_ip, target_port);
    info!(
        "contact_sync: exchanging summaries with {} @ {}",
        target_id, addr
    );
    deliver_response(&addr, &msg).await;
}
