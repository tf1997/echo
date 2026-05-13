use serde::{Deserialize, Serialize};
use std::fmt;
use std::net::IpAddr;

/// Represents a discovered peer on the local network.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Peer {
    pub id: String,
    pub username: String,
    pub department: String,
    pub ip: IpAddr,
    pub port: u16,
    pub online: bool,
    pub last_seen: i64,
}

impl Peer {
    pub fn new(id: String, username: String, department: String, ip: IpAddr, port: u16) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        Self {
            id,
            username,
            department,
            ip,
            port,
            online: true,
            last_seen: now,
        }
    }

    pub fn with_online(id: String, username: String, department: String, ip: IpAddr, port: u16, online: bool, last_seen: i64) -> Self {
        Self { id, username, department, ip, port, online, last_seen }
    }

    pub fn address(&self) -> String {
        format!("{}:{}", self.ip, self.port)
    }
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