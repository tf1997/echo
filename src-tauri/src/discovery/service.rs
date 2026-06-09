use anyhow::{Context, Result};
use local_ip_address::local_ip;
use log::{info, warn};
use mdns_sd::{ServiceDaemon, ServiceEvent};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use tokio::sync::mpsc;

use crate::contact_filter;

use super::broadcast::{LanDiscovery, LanDiscoveryConfig};
use super::peer::{Peer, PeerEntry};

const SERVICE_TYPE: &str = "_echo-p2p._tcp.local.";

#[derive(Debug, Clone)]
pub struct DiscoveryConfig {
    pub peer_id: String,
    pub username: String,
    pub department: String,
    pub software_version: String,
    pub mac_address: String,
    pub avatar_hash: String,
    pub avatar_updated_at: i64,
    pub listen_port: u16,
    pub scan_subnets: Vec<String>,
    /// Channel to forward UDP-relayed peers to async DB layer.
    pub relay_tx: Option<tokio::sync::mpsc::UnboundedSender<Vec<PeerEntry>>>,
}

impl DiscoveryConfig {
    pub fn new(
        peer_id: impl Into<String>,
        username: impl Into<String>,
        department: impl Into<String>,
        listen_port: u16,
        scan_subnets: Vec<String>,
        avatar_hash: impl Into<String>,
        avatar_updated_at: i64,
    ) -> Self {
        Self {
            peer_id: peer_id.into(),
            username: username.into(),
            department: department.into(),
            software_version: crate::profile_metadata::software_version(),
            mac_address: crate::profile_metadata::mac_address(),
            avatar_hash: avatar_hash.into(),
            avatar_updated_at,
            listen_port,
            scan_subnets,
            relay_tx: None,
        }
    }
}

pub struct DiscoveryService {
    config: DiscoveryConfig,
    mdns: ServiceDaemon,
    peers: Arc<RwLock<HashMap<String, Peer>>>,
    lan: Mutex<Option<LanDiscovery>>,
}

impl DiscoveryService {
    pub fn new(config: DiscoveryConfig) -> Result<Self> {
        let mdns = ServiceDaemon::new()
            .context("Failed to create mDNS daemon. Is the port 5353 free?")?;
        Ok(Self {
            config,
            mdns,
            peers: Arc::new(RwLock::new(HashMap::new())),
            lan: Mutex::new(None),
        })
    }

    pub fn peers_arc(&self) -> Arc<RwLock<HashMap<String, Peer>>> {
        Arc::clone(&self.peers)
    }

    pub fn get_peers(&self) -> Vec<Peer> {
        self.peers
            .read()
            .expect("peers lock poisoned")
            .values()
            .filter(|peer| {
                contact_filter::has_contact_identity(&peer.username, &peer.department)
                    && peer.port != 0
            })
            .cloned()
            .collect()
    }

    /// Directly register a peer (used by manual IP discovery).
    /// Deduplicates by IP:port — if a peer with same IP:port already exists, updates it.
    pub fn register_peer(&self, peer: Peer) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let mut map = self.peers.write().expect("peers lock poisoned");

        // Check if a peer with same IP:port already exists
        let existing_id = map
            .values()
            .find(|p| p.ip == peer.ip && p.port == peer.port)
            .map(|p| p.id.clone());

        if existing_id.is_none()
            && (!contact_filter::has_contact_identity(&peer.username, &peer.department)
                || peer.port == 0)
        {
            log::debug!("Skipping peer without contact identity: {}", peer.id);
            return;
        }

