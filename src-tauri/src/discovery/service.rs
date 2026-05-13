use anyhow::{Context, Result};
use local_ip_address::local_ip;
use log::{info, warn};
use mdns_sd::{ServiceDaemon, ServiceEvent};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tokio::sync::mpsc;

use super::peer::Peer;

const SERVICE_TYPE: &str = "_echo-p2p._tcp.local.";

#[derive(Debug, Clone)]
pub struct DiscoveryConfig {
    pub peer_id: String,
    pub username: String,
    pub department: String,
    pub listen_port: u16,
}

impl DiscoveryConfig {
    pub fn new(
        peer_id: impl Into<String>,
        username: impl Into<String>,
        department: impl Into<String>,
        listen_port: u16,
    ) -> Self {
        Self {
            peer_id: peer_id.into(),
            username: username.into(),
            department: department.into(),
            listen_port,
        }
    }
}

pub struct DiscoveryService {
    config: DiscoveryConfig,
    mdns: ServiceDaemon,
    peers: Arc<RwLock<HashMap<String, Peer>>>,
}

impl DiscoveryService {
    pub fn new(config: DiscoveryConfig) -> Result<Self> {
        let mdns = ServiceDaemon::new()
            .context("Failed to create mDNS daemon. Is the port 5353 free?")?;
        Ok(Self {
            config,
            mdns,
            peers: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    pub fn get_peers(&self) -> Vec<Peer> {
        self.peers
            .read()
            .expect("peers lock poisoned")
            .values()
            .cloned()
            .collect()
    }

    pub fn get_peer(&self, peer_id: &str) -> Option<Peer> {
        self.peers
            .read()
            .expect("peers lock poisoned")
            .get(peer_id)
            .cloned()
    }

    pub fn my_id(&self) -> &str {
        &self.config.peer_id
    }

    pub async fn start(&self) -> Result<()> {
        let local_ip = self.resolve_local_ip()?;

        info!(
            "Starting discovery as '{}' ({}) on {}:{}",
            self.config.username, self.config.peer_id, local_ip, self.config.listen_port
        );

        let instance_name = format!("echo-{}", &self.config.peer_id[..8]);
        let port_str = self.config.listen_port.to_string();
        let properties = vec![
            ("id", self.config.peer_id.as_str()),
            ("username", self.config.username.as_str()),
            ("department", self.config.department.as_str()),
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

        Ok(())
    }

    pub fn stop(&self) -> Result<()> {
        info!("Stopping discovery service...");
        let _ = self.mdns.unregister(SERVICE_TYPE);
        self.mdns.shutdown()?;
        Ok(())
    }

    pub async fn update_identity(&self, username: &str, department: &str) -> Result<()> {
        let local_ip = self.resolve_local_ip()?;

        let _ = self.mdns.unregister(SERVICE_TYPE);

        let instance_name = format!("echo-{}", &self.config.peer_id[..8]);
        let port_str = self.config.listen_port.to_string();
        let properties = vec![
            ("id", self.config.peer_id.as_str()),
            ("username", username),
            ("department", department),
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

        let peer = Peer::new(peer_id.clone(), username.clone(), department, ip, port);

        let mut peers_map = peers.write().expect("peers lock poisoned");
        let changed = peers_map
            .get(&peer_id)
            .map(|existing| {
                existing.username != peer.username
                    || existing.department != peer.department
                    || existing.ip != peer.ip
                    || existing.port != peer.port
                    || !existing.online
            })
            .unwrap_or(true);

        peers_map.insert(peer_id, peer.clone());

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