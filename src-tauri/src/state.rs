use anyhow::Result;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex, RwLock};

use crate::chat::{ChatServer, LocalPeerId};
use crate::db::{Database, UserProfile};
use crate::discovery::{DiscoveryConfig, DiscoveryService, PeerEntry};
use tauri::AppHandle;

pub struct RuntimeServices {
    pub discovery: RwLock<DiscoveryService>,
    pub chat: Mutex<ChatServer>,
    my_id: LocalPeerId,
    pub my_node_id: String,
    pub listen_port: u16,
    db: Arc<Database>,
    endpoint_update: Mutex<()>,
}

impl RuntimeServices {
    pub async fn start(
        app_handle: AppHandle,
        db: Arc<Database>,
        profile: &UserProfile,
        listen_port: u16,
        relay_tx: Option<mpsc::UnboundedSender<Vec<PeerEntry>>>,
    ) -> Result<Self> {
        // peer_id remains the legacy wire address; node_id is the stable identity.
        let local_ip = local_ip_address::local_ip()
            .map_err(|e| anyhow::anyhow!("Failed to get local IP: {}", e))?;
        let my_id = format!("{}:{}", local_ip, listen_port);
        let my_node_id = if profile.node_id.trim().is_empty() {
            db.ensure_user_node_id().await?
        } else {
            profile.node_id.clone()
        };

        let old_peer_id = profile.peer_id.clone();
        let peer_id_changed = !old_peer_id.is_empty() && old_peer_id != my_id;

        // Record local endpoint aliases when DHCP or adapter changes the IP.
        if peer_id_changed {
            log::info!(
                "IP changed: {} -> {}, recording aliases for node {}",
                old_peer_id,
                my_id,
                my_node_id
            );
            db.upsert_peer_alias(&my_node_id, &old_peer_id).await?;
            db.upsert_peer_alias(&my_node_id, &my_id).await?;
        }

        // Persist peer_id for profile
        if profile.peer_id.is_empty() || profile.peer_id != my_id {
            db.save_user_profile(
                &my_id,
                &profile.username,
                &profile.department,
                &crate::profile_metadata::software_version(),
                &crate::profile_metadata::mac_address(),
            )
            .await
            .ok();
        }

        let scan_subnets = db.get_scan_subnets().await.unwrap_or_default();

        let mut config = DiscoveryConfig::new(
            &my_id,
            &my_node_id,
            &profile.username,
            &profile.department,
            listen_port,
            scan_subnets,
            &profile.avatar_hash,
            profile.avatar_updated_at,
        );
        config.relay_tx = relay_tx;
        let discovery = DiscoveryService::new(config)?;
        discovery.start().await?;

        let peers = discovery.peers_arc();
        let local_peer_id = LocalPeerId::new(my_id.clone());

        let chat = ChatServer::new(
            app_handle.clone(),
            listen_port,
            local_peer_id.clone(),
            my_node_id.clone(),
            profile.username.clone(),
            profile.department.clone(),
            crate::profile_metadata::software_version(),
            crate::profile_metadata::mac_address(),
            db.clone(),
            peers,
        );
        chat.start().await?;

        // Broadcast peer_id change to all peers
        if peer_id_changed {
            log::info!(
                "Broadcasting peer_id change to all peers: old={} new={}",
                old_peer_id,
                my_id
            );
            let new_id = my_id.clone();
            let node_id = my_node_id.clone();
            let old_id = old_peer_id.clone();
            let username = profile.username.clone();
            let department = profile.department.clone();
            let avatar_hash = profile.avatar_hash.clone();
            let avatar_updated_at = profile.avatar_updated_at;
            let db_clone = db.clone();

            // Cannot access AppState from here during startup, so just schedule the broadcast
            // It will be handled by the startup sequence after RuntimeServices is registered
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;

                let stored = db_clone.list_stored_peers().await.unwrap_or_default();
                let targets: Vec<String> = stored.iter().map(|p| p.peer_id.clone()).collect();

                // Use profile_updated with old_peer_id for backward compatibility
                let payload = serde_json::json!({
                    "old_peer_id": old_id,  // NEW field - old versions ignore it
                    "username": username,
                    "department": department,
                    "software_version": crate::profile_metadata::software_version(),
                    "mac_address": crate::profile_metadata::mac_address(),
                    "avatar_hash": avatar_hash,
                    "avatar_updated_at": avatar_updated_at,
                })
                .to_string();

                for peer_id in &targets {
                    if peer_id == &new_id {
                        continue;
                    }
                    let json = crate::commands::build_notification_json(
                        &new_id,
                        &node_id,
                        &username,
                        &department,
                        listen_port,
                        peer_id,
                        "",
                        &payload,
                        "profile_updated",
                        None,
                        None,
                        None,
                        &[],
                    );
                    let _ = db_clone
                        .queue_pending_notification(peer_id, "profile_updated", &json)
                        .await;
                }
                log::info!(
                    "Queued profile_updated (with peer_id change) for {} peers",
                    targets.len()
                );
            });
        }

        Ok(Self {
            discovery: RwLock::new(discovery),
            chat: Mutex::new(chat),
            my_id: local_peer_id,
            my_node_id,
            listen_port,
            db,
            endpoint_update: Mutex::new(()),
        })
    }

    pub fn my_id(&self) -> String {
        self.my_id.get()
    }

    /// Publish a new local endpoint without restarting the TCP chat listener.
    /// Database identity is persisted before the in-memory route becomes visible.
    pub async fn update_local_endpoint(
        &self,
        profile: &UserProfile,
        new_ip: std::net::IpAddr,
    ) -> Result<Option<(String, String)>> {
        let _update_guard = self.endpoint_update.lock().await;
        let old_id = self.my_id();
        let new_id = format!("{}:{}", new_ip, self.listen_port);
        if old_id == new_id {
            return Ok(None);
        }

        log::info!("Runtime local IP changed: {} -> {}", old_id, new_id);
        self.db.upsert_peer_alias(&self.my_node_id, &old_id).await?;
        self.db.upsert_peer_alias(&self.my_node_id, &new_id).await?;
        self.db.migrate_peer_references(&old_id, &new_id).await?;
        self.db
            .save_user_profile(
                &new_id,
                &profile.username,
                &profile.department,
                &crate::profile_metadata::software_version(),
                &crate::profile_metadata::mac_address(),
            )
            .await?;

        self.my_id.set(new_id.clone());
        if let Err(error) = self
            .discovery
            .write()
            .await
            .update_local_endpoint(&new_id, new_ip)
        {
            self.my_id.set(old_id.clone());
            let _ = self.db.migrate_peer_references(&new_id, &old_id).await;
            let _ = self
                .db
                .save_user_profile(
                    &old_id,
                    &profile.username,
                    &profile.department,
                    &crate::profile_metadata::software_version(),
                    &crate::profile_metadata::mac_address(),
                )
                .await;
            return Err(error);
        }

        let payload = serde_json::json!({
            "old_peer_id": old_id,
            "username": profile.username,
            "department": profile.department,
            "software_version": crate::profile_metadata::software_version(),
            "mac_address": crate::profile_metadata::mac_address(),
            "avatar_hash": profile.avatar_hash,
            "avatar_updated_at": profile.avatar_updated_at,
        })
        .to_string();
        let stored = self.db.list_stored_peers().await.unwrap_or_default();
        let mut queued = 0usize;
        for peer in stored {
            if peer.peer_id == old_id || peer.peer_id == new_id {
                continue;
            }
            let json = crate::commands::build_notification_json(
                &new_id,
                &self.my_node_id,
                &profile.username,
                &profile.department,
                self.listen_port,
                &peer.peer_id,
                "",
                &payload,
                "profile_updated",
                None,
                None,
                None,
                &[],
            );
            if self
                .db
                .queue_pending_notification(&peer.peer_id, "profile_updated", &json)
                .await
                .is_ok()
            {
                queued += 1;
            }
        }
        log::info!(
            "Queued runtime peer_id change {} -> {} for {} peers",
            old_id,
            new_id,
            queued
        );
        Ok(Some((old_id, new_id)))
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
        self.chat.lock().await.update_identity(username, department);
        Ok(())
    }
}

pub struct AppState {
    pub db: Arc<Database>,
    pub profile: Mutex<Option<UserProfile>>,
    // Use RwLock for runtime: multiple readers (clone Arc handle), single writer (initialization)
    pub runtime: RwLock<Option<Arc<RuntimeServices>>>,
    /// Channel to forward UDP-relayed peers to the async contact-sync processor.
    pub relay_tx: Option<mpsc::UnboundedSender<Vec<PeerEntry>>>,
}
