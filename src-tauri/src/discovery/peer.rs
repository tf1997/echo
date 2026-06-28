use serde::{Deserialize, Serialize};
use std::fmt;
use std::net::IpAddr;

/// Lightweight peer info for peer relay lists.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerEntry {
    pub id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub node_id: String,
    pub username: String,
    pub department: String,
    #[serde(default)]
    pub software_version: String,
    #[serde(default)]
    pub mac_address: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub avatar_hash: String,
    #[serde(default, skip_serializing_if = "is_zero_i64")]
    pub avatar_updated_at: i64,
    pub ip: String,
    pub port: u16,
}

/// Represents a discovered peer on the local network.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Peer {
    pub id: String,
    #[serde(default)]
    pub node_id: String,
    pub username: String,
    pub department: String,
    #[serde(default)]
    pub software_version: String,
    #[serde(default)]
    pub mac_address: String,
    #[serde(default)]
    pub avatar_path: String,
    #[serde(default)]
    pub avatar_hash: String,
    #[serde(default)]
    pub avatar_updated_at: i64,
    pub ip: IpAddr,
    pub port: u16,
    pub online: bool,
    pub last_seen: i64,
}

impl Peer {
    pub fn new(id: String, username: String, department: String, ip: IpAddr, port: u16) -> Self {
        Self::new_with_profile(
            id,
            username,
            department,
            String::new(),
            String::new(),
            ip,
            port,
        )
    }

    pub fn new_with_profile(
        id: String,
        username: String,
        department: String,
        software_version: String,
        mac_address: String,
        ip: IpAddr,
        port: u16,
    ) -> Self {
        Self::new_with_avatar(
            id,
            username,
            department,
            software_version,
            mac_address,
            String::new(),
            String::new(),
            0,
            ip,
            port,
        )
    }

    pub fn new_with_avatar(
        id: String,
        username: String,
        department: String,
        software_version: String,
        mac_address: String,
        avatar_path: String,
        avatar_hash: String,
        avatar_updated_at: i64,
        ip: IpAddr,
        port: u16,
    ) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        Self {
            id,
            node_id: String::new(),
            username,
            department,
            software_version,
            mac_address,
            avatar_path,
            avatar_hash,
            avatar_updated_at,
            ip,
            port,
            online: true,
            last_seen: now,
        }
    }

    // Compatibility constructor for callers that do not yet provide profile metadata.
    #[allow(dead_code)]
    pub fn with_online(
        id: String,
        username: String,
        department: String,
        ip: IpAddr,
        port: u16,
        online: bool,
        last_seen: i64,
    ) -> Self {
        Self::with_online_details(
            id,
            username,
            department,
            String::new(),
            String::new(),
            ip,
            port,
            online,
            last_seen,
        )
    }

    pub fn with_online_details(
        id: String,
        username: String,
        department: String,
        software_version: String,
        mac_address: String,
        ip: IpAddr,
        port: u16,
        online: bool,
        last_seen: i64,
    ) -> Self {
        Self::with_online_avatar(
            id,
            username,
            department,
            software_version,
            mac_address,
            String::new(),
            String::new(),
            0,
            ip,
            port,
            online,
            last_seen,
        )
    }

    pub fn with_online_avatar(
        id: String,
        username: String,
        department: String,
        software_version: String,
        mac_address: String,
        avatar_path: String,
        avatar_hash: String,
        avatar_updated_at: i64,
        ip: IpAddr,
        port: u16,
        online: bool,
        last_seen: i64,
    ) -> Self {
        Self {
            id,
            node_id: String::new(),
            username,
            department,
            software_version,
            mac_address,
            avatar_path,
            avatar_hash,
            avatar_updated_at,
            ip,
            port,
            online,
            last_seen,
        }
    }

    pub fn address(&self) -> String {
        format!("{}:{}", self.ip, self.port)
    }
}

fn is_zero_i64(value: &i64) -> bool {
    *value == 0
}

impl fmt::Display for Peer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Peer({} [{}] @ {}:{}, online={})",
            self.username, self.id, self.ip, self.port, self.online
        )
    }
}

impl PartialEq for Peer {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for Peer {}

impl std::hash::Hash for Peer {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}
