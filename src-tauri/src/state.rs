use anyhow::Result;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::chat::ChatServer;
use crate::db::{Database, UserProfile};
use crate::discovery::{DiscoveryConfig, DiscoveryService};

pub struct RuntimeServices {
    pub discovery: Mutex<DiscoveryService>,
    pub chat: Mutex<ChatServer>,
    pub my_id: String,
    pub listen_port: u16,
}

impl RuntimeServices {
    pub async fn start(db: Arc<Database>, profile: &UserProfile, listen_port: u16) -> Result<Self> {
        let mut pid = profile.peer_id.clone();
        if pid.is_empty() {
            pid = uuid::Uuid::new_v4().to_string();
            db.save_user_profile(&pid, &profile.username, &profile.department)
                .await
                .ok();
        }
        let scan_subnets = db
            .get_scan_subnets()
            .await
            .unwrap_or_default();

        let config = DiscoveryConfig::new(
            &pid,
            &profile.username,
            &profile.department,
            listen_port,
            scan_subnets,
        );
        let discovery = DiscoveryService::new(config)?;
        let my_id = discovery.my_id().to_string();
        discovery.start().await?;

        let chat = ChatServer::new(
            listen_port,
            my_id.clone(),
            profile.username.clone(),
            profile.department.clone(),
            db,
        );
        chat.start().await?;

        Ok(Self {
            discovery: Mutex::new(discovery),
            chat: Mutex::new(chat),
            my_id,
            listen_port,
        })
    }

    #[allow(dead_code)]
    pub async fn shutdown(&self) {
        let discovery = self.discovery.lock().await;
        let _ = discovery.stop();
    }

    pub async fn update_profile(&self, username: &str, department: &str) -> Result<()> {
        self.discovery
            .lock()
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
    pub runtime: Mutex<Option<RuntimeServices>>,
}
