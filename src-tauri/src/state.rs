use anyhow::Result;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

use crate::chat::ChatServer;
use crate::db::{Database, UserProfile};
use crate::discovery::{DiscoveryConfig, DiscoveryService};

pub struct RuntimeServices {
    pub discovery: RwLock<DiscoveryService>,
    pub chat: Mutex<ChatServer>,
    pub my_id: String,
    pub listen_port: u16,
}

impl RuntimeServices {
    pub async fn start(db: Arc<Database>, profile: &UserProfile, listen_port: u16) -> Result<Self> {
        // Identity = IP:port — no UUID, the network address IS the identity
        let local_ip = local_ip_address::local_ip()
            .map_err(|e| anyhow::anyhow!("Failed to get local IP: {}", e))?;
        let my_id = format!("{}:{}", local_ip, listen_port);

        // Persist peer_id for profile
        if profile.peer_id.is_empty() || profile.peer_id != my_id {
            db.save_user_profile(&my_id, &profile.username, &profile.department)
                .await
                .ok();
        }

        let scan_subnets = db
            .get_scan_subnets()
            .await
            .unwrap_or_default();

        let config = DiscoveryConfig::new(
            &my_id,
            &profile.username,
            &profile.department,
            listen_port,
            scan_subnets,
        );
        let discovery = DiscoveryService::new(config)?;
        discovery.start().await?;

        let peers = discovery.peers_arc();

        let chat = ChatServer::new(
            listen_port,
            my_id.clone(),
            profile.username.clone(),
            profile.department.clone(),
            db,
            peers,
        );
        chat.start().await?;

        Ok(Self {
            discovery: RwLock::new(discovery),
            chat: Mutex::new(chat),
            my_id,
            listen_port,
        })
    }

    #[allow(dead_code)]
    pub async fn shutdown(&self) {
        let discovery = self.discovery.write().await;
        let _ = discovery.stop();
    }

    pub async fn update_profile(&self, username: &str, department: &str) -> Result<()> {
        self.discovery
            .write()
            .await
            .update_identity(username, department)
            .await?;
        self.chat
            .lock()
            .await
            .update_identity(username, department);
        Ok(())
    }
}

pub struct AppState {
    pub db: Arc<Database>,
    pub profile: Mutex<Option<UserProfile>>,
    // Use RwLock for runtime: multiple readers (clone Arc handle), single writer (initialization)
    pub runtime: RwLock<Option<Arc<RuntimeServices>>>,
}
