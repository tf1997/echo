use log::{debug, info, warn};
use rand::seq::SliceRandom;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use super::peer::Peer;

const DISCOVERY_PORT: u16 = 9529;
const MULTICAST_ADDR: Ipv4Addr = Ipv4Addr::new(239, 255, 42, 42);
const ANNOUNCE_INTERVAL_SECS: u64 = 3;
const READ_TIMEOUT_SECS: u64 = 1;
const SCAN_INTERVAL_SECS: u64 = 300; // 5 minutes — avoids IDS triggering
const PROBE_DELAY_MS_MIN: u64 = 3;
const PROBE_DELAY_MS_MAX: u64 = 15;

/// Wire format for LAN discovery packets.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct AnnouncePacket {
    id: String,
    username: String,
    department: String,
    ip: String,
    port: u16,
    /// Optional: list of peers this node knows about (for peer relay / 网桥)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    known_peers: Vec<PeerEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PeerEntry {
    id: String,
    username: String,
    department: String,
    ip: String,
    port: u16,
}

/// Configuration for LAN discovery.
pub struct LanDiscoveryConfig {
    pub peer_id: String,
    pub username: String,
    pub department: String,
    pub listen_port: u16,
    pub local_ip: IpAddr,
    pub scan_subnets: Vec<String>,
}

/// Manages UDP broadcast + multicast + unicast subnet scan + unicast-response discovery.
pub struct LanDiscovery {
    cancel: Arc<AtomicBool>,
    sender_handle: Option<JoinHandle<()>>,
    listener_handle: Option<JoinHandle<()>>,
    scanner_handle: Option<JoinHandle<()>>,
    socket: Option<Arc<UdpSocket>>,
    scan_subnets: Arc<RwLock<Vec<String>>>,
}

impl LanDiscovery {
    pub fn new(
        config: LanDiscoveryConfig,
        peers: Arc<RwLock<HashMap<String, Peer>>>,
    ) -> io::Result<Self> {
        let bind_addr = format!("0.0.0.0:{}", DISCOVERY_PORT);
        let socket = UdpSocket::bind(&bind_addr)?;

        socket.set_broadcast(true)?;
        socket.set_read_timeout(Some(Duration::from_secs(READ_TIMEOUT_SECS)))?;

        // Join multicast group
        if let Err(e) = socket.join_multicast_v4(&MULTICAST_ADDR, &Ipv4Addr::UNSPECIFIED) {
            warn!("Failed to join multicast group {}: {}", MULTICAST_ADDR, e);
        }

        let socket = Arc::new(socket);

        let announce = AnnouncePacket {
            id: config.peer_id.clone(),
            username: config.username,
            department: config.department,
            ip: config.local_ip.to_string(),
            port: config.listen_port,
            known_peers: Vec::new(),
        };

        let announce_bytes =
            serde_json::to_vec(&announce).expect("Failed to serialize announce packet");

        let cancel = Arc::new(AtomicBool::new(false));

        // Build initial subnet prefixes from config
        let initial_prefixes = Self::build_subnet_prefixes(config.local_ip, &config.scan_subnets);
        let scan_subnets = Arc::new(RwLock::new(initial_prefixes));

        // Sender thread (broadcast + multicast heartbeat)
        let sender_socket = Arc::clone(&socket);
        let sender_bytes = announce_bytes.clone();
        let sender_cancel = Arc::clone(&cancel);
        let sender_id = config.peer_id.clone();
        let sender_handle = thread::spawn(move || {
            Self::sender_loop(sender_socket, sender_bytes, sender_id, sender_cancel);
        });

        // Listener thread (receive broadcasts + unicast responses + peer relay)
        let listener_socket = Arc::clone(&socket);
        let listener_bytes = announce_bytes.clone();
        let listener_cancel = Arc::clone(&cancel);
        let listener_my_id = config.peer_id.clone();
        let listener_peers = Arc::clone(&peers);
        let listener_handle = thread::spawn(move || {
            Self::listener_loop(
                listener_socket,
                listener_bytes,
                listener_peers,
                listener_my_id,
                listener_cancel,
            );
        });

        // Scanner thread (unicast subnet probe)
        let scanner_socket = Arc::clone(&socket);
        let scanner_bytes = announce_bytes.clone();
        let scanner_cancel = Arc::clone(&cancel);
        let scanner_id = config.peer_id;
        let scanner_subnets = Arc::clone(&scan_subnets);
        let scanner_peers = Arc::clone(&peers);
        let scanner_handle = thread::spawn(move || {
            Self::scanner_loop(
                scanner_socket,
                scanner_bytes,
                scanner_id,
                scanner_subnets,
                scanner_peers,
                scanner_cancel,
            );
        });

        let prefixes: Vec<String> = scan_subnets.read().unwrap().clone();
        info!(
            "LAN discovery started on {} (broadcast + multicast {} + {} subnet(s))",
            bind_addr,
            MULTICAST_ADDR,
            prefixes.len()
        );
        for s in &prefixes {
            info!("  Scanning subnet: {}.0/24", s);
        }
        drop(prefixes);

        Ok(Self {
            cancel,
            sender_handle: Some(sender_handle),
            listener_handle: Some(listener_handle),
            scanner_handle: Some(scanner_handle),
            socket: Some(socket),
            scan_subnets,
        })
    }

