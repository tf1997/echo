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

use super::peer::{Peer, PeerEntry};

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

/// Configuration for LAN discovery.
pub struct LanDiscoveryConfig {
    pub peer_id: String,
    pub username: String,
    pub department: String,
    pub listen_port: u16,
    pub local_ip: IpAddr,
    pub scan_subnets: Vec<String>,
    pub discovery_port: u16,
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
        let discovery_port = config.discovery_port;
        let bind_addr = format!("0.0.0.0:{}", discovery_port);

        // Use socket2 for cross-platform SO_REUSEADDR/SO_REUSEPORT
        let sock2 = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::DGRAM, Some(socket2::Protocol::UDP))
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        sock2.set_reuse_address(true).ok();
        #[cfg(unix)]
        sock2.set_reuse_port(true).ok();
        sock2.set_broadcast(true)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        sock2.set_read_timeout(Some(Duration::from_secs(READ_TIMEOUT_SECS)))
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        let sock_addr = socket2::SockAddr::from(std::net::SocketAddr::new(
            std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
            discovery_port,
        ));
        sock2.bind(&sock_addr)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        let socket: UdpSocket = sock2.into();

        // Join multicast group
        if let Err(e) = socket.join_multicast_v4(&MULTICAST_ADDR, &Ipv4Addr::UNSPECIFIED) {
            warn!("Failed to join multicast group {}: {}", MULTICAST_ADDR, e);
        }

        let socket = Arc::new(socket);

        let cancel = Arc::new(AtomicBool::new(false));

        // Build initial subnet prefixes from config
        let initial_prefixes = Self::build_subnet_prefixes(config.local_ip, &config.scan_subnets);
        let scan_subnets = Arc::new(RwLock::new(initial_prefixes));

        // Build the base packet (without known_peers — filled dynamically)
        let base_announce = AnnouncePacket {
            id: config.peer_id.clone(),
            username: config.username.clone(),
            department: config.department.clone(),
            ip: config.local_ip.to_string(),
            port: config.listen_port,
            known_peers: Vec::new(),
        };

        // Sender thread — rebuilds known_peers each cycle
        let sender_socket = Arc::clone(&socket);
        let sender_cancel = Arc::clone(&cancel);
        let sender_my_info = base_announce.clone();
        let sender_peers = Arc::clone(&peers);
        let sender_handle = thread::spawn(move || {
            Self::sender_loop(sender_socket, sender_my_info, sender_peers, sender_cancel, discovery_port);
        });

        // Listener thread
        let listener_socket = Arc::clone(&socket);
        let listener_cancel = Arc::clone(&cancel);
        let listener_my_info = base_announce.clone();
        let listener_peers = Arc::clone(&peers);
        let listener_handle = thread::spawn(move || {
            Self::listener_loop(
                listener_socket,
                listener_my_info,
                listener_peers,
                listener_cancel,
                discovery_port,
            );
        });

        // Scanner thread (unicast subnet probe)
        let scanner_socket = Arc::clone(&socket);
        let scanner_bytes = serde_json::to_vec(&base_announce).unwrap_or_default();
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
                discovery_port,
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

    fn build_announce_data(my_info: &AnnouncePacket, peers: &Arc<RwLock<HashMap<String, Peer>>>) -> Vec<u8> {
        let mut pkt = my_info.clone();
        {
            let map = peers.read().unwrap();
            pkt.known_peers = map.values()
                .filter(|p| p.online)
                .map(|p| PeerEntry {
                    id: p.id.clone(), username: p.username.clone(),
                    department: p.department.clone(), ip: p.ip.to_string(), port: p.port,
                })
                .collect();
        }
        serde_json::to_vec(&pkt).unwrap_or_default()
    }

    fn sender_loop(
        socket: Arc<UdpSocket>,
        my_info: AnnouncePacket,
        peers: Arc<RwLock<HashMap<String, Peer>>>,
        cancel: Arc<AtomicBool>,
        discovery_port: u16,
    ) {
        let broadcast_target = SocketAddr::new(IpAddr::V4(Ipv4Addr::BROADCAST), discovery_port);
        let multicast_target = SocketAddr::new(IpAddr::V4(MULTICAST_ADDR), discovery_port);

        loop {
            if cancel.load(Ordering::Relaxed) { return; }

            let data = Self::build_announce_data(&my_info, &peers);

            let _ = socket.send_to(&data, broadcast_target);
            let _ = socket.send_to(&data, multicast_target);

            for _ in 0..ANNOUNCE_INTERVAL_SECS {
                if cancel.load(Ordering::Relaxed) { return; }
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
        discovery_port: u16,
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
            let my_info = AnnouncePacket {
                id: my_id.clone(),
                username: String::new(),
                department: String::new(),
                ip: String::new(),
                port: 0,
                known_peers: Vec::new(),
            };
            base_data = Self::build_announce_data(&my_info, &peers);

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
                        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(a, b, c, host)), discovery_port);
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
        my_info: AnnouncePacket,
        peers: Arc<RwLock<HashMap<String, Peer>>>,
        cancel: Arc<AtomicBool>,
        discovery_port: u16,
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

                    info!("UDP recv: id={} from {} ({} known_peers)", packet.id, src_addr, packet.known_peers.len());

                    // Skip self
                    if packet.id == my_info.id {
                        debug!("UDP recv: skipping self");
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
                        if entry.id != my_info.id && !peers_map.contains_key(&entry.id) {
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
                    // Collect existing contacts BEFORE dropping the write lock
                    let existing_targets: Vec<SocketAddr> = peers_map.iter()
                        .filter(|(id, p)| *id != &packet.id && p.online)
                        .map(|(_, p)| SocketAddr::new(p.ip, p.port + 2))
                        .collect();

                    drop(peers_map); // Release write lock so build_announce_data can read

                    if is_new {
                        info!("LAN discovered NEW peer: {}", peer);

                        // Unicast response + our known_peers to the new peer
                        let remote_discovery_port = packet.port + 2;
                        let response_data = Self::build_announce_data(&my_info, &peers);
                        let response_target = SocketAddr::new(remote_ip, remote_discovery_port);
                        info!("UDP response to {} ({} bytes)", response_target, response_data.len());
                        let _ = socket.send_to(&response_data, response_target);

                        // Tell ALL our existing contacts about this new peer
                        let intro = AnnouncePacket {
                            id: my_info.id.clone(),
                            username: my_info.username.clone(),
                            department: my_info.department.clone(),
                            ip: my_info.ip.clone(),
                            port: my_info.port,
                            known_peers: vec![PeerEntry {
                                id: peer.id.clone(),
                                username: peer.username.clone(),
                                department: peer.department.clone(),
                                ip: remote_ip.to_string(),
                                port: packet.port,
                            }],
                        };
                        let intro_bytes = serde_json::to_vec(&intro).unwrap_or_default();
                        for target in &existing_targets {
                            let _ = socket.send_to(&intro_bytes, *target);
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