        if let Some(id) = existing_id {
            // Update existing peer
            if let Some(existing) = map.get_mut(&id) {
                existing.online = peer.online;
                existing.last_seen = now;
                if !peer.username.is_empty() {
                    existing.username = peer.username;
                }
                if !peer.department.is_empty() {
                    existing.department = peer.department;
                }
                if !peer.software_version.is_empty() {
                    existing.software_version = peer.software_version;
                }
                if !peer.mac_address.is_empty() {
                    existing.mac_address = peer.mac_address;
                }
                if peer.avatar_updated_at > existing.avatar_updated_at {
                    existing.avatar_path = peer.avatar_path;
                    existing.avatar_hash = peer.avatar_hash;
                    existing.avatar_updated_at = peer.avatar_updated_at;
                } else if existing.avatar_hash.is_empty() && !peer.avatar_hash.is_empty() {
                    existing.avatar_hash = peer.avatar_hash;
                    existing.avatar_updated_at = peer.avatar_updated_at;
                }
            }
        } else {
            map.insert(
                peer.id.clone(),
                Peer {
                    id: peer.id,
                    username: peer.username,
                    department: peer.department,
                    software_version: peer.software_version,
                    mac_address: peer.mac_address,
                    avatar_path: peer.avatar_path,
                    avatar_hash: peer.avatar_hash,
                    avatar_updated_at: peer.avatar_updated_at,
                    ip: peer.ip,
                    port: peer.port,
                    online: peer.online,
                    last_seen: now,
                },
            );
        }
    }

    pub fn get_peer(&self, peer_id: &str) -> Option<Peer> {
        self.peers
            .read()
            .expect("peers lock poisoned")
            .get(peer_id)
            .cloned()
    }

    /// Update last_seen and set online=true (called by health check when TCP succeeds).
    /// If the peer isn't in the map yet, it's inserted.
    pub fn touch_peer(&self, peer_id: &str) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let mut map = self.peers.write().expect("peers lock poisoned");
        if let Some(peer) = map.get_mut(peer_id) {
            peer.online = true;
            peer.last_seen = now;
        }
    }

    pub fn set_online(&self, peer_id: &str, online: bool) {
        if let Some(peer) = self.peers.write().expect("peers lock poisoned").get_mut(peer_id) {
            peer.online = online;
        }
    }

    // Accessor retained for older discovery integrations; chat server owns the active call sites.
    #[allow(dead_code)]
    pub fn my_id(&self) -> &str {
        &self.config.peer_id
    }

    pub async fn start(&self) -> Result<()> {
        let local_ip = self.resolve_local_ip()?;

        info!(
            "Starting discovery as '{}' ({}) on {}:{}",
            self.config.username, self.config.peer_id, local_ip, self.config.listen_port
        );

        // mDNS disabled — unreliable on this network, using UDP broadcast instead
        // Keeping the code but skipping registration and browsing
        if false {
            let instance_name = format!("echo-{}", &self.config.peer_id.get(..8).unwrap_or("00000000"));
            let port_str = self.config.listen_port.to_string();
            let avatar_updated_at_str = self.config.avatar_updated_at.to_string();
            let properties = vec![
                ("id", self.config.peer_id.as_str()),
                ("username", self.config.username.as_str()),
                ("department", self.config.department.as_str()),
                ("software_version", self.config.software_version.as_str()),
                ("mac_address", self.config.mac_address.as_str()),
                ("avatar_hash", self.config.avatar_hash.as_str()),
                ("avatar_updated_at", avatar_updated_at_str.as_str()),
                ("port", port_str.as_str()),
            ];

            let service_info = mdns_sd::ServiceInfo::new(
                SERVICE_TYPE,
                &instance_name,
                &format!("{}.local.", instance_name),
                local_ip,
                self.config.listen_port,
                &properties[..],
            )
            .context("Failed to create mDNS ServiceInfo")?;

            self.mdns
                .register(service_info)
                .context("Failed to register mDNS service")?;

            let receiver = self
                .mdns
                .browse(SERVICE_TYPE)
                .context("Failed to start mDNS browse")?;

            let peers = Arc::clone(&self.peers);
            let my_id = self.config.peer_id.clone();

            tauri::async_runtime::spawn(async move {
                Self::handle_mdns_events(receiver, peers, my_id).await;
            });
        }

        // Also start LAN discovery (broadcast + multicast + unicast response)
        let discovery_port = self.config.listen_port + 2;
        let lan_config = LanDiscoveryConfig {
            peer_id: self.config.peer_id.clone(),
            username: self.config.username.clone(),
            department: self.config.department.clone(),
            software_version: self.config.software_version.clone(),
            mac_address: self.config.mac_address.clone(),
            avatar_hash: self.config.avatar_hash.clone(),
            avatar_updated_at: self.config.avatar_updated_at,
            listen_port: self.config.listen_port,
            local_ip,
            scan_subnets: self.config.scan_subnets.clone(),
            discovery_port,
            relay_tx: self.config.relay_tx.clone(),
        };
        let lan = LanDiscovery::new(lan_config, Arc::clone(&self.peers))
            .context("Failed to start LAN discovery")?;
        *self.lan.lock().unwrap() = Some(lan);

        Ok(())
    }

    pub fn get_scan_subnets(&self) -> Vec<String> {
        self.lan.lock().unwrap()
            .as_ref()
            .map(|lan| lan.get_scan_subnets())
            .unwrap_or_default()
    }

    pub fn update_scan_subnets(&self, subnets: &[String]) {
        if let Some(ref lan) = *self.lan.lock().unwrap() {
            lan.update_scan_subnets(subnets);
        }
    }

    pub fn stop(&self) -> Result<()> {
        info!("Stopping discovery service...");
        // Shutdown LAN discovery first (joins threads)
        if let Some(mut lan) = self.lan.lock().unwrap().take() {
            lan.shutdown();
        }
        let _ = self.mdns.unregister(SERVICE_TYPE);
        self.mdns.shutdown()?;
        Ok(())
    }

    pub async fn update_identity(&self, username: &str, department: &str) -> Result<()> {
        let local_ip = self.resolve_local_ip()?;

        let _ = self.mdns.unregister(SERVICE_TYPE);

        let instance_name = format!("echo-{}", &self.config.peer_id.get(..8).unwrap_or("00000000"));
        let port_str = self.config.listen_port.to_string();
        let avatar_updated_at_str = self.config.avatar_updated_at.to_string();
        let properties = vec![
            ("id", self.config.peer_id.as_str()),
            ("username", username),
            ("department", department),
            ("software_version", self.config.software_version.as_str()),
            ("mac_address", self.config.mac_address.as_str()),
            ("avatar_hash", self.config.avatar_hash.as_str()),
            ("avatar_updated_at", avatar_updated_at_str.as_str()),
            ("port", port_str.as_str()),
        ];

        let service_info = mdns_sd::ServiceInfo::new(
            SERVICE_TYPE,
            &instance_name,
            &format!("{}.local.", instance_name),
            local_ip,
            self.config.listen_port,
            &properties[..],
        )
        .context("Failed to create updated mDNS ServiceInfo")?;

        self.mdns
            .register(service_info)
            .context("Failed to re-register mDNS service")?;

        info!("Discovery identity updated to '{}' ({})", username, department);
        Ok(())
    }

    fn resolve_local_ip(&self) -> Result<std::net::IpAddr> {
        local_ip().context("Failed to detect local IP address.")
    }

    async fn handle_mdns_events(
        receiver: mdns_sd::Receiver<ServiceEvent>,
        peers: Arc<RwLock<HashMap<String, Peer>>>,
        my_id: String,
    ) {
        let (inner_tx, mut inner_rx) = mpsc::unbounded_channel::<ServiceEvent>();

        std::thread::spawn(move || {
            while let Ok(event) = receiver.recv() {
                if inner_tx.send(event).is_err() {
                    break;
                }
            }
        });

        while let Some(event) = inner_rx.recv().await {
            match event {
                ServiceEvent::ServiceResolved(info) => {
                    Self::on_service_resolved(&info, &peers, &my_id);
                }
                ServiceEvent::ServiceRemoved(_, full_name) => {
                    Self::on_service_removed(&full_name, &peers);
                }
                _ => {}
            }
        }
    }

    fn on_service_resolved(
        info: &mdns_sd::ServiceInfo,
        peers: &Arc<RwLock<HashMap<String, Peer>>>,
        my_id: &str,
    ) {
        let properties = info.get_properties();

        let peer_id = match properties.get("id") {
            Some(val) => val.val_str().to_string(),
            None => {
                warn!("mDNS service without 'id', skipping.");
                return;
            }
        };

        if peer_id == my_id {
            return;
        }

        let username = properties
            .get("username")
            .map(|v| v.val_str().to_string())
            .unwrap_or_else(|| "Unknown".to_string());

        let department = properties
            .get("department")
            .map(|v| v.val_str().to_string())
            .unwrap_or_else(|| "未分组".to_string());

        let software_version = properties
            .get("software_version")
            .map(|v| v.val_str().to_string())
            .unwrap_or_default();

        let mac_address = properties
            .get("mac_address")
            .map(|v| v.val_str().to_string())
            .unwrap_or_default();

        let avatar_hash = properties
            .get("avatar_hash")
            .map(|v| v.val_str().to_string())
            .unwrap_or_default();

        let avatar_updated_at = properties
            .get("avatar_updated_at")
            .and_then(|v| v.val_str().parse::<i64>().ok())
            .unwrap_or_default();

        let port: u16 = properties
            .get("port")
            .and_then(|v| v.val_str().parse().ok())
            .unwrap_or(0);

        let ip = match info.get_addresses().iter().next() {
            Some(addr) => *addr,
            None => {
                warn!("Resolved service has no addresses.");
                return;
            }
        };

        let peer = Peer::new_with_avatar(
            peer_id.clone(),
            username.clone(),
            department,
            software_version,
            mac_address,
            String::new(),
            avatar_hash,
            avatar_updated_at,
            ip,
            port,
        );

        let mut peers_map = peers.write().expect("peers lock poisoned");
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let changed = peers_map
            .get(&peer_id)
            .map(|existing| {
                existing.username != peer.username
                    || existing.department != peer.department
                    || existing.software_version != peer.software_version
                    || existing.mac_address != peer.mac_address
                    || existing.avatar_hash != peer.avatar_hash
                    || existing.avatar_updated_at != peer.avatar_updated_at
                    || existing.ip != peer.ip
                    || existing.port != peer.port
                    || !existing.online
            })
            .unwrap_or(true);

        peers_map.insert(
            peer_id.clone(),
            Peer {
                id: peer.id.clone(),
                username: peer.username.clone(),
                department: peer.department.clone(),
                software_version: peer.software_version.clone(),
                mac_address: peer.mac_address.clone(),
                avatar_path: String::new(),
                avatar_hash: peer.avatar_hash.clone(),
                avatar_updated_at: peer.avatar_updated_at,
                ip: peer.ip,
                port: peer.port,
                online: true,
                last_seen: now,
            },
        );

        if changed {
            info!("Peer updated: {}", peer);
        }
    }

    fn on_service_removed(
        full_name: &str,
        peers: &Arc<RwLock<HashMap<String, Peer>>>,
    ) {
        let removed_id = {
            let peers_map = peers.read().expect("peers lock poisoned");
            peers_map
                .values()
                .find(|p| full_name.contains(&p.id[..8]))
                .map(|p| p.id.clone())
        };

        if let Some(peer_id) = removed_id {
            let mut peers_map = peers.write().expect("peers lock poisoned");
            if let Some(peer) = peers_map.get_mut(&peer_id) {
                peer.online = false;
                info!("Peer lost: {}", peer);
            }
        }
    }
}

impl Drop for DiscoveryService {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
