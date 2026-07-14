use anyhow::Result;
use std::{collections::HashSet, sync::Arc};
use tokio::sync::{mpsc, Mutex, RwLock};

use crate::chat::{ChatServer, LocalPeerId};
use crate::db::{AliasBindOutcome, Database, UserProfile};
use crate::discovery::{DiscoveryConfig, DiscoveryService, Peer, PeerEntry};
use tauri::AppHandle;

async fn bind_local_alias_best_effort(db: &Database, node_id: &str, peer_id: &str) -> bool {
    match db.bind_peer_alias_checked(node_id, peer_id).await {
        Ok(AliasBindOutcome::Bound) => true,
        Ok(AliasBindOutcome::Conflict { owner_node_id }) => {
            log::warn!(
                "Local endpoint {} conflicts with historical node {}; keeping ownership isolated",
                peer_id,
                owner_node_id
            );
            false
        }
        Err(error) => {
            log::warn!(
                "Failed to record local endpoint alias {} for node {}: {}",
                peer_id,
                node_id,
                error
            );
            false
        }
    }
}

fn peer_id_change_targets<'a>(
    stored_peer_ids: impl IntoIterator<Item = &'a str>,
    online_peers: &[Peer],
    old_id: &str,
    new_id: &str,
) -> Vec<String> {
    let mut targets: HashSet<String> = stored_peer_ids.into_iter().map(str::to_string).collect();
    targets.extend(
        online_peers
            .iter()
            .filter(|peer| peer.online)
            .map(|peer| peer.id.clone()),
    );
    targets.remove(old_id);
    targets.remove(new_id);

    let mut targets: Vec<String> = targets.into_iter().collect();
    targets.sort();
    targets
}

#[allow(clippy::too_many_arguments)]
async fn send_peer_id_change_notification(
    db: &Database,
    online_peers: &[Peer],
    old_id: &str,
    new_id: &str,
    node_id: &str,
    username: &str,
    department: &str,
    listen_port: u16,
    avatar_hash: &str,
    avatar_updated_at: i64,
) {
    let stored = db.list_stored_peers().await.unwrap_or_default();
    let targets = peer_id_change_targets(
        stored.iter().map(|peer| peer.peer_id.as_str()),
        online_peers,
        old_id,
        new_id,
    );
    let payload = serde_json::json!({
        "old_peer_id": old_id,
        "node_id": node_id,
        "username": username,
        "department": department,
        "software_version": crate::profile_metadata::software_version(),
        "mac_address": crate::profile_metadata::mac_address(),
        "avatar_hash": avatar_hash,
        "avatar_updated_at": avatar_updated_at,
    })
    .to_string();

    let (delivered, queued, failed) = crate::commands::send_or_queue_notification(
        db,
        online_peers,
        &targets,
        new_id,
        node_id,
        username,
        department,
        listen_port,
        &payload,
        "profile_updated",
        None,
        None,
        None,
        &[],
    )
    .await;
    log::info!(
        "Published peer_id change {} -> {} to {} peers (delivered={}, queued={}, failed={})",
        old_id,
        new_id,
        targets.len(),
        delivered,
        queued,
        failed
    );
}

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
            let _ = bind_local_alias_best_effort(db.as_ref(), &my_node_id, &old_peer_id).await;
            let _ = bind_local_alias_best_effort(db.as_ref(), &my_node_id, &my_id).await;
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
        let notification_peers = Arc::clone(&peers);
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
                let online_peers = notification_peers
                    .read()
                    .map(|peers| peers.values().cloned().collect::<Vec<_>>())
                    .unwrap_or_default();
                send_peer_id_change_notification(
                    db_clone.as_ref(),
                    &online_peers,
                    &old_id,
                    &new_id,
                    &node_id,
                    &username,
                    &department,
                    listen_port,
                    &avatar_hash,
                    avatar_updated_at,
                )
                .await;
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
        let old_alias_bound =
            bind_local_alias_best_effort(self.db.as_ref(), &self.my_node_id, &old_id).await;
        let new_alias_bound =
            bind_local_alias_best_effort(self.db.as_ref(), &self.my_node_id, &new_id).await;
        let references_migrated = old_alias_bound && new_alias_bound;
        if references_migrated {
            self.db.migrate_peer_references(&old_id, &new_id).await?;
        } else {
            log::warn!(
                "Skipping local reference migration {} -> {} because endpoint ownership is conflicting",
                old_id,
                new_id
            );
        }
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
            if references_migrated {
                let _ = self.db.migrate_peer_references(&new_id, &old_id).await;
            }
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

        let online_peers = self.discovery.read().await.get_peers();
        send_peer_id_change_notification(
            self.db.as_ref(),
            &online_peers,
            &old_id,
            &new_id,
            &self.my_node_id,
            &profile.username,
            &profile.department,
            self.listen_port,
            &profile.avatar_hash,
            profile.avatar_updated_at,
        )
        .await;
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

#[cfg(test)]
mod tests {
    use super::{peer_id_change_targets, Peer};

    fn peer(id: &str, online: bool) -> Peer {
        let mut peer = Peer::new(
            id.to_string(),
            id.to_string(),
            "研发部".to_string(),
            "127.0.0.1".parse().unwrap(),
            9527,
        );
        peer.online = online;
        peer
    }

    #[test]
    fn peer_id_change_targets_merge_dedupe_and_exclude_self() {
        let stored = ["stored", "overlap", "old-self", "new-self"];
        let online = vec![
            peer("online-only", true),
            peer("overlap", true),
            peer("offline-only", false),
            peer("old-self", true),
            peer("new-self", true),
        ];

        let targets = peer_id_change_targets(stored, &online, "old-self", "new-self");

        assert_eq!(
            targets,
            vec![
                "online-only".to_string(),
                "overlap".to_string(),
                "stored".to_string(),
            ]
        );
    }
}