    /// Update the subnet scan list at runtime (called when user changes config).
    pub fn update_scan_subnets(&self, raw: &[String]) {
        let prefixes = Self::build_subnet_prefixes_from_raw(raw);
        info!("Updating scan subnets to {} prefix(es)", prefixes.len());
        *self.scan_subnets.write().unwrap() = prefixes;
    }

    /// Get the current scan subnet list.
    pub fn get_scan_subnets(&self) -> Vec<String> {
        self.scan_subnets.read().unwrap().clone()
    }

    pub fn shutdown(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);

        if let Some(h) = self.sender_handle.take() {
            let _ = h.join();
        }
        if let Some(h) = self.listener_handle.take() {
            let _ = h.join();
        }
        if let Some(h) = self.scanner_handle.take() {
            let _ = h.join();
        }
        if let Some(socket) = self.socket.take() {
            if let Ok(sock) = Arc::try_unwrap(socket) {
                let _ = sock.leave_multicast_v4(&MULTICAST_ADDR, &Ipv4Addr::UNSPECIFIED);
            }
        }

        info!("LAN discovery shutdown complete");
    }

    /// Build subnet prefixes from user-configured subnets only.
    /// The local subnet is already covered by broadcast + multicast.
    /// Scanning is only needed for cross-subnet reachability.
    fn build_subnet_prefixes(_local_ip: IpAddr, raw: &[String]) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut prefixes: Vec<String> = Vec::new();
        for s in Self::build_subnet_prefixes_from_raw(raw) {
            if seen.insert(s.clone()) {
                prefixes.push(s);
            }
        }
        prefixes
    }

    fn build_subnet_prefixes_from_raw(raw: &[String]) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for part in raw {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            // Accept formats: "192.168.2.0", "192.168.2", "192.168.2.0/24", "10.100.0"
            let stripped = part.trim_end_matches("/24").trim_end_matches(".0");
            let stripped = stripped.trim_end_matches('.');
            if !stripped.is_empty() {
                // Validate: expect "x.y.z" (3 octets)
                let parts: Vec<&str> = stripped.split('.').collect();
                if parts.len() == 3
                    && parts.iter().all(|p| p.parse::<u8>().is_ok())
                {
                    if !out.iter().any(|p| p == stripped) {
                        out.push(stripped.to_string());
                    }
                }
            }
        }
        out
    }

    fn sender_loop(
        socket: Arc<UdpSocket>,
        data: Vec<u8>,
        my_id: String,
        cancel: Arc<AtomicBool>,
    ) {
        let broadcast_target = SocketAddr::new(IpAddr::V4(Ipv4Addr::BROADCAST), DISCOVERY_PORT);
        let multicast_target = SocketAddr::new(IpAddr::V4(MULTICAST_ADDR), DISCOVERY_PORT);

        loop {
            if cancel.load(Ordering::Relaxed) {
                return;
            }

            if let Err(e) = socket.send_to(&data, broadcast_target) {
                warn!("UDP broadcast send failed: {}", e);
            } else {
                debug!("UDP broadcast sent (id={})", &my_id[..8.min(my_id.len())]);
            }

            if let Err(e) = socket.send_to(&data, multicast_target) {
                warn!("UDP multicast send failed: {}", e);
            }

            for _ in 0..ANNOUNCE_INTERVAL_SECS {
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                thread::sleep(Duration::from_secs(1));
            }
        }
    }

    /// Subnet scanner: periodically probes configured /24 subnets via unicast UDP.
    fn scanner_loop(
        socket: Arc<UdpSocket>,
        mut base_data: Vec<u8>,
        my_id: String,
        subnets: Arc<RwLock<Vec<String>>>,
        peers: Arc<RwLock<HashMap<String, Peer>>>,
        cancel: Arc<AtomicBool>,
    ) {
        loop {
            // Wait before first scan
            for _ in 0..SCAN_INTERVAL_SECS {
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                thread::sleep(Duration::from_secs(1));
            }

            // Rebuild data with known peers for peer relay
            {
                let peers_map = peers.read().unwrap();
                let known: Vec<PeerEntry> = peers_map
                    .values()
                    .filter(|p| p.online)
                    .map(|p| PeerEntry {
                        id: p.id.clone(),
                        username: p.username.clone(),
                        department: p.department.clone(),
                        ip: p.ip.to_string(),
                        port: p.port,
                    })
                    .collect();
                let packet = AnnouncePacket {
                    id: my_id.clone(),
                    username: String::new(), // parsed from base_data; just rebuild
                    department: String::new(),
                    ip: String::new(),
                    port: 0,
                    known_peers: known,
                };
                if let Ok(json) = serde_json::to_vec(&packet) {
                    base_data = json;
                }
            }

            let prefixes = subnets.read().unwrap().clone();
            let start = std::time::Instant::now();
            let mut sent: u32 = 0;

            for prefix in &prefixes {
                let parts: Vec<&str> = prefix.split('.').collect();
                if parts.len() != 3 {
                    continue;
                }
                let a: u8 = match parts[0].parse() {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let b: u8 = match parts[1].parse() {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let c: u8 = match parts[2].parse() {
                    Ok(v) => v,
                    Err(_) => continue,
                };

                // Randomize IP order to avoid sequential scan pattern
                let mut hosts: Vec<u8> = (1..=254).collect();
                hosts.shuffle(&mut rand::thread_rng());

                for host in hosts {
                    if cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    let target =
                        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(a, b, c, host)), DISCOVERY_PORT);
                    if socket.send_to(&base_data, target).is_ok() {
                        sent += 1;
                    }
                    // Random jitter between probes
                    let delay = rand::thread_rng().gen_range(PROBE_DELAY_MS_MIN..=PROBE_DELAY_MS_MAX);
                    thread::sleep(Duration::from_millis(delay));
                }
            }

            debug!(
                "Subnet scan done: {} probes in {:?} (id={})",
                sent,
                start.elapsed(),
                &my_id[..8.min(my_id.len())]
            );
        }
    }

    fn listener_loop(
        socket: Arc<UdpSocket>,
        own_announce: Vec<u8>,
        peers: Arc<RwLock<HashMap<String, Peer>>>,
        my_id: String,
        cancel: Arc<AtomicBool>,
    ) {
        let mut buf = [0u8; 4096];

        loop {
            if cancel.load(Ordering::Relaxed) {
                return;
            }

            match socket.recv_from(&mut buf) {
                Ok((len, src_addr)) => {
                    let packet: AnnouncePacket = match serde_json::from_slice(&buf[..len]) {
                        Ok(p) => p,
                        Err(_) => continue,
                    };

                    // Skip self
                    if packet.id == my_id {
                        continue;
                    }

                    let remote_ip = match src_addr.ip() {
                        IpAddr::V4(ip) => IpAddr::V4(ip),
                        IpAddr::V6(ip) => IpAddr::V6(ip),
                    };

                    // Register the announcing peer
                    let peer = Peer::new(
                        packet.id.clone(),
                        packet.username.clone(),
                        packet.department.clone(),
                        remote_ip,
                        packet.port,
                    );

                    let mut peers_map = peers.write().expect("peers lock poisoned");
                    let is_new = !peers_map.contains_key(&packet.id);

                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs() as i64;

                    peers_map.insert(
                        packet.id.clone(),
                        Peer {
                            id: peer.id.clone(),
                            username: peer.username.clone(),
                            department: peer.department.clone(),
                            ip: remote_ip,
                            port: packet.port,
                            online: true,
                            last_seen: now,
                        },
                    );

                    // Process peer relay: register all known_peers too
                    for entry in &packet.known_peers {
                        if entry.id != my_id && !peers_map.contains_key(&entry.id) {
                            if let Ok(ip) = entry.ip.parse::<IpAddr>() {
                                peers_map.insert(
                                    entry.id.clone(),
                                    Peer {
                                        id: entry.id.clone(),
                                        username: entry.username.clone(),
                                        department: entry.department.clone(),
                                        ip,
                                        port: entry.port,
                                        online: true,
                                        last_seen: now,
                                    },
                                );
                                info!(
                                    "Peer relay discovered: {} ({}) @ {}:{}",
                                    entry.username, entry.id, entry.ip, entry.port
                                );
                            }
                        }
                    }
                    drop(peers_map);

                    if is_new {
                        info!("LAN discovered NEW peer: {}", peer);

                        // Unicast response with our own peer list (peer relay)
                        let response_target = SocketAddr::new(remote_ip, DISCOVERY_PORT);
                        if let Err(e) = socket.send_to(&own_announce, response_target) {
                            warn!("Unicast response to {} failed: {}", response_target, e);
                        } else {
                            debug!("Unicast response sent to {}", response_target);
                        }
                    }
                }
                Err(ref e)
                    if e.kind() == io::ErrorKind::WouldBlock
                        || e.kind() == io::ErrorKind::TimedOut =>
                {
                    continue;
                }
                Err(e) => {
                    warn!("UDP recv error: {}", e);
                    continue;
                }
            }
        }
    }
}
