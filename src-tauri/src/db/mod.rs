use anyhow::{Context, Result};
use chrono::Utc;
use log::info;
use serde::{Deserialize, Serialize};
use sqlx::{
    sqlite::{SqlitePoolOptions, SqliteRow},
    Pool, Row, Sqlite,
};
use std::net::IpAddr;

use crate::contact_filter;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserProfile {
    pub peer_id: String,
    #[serde(default)]
    pub node_id: String,
    pub username: String,
    pub department: String,
    pub software_version: String,
    pub mac_address: String,
    #[serde(default)]
    pub avatar_path: String,
    #[serde(default)]
    pub avatar_hash: String,
    #[serde(default)]
    pub avatar_updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingGroupMsg {
    pub id: i64,
    pub group_id: String,
    pub peer_id: String,
    pub sender_id: String,
    pub sender_name: String,
    pub content: String,
    pub msg_type: String,
    pub original_timestamp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingFileTransfer {
    pub id: i64,
    pub group_id: String,
    pub peer_id: String,
    pub sender_id: String,
    pub sender_name: String,
    pub sender_department: String,
    pub sender_port: u16,
    pub file_path: String,
    pub file_name: String,
    pub file_size: i64,
    pub file_kind: String,
    #[serde(default)]
    pub client_msg_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupInfo {
    pub group_id: String,
    pub name: String,
    pub creator_id: String,
    pub created_at: String,
    pub members: Vec<StoredPeer>,
    #[serde(default)]
    pub last_message: Option<String>,
    #[serde(default)]
    pub last_message_at: Option<String>,
    #[serde(default)]
    pub last_message_sender: Option<String>,
    #[serde(default)]
    pub unread_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupUnread {
    pub group_id: String,
    pub count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredPeer {
    pub peer_id: String,
    #[serde(default)]
    pub node_id: String,
    pub username: String,
    pub department: String,
    pub software_version: String,
    pub mac_address: String,
    #[serde(default)]
    pub avatar_path: String,
    #[serde(default)]
    pub avatar_hash: String,
    #[serde(default)]
    pub avatar_updated_at: i64,
    pub ip: String,
    pub port: u16,
    pub is_online: bool,
    pub first_seen_at: String,
    pub last_seen_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnreadCount {
    pub peer_id: String,
    pub count: u32,
    pub username: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: i64,
    pub sender_id: String,
    pub sender_name: String,
    pub receiver_id: String,
    pub content: String,
    pub msg_type: String, // "text", "file", "image"
    pub file_path: Option<String>,
    pub file_name: Option<String>,
    pub file_size: Option<i64>,
    pub timestamp: String,
    pub is_read: bool,
    #[serde(default)]
    pub client_msg_id: Option<String>,
}

pub struct Database {
    pub(crate) pool: Pool<Sqlite>,
}

const DEFAULT_MESSAGE_LIMIT: i64 = 500;
const MAX_MESSAGE_LIMIT: i64 = 1000;
const DEFAULT_SEARCH_LIMIT: i64 = 200;
const MAX_SEARCH_LIMIT: i64 = 500;

fn normalize_message_limit(limit: Option<i64>) -> i64 {
    limit
        .unwrap_or(DEFAULT_MESSAGE_LIMIT)
        .max(1)
        .min(MAX_MESSAGE_LIMIT)
}

fn normalize_search_limit(limit: Option<i64>) -> i64 {
    limit
        .unwrap_or(DEFAULT_SEARCH_LIMIT)
        .max(1)
        .min(MAX_SEARCH_LIMIT)
}

fn endpoint_peer_id(ip: &str, port: u16) -> String {
    format!("{}:{}", ip.trim(), port)
}

fn canonicalize_endpoint_peer_id(peer_id: &str, ip: &str, port: u16) -> String {
    let trimmed = peer_id.trim();
    if is_endpoint_peer_id(trimmed) {
        endpoint_peer_id(ip, port)
    } else {
        trimmed.to_string()
    }
}

fn is_endpoint_peer_id(peer_id: &str) -> bool {
    let Some((host, port_text)) = peer_id.rsplit_once(':') else {
        return false;
    };
    if host.trim().parse::<IpAddr>().is_err() {
        return false;
    }
    port_text.parse::<u16>().is_ok()
}

fn rewrite_peer_ids_in_json(
    value: &mut serde_json::Value,
    old_peer_id: &str,
    new_peer_id: &str,
    changed: &mut bool,
) {
    match value {
        serde_json::Value::String(text) => {
            if text == old_peer_id {
                *text = new_peer_id.to_string();
                *changed = true;
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                rewrite_peer_ids_in_json(item, old_peer_id, new_peer_id, changed);
            }
        }
        serde_json::Value::Object(map) => {
            for value in map.values_mut() {
                rewrite_peer_ids_in_json(value, old_peer_id, new_peer_id, changed);
            }
        }
        _ => {}
    }
}

fn message_filter_clause(filter: Option<&str>) -> &'static str {
    match filter {
        Some("file") => "AND msg_type = 'file'",
        Some("image") => {
            "AND (msg_type = 'sticker' OR (msg_type = 'file' AND (
                lower(COALESCE(file_name, '')) LIKE '%.png'
                OR lower(COALESCE(file_name, '')) LIKE '%.jpg'
                OR lower(COALESCE(file_name, '')) LIKE '%.jpeg'
                OR lower(COALESCE(file_name, '')) LIKE '%.gif'
                OR lower(COALESCE(file_name, '')) LIKE '%.webp'
                OR lower(COALESCE(file_name, '')) LIKE '%.bmp'
                OR lower(COALESCE(file_name, '')) LIKE '%.svg'
                OR lower(COALESCE(file_name, '')) LIKE '%.ico'
                OR lower(COALESCE(file_name, '')) LIKE '%.tiff'
            )))"
        }
        _ => "",
    }
}

fn chat_message_from_row(row: &SqliteRow) -> ChatMessage {
    ChatMessage {
        id: row.get("id"),
        sender_id: row.get("sender_id"),
        sender_name: row.get("sender_name"),
        receiver_id: row.get("receiver_id"),
        content: row.get("content"),
        msg_type: row.get("msg_type"),
        file_path: row.get("file_path"),
        file_name: row.get("file_name"),
        file_size: row.get("file_size"),
        timestamp: row.get("timestamp"),
        is_read: row.get::<bool, _>("is_read"),
        client_msg_id: row.try_get("client_msg_id").ok(),
    }
}

impl Database {
    pub async fn new(db_path: &str) -> Result<Self> {
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(&format!("sqlite:{}?mode=rwc", db_path))
            .await
            .context("Failed to connect to SQLite database")?;

        let db = Self { pool };
        db.init_tables().await?;
        Ok(db)
    }

    async fn init_tables(&self) -> Result<()> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS user_profile (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                peer_id TEXT,
                node_id TEXT NOT NULL DEFAULT '',
                username TEXT NOT NULL,
                department TEXT NOT NULL,
                software_version TEXT NOT NULL DEFAULT '',
                mac_address TEXT NOT NULL DEFAULT '',
                avatar_path TEXT NOT NULL DEFAULT '',
                avatar_hash TEXT NOT NULL DEFAULT '',
                avatar_updated_at INTEGER NOT NULL DEFAULT 0
            )",
        )
        .execute(&self.pool)
        .await
        .context("Failed to create user_profile table")?;

        if let Err(error) = sqlx::query("ALTER TABLE user_profile ADD COLUMN peer_id TEXT")
            .execute(&self.pool)
            .await
        {
            let message = error.to_string();
            if !message.contains("duplicate column name") {
                return Err(error).context("Failed to add peer_id column to user_profile");
            }
        }

        if let Err(error) =
            sqlx::query("ALTER TABLE user_profile ADD COLUMN node_id TEXT NOT NULL DEFAULT ''")
                .execute(&self.pool)
                .await
        {
            let message = error.to_string();
            if !message.contains("duplicate column name") {
                return Err(error).context("Failed to add node_id column to user_profile");
            }
        }

        if let Err(error) =
            sqlx::query("ALTER TABLE user_profile ADD COLUMN scan_subnets TEXT NOT NULL DEFAULT ''")
                .execute(&self.pool)
                .await
        {
            let message = error.to_string();
            if !message.contains("duplicate column name") {
                return Err(error).context("Failed to add scan_subnets column to user_profile");
            }
        }

        if let Err(error) = sqlx::query(
            "ALTER TABLE user_profile ADD COLUMN software_version TEXT NOT NULL DEFAULT ''",
        )
        .execute(&self.pool)
        .await
        {
            let message = error.to_string();
            if !message.contains("duplicate column name") {
                return Err(error).context("Failed to add software_version column to user_profile");
            }
        }

        if let Err(error) =
            sqlx::query("ALTER TABLE user_profile ADD COLUMN mac_address TEXT NOT NULL DEFAULT ''")
                .execute(&self.pool)
                .await
        {
            let message = error.to_string();
            if !message.contains("duplicate column name") {
                return Err(error).context("Failed to add mac_address column to user_profile");
            }
        }

        if let Err(error) =
            sqlx::query("ALTER TABLE user_profile ADD COLUMN avatar_path TEXT NOT NULL DEFAULT ''")
                .execute(&self.pool)
                .await
        {
            let message = error.to_string();
            if !message.contains("duplicate column name") {
                return Err(error).context("Failed to add avatar_path column to user_profile");
            }
        }

        if let Err(error) =
            sqlx::query("ALTER TABLE user_profile ADD COLUMN avatar_hash TEXT NOT NULL DEFAULT ''")
                .execute(&self.pool)
                .await
        {
            let message = error.to_string();
            if !message.contains("duplicate column name") {
                return Err(error).context("Failed to add avatar_hash column to user_profile");
            }
        }

        if let Err(error) = sqlx::query(
            "ALTER TABLE user_profile ADD COLUMN avatar_updated_at INTEGER NOT NULL DEFAULT 0",
        )
        .execute(&self.pool)
        .await
        {
            let message = error.to_string();
            if !message.contains("duplicate column name") {
                return Err(error)
                    .context("Failed to add avatar_updated_at column to user_profile");
            }
        }

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS peers (
                peer_id TEXT PRIMARY KEY,
                node_id TEXT NOT NULL DEFAULT '',
                username TEXT NOT NULL,
                department TEXT NOT NULL,
                software_version TEXT NOT NULL DEFAULT '',
                mac_address TEXT NOT NULL DEFAULT '',
                avatar_path TEXT NOT NULL DEFAULT '',
                avatar_hash TEXT NOT NULL DEFAULT '',
                avatar_updated_at INTEGER NOT NULL DEFAULT 0,
                ip TEXT NOT NULL,
                port INTEGER NOT NULL,
                is_online INTEGER NOT NULL DEFAULT 1,
                first_seen_at TEXT NOT NULL,
                last_seen_at TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await
        .context("Failed to create peers table")?;

        if let Err(error) =
            sqlx::query("ALTER TABLE peers ADD COLUMN node_id TEXT NOT NULL DEFAULT ''")
                .execute(&self.pool)
                .await
        {
            let message = error.to_string();
            if !message.contains("duplicate column name") {
                return Err(error).context("Failed to add node_id column to peers");
            }
        }

        if let Err(error) =
            sqlx::query("ALTER TABLE peers ADD COLUMN software_version TEXT NOT NULL DEFAULT ''")
                .execute(&self.pool)
                .await
        {
            let message = error.to_string();
            if !message.contains("duplicate column name") {
                return Err(error).context("Failed to add software_version column to peers");
            }
        }

        if let Err(error) =
            sqlx::query("ALTER TABLE peers ADD COLUMN mac_address TEXT NOT NULL DEFAULT ''")
                .execute(&self.pool)
                .await
        {
            let message = error.to_string();
            if !message.contains("duplicate column name") {
                return Err(error).context("Failed to add mac_address column to peers");
            }
        }

        if let Err(error) =
            sqlx::query("ALTER TABLE peers ADD COLUMN avatar_path TEXT NOT NULL DEFAULT ''")
                .execute(&self.pool)
                .await
        {
            let message = error.to_string();
            if !message.contains("duplicate column name") {
                return Err(error).context("Failed to add avatar_path column to peers");
            }
        }

        if let Err(error) =
            sqlx::query("ALTER TABLE peers ADD COLUMN avatar_hash TEXT NOT NULL DEFAULT ''")
                .execute(&self.pool)
                .await
        {
            let message = error.to_string();
            if !message.contains("duplicate column name") {
                return Err(error).context("Failed to add avatar_hash column to peers");
            }
        }

        if let Err(error) =
            sqlx::query("ALTER TABLE peers ADD COLUMN avatar_updated_at INTEGER NOT NULL DEFAULT 0")
                .execute(&self.pool)
                .await
        {
            let message = error.to_string();
            if !message.contains("duplicate column name") {
                return Err(error).context("Failed to add avatar_updated_at column to peers");
            }
        }

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS peer_aliases (
                alias_peer_id TEXT PRIMARY KEY,
                node_id TEXT NOT NULL,
                created_at TEXT NOT NULL,
                last_seen_at TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await
        .context("Failed to create peer_aliases table")?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_peers_node_id ON peers(node_id)")
            .execute(&self.pool)
            .await
            .context("Failed to create peers node_id index")?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_peer_aliases_node_id ON peer_aliases(node_id)")
            .execute(&self.pool)
            .await
            .context("Failed to create peer_aliases node_id index")?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                sender_id TEXT NOT NULL,
                sender_name TEXT NOT NULL,
                receiver_id TEXT NOT NULL,
                content TEXT NOT NULL,
                msg_type TEXT NOT NULL DEFAULT 'text',
                file_path TEXT,
                file_name TEXT,
                file_size INTEGER,
                timestamp TEXT NOT NULL,
                is_read INTEGER NOT NULL DEFAULT 0
            )",
        )
        .execute(&self.pool)
        .await
        .context("Failed to create messages table")?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_messages_conversation
             ON messages(sender_id, receiver_id)",
        )
        .execute(&self.pool)
        .await
        .context("Failed to create messages index")?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_messages_conversation_recent
             ON messages(sender_id, receiver_id, id DESC)",
        )
        .execute(&self.pool)
        .await
        .context("Failed to create recent conversation messages index")?;

        // Add group_id to messages before any group-message indexes are created.
        // On a fresh database the base CREATE TABLE does not include this column,
        // so creating idx_messages_group_recent first would fail during startup.
        if let Err(error) = sqlx::query("ALTER TABLE messages ADD COLUMN group_id TEXT")
            .execute(&self.pool)
            .await
        {
            let msg = error.to_string();
            if !msg.contains("duplicate column name") {
                return Err(error).context("Failed to add group_id to messages");
            }
        }

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_messages_group_recent
             ON messages(group_id, id DESC)",
        )
        .execute(&self.pool)
        .await
        .context("Failed to create recent group messages index")?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_messages_search
             ON messages(content)",
        )
        .execute(&self.pool)
        .await
        .context("Failed to create messages search index")?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS recent_contacts (
                peer_id TEXT PRIMARY KEY,
                added_at TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await
        .context("Failed to create recent_contacts table")?;

        sqlx::query(
            "DELETE FROM recent_contacts
             WHERE peer_id IN (
                SELECT r.peer_id
                FROM recent_contacts r
                LEFT JOIN peers p ON p.peer_id = r.peer_id
                WHERE p.peer_id IS NULL
                   OR (TRIM(COALESCE(p.username, '')) = ''
                       AND TRIM(COALESCE(p.department, '')) = '')
             )",
        )
        .execute(&self.pool)
        .await
        .context("Failed to clean dirty recent contacts")?;

        sqlx::query(
            "DELETE FROM peers
             WHERE TRIM(COALESCE(username, '')) = ''
               AND TRIM(COALESCE(department, '')) = ''",
        )
        .execute(&self.pool)
        .await
        .context("Failed to clean dirty stored peers")?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS groups (
                group_id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                creator_id TEXT NOT NULL,
                created_at TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await
        .context("Failed to create groups table")?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS group_members (
                group_id TEXT NOT NULL,
                peer_id TEXT NOT NULL,
                joined_at TEXT NOT NULL,
                PRIMARY KEY (group_id, peer_id)
            )",
        )
        .execute(&self.pool)
        .await
        .context("Failed to create group_members table")?;

        // Add client_msg_id to messages if not exists
        if let Err(error) = sqlx::query("ALTER TABLE messages ADD COLUMN client_msg_id TEXT")
            .execute(&self.pool)
            .await
        {
            let msg = error.to_string();
            if !msg.contains("duplicate column name") {
                return Err(error).context("Failed to add client_msg_id to messages");
            }
        }

        // Create index on client_msg_id for fast lookups
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_messages_client_msg_id ON messages(client_msg_id)",
        )
        .execute(&self.pool)
        .await
        .context("Failed to create client_msg_id index")?;

        // Deduplicate existing rows before adding the UNIQUE constraint, otherwise
        // CREATE UNIQUE INDEX fails on databases upgraded from versions that allowed
        // duplicate (sender_id, group_id, client_msg_id) rows. Keep the earliest row
        // (MIN(id)) per dedup key. NULL/empty client_msg_id rows are left untouched.
        sqlx::query(
            "DELETE FROM messages
             WHERE client_msg_id IS NOT NULL AND TRIM(client_msg_id) <> ''
               AND id NOT IN (
                 SELECT MIN(id) FROM messages
                 WHERE client_msg_id IS NOT NULL AND TRIM(client_msg_id) <> ''
                 GROUP BY sender_id, COALESCE(group_id, ''), client_msg_id
               )",
        )
        .execute(&self.pool)
        .await
        .context("Failed to dedup messages before unique index")?;

        // Enforce dedup at the storage layer. Partial index skips legacy rows with
        // no client_msg_id (multiple NULLs would otherwise be treated as distinct,
        // and NULL group_id is normalized to '' so private-message keys collide
        // correctly). This is the race backstop behind the SELECT-first dedup path.
        sqlx::query(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_messages_client_dedup
             ON messages(sender_id, COALESCE(group_id, ''), client_msg_id)
             WHERE client_msg_id IS NOT NULL AND TRIM(client_msg_id) <> ''",
        )
        .execute(&self.pool)
        .await
        .context("Failed to create unique client_msg_id dedup index")?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS pending_group_messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                group_id TEXT NOT NULL,
                peer_id TEXT NOT NULL,
                sender_id TEXT NOT NULL,
                sender_name TEXT NOT NULL,
                content TEXT NOT NULL,
                msg_type TEXT NOT NULL DEFAULT 'text',
                original_timestamp TEXT NOT NULL,
                created_at TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await
        .context("Failed to create pending_group_messages table")?;

        // Generic offline-delivery queue. `payload` is a full WireMessage JSON
        // (any msg_type). On the receiver, the same TCP handler dispatches by
        // msg_type, so we don't need a per-kind table.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS pending_notifications (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                peer_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                payload TEXT NOT NULL,
                created_at TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await
        .context("Failed to create pending_notifications table")?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_pending_notif_peer ON pending_notifications(peer_id)",
        )
        .execute(&self.pool)
        .await
        .ok();

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS pending_file_transfers (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                group_id TEXT NOT NULL,
                peer_id TEXT NOT NULL,
                sender_id TEXT NOT NULL,
                sender_name TEXT NOT NULL,
                sender_department TEXT NOT NULL,
                sender_port INTEGER NOT NULL,
                file_path TEXT NOT NULL,
                file_name TEXT NOT NULL,
                file_size INTEGER NOT NULL,
                file_kind TEXT NOT NULL DEFAULT 'file',
                client_msg_id TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await
        .context("Failed to create pending_file_transfers table")?;

        if let Err(error) = sqlx::query(
            "ALTER TABLE pending_file_transfers ADD COLUMN file_kind TEXT NOT NULL DEFAULT 'file'",
        )
        .execute(&self.pool)
        .await
        {
            let msg = error.to_string();
            if !msg.contains("duplicate column name") {
                return Err(error).context("Failed to add file_kind to pending_file_transfers");
            }
        }

        if let Err(error) = sqlx::query(
            "ALTER TABLE pending_file_transfers ADD COLUMN client_msg_id TEXT NOT NULL DEFAULT ''",
        )
        .execute(&self.pool)
        .await
        {
            let msg = error.to_string();
            if !msg.contains("duplicate column name") {
                return Err(error).context("Failed to add client_msg_id to pending_file_transfers");
            }
        }

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_pending_file_peer ON pending_file_transfers(peer_id)",
        )
        .execute(&self.pool)
        .await
        .ok();

        self.normalize_legacy_endpoint_peer_ids().await?;
        self.clean_duplicate_peer_endpoints().await?;

        info!("Database initialized successfully.");
        Ok(())
    }

    async fn normalize_legacy_endpoint_peer_ids(&self) -> Result<()> {
        let rows = sqlx::query("SELECT peer_id, ip, port FROM peers")
            .fetch_all(&self.pool)
            .await
            .context("Failed to load peers for endpoint normalization")?;

        for row in rows {
            let old_peer_id: String = row.get("peer_id");
            let ip: String = row.get("ip");
            let port_i64: i64 = row.get("port");
            let Ok(port) = u16::try_from(port_i64) else {
                continue;
            };
            if !contact_filter::has_valid_endpoint(&ip, port) {
                continue;
            }

            let new_peer_id = canonicalize_endpoint_peer_id(&old_peer_id, &ip, port);
            if old_peer_id != new_peer_id {
                self.migrate_legacy_endpoint_peer(&old_peer_id, &new_peer_id)
                    .await?;
            }
        }

        Ok(())
    }

    async fn clean_duplicate_peer_endpoints(&self) -> Result<()> {
        let rows = sqlx::query(
            "SELECT stale.peer_id AS old_peer_id, keep.peer_id AS new_peer_id
             FROM peers stale
             JOIN (
                SELECT ip, port, MAX(rowid) AS keep_rowid
                FROM peers
                GROUP BY ip, port
                HAVING COUNT(*) > 1
             ) endpoint_keep
                ON stale.ip = endpoint_keep.ip
               AND stale.port = endpoint_keep.port
               AND stale.rowid <> endpoint_keep.keep_rowid
             JOIN peers keep ON keep.rowid = endpoint_keep.keep_rowid",
        )
        .fetch_all(&self.pool)
        .await
        .context("Failed to load duplicate peer endpoints")?;

        for row in rows {
            let old_peer_id: String = row.get("old_peer_id");
            let new_peer_id: String = row.get("new_peer_id");
            self.migrate_peer_references(&old_peer_id, &new_peer_id)
                .await?;
        }

        sqlx::query(
            "DELETE FROM peers
             WHERE rowid NOT IN (
                SELECT MAX(rowid) FROM peers GROUP BY ip, port
             )",
        )
        .execute(&self.pool)
        .await
        .context("Failed to clean duplicate peer endpoints")?;

        Ok(())
    }

    pub async fn add_recent_contact(&self, peer_id: &str) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        log::info!("add_recent_contact: {}", peer_id);
        sqlx::query("INSERT INTO recent_contacts (peer_id, added_at) VALUES (?, ?) ON CONFLICT(peer_id) DO UPDATE SET added_at = excluded.added_at")
            .bind(peer_id).bind(&now)
            .execute(&self.pool).await
            .context("Failed to add recent contact")?;
        Ok(())
    }

    pub async fn list_recent_contacts(&self) -> Result<Vec<StoredPeer>> {
        let rows = sqlx::query(
            "SELECT r.peer_id, p.node_id as node_id, p.username as username,
                    p.department as department,
                    p.software_version as software_version,
                    p.mac_address as mac_address,
                    p.avatar_path as avatar_path,
                    p.avatar_hash as avatar_hash,
                    p.avatar_updated_at as avatar_updated_at,
                    p.ip as ip, p.port as port,
                    p.is_online as is_online,
                    p.first_seen_at as first_seen_at,
                    p.last_seen_at as last_seen_at,
                    (
                        SELECT MAX(m.id)
                        FROM messages m
                        WHERE m.group_id IS NULL
                          AND (m.sender_id = r.peer_id OR m.receiver_id = r.peer_id)
                          AND m.msg_type NOT IN ('file_chunk', 'file_end')
                    ) as last_message_id
             FROM recent_contacts r
             JOIN peers p ON r.peer_id = p.peer_id
             WHERE TRIM(p.username) <> '' OR TRIM(p.department) <> ''
             ORDER BY
                CASE WHEN last_message_id IS NULL THEN 1 ELSE 0 END,
                last_message_id DESC,
                r.added_at DESC",
        )
        .fetch_all(&self.pool)
        .await
        .context("Failed to list recent contacts")?;

        Ok(rows
            .iter()
            .map(|r| StoredPeer {
                peer_id: r.get("peer_id"),
                node_id: r.try_get("node_id").unwrap_or_default(),
                username: r.get("username"),
                department: r.get("department"),
                software_version: r.get("software_version"),
                mac_address: r.get("mac_address"),
                avatar_path: r.get("avatar_path"),
                avatar_hash: r.get("avatar_hash"),
                avatar_updated_at: r.get("avatar_updated_at"),
                ip: r.get("ip"),
                port: r.get::<i64, _>("port") as u16,
                is_online: r.get::<bool, _>("is_online"),
                first_seen_at: r.get("first_seen_at"),
                last_seen_at: r.get("last_seen_at"),
            })
            .collect())
    }

    pub async fn remove_recent_contact(&self, peer_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM recent_contacts WHERE peer_id = ?")
            .bind(peer_id)
            .execute(&self.pool)
            .await
            .context("Failed to remove recent contact")?;
        Ok(())
    }

    pub async fn delete_messages(&self, message_ids: &[i64]) -> Result<u64> {
        let mut deleted = 0;
        for message_id in message_ids
            .iter()
            .copied()
            .filter(|message_id| *message_id > 0)
        {
            let result = sqlx::query("DELETE FROM messages WHERE id = ?")
                .bind(message_id)
                .execute(&self.pool)
                .await
                .context("Failed to delete message")?;
            deleted += result.rows_affected();
        }
        Ok(deleted)
    }

    pub async fn get_user_profile(&self) -> Result<Option<UserProfile>> {
        let row = sqlx::query("SELECT peer_id, node_id, username, department, software_version, mac_address, avatar_path, avatar_hash, avatar_updated_at FROM user_profile WHERE id = 1")
            .fetch_optional(&self.pool)
            .await
            .context("Failed to load user profile")?;

        Ok(row.map(|row| UserProfile {
            peer_id: row
                .try_get::<Option<String>, _>("peer_id")
                .ok()
                .flatten()
                .unwrap_or_default(),
            node_id: row
                .try_get::<Option<String>, _>("node_id")
                .ok()
                .flatten()
                .unwrap_or_default(),
            username: row.get("username"),
            department: row.get("department"),
            software_version: row.try_get("software_version").unwrap_or_default(),
            mac_address: row.try_get("mac_address").unwrap_or_default(),
            avatar_path: row.try_get("avatar_path").unwrap_or_default(),
            avatar_hash: row.try_get("avatar_hash").unwrap_or_default(),
            avatar_updated_at: row.try_get("avatar_updated_at").unwrap_or_default(),
        }))
    }

    pub async fn ensure_user_node_id(&self) -> Result<String> {
        let existing = sqlx::query("SELECT node_id FROM user_profile WHERE id = 1")
            .fetch_optional(&self.pool)
            .await
            .context("Failed to load existing user_profile node_id")?;

        if let Some(row) = existing {
            let node_id = row
                .try_get::<Option<String>, _>("node_id")
                .unwrap_or_default()
                .unwrap_or_default();
            let node_id = node_id.trim();
            if !node_id.is_empty() {
                return Ok(node_id.to_string());
            }
        }

        let node_id = format!("node_{}", uuid::Uuid::new_v4());
        sqlx::query("UPDATE user_profile SET node_id = ? WHERE id = 1")
            .bind(&node_id)
            .execute(&self.pool)
            .await
            .context("Failed to persist generated user_profile node_id")?;
        Ok(node_id)
    }

    pub async fn save_user_profile(
        &self,
        peer_id: &str,
        username: &str,
        department: &str,
        software_version: &str,
        mac_address: &str,
    ) -> Result<()> {
        let peer_id = peer_id.trim();
        let mut existing_node_id = String::new();
        if !peer_id.is_empty() {
            let existing = sqlx::query("SELECT peer_id, node_id FROM user_profile WHERE id = 1")
                .fetch_optional(&self.pool)
                .await
                .context("Failed to load existing user profile identity")?;
            if let Some(row) = existing {
                let old_peer_id = row
                    .try_get::<Option<String>, _>("peer_id")
                    .unwrap_or_default()
                    .unwrap_or_default();
                existing_node_id = row
                    .try_get::<Option<String>, _>("node_id")
                    .unwrap_or_default()
                    .unwrap_or_default();
                let old_peer_id = old_peer_id.trim();
                let node_id = existing_node_id.trim();
                if !old_peer_id.is_empty()
                    && old_peer_id != peer_id
                    && is_endpoint_peer_id(old_peer_id)
                    && is_endpoint_peer_id(peer_id)
                {
                    if node_id.is_empty() {
                        self.migrate_self_endpoint_peer(old_peer_id, peer_id)
                            .await?;
                    } else {
                        self.upsert_peer_alias(node_id, old_peer_id).await?;
                        self.upsert_peer_alias(node_id, peer_id).await?;
                    }
                }
            }
        }

        sqlx::query(
            "INSERT INTO user_profile (id, peer_id, node_id, username, department, software_version, mac_address)
             VALUES (1, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                peer_id = excluded.peer_id,
                node_id = CASE WHEN user_profile.node_id = '' THEN excluded.node_id ELSE user_profile.node_id END,
                username = excluded.username,
                department = excluded.department,
                software_version = excluded.software_version,
                mac_address = excluded.mac_address",
        )
        .bind(peer_id)
        .bind(existing_node_id.trim())
        .bind(username)
        .bind(department)
        .bind(software_version)
        .bind(mac_address)
        .execute(&self.pool)
        .await
        .context("Failed to save user profile")?;

        Ok(())
    }

    pub async fn update_user_avatar(
        &self,
        avatar_path: &str,
        avatar_hash: &str,
        avatar_updated_at: i64,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE user_profile
             SET avatar_path = ?, avatar_hash = ?, avatar_updated_at = ?
             WHERE id = 1",
        )
        .bind(avatar_path)
        .bind(avatar_hash)
        .bind(avatar_updated_at)
        .execute(&self.pool)
        .await
        .context("Failed to update user avatar")?;
        Ok(())
    }

    pub async fn get_scan_subnets(&self) -> Result<Vec<String>> {
        let row = sqlx::query("SELECT scan_subnets FROM user_profile WHERE id = 1")
            .fetch_optional(&self.pool)
            .await
            .context("Failed to load scan subnets")?;

        let raw: String = row
            .and_then(|r| r.try_get("scan_subnets").ok())
            .unwrap_or_default();

        Ok(raw
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect())
    }

    pub async fn save_scan_subnets(&self, subnets: &str) -> Result<()> {
        sqlx::query("UPDATE user_profile SET scan_subnets = ? WHERE id = 1")
            .bind(subnets)
            .execute(&self.pool)
            .await
            .context("Failed to save scan subnets")?;
        Ok(())
    }

    // ── Pending group message delivery ──

    // Kept for older queued-group-message migration paths; current delivery uses the
    // generic pending notification queue below.
    #[allow(dead_code)]
    pub async fn store_pending_group_msg(
        &self,
        group_id: &str,
        peer_id: &str,
        sender_id: &str,
        sender_name: &str,
        content: &str,
        msg_type: &str,
        timestamp: &str,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query("INSERT INTO pending_group_messages (group_id, peer_id, sender_id, sender_name, content, msg_type, original_timestamp, created_at) VALUES (?,?,?,?,?,?,?,?)")
            .bind(group_id).bind(peer_id).bind(sender_id).bind(sender_name)
            .bind(content).bind(msg_type).bind(timestamp).bind(&now)
            .execute(&self.pool).await.context("Failed to store pending group msg")?;
        Ok(())
    }

    pub async fn get_pending_for_peer(&self, peer_id: &str) -> Result<Vec<PendingGroupMsg>> {
        let rows = sqlx::query("SELECT id, group_id, peer_id, sender_id, sender_name, content, msg_type, original_timestamp FROM pending_group_messages WHERE peer_id = ? ORDER BY id ASC")
            .bind(peer_id).fetch_all(&self.pool).await.context("Failed to get pending msgs")?;
        Ok(rows
            .iter()
            .map(|r| PendingGroupMsg {
                id: r.get("id"),
                group_id: r.get("group_id"),
                peer_id: r.get("peer_id"),
                sender_id: r.get("sender_id"),
                sender_name: r.get("sender_name"),
                content: r.get("content"),
                msg_type: r.get("msg_type"),
                original_timestamp: r.get("original_timestamp"),
            })
            .collect())
    }

    pub async fn delete_pending_msgs(&self, ids: &[i64]) -> Result<()> {
        for id in ids {
            sqlx::query("DELETE FROM pending_group_messages WHERE id = ?")
                .bind(id)
                .execute(&self.pool)
                .await
                .ok();
        }
        Ok(())
    }

    // ── Generic offline-notification queue ──
    //
    // Any wire-protocol message that needs to be delivered to a peer who's
    // currently unreachable can be queued here. The receiver's normal TCP
    // handler dispatches by `msg_type` inside the payload, so a single table
    // covers private messages, group control messages, profile updates, etc.

    pub async fn queue_pending_notification(
        &self,
        peer_id: &str,
        kind: &str,
        payload: &str,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        // Dedupe profile_updated — only one queued copy per peer makes sense.
        if kind == "profile_updated" {
            sqlx::query(
                "DELETE FROM pending_notifications WHERE peer_id = ? AND kind = 'profile_updated'",
            )
            .bind(peer_id)
            .execute(&self.pool)
            .await
            .ok();
        }
        sqlx::query("INSERT INTO pending_notifications (peer_id, kind, payload, created_at) VALUES (?, ?, ?, ?)")
            .bind(peer_id).bind(kind).bind(payload).bind(&now)
            .execute(&self.pool).await.context("Failed to queue pending notification")?;
        Ok(())
    }

    pub async fn get_pending_notifications(
        &self,
        peer_id: &str,
    ) -> Result<Vec<(i64, String, String)>> {
        let rows = sqlx::query(
            "SELECT id, kind, payload FROM pending_notifications WHERE peer_id = ? ORDER BY id ASC",
        )
        .bind(peer_id)
        .fetch_all(&self.pool)
        .await
        .context("Failed to load pending notifications")?;
        Ok(rows
            .iter()
            .map(|r| {
                (
                    r.get::<i64, _>("id"),
                    r.get::<String, _>("kind"),
                    r.get::<String, _>("payload"),
                )
            })
            .collect())
    }

    pub async fn delete_pending_notifications(&self, ids: &[i64]) -> Result<()> {
        for id in ids {
            sqlx::query("DELETE FROM pending_notifications WHERE id = ?")
                .bind(id)
                .execute(&self.pool)
                .await
                .ok();
        }
        Ok(())
    }

    // ── Group operations ──

    pub async fn queue_pending_file_transfer(
        &self,
        group_id: &str,
        peer_id: &str,
        sender_id: &str,
        sender_name: &str,
        sender_department: &str,
        sender_port: u16,
        file_path: &str,
        file_name: &str,
        file_size: i64,
        file_kind: &str,
        client_msg_id: Option<&str>,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO pending_file_transfers
             (group_id, peer_id, sender_id, sender_name, sender_department, sender_port, file_path, file_name, file_size, file_kind, client_msg_id, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(group_id)
        .bind(peer_id)
        .bind(sender_id)
        .bind(sender_name)
        .bind(sender_department)
        .bind(sender_port as i64)
        .bind(file_path)
        .bind(file_name)
        .bind(file_size)
        .bind(file_kind)
        .bind(client_msg_id.unwrap_or_default())
        .bind(&now)
        .execute(&self.pool).await
        .context("Failed to queue pending file transfer")?;
        Ok(())
    }

    pub async fn get_pending_file_transfers(
        &self,
        peer_id: &str,
    ) -> Result<Vec<PendingFileTransfer>> {
        let rows = sqlx::query(
            "SELECT id, group_id, peer_id, sender_id, sender_name, sender_department, sender_port, file_path, file_name, file_size, file_kind, client_msg_id
             FROM pending_file_transfers WHERE peer_id = ? ORDER BY id ASC",
        )
        .bind(peer_id).fetch_all(&self.pool).await
        .context("Failed to load pending file transfers")?;

        Ok(rows
            .iter()
            .map(|r| PendingFileTransfer {
                id: r.get("id"),
                group_id: r.get("group_id"),
                peer_id: r.get("peer_id"),
                sender_id: r.get("sender_id"),
                sender_name: r.get("sender_name"),
                sender_department: r.get("sender_department"),
                sender_port: r.get::<i64, _>("sender_port") as u16,
                file_path: r.get("file_path"),
                file_name: r.get("file_name"),
                file_size: r.get("file_size"),
                file_kind: r.get("file_kind"),
                client_msg_id: r.try_get("client_msg_id").unwrap_or_default(),
            })
            .collect())
    }

    pub async fn delete_pending_file_transfer(&self, id: i64) -> Result<()> {
        sqlx::query("DELETE FROM pending_file_transfers WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await
            .context("Failed to delete pending file transfer")?;
        Ok(())
    }

    pub async fn count_pending_file_transfers_by_path(&self, file_path: &str) -> Result<i64> {
        let row =
            sqlx::query("SELECT COUNT(*) AS count FROM pending_file_transfers WHERE file_path = ?")
                .bind(file_path)
                .fetch_one(&self.pool)
                .await
                .context("Failed to count pending file transfers")?;
        Ok(row.get("count"))
    }

    pub async fn create_group(
        &self,
        group_id: &str,
        name: &str,
        creator_id: &str,
        member_ids: &[String],
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query("INSERT OR IGNORE INTO groups (group_id, name, creator_id, created_at) VALUES (?, ?, ?, ?)")
            .bind(group_id).bind(name).bind(creator_id).bind(&now)
            .execute(&self.pool).await.context("Failed to create group")?;
        for mid in member_ids {
            sqlx::query("INSERT OR IGNORE INTO group_members (group_id, peer_id, joined_at) VALUES (?, ?, ?)")
                .bind(group_id).bind(mid).bind(&now)
                .execute(&self.pool).await.ok();
        }
        Ok(())
    }

    pub async fn list_groups(&self, my_id: &str) -> Result<Vec<GroupInfo>> {
        let rows = sqlx::query(
            "SELECT g.group_id, g.name, g.creator_id, g.created_at FROM groups g
             INNER JOIN group_members gm ON g.group_id = gm.group_id WHERE gm.peer_id = ?",
        )
        .bind(my_id)
        .fetch_all(&self.pool)
        .await
        .context("Failed to list groups")?;
        let mut out = Vec::with_capacity(rows.len());
        for r in &rows {
            let group_id: String = r.get("group_id");
            // Latest message in this group
            let last_row = sqlx::query(
                "SELECT content, msg_type, file_name, sender_name, timestamp FROM messages
                 WHERE group_id = ? AND msg_type NOT IN ('file_chunk','file_end')
                 ORDER BY id DESC LIMIT 1",
            )
            .bind(&group_id)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten();
            let (last_message, last_message_at, last_message_sender) = if let Some(lr) = last_row {
                let msg_type: String = lr.get("msg_type");
                let preview = if msg_type == "file" {
                    let fname: Option<String> = lr.try_get("file_name").ok();
                    format!("📎 {}", fname.unwrap_or_else(|| "文件".to_string()))
                } else if msg_type == "sticker" {
                    "[表情]".to_string()
                } else {
                    lr.get::<String, _>("content")
                };
                (
                    Some(preview),
                    Some(lr.get::<String, _>("timestamp")),
                    Some(lr.get::<String, _>("sender_name")),
                )
            } else {
                (None, None, None)
            };
            // Unread count: messages in group from someone other than me
            let unread: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM messages WHERE group_id = ? AND sender_id <> ? AND is_read = 0"
            ).bind(&group_id).bind(my_id).fetch_one(&self.pool).await.unwrap_or(0);

            out.push(GroupInfo {
                group_id,
                name: r.get("name"),
                creator_id: r.get("creator_id"),
                created_at: r.get("created_at"),
                members: Vec::new(),
                last_message,
                last_message_at,
                last_message_sender,
                unread_count: unread as u32,
            });
        }
        Ok(out)
    }

    pub async fn get_group_members(&self, group_id: &str) -> Result<Vec<StoredPeer>> {
        sqlx::query(
            "INSERT OR IGNORE INTO group_members (group_id, peer_id, joined_at)
             SELECT DISTINCT group_id, sender_id, timestamp
             FROM messages
             WHERE group_id = ? AND sender_id <> ''",
        )
        .bind(group_id)
        .execute(&self.pool)
        .await
        .context("Failed to repair group members from messages")?;

        // Always return gm.peer_id (never NULL) so callers don't see phantom rows.
        // For "myself" we have no row in `peers` — fall back to `user_profile`.
        let rows = sqlx::query(
            "SELECT gm.peer_id AS peer_id,
                    COALESCE(NULLIF(p.node_id, ''), up.node_id, '') AS node_id,
                    COALESCE(NULLIF(p.username, ''), up.username, '') AS username,
                    COALESCE(NULLIF(p.department, ''), up.department, '') AS department,
                    COALESCE(NULLIF(p.software_version, ''), up.software_version, '') AS software_version,
                    COALESCE(NULLIF(p.mac_address, ''), up.mac_address, '') AS mac_address,
                    COALESCE(NULLIF(p.avatar_path, ''), up.avatar_path, '') AS avatar_path,
                    COALESCE(NULLIF(p.avatar_hash, ''), up.avatar_hash, '') AS avatar_hash,
                    COALESCE(NULLIF(p.avatar_updated_at, 0), up.avatar_updated_at, 0) AS avatar_updated_at,
                    COALESCE(p.ip, '') AS ip,
                    COALESCE(p.port, 0) AS port,
                    COALESCE(p.is_online, 0) AS is_online,
                    COALESCE(p.first_seen_at, '') AS first_seen_at,
                    COALESCE(p.last_seen_at, '') AS last_seen_at,
                    CASE WHEN up.peer_id IS NOT NULL THEN 1 ELSE 0 END AS is_self
             FROM group_members gm
             LEFT JOIN peers p ON gm.peer_id = p.peer_id
             LEFT JOIN user_profile up ON up.id = 1 AND up.peer_id = gm.peer_id
             WHERE gm.group_id = ?"
        ).bind(group_id).fetch_all(&self.pool).await.context("Failed to get group members")?;
        let mut members: Vec<StoredPeer> = rows
            .iter()
            .map(|r| {
                let is_self: i64 = r.try_get("is_self").unwrap_or(0);
                StoredPeer {
                    peer_id: r.get("peer_id"),
                    node_id: r.try_get("node_id").unwrap_or_default(),
                    username: r.try_get("username").unwrap_or_default(),
                    department: r.try_get("department").unwrap_or_default(),
                    software_version: r.try_get("software_version").unwrap_or_default(),
                    mac_address: r.try_get("mac_address").unwrap_or_default(),
                    avatar_path: r.try_get("avatar_path").unwrap_or_default(),
                    avatar_hash: r.try_get("avatar_hash").unwrap_or_default(),
                    avatar_updated_at: r.try_get("avatar_updated_at").unwrap_or_default(),
                    ip: r.try_get("ip").unwrap_or_default(),
                    port: r.try_get::<i64, _>("port").unwrap_or(0) as u16,
                    // Treat self as always online — UI uses this to render the green dot.
                    is_online: is_self == 1 || r.try_get::<bool, _>("is_online").unwrap_or(false),
                    first_seen_at: r.try_get("first_seen_at").unwrap_or_default(),
                    last_seen_at: r.try_get("last_seen_at").unwrap_or_default(),
                }
            })
            .collect();

        members.sort_by(|a, b| {
            let a_key = if !a.username.is_empty() {
                format!("{}\u{1f}{}", a.username, a.department)
            } else {
                a.peer_id.clone()
            };
            let b_key = if !b.username.is_empty() {
                format!("{}\u{1f}{}", b.username, b.department)
            } else {
                b.peer_id.clone()
            };
            a_key
                .cmp(&b_key)
                .then_with(|| b.is_online.cmp(&a.is_online))
                .then_with(|| b.last_seen_at.cmp(&a.last_seen_at))
        });
        members.dedup_by(|a, b| {
            if a.username.is_empty() || b.username.is_empty() {
                a.peer_id == b.peer_id
            } else {
                a.username == b.username && a.department == b.department
            }
        });

        Ok(members)
    }

    pub async fn rename_group(&self, group_id: &str, new_name: &str) -> Result<()> {
        sqlx::query("UPDATE groups SET name = ? WHERE group_id = ?")
            .bind(new_name)
            .bind(group_id)
            .execute(&self.pool)
            .await
            .context("Failed to rename group")?;
        Ok(())
    }

    pub async fn add_group_members(&self, group_id: &str, member_ids: &[String]) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        for mid in member_ids {
            sqlx::query("INSERT OR IGNORE INTO group_members (group_id, peer_id, joined_at) VALUES (?, ?, ?)")
                .bind(group_id).bind(mid).bind(&now).execute(&self.pool).await.ok();
        }
        Ok(())
    }

    pub async fn remove_group_member(&self, group_id: &str, peer_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM group_members WHERE group_id = ? AND peer_id = ?")
            .bind(group_id)
            .bind(peer_id)
            .execute(&self.pool)
            .await
            .context("Failed to remove group member")?;
        Ok(())
    }

    pub async fn dissolve_group(&self, group_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM group_members WHERE group_id = ?")
            .bind(group_id)
            .execute(&self.pool)
            .await
            .ok();
        sqlx::query("DELETE FROM groups WHERE group_id = ?")
            .bind(group_id)
            .execute(&self.pool)
            .await
            .context("Failed to dissolve group")?;
        Ok(())
    }

    pub async fn find_message_by_client_msg_id(
        &self,
        sender_id: &str,
        group_id: Option<&str>,
        client_msg_id: Option<&str>,
    ) -> Result<Option<ChatMessage>> {
        let Some(client_msg_id) = client_msg_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return Ok(None);
        };

        let row = if let Some(group_id) = group_id {
            sqlx::query(
                "SELECT id, sender_id, sender_name, receiver_id, content, msg_type, file_path, file_name, file_size, timestamp, is_read, client_msg_id
                 FROM messages
                 WHERE sender_id = ? AND group_id = ? AND client_msg_id = ?
                 ORDER BY id ASC
                 LIMIT 1",
            )
            .bind(sender_id)
            .bind(group_id)
            .bind(client_msg_id)
            .fetch_optional(&self.pool)
            .await
            .context("Failed to find group message by client_msg_id")?
        } else {
            sqlx::query(
                "SELECT id, sender_id, sender_name, receiver_id, content, msg_type, file_path, file_name, file_size, timestamp, is_read, client_msg_id
                 FROM messages
                 WHERE sender_id = ? AND (group_id IS NULL OR group_id = '') AND client_msg_id = ?
                 ORDER BY id ASC
                 LIMIT 1",
            )
            .bind(sender_id)
            .bind(client_msg_id)
            .fetch_optional(&self.pool)
            .await
            .context("Failed to find message by client_msg_id")?
        };

        Ok(row.as_ref().map(chat_message_from_row))
    }

    pub async fn save_group_message_dedup(
        &self,
        group_id: &str,
        sender_id: &str,
        sender_name: &str,
        content: &str,
        msg_type: &str,
        file_path: Option<&str>,
        file_name: Option<&str>,
        file_size: Option<i64>,
        is_read: bool,
        client_msg_id: Option<&str>,
    ) -> Result<ChatMessage> {
        if let Some(existing) = self
            .find_message_by_client_msg_id(sender_id, Some(group_id), client_msg_id)
            .await?
        {
            return Ok(existing);
        }

        self.save_group_message(
            group_id,
            sender_id,
            sender_name,
            content,
            msg_type,
            file_path,
            file_name,
            file_size,
            is_read,
            client_msg_id,
        )
        .await
    }

    pub async fn save_group_message(
        &self,
        group_id: &str,
        sender_id: &str,
        sender_name: &str,
        content: &str,
        msg_type: &str,
        file_path: Option<&str>,
        file_name: Option<&str>,
        file_size: Option<i64>,
        is_read: bool,
        client_msg_id: Option<&str>,
    ) -> Result<ChatMessage> {
        let timestamp = Utc::now().to_rfc3339();
        let result = sqlx::query(
            "INSERT INTO messages (sender_id, sender_name, receiver_id, content, msg_type, file_path, file_name, file_size, timestamp, is_read, group_id, client_msg_id)
             VALUES (?, ?, '', ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT DO NOTHING"
        ).bind(sender_id).bind(sender_name).bind(content).bind(msg_type)
         .bind(file_path).bind(file_name).bind(file_size).bind(&timestamp)
         .bind(if is_read { 1 } else { 0 }).bind(group_id).bind(client_msg_id)
         .execute(&self.pool).await.context("Failed to save group message")?;
        // Concurrent writer won the unique dedup race — return its row.
        if result.rows_affected() == 0 {
            if let Some(existing) = self
                .find_message_by_client_msg_id(sender_id, Some(group_id), client_msg_id)
                .await?
            {
                return Ok(existing);
            }
        }
        let id = result.last_insert_rowid();
        Ok(ChatMessage {
            id,
            sender_id: sender_id.to_string(),
            sender_name: sender_name.to_string(),
            receiver_id: String::new(),
            content: content.to_string(),
            msg_type: msg_type.to_string(),
            file_path: file_path.map(|s| s.to_string()),
            file_name: file_name.map(|s| s.to_string()),
            file_size,
            timestamp,
            is_read,
            client_msg_id: client_msg_id.map(|s| s.to_string()),
        })
    }

    pub async fn get_group_unread_counts(&self, my_id: &str) -> Result<Vec<GroupUnread>> {
        let rows = sqlx::query(
            "SELECT group_id, COUNT(*) as cnt FROM messages
             WHERE group_id IS NOT NULL AND group_id <> '' AND sender_id <> ? AND is_read = 0
             GROUP BY group_id",
        )
        .bind(my_id)
        .fetch_all(&self.pool)
        .await
        .context("Failed to get group unread counts")?;
        Ok(rows
            .iter()
            .map(|r| GroupUnread {
                group_id: r.get("group_id"),
                count: r.get::<i64, _>("cnt") as u32,
            })
            .collect())
    }

    pub async fn mark_group_read(&self, group_id: &str, my_id: &str) -> Result<()> {
        sqlx::query("UPDATE messages SET is_read = 1 WHERE group_id = ? AND sender_id <> ?")
            .bind(group_id)
            .bind(my_id)
            .execute(&self.pool)
            .await
            .context("Failed to mark group read")?;
        Ok(())
    }

    pub async fn get_group_messages(
        &self,
        group_id: &str,
        limit: Option<i64>,
    ) -> Result<Vec<ChatMessage>> {
        let limit = normalize_message_limit(limit);
        let rows = sqlx::query(
            "SELECT id, sender_id, sender_name, content, msg_type, file_path, file_name, file_size, timestamp, is_read, client_msg_id
             FROM (
                 SELECT id, sender_id, sender_name, content, msg_type, file_path, file_name, file_size, timestamp, is_read, client_msg_id
                 FROM messages WHERE group_id = ? ORDER BY id DESC LIMIT ?
             ) AS recent_messages
             ORDER BY id ASC"
        ).bind(group_id).bind(limit).fetch_all(&self.pool).await.context("Failed to get group messages")?;
        Ok(rows
            .iter()
            .map(|r| ChatMessage {
                id: r.get("id"),
                sender_id: r.get("sender_id"),
                sender_name: r.get("sender_name"),
                receiver_id: String::new(),
                content: r.get("content"),
                msg_type: r.get("msg_type"),
                file_path: r.get("file_path"),
                file_name: r.get("file_name"),
                file_size: r.get("file_size"),
                timestamp: r.get("timestamp"),
                is_read: r.get::<bool, _>("is_read"),
                client_msg_id: r.try_get("client_msg_id").ok(),
            })
            .collect())
    }

    pub async fn get_departments(&self) -> Result<Vec<String>> {
        let rows = sqlx::query(
            "SELECT DISTINCT department FROM (
                SELECT department FROM user_profile
                UNION ALL
                SELECT department FROM peers
            )
             WHERE TRIM(department) <> ''
             ORDER BY department COLLATE NOCASE ASC",
        )
        .fetch_all(&self.pool)
        .await
        .context("Failed to load departments")?;

        Ok(rows
            .iter()
            .map(|row| row.get::<String, _>("department"))
            .collect())
    }

    pub async fn upsert_peer(
        &self,
        peer_id: &str,
        username: &str,
        department: &str,
        ip: &str,
        port: u16,
        is_online: bool,
    ) -> Result<()> {
        self.upsert_peer_with_profile(peer_id, username, department, "", "", ip, port, is_online)
            .await
    }

    pub async fn upsert_peer_with_profile(
        &self,
        peer_id: &str,
        username: &str,
        department: &str,
        software_version: &str,
        mac_address: &str,
        ip: &str,
        port: u16,
        is_online: bool,
    ) -> Result<()> {
        self.upsert_peer_with_avatar(
            peer_id,
            username,
            department,
            software_version,
            mac_address,
            "",
            "",
            0,
            ip,
            port,
            is_online,
        )
        .await
    }

    pub async fn upsert_peer_with_avatar(
        &self,
        peer_id: &str,
        username: &str,
        department: &str,
        software_version: &str,
        mac_address: &str,
        avatar_path: &str,
        avatar_hash: &str,
        avatar_updated_at: i64,
        ip: &str,
        port: u16,
        is_online: bool,
    ) -> Result<()> {
        self.upsert_peer_with_node_id_avatar(
            peer_id,
            "",
            username,
            department,
            software_version,
            mac_address,
            avatar_path,
            avatar_hash,
            avatar_updated_at,
            ip,
            port,
            is_online,
        )
        .await
    }

    pub async fn upsert_peer_with_node_id_avatar(
        &self,
        peer_id: &str,
        node_id: &str,
        username: &str,
        department: &str,
        software_version: &str,
        mac_address: &str,
        avatar_path: &str,
        avatar_hash: &str,
        avatar_updated_at: i64,
        ip: &str,
        port: u16,
        is_online: bool,
    ) -> Result<()> {
        let incoming_peer_id = peer_id.trim();
        let node_id = node_id.trim();
        if incoming_peer_id.is_empty() || !contact_filter::has_valid_endpoint(ip, port) {
            log::debug!(
                "Skipping peer with invalid endpoint: {} @ {}:{}",
                incoming_peer_id,
                ip,
                port
            );
            return Ok(());
        }

        let canonical_peer_id = canonicalize_endpoint_peer_id(incoming_peer_id, ip, port);
        let peer_id = canonical_peer_id.as_str();

        if incoming_peer_id != peer_id {
            self.migrate_legacy_endpoint_peer(incoming_peer_id, peer_id)
                .await?;
        }

        if !contact_filter::has_contact_identity(username, department) {
            let existing_identity = sqlx::query(
                "SELECT 1
                 FROM peers
                 WHERE peer_id = ?
                   AND (TRIM(username) <> '' OR TRIM(department) <> '')
                 LIMIT 1",
            )
            .bind(peer_id)
            .fetch_optional(&self.pool)
            .await
            .context("Failed to check existing peer identity")?;

            if existing_identity.is_none() {
                log::debug!("Skipping peer without contact identity: {}", peer_id);
                return Ok(());
            }
        }

        let now = Utc::now().to_rfc3339();

        let existing_node_peer_ids = if node_id.is_empty() {
            Vec::new()
        } else {
            sqlx::query("SELECT peer_id FROM peers WHERE node_id = ? AND peer_id <> ?")
                .bind(node_id)
                .bind(peer_id)
                .fetch_all(&self.pool)
                .await
                .context("Failed to load existing peers for node_id")?
                .into_iter()
                .map(|row| row.get::<String, _>("peer_id"))
                .collect::<Vec<_>>()
        };

        // Check for same-identity peer at this endpoint (IP changed, old client didn't broadcast)
        if !username.is_empty() && !department.is_empty() {
            let existing_at_endpoint = sqlx::query(
                "SELECT peer_id FROM peers
                 WHERE ip = ? AND port = ? AND peer_id <> ?
                 AND username = ? AND department = ?
                 LIMIT 1",
            )
            .bind(ip)
            .bind(port as i64)
            .bind(peer_id)
            .bind(username)
            .bind(department)
            .fetch_optional(&self.pool)
            .await
            .context("Failed to check identity at endpoint")?;

            if let Some(row) = existing_at_endpoint {
                let old_peer_id: String = row.get("peer_id");
                log::info!(
                    "Same identity ({} {}) moved from {} to {} at {}:{} - migrating references",
                    username,
                    department,
                    old_peer_id,
                    peer_id,
                    ip,
                    port
                );
                self.migrate_peer_references(&old_peer_id, peer_id).await?;
                sqlx::query("DELETE FROM peers WHERE peer_id = ?")
                    .bind(&old_peer_id)
                    .execute(&self.pool)
                    .await
                    .context("Failed to remove old peer_id after identity match")?;
            }
        }

        let endpoint_duplicates =
            sqlx::query("SELECT peer_id FROM peers WHERE ip = ? AND port = ? AND peer_id <> ?")
                .bind(ip)
                .bind(port as i64)
                .bind(peer_id)
                .fetch_all(&self.pool)
                .await
                .context("Failed to load stale peer endpoints")?;
        for row in endpoint_duplicates {
            let old_peer_id: String = row.get("peer_id");
            self.migrate_peer_references(&old_peer_id, peer_id).await?;
        }

        // Cleanup duplicates at the same endpoint (different peer_id, same ip:port).
        sqlx::query("DELETE FROM peers WHERE ip = ? AND port = ? AND peer_id <> ?")
            .bind(ip)
            .bind(port as i64)
            .bind(peer_id)
            .execute(&self.pool)
            .await
            .context("Failed to remove stale peer endpoints")?;

        // With node_id present, endpoint changes are tracked as aliases; same-name legacy peers stay separate.
        // Upsert. Preserve existing non-empty username/department when the incoming row has
        // empty values — system messages (group_created, group_dissolved, group_member_left)
        // historically carried empty sender_name and would otherwise wipe good peer data.
        sqlx::query(
            "INSERT INTO peers (peer_id, node_id, username, department, software_version, mac_address, avatar_path, avatar_hash, avatar_updated_at, ip, port, is_online, first_seen_at, last_seen_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(peer_id) DO UPDATE SET
                node_id = CASE WHEN excluded.node_id = '' THEN peers.node_id ELSE excluded.node_id END,
                username = CASE WHEN TRIM(excluded.username) = '' THEN peers.username ELSE excluded.username END,
                department = CASE WHEN TRIM(excluded.department) = '' THEN peers.department ELSE excluded.department END,
                software_version = CASE WHEN excluded.software_version = '' THEN peers.software_version ELSE excluded.software_version END,
                mac_address = CASE WHEN excluded.mac_address = '' THEN peers.mac_address ELSE excluded.mac_address END,
                avatar_path = CASE
                    WHEN excluded.avatar_path <> '' THEN excluded.avatar_path
                    WHEN excluded.avatar_updated_at > peers.avatar_updated_at THEN excluded.avatar_path
                    ELSE peers.avatar_path
                END,
                avatar_hash = CASE
                    WHEN excluded.avatar_updated_at > peers.avatar_updated_at THEN excluded.avatar_hash
                    WHEN peers.avatar_hash = '' AND excluded.avatar_hash <> '' THEN excluded.avatar_hash
                    ELSE peers.avatar_hash
                END,
                avatar_updated_at = CASE
                    WHEN excluded.avatar_updated_at > peers.avatar_updated_at THEN excluded.avatar_updated_at
                    WHEN peers.avatar_hash = '' AND excluded.avatar_hash <> '' THEN excluded.avatar_updated_at
                    ELSE peers.avatar_updated_at
                END,
                ip = excluded.ip,
                port = excluded.port,
                is_online = excluded.is_online,
                last_seen_at = excluded.last_seen_at",
        )
        .bind(peer_id)
        .bind(node_id)
        .bind(username)
        .bind(department)
        .bind(software_version)
        .bind(mac_address)
        .bind(avatar_path)
        .bind(avatar_hash)
        .bind(avatar_updated_at)
        .bind(ip)
        .bind(port as i64)
        .bind(if is_online { 1 } else { 0 })
        .bind(&now)
        .bind(&now)
        .execute(&self.pool)
        .await
        .context("Failed to upsert peer")?;

        if !node_id.is_empty() {
            self.upsert_peer_alias(node_id, peer_id).await?;
            for old_peer_id in existing_node_peer_ids {
                self.upsert_peer_alias(node_id, &old_peer_id).await?;
                self.move_recent_contact_reference(&old_peer_id, peer_id)
                    .await?;
                sqlx::query("DELETE FROM peers WHERE peer_id = ?")
                    .bind(&old_peer_id)
                    .execute(&self.pool)
                    .await
                    .context("Failed to remove stale peer row for node_id")?;
            }
        }

        Ok(())
    }

    pub async fn upsert_peer_alias(&self, node_id: &str, alias_peer_id: &str) -> Result<()> {
        let node_id = node_id.trim();
        let alias_peer_id = alias_peer_id.trim();
        if node_id.is_empty() || alias_peer_id.is_empty() {
            return Ok(());
        }
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO peer_aliases (alias_peer_id, node_id, created_at, last_seen_at)
             VALUES (?, ?, ?, ?)
             ON CONFLICT(alias_peer_id) DO UPDATE SET
                node_id = excluded.node_id,
                last_seen_at = excluded.last_seen_at",
        )
        .bind(alias_peer_id)
        .bind(node_id)
        .bind(&now)
        .bind(&now)
        .execute(&self.pool)
        .await
        .context("Failed to upsert peer alias")?;
        Ok(())
    }

    #[allow(dead_code)]
    pub async fn list_peer_aliases(&self, node_id: &str) -> Result<Vec<String>> {
        let rows = sqlx::query(
            "SELECT alias_peer_id FROM peer_aliases WHERE node_id = ? ORDER BY alias_peer_id",
        )
        .bind(node_id)
        .fetch_all(&self.pool)
        .await
        .context("Failed to list peer aliases")?;
        Ok(rows
            .into_iter()
            .map(|row| row.get("alias_peer_id"))
            .collect())
    }

    async fn move_recent_contact_reference(
        &self,
        old_peer_id: &str,
        new_peer_id: &str,
    ) -> Result<()> {
        if old_peer_id == new_peer_id {
            return Ok(());
        }
        sqlx::query(
            "INSERT OR IGNORE INTO recent_contacts (peer_id, added_at)
             SELECT ?, added_at FROM recent_contacts WHERE peer_id = ?",
        )
        .bind(new_peer_id)
        .bind(old_peer_id)
        .execute(&self.pool)
        .await
        .context("Failed to move recent contact reference")?;
        sqlx::query("DELETE FROM recent_contacts WHERE peer_id = ?")
            .bind(old_peer_id)
            .execute(&self.pool)
            .await
            .context("Failed to remove old recent contact reference")?;
        Ok(())
    }

    /// Public wrapper over `identity_keys_for`: returns every peer_id/node_id known
    /// to belong to the same identity (current endpoint + historical aliases).
    /// Used by offline delivery to redirect a stale peer_id to the node's current
    /// endpoint after an IP change.
    pub(crate) async fn identity_aliases(&self, identity: &str) -> Result<Vec<String>> {
        self.identity_keys_for(identity).await
    }

    async fn identity_keys_for(&self, identity: &str) -> Result<Vec<String>> {
        let identity = identity.trim();
        let mut keys = Vec::<String>::new();
        if !identity.is_empty() {
            keys.push(identity.to_string());
        }

        let node_rows = sqlx::query(
            "SELECT node_id FROM peers WHERE peer_id = ? AND TRIM(node_id) <> ''
             UNION
             SELECT node_id FROM peer_aliases WHERE alias_peer_id = ? AND TRIM(node_id) <> ''
             UNION
             SELECT node_id FROM user_profile WHERE (peer_id = ? OR node_id = ?) AND TRIM(node_id) <> ''",
        )
        .bind(identity)
        .bind(identity)
        .bind(identity)
        .bind(identity)
        .fetch_all(&self.pool)
        .await
        .context("Failed to resolve identity node_id")?;

        for row in node_rows {
            let node_id: String = row.get("node_id");
            if !keys.iter().any(|key| key == &node_id) {
                keys.push(node_id.clone());
            }

            let alias_rows = sqlx::query(
                "SELECT peer_id AS id FROM peers WHERE node_id = ?
                 UNION
                 SELECT alias_peer_id AS id FROM peer_aliases WHERE node_id = ?
                 UNION
                 SELECT peer_id AS id FROM user_profile WHERE node_id = ? AND TRIM(peer_id) <> ''",
            )
            .bind(&node_id)
            .bind(&node_id)
            .bind(&node_id)
            .fetch_all(&self.pool)
            .await
            .context("Failed to resolve identity aliases")?;

            for alias_row in alias_rows {
                let alias: String = alias_row.get("id");
                if !alias.trim().is_empty() && !keys.iter().any(|key| key == &alias) {
                    keys.push(alias);
                }
            }
        }

        Ok(keys)
    }

    fn placeholders(count: usize) -> String {
        std::iter::repeat("?")
            .take(count.max(1))
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub(crate) async fn migrate_peer_references(
        &self,
        old_peer_id: &str,
        new_peer_id: &str,
    ) -> Result<()> {
        if old_peer_id == new_peer_id {
            return Ok(());
        }

        sqlx::query(
            "INSERT OR IGNORE INTO group_members (group_id, peer_id, joined_at)
             SELECT group_id, ?, joined_at FROM group_members WHERE peer_id = ?",
        )
        .bind(new_peer_id)
        .bind(old_peer_id)
        .execute(&self.pool)
        .await
        .context("Failed to migrate group members")?;

        sqlx::query("DELETE FROM group_members WHERE peer_id = ?")
            .bind(old_peer_id)
            .execute(&self.pool)
            .await
            .context("Failed to remove stale group member refs")?;

        sqlx::query(
            "INSERT OR IGNORE INTO recent_contacts (peer_id, added_at)
             SELECT ?, added_at FROM recent_contacts WHERE peer_id = ?",
        )
        .bind(new_peer_id)
        .bind(old_peer_id)
        .execute(&self.pool)
        .await
        .context("Failed to migrate recent contacts")?;

        sqlx::query("DELETE FROM recent_contacts WHERE peer_id = ?")
            .bind(old_peer_id)
            .execute(&self.pool)
            .await
            .context("Failed to remove stale recent contacts")?;

        sqlx::query("UPDATE pending_group_messages SET peer_id = ? WHERE peer_id = ?")
            .bind(new_peer_id)
            .bind(old_peer_id)
            .execute(&self.pool)
            .await
            .context("Failed to migrate pending group messages")?;

        sqlx::query("UPDATE pending_group_messages SET sender_id = ? WHERE sender_id = ?")
            .bind(new_peer_id)
            .bind(old_peer_id)
            .execute(&self.pool)
            .await
            .context("Failed to migrate pending group message senders")?;

        sqlx::query("UPDATE pending_notifications SET peer_id = ? WHERE peer_id = ?")
            .bind(new_peer_id)
            .bind(old_peer_id)
            .execute(&self.pool)
            .await
            .context("Failed to migrate pending notifications")?;

        self.migrate_pending_notification_payloads(old_peer_id, new_peer_id)
            .await?;

        sqlx::query("UPDATE pending_file_transfers SET peer_id = ? WHERE peer_id = ?")
            .bind(new_peer_id)
            .bind(old_peer_id)
            .execute(&self.pool)
            .await
            .context("Failed to migrate pending file transfers")?;

        sqlx::query("UPDATE pending_file_transfers SET sender_id = ? WHERE sender_id = ?")
            .bind(new_peer_id)
            .bind(old_peer_id)
            .execute(&self.pool)
            .await
            .context("Failed to migrate pending file transfer senders")?;

        sqlx::query("UPDATE messages SET sender_id = ? WHERE sender_id = ?")
            .bind(new_peer_id)
            .bind(old_peer_id)
            .execute(&self.pool)
            .await
            .context("Failed to migrate message senders")?;

        sqlx::query("UPDATE messages SET receiver_id = ? WHERE receiver_id = ?")
            .bind(new_peer_id)
            .bind(old_peer_id)
            .execute(&self.pool)
            .await
            .context("Failed to migrate message receivers")?;

        sqlx::query("UPDATE groups SET creator_id = ? WHERE creator_id = ?")
            .bind(new_peer_id)
            .bind(old_peer_id)
            .execute(&self.pool)
            .await
            .context("Failed to migrate group creators")?;

        Ok(())
    }

    async fn migrate_pending_notification_payloads(
        &self,
        old_peer_id: &str,
        new_peer_id: &str,
    ) -> Result<()> {
        let rows = sqlx::query("SELECT id, payload FROM pending_notifications")
            .fetch_all(&self.pool)
            .await
            .context("Failed to load pending notification payloads")?;

        for row in rows {
            let id: i64 = row.get("id");
            let payload: String = row.get("payload");
            let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&payload) else {
                continue;
            };
            let mut changed = false;
            rewrite_peer_ids_in_json(&mut value, old_peer_id, new_peer_id, &mut changed);
            if !changed {
                continue;
            }

            let updated = serde_json::to_string(&value)
                .context("Failed to serialize migrated pending notification payload")?;
            sqlx::query("UPDATE pending_notifications SET payload = ? WHERE id = ?")
                .bind(updated)
                .bind(id)
                .execute(&self.pool)
                .await
                .context("Failed to update pending notification payload")?;
        }

        Ok(())
    }

    async fn migrate_legacy_endpoint_peer(
        &self,
        old_peer_id: &str,
        new_peer_id: &str,
    ) -> Result<()> {
        if old_peer_id == new_peer_id {
            return Ok(());
        }

        self.copy_peer_row(old_peer_id, new_peer_id).await?;
        self.migrate_peer_references(old_peer_id, new_peer_id)
            .await?;

        sqlx::query("DELETE FROM peers WHERE peer_id = ?")
            .bind(old_peer_id)
            .execute(&self.pool)
            .await
            .context("Failed to remove legacy endpoint peer")?;

        Ok(())
    }

    async fn copy_peer_row(&self, old_peer_id: &str, new_peer_id: &str) -> Result<()> {
        sqlx::query(
            "INSERT INTO peers (
                peer_id, username, department, software_version, mac_address,
                avatar_path, avatar_hash, avatar_updated_at,
                ip, port, is_online, first_seen_at, last_seen_at
             )
             SELECT
                ?, username, department, software_version, mac_address,
                avatar_path, avatar_hash, avatar_updated_at,
                ip, port, is_online, first_seen_at, last_seen_at
             FROM peers
             WHERE peer_id = ?
             ON CONFLICT(peer_id) DO UPDATE SET
                username = CASE WHEN TRIM(excluded.username) = '' THEN peers.username ELSE excluded.username END,
                department = CASE WHEN TRIM(excluded.department) = '' THEN peers.department ELSE excluded.department END,
                software_version = CASE WHEN excluded.software_version = '' THEN peers.software_version ELSE excluded.software_version END,
                mac_address = CASE WHEN excluded.mac_address = '' THEN peers.mac_address ELSE excluded.mac_address END,
                avatar_path = CASE
                    WHEN peers.avatar_path = '' THEN excluded.avatar_path
                    WHEN excluded.avatar_updated_at > peers.avatar_updated_at THEN excluded.avatar_path
                    ELSE peers.avatar_path
                END,
                avatar_hash = CASE
                    WHEN excluded.avatar_updated_at > peers.avatar_updated_at THEN excluded.avatar_hash
                    WHEN peers.avatar_hash = '' AND excluded.avatar_hash <> '' THEN excluded.avatar_hash
                    ELSE peers.avatar_hash
                END,
                avatar_updated_at = CASE
                    WHEN excluded.avatar_updated_at > peers.avatar_updated_at THEN excluded.avatar_updated_at
                    WHEN peers.avatar_hash = '' AND excluded.avatar_hash <> '' THEN excluded.avatar_updated_at
                    ELSE peers.avatar_updated_at
                END,
                ip = excluded.ip,
                port = excluded.port,
                is_online = CASE WHEN excluded.is_online THEN excluded.is_online ELSE peers.is_online END,
                first_seen_at = CASE
                    WHEN peers.first_seen_at = '' THEN excluded.first_seen_at
                    WHEN excluded.first_seen_at = '' THEN peers.first_seen_at
                    WHEN excluded.first_seen_at < peers.first_seen_at THEN excluded.first_seen_at
                    ELSE peers.first_seen_at
                END,
                last_seen_at = CASE
                    WHEN excluded.last_seen_at > peers.last_seen_at THEN excluded.last_seen_at
                    ELSE peers.last_seen_at
                END",
        )
        .bind(new_peer_id)
        .bind(old_peer_id)
        .execute(&self.pool)
        .await
        .context("Failed to copy peer row")?;

        Ok(())
    }

    async fn migrate_self_endpoint_peer(&self, old_peer_id: &str, new_peer_id: &str) -> Result<()> {
        if old_peer_id == new_peer_id {
            return Ok(());
        }

        self.migrate_peer_references(old_peer_id, new_peer_id)
            .await?;

        sqlx::query("DELETE FROM peers WHERE peer_id IN (?, ?)")
            .bind(old_peer_id)
            .bind(new_peer_id)
            .execute(&self.pool)
            .await
            .context("Failed to remove stale self peer rows")?;

        Ok(())
    }

    // Useful for startup/offline recovery flows; current startup applies a grace window instead.
    #[allow(dead_code)]
    pub async fn mark_all_peers_offline(&self) -> Result<()> {
        sqlx::query("UPDATE peers SET is_online = 0")
            .execute(&self.pool)
            .await
            .context("Failed to mark all peers offline")?;
        Ok(())
    }

    pub async fn list_stored_peers(&self) -> Result<Vec<StoredPeer>> {
        let rows = sqlx::query(
            "SELECT peer_id, node_id, username, department, software_version, mac_address, avatar_path, avatar_hash, avatar_updated_at, ip, port, is_online, first_seen_at, last_seen_at
             FROM peers
             WHERE TRIM(username) <> '' OR TRIM(department) <> ''
             ORDER BY is_online DESC, last_seen_at DESC",
        )
        .fetch_all(&self.pool)
        .await
        .context("Failed to list stored peers")?;

        Ok(rows
            .iter()
            .map(|row| StoredPeer {
                peer_id: row.get("peer_id"),
                node_id: row.try_get("node_id").unwrap_or_default(),
                username: row.get("username"),
                department: row.get("department"),
                software_version: row.get("software_version"),
                mac_address: row.get("mac_address"),
                avatar_path: row.get("avatar_path"),
                avatar_hash: row.get("avatar_hash"),
                avatar_updated_at: row.get("avatar_updated_at"),
                ip: row.get("ip"),
                port: row.get::<i64, _>("port") as u16,
                is_online: row.get::<bool, _>("is_online"),
                first_seen_at: row.get("first_seen_at"),
                last_seen_at: row.get("last_seen_at"),
            })
            .collect())
    }

    pub async fn get_stored_peer(&self, peer_id: &str) -> Result<Option<StoredPeer>> {
        let row = sqlx::query(
            "SELECT peer_id, node_id, username, department, software_version, mac_address, avatar_path, avatar_hash, avatar_updated_at, ip, port, is_online, first_seen_at, last_seen_at
             FROM peers
             WHERE peer_id = ?
               AND (TRIM(username) <> '' OR TRIM(department) <> '')",
        )
        .bind(peer_id)
        .fetch_optional(&self.pool)
        .await
        .context("Failed to get stored peer")?;

        Ok(row.map(|row| StoredPeer {
            peer_id: row.get("peer_id"),
            node_id: row.try_get("node_id").unwrap_or_default(),
            username: row.get("username"),
            department: row.get("department"),
            software_version: row.get("software_version"),
            mac_address: row.get("mac_address"),
            avatar_path: row.get("avatar_path"),
            avatar_hash: row.get("avatar_hash"),
            avatar_updated_at: row.get("avatar_updated_at"),
            ip: row.get("ip"),
            port: row.get::<i64, _>("port") as u16,
            is_online: row.get::<bool, _>("is_online"),
            first_seen_at: row.get("first_seen_at"),
            last_seen_at: row.get("last_seen_at"),
        }))
    }

    pub async fn save_message_dedup(
        &self,
        sender_id: &str,
        sender_name: &str,
        receiver_id: &str,
        content: &str,
        msg_type: &str,
        file_path: Option<&str>,
        file_name: Option<&str>,
        file_size: Option<i64>,
        client_msg_id: Option<&str>,
    ) -> Result<ChatMessage> {
        if let Some(existing) = self
            .find_message_by_client_msg_id(sender_id, None, client_msg_id)
            .await?
        {
            return Ok(existing);
        }

        self.save_message(
            sender_id,
            sender_name,
            receiver_id,
            content,
            msg_type,
            file_path,
            file_name,
            file_size,
            client_msg_id,
        )
        .await
    }

    pub async fn save_message(
        &self,
        sender_id: &str,
        sender_name: &str,
        receiver_id: &str,
        content: &str,
        msg_type: &str,
        file_path: Option<&str>,
        file_name: Option<&str>,
        file_size: Option<i64>,
        client_msg_id: Option<&str>,
    ) -> Result<ChatMessage> {
        let timestamp = Utc::now().to_rfc3339();

        let result = sqlx::query(
            "INSERT INTO messages (sender_id, sender_name, receiver_id, content, msg_type, file_path, file_name, file_size, timestamp, is_read, client_msg_id)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 0, ?)
             ON CONFLICT DO NOTHING"
        )
        .bind(sender_id)
        .bind(sender_name)
        .bind(receiver_id)
        .bind(content)
        .bind(msg_type)
        .bind(file_path)
        .bind(file_name)
        .bind(file_size)
        .bind(&timestamp)
        .bind(client_msg_id)
        .execute(&self.pool)
        .await
        .context("Failed to save message")?;

        // A concurrent writer won the unique dedup race — return the row it wrote
        // instead of a bogus last_insert_rowid(). The SELECT-first path in
        // save_message_dedup handles the common case; this covers the tight race.
        if result.rows_affected() == 0 {
            if let Some(existing) = self
                .find_message_by_client_msg_id(sender_id, None, client_msg_id)
                .await?
            {
                return Ok(existing);
            }
        }

        Ok(ChatMessage {
            id: result.last_insert_rowid(),
            sender_id: sender_id.to_string(),
            sender_name: sender_name.to_string(),
            receiver_id: receiver_id.to_string(),
            content: content.to_string(),
            msg_type: msg_type.to_string(),
            file_path: file_path.map(|s| s.to_string()),
            file_name: file_name.map(|s| s.to_string()),
            file_size,
            timestamp,
            is_read: false,
            client_msg_id: client_msg_id.map(|s| s.to_string()),
        })
    }

    pub async fn get_conversation(
        &self,
        peer_id: &str,
        my_id: &str,
        limit: Option<i64>,
    ) -> Result<Vec<ChatMessage>> {
        let limit = normalize_message_limit(limit);
        log::info!("get_conversation: peer_id={}, my_id={}", peer_id, my_id);
        let peer_keys = self.identity_keys_for(peer_id).await?;
        let my_keys = self.identity_keys_for(my_id).await?;
        let peer_placeholders = Self::placeholders(peer_keys.len());
        let my_placeholders = Self::placeholders(my_keys.len());
        let sql = format!(
            "SELECT id, sender_id, sender_name, receiver_id, content, msg_type, file_path, file_name, file_size, timestamp, is_read, client_msg_id
             FROM (
                 SELECT id, sender_id, sender_name, receiver_id, content, msg_type, file_path, file_name, file_size, timestamp, is_read, client_msg_id
                 FROM messages
                 WHERE (sender_id IN ({my_placeholders}) AND receiver_id IN ({peer_placeholders}))
                    OR (sender_id IN ({peer_placeholders}) AND receiver_id IN ({my_placeholders}))
                 ORDER BY id DESC LIMIT ?
             ) AS recent_messages
             ORDER BY id ASC"
        );
        let mut query = sqlx::query(&sql);
        for key in &my_keys {
            query = query.bind(key);
        }
        for key in &peer_keys {
            query = query.bind(key);
        }
        for key in &peer_keys {
            query = query.bind(key);
        }
        for key in &my_keys {
            query = query.bind(key);
        }
        let rows = query
            .bind(limit)
            .fetch_all(&self.pool)
            .await
            .context("Failed to fetch conversation")?;

        let messages = rows
            .iter()
            .map(|row| ChatMessage {
                id: row.get("id"),
                sender_id: row.get("sender_id"),
                sender_name: row.get("sender_name"),
                receiver_id: row.get("receiver_id"),
                content: row.get("content"),
                msg_type: row.get("msg_type"),
                file_path: row.get("file_path"),
                file_name: row.get("file_name"),
                file_size: row.get("file_size"),
                timestamp: row.get("timestamp"),
                is_read: row.get::<bool, _>("is_read"),
                client_msg_id: row.try_get("client_msg_id").ok(),
            })
            .collect();

        Ok(messages)
    }

    pub async fn get_conversation_history(
        &self,
        peer_id: &str,
        my_id: &str,
        before_id: Option<i64>,
        limit: Option<i64>,
        filter: Option<&str>,
        day_start: Option<&str>,
        day_end: Option<&str>,
    ) -> Result<Vec<ChatMessage>> {
        let limit = normalize_message_limit(limit);
        let peer_keys = self.identity_keys_for(peer_id).await?;
        let my_keys = self.identity_keys_for(my_id).await?;
        let peer_placeholders = Self::placeholders(peer_keys.len());
        let my_placeholders = Self::placeholders(my_keys.len());
        let before_clause = if before_id.is_some() {
            "AND id < ?"
        } else {
            ""
        };
        let filter_clause = message_filter_clause(filter);
        let day_clause = if day_start.is_some() && day_end.is_some() {
            "AND timestamp >= ? AND timestamp < ?"
        } else {
            ""
        };
        let sql = format!(
            "SELECT id, sender_id, sender_name, receiver_id, content, msg_type, file_path, file_name, file_size, timestamp, is_read, client_msg_id
             FROM (
                 SELECT id, sender_id, sender_name, receiver_id, content, msg_type, file_path, file_name, file_size, timestamp, is_read, client_msg_id
                 FROM messages
                 WHERE ((sender_id IN ({my_placeholders}) AND receiver_id IN ({peer_placeholders}))
                    OR (sender_id IN ({peer_placeholders}) AND receiver_id IN ({my_placeholders})))
                   {before_clause}
                   {filter_clause}
                   {day_clause}
                 ORDER BY id DESC LIMIT ?
             ) AS history_messages
             ORDER BY id ASC"
        );
        let mut query = sqlx::query(&sql);
        for key in &my_keys {
            query = query.bind(key);
        }
        for key in &peer_keys {
            query = query.bind(key);
        }
        for key in &peer_keys {
            query = query.bind(key);
        }
        for key in &my_keys {
            query = query.bind(key);
        }
        if let Some(before_id) = before_id {
            query = query.bind(before_id);
        }
        if let (Some(day_start), Some(day_end)) = (day_start, day_end) {
            query = query.bind(day_start).bind(day_end);
        }
        let rows = query
            .bind(limit)
            .fetch_all(&self.pool)
            .await
            .context("Failed to fetch conversation history")?;

        Ok(rows.iter().map(chat_message_from_row).collect())
    }

    pub async fn get_group_history(
        &self,
        group_id: &str,
        before_id: Option<i64>,
        limit: Option<i64>,
        filter: Option<&str>,
        day_start: Option<&str>,
        day_end: Option<&str>,
    ) -> Result<Vec<ChatMessage>> {
        let limit = normalize_message_limit(limit);
        let before_clause = if before_id.is_some() {
            "AND id < ?"
        } else {
            ""
        };
        let filter_clause = message_filter_clause(filter);
        let day_clause = if day_start.is_some() && day_end.is_some() {
            "AND timestamp >= ? AND timestamp < ?"
        } else {
            ""
        };
        let sql = format!(
            "SELECT id, sender_id, sender_name, receiver_id, content, msg_type, file_path, file_name, file_size, timestamp, is_read, client_msg_id
             FROM (
                 SELECT id, sender_id, sender_name, receiver_id, content, msg_type, file_path, file_name, file_size, timestamp, is_read, client_msg_id
                 FROM messages
                 WHERE group_id = ?
                   {before_clause}
                   {filter_clause}
                   {day_clause}
                 ORDER BY id DESC LIMIT ?
             ) AS history_messages
             ORDER BY id ASC"
        );
        let mut query = sqlx::query(&sql).bind(group_id);
        if let Some(before_id) = before_id {
            query = query.bind(before_id);
        }
        if let (Some(day_start), Some(day_end)) = (day_start, day_end) {
            query = query.bind(day_start).bind(day_end);
        }
        let rows = query
            .bind(limit)
            .fetch_all(&self.pool)
            .await
            .context("Failed to fetch group history")?;

        Ok(rows.iter().map(chat_message_from_row).collect())
    }

    pub async fn search_conversation_messages(
        &self,
        peer_id: &str,
        my_id: &str,
        query: &str,
        limit: Option<i64>,
        filter: Option<&str>,
        day_start: Option<&str>,
        day_end: Option<&str>,
    ) -> Result<Vec<ChatMessage>> {
        let pattern = format!("%{}%", query);
        let limit = normalize_search_limit(limit);
        let peer_keys = self.identity_keys_for(peer_id).await?;
        let my_keys = self.identity_keys_for(my_id).await?;
        let peer_placeholders = Self::placeholders(peer_keys.len());
        let my_placeholders = Self::placeholders(my_keys.len());
        let filter_clause = message_filter_clause(filter);
        let day_clause = if day_start.is_some() && day_end.is_some() {
            "AND timestamp >= ? AND timestamp < ?"
        } else {
            ""
        };
        let sql = format!(
            "SELECT id, sender_id, sender_name, receiver_id, content, msg_type, file_path, file_name, file_size, timestamp, is_read, client_msg_id
             FROM messages
             WHERE ((sender_id IN ({my_placeholders}) AND receiver_id IN ({peer_placeholders}))
                OR (sender_id IN ({peer_placeholders}) AND receiver_id IN ({my_placeholders})))
               AND (content LIKE ? OR file_name LIKE ?)
               {filter_clause}
               {day_clause}
             ORDER BY id DESC
             LIMIT ?"
        );
        let mut query = sqlx::query(&sql);
        for key in &my_keys {
            query = query.bind(key);
        }
        for key in &peer_keys {
            query = query.bind(key);
        }
        for key in &peer_keys {
            query = query.bind(key);
        }
        for key in &my_keys {
            query = query.bind(key);
        }
        query = query.bind(&pattern).bind(&pattern);
        if let (Some(day_start), Some(day_end)) = (day_start, day_end) {
            query = query.bind(day_start).bind(day_end);
        }
        let rows = query
            .bind(limit)
            .fetch_all(&self.pool)
            .await
            .context("Failed to search conversation messages")?;

        Ok(rows.iter().map(chat_message_from_row).collect())
    }

    pub async fn search_group_messages(
        &self,
        group_id: &str,
        query: &str,
        limit: Option<i64>,
        filter: Option<&str>,
        day_start: Option<&str>,
        day_end: Option<&str>,
    ) -> Result<Vec<ChatMessage>> {
        let pattern = format!("%{}%", query);
        let limit = normalize_search_limit(limit);
        let filter_clause = message_filter_clause(filter);
        let day_clause = if day_start.is_some() && day_end.is_some() {
            "AND timestamp >= ? AND timestamp < ?"
        } else {
            ""
        };
        let sql = format!(
            "SELECT id, sender_id, sender_name, receiver_id, content, msg_type, file_path, file_name, file_size, timestamp, is_read, client_msg_id
             FROM messages
             WHERE group_id = ?
               AND (content LIKE ? OR file_name LIKE ?)
               {filter_clause}
               {day_clause}
             ORDER BY id DESC
             LIMIT ?"
        );
        let mut query = sqlx::query(&sql)
            .bind(group_id)
            .bind(&pattern)
            .bind(&pattern);
        if let (Some(day_start), Some(day_end)) = (day_start, day_end) {
            query = query.bind(day_start).bind(day_end);
        }
        let rows = query
            .bind(limit)
            .fetch_all(&self.pool)
            .await
            .context("Failed to search group messages")?;

        Ok(rows.iter().map(chat_message_from_row).collect())
    }

    pub async fn search_messages(&self, my_id: &str, query: &str) -> Result<Vec<ChatMessage>> {
        let pattern = format!("%{}%", query);
        let my_keys = self.identity_keys_for(my_id).await?;
        let my_placeholders = Self::placeholders(my_keys.len());
        let sql = format!(
            "SELECT id, sender_id, sender_name, receiver_id, content, msg_type, file_path, file_name, file_size, timestamp, is_read, client_msg_id
             FROM messages
             WHERE (sender_id IN ({my_placeholders}) OR receiver_id IN ({my_placeholders}))
               AND (content LIKE ? OR file_name LIKE ?)
             ORDER BY id DESC
             LIMIT 100"
        );
        let mut query = sqlx::query(&sql);
        for key in &my_keys {
            query = query.bind(key);
        }
        for key in &my_keys {
            query = query.bind(key);
        }
        let rows = query
            .bind(&pattern)
            .bind(&pattern)
            .fetch_all(&self.pool)
            .await
            .context("Failed to search messages")?;

        Ok(rows.iter().map(chat_message_from_row).collect())
    }

    pub async fn mark_read(&self, sender_id: &str, receiver_id: &str) -> Result<()> {
        let sender_keys = self.identity_keys_for(sender_id).await?;
        let receiver_keys = self.identity_keys_for(receiver_id).await?;
        let sender_placeholders = Self::placeholders(sender_keys.len());
        let receiver_placeholders = Self::placeholders(receiver_keys.len());
        let sql = format!(
            "UPDATE messages
             SET is_read = 1
             WHERE receiver_id IN ({receiver_placeholders})
               AND (
                    sender_id IN ({sender_placeholders})
                    OR (
                        sender_name <> ''
                        AND sender_name = (
                            SELECT username FROM peers
                            WHERE peer_id IN ({sender_placeholders}) AND username <> ''
                            LIMIT 1
                        )
                    )
               )"
        );
        let mut query = sqlx::query(&sql);
        for key in &receiver_keys {
            query = query.bind(key);
        }
        for key in &sender_keys {
            query = query.bind(key);
        }
        for key in &sender_keys {
            query = query.bind(key);
        }
        query
            .execute(&self.pool)
            .await
            .context("Failed to mark messages as read")?;
        Ok(())
    }

    pub async fn get_unread_counts(&self, my_id: &str) -> Result<Vec<UnreadCount>> {
        let my_keys = self.identity_keys_for(my_id).await?;
        let my_placeholders = Self::placeholders(my_keys.len());
        let sql = format!(
            "WITH unread AS (
                 SELECT m.sender_id,
                        COALESCE(NULLIF(p.node_id, ''), NULLIF(pa.node_id, ''), '') AS resolved_node_id,
                        COUNT(*) AS cnt,
                        COALESCE(NULLIF(p.username, ''), NULLIF(MAX(m.sender_name), ''), m.sender_id) AS username
                 FROM messages m
                 LEFT JOIN peers p ON m.sender_id = p.peer_id
                 LEFT JOIN peer_aliases pa ON m.sender_id = pa.alias_peer_id
                 WHERE m.receiver_id IN ({my_placeholders}) AND m.is_read = 0
                 GROUP BY m.sender_id,
                          COALESCE(NULLIF(p.node_id, ''), NULLIF(pa.node_id, ''), '')
             )
             SELECT COALESCE(
                        (
                            SELECT p2.peer_id
                            FROM peers p2
                            WHERE p2.node_id = unread.resolved_node_id
                              AND TRIM(p2.node_id) <> ''
                            ORDER BY p2.is_online DESC, p2.last_seen_at DESC
                            LIMIT 1
                        ),
                        unread.sender_id
                    ) AS sender_id,
                    SUM(unread.cnt) AS cnt,
                    COALESCE(NULLIF(MAX(unread.username), ''), unread.sender_id) AS username
             FROM unread
             GROUP BY CASE
                          WHEN unread.resolved_node_id <> '' THEN unread.resolved_node_id
                          ELSE unread.sender_id
                      END"
        );
        let mut query = sqlx::query(&sql);
        for key in &my_keys {
            query = query.bind(key);
        }
        let rows = query
            .fetch_all(&self.pool)
            .await
            .context("Failed to get unread counts")?;

        Ok(rows
            .iter()
            .map(|row| UnreadCount {
                peer_id: row.get("sender_id"),
                count: row.get::<i64, _>("cnt") as u32,
                username: row.get("username"),
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::Database;

    fn temp_db_path(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "echo-{}-{}-{}.sqlite",
            label,
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ))
    }

    #[tokio::test]
    async fn initializes_fresh_database_with_group_message_index() {
        let db_path = temp_db_path("fresh-db");
        let db_path_str = db_path.to_string_lossy().to_string();

        let db = Database::new(&db_path_str)
            .await
            .expect("fresh database should initialize");
        drop(db);

        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test]
    async fn deduplicates_messages_by_sender_group_and_client_msg_id() {
        let db_path = temp_db_path("dedupe");
        let db_path_str = db_path.to_string_lossy().to_string();
        let db = Database::new(&db_path_str)
            .await
            .expect("database should initialize");

        let first = db
            .save_message_dedup(
                "sender",
                "Sender",
                "receiver",
                "hello",
                "text",
                None,
                None,
                None,
                Some("client-1"),
            )
            .await
            .expect("first private message should save");
        let duplicate = db
            .save_message_dedup(
                "sender",
                "Sender",
                "receiver",
                "hello again",
                "text",
                None,
                None,
                None,
                Some("client-1"),
            )
            .await
            .expect("duplicate private message should resolve");

        assert_eq!(first.id, duplicate.id);
        assert_eq!(first.content, duplicate.content);

        let group_first = db
            .save_group_message_dedup(
                "group-1",
                "sender",
                "Sender",
                "group hello",
                "text",
                None,
                None,
                None,
                false,
                Some("client-2"),
            )
            .await
            .expect("first group message should save");
        let group_duplicate = db
            .save_group_message_dedup(
                "group-1",
                "sender",
                "Sender",
                "group hello again",
                "text",
                None,
                None,
                None,
                false,
                Some("client-2"),
            )
            .await
            .expect("duplicate group message should resolve");

        assert_eq!(group_first.id, group_duplicate.id);
        assert_eq!(group_first.content, group_duplicate.content);

        drop(db);
        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test]
    async fn filters_contacts_without_identity_from_storage_and_recent() {
        let db_path = temp_db_path("dirty-contact");
        let db_path_str = db_path.to_string_lossy().to_string();
        let db = Database::new(&db_path_str)
            .await
            .expect("database should initialize");

        db.upsert_peer("10.0.0.2:9527", "", "", "10.0.0.2", 9527, false)
            .await
            .expect("dirty peer should be ignored without error");
        db.add_recent_contact("10.0.0.2:9527")
            .await
            .expect("recent row can exist independently");

        assert!(db
            .get_stored_peer("10.0.0.2:9527")
            .await
            .expect("dirty peer lookup should succeed")
            .is_none());
        assert!(db
            .list_stored_peers()
            .await
            .expect("stored peers should load")
            .is_empty());
        assert!(db
            .list_recent_contacts()
            .await
            .expect("recent contacts should load")
            .is_empty());

        db.upsert_peer("10.0.0.3:9527", "Manual", "", "10.0.0.3", 9527, true)
            .await
            .expect("manual contact should persist");
        db.add_recent_contact("10.0.0.3:9527")
            .await
            .expect("manual contact should be recent");

        let recent = db
            .list_recent_contacts()
            .await
            .expect("recent contacts should load");
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].peer_id, "10.0.0.3:9527");

        drop(db);
        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test]
    async fn startup_removes_legacy_dirty_contacts() {
        let db_path = temp_db_path("legacy-dirty-contact");
        let db_path_str = db_path.to_string_lossy().to_string();
        let db = Database::new(&db_path_str)
            .await
            .expect("database should initialize");
        let now = chrono::Utc::now().to_rfc3339();

        sqlx::query(
            "INSERT INTO peers (peer_id, username, department, software_version, mac_address, avatar_path, avatar_hash, avatar_updated_at, ip, port, is_online, first_seen_at, last_seen_at)
             VALUES (?, '', '', '', '', '', '', 0, ?, ?, 0, ?, ?)",
        )
        .bind("10.0.0.8:9527")
        .bind("10.0.0.8")
        .bind(9527_i64)
        .bind(&now)
        .bind(&now)
        .execute(&db.pool)
        .await
        .expect("legacy dirty peer should insert");
        sqlx::query(
            "INSERT INTO peers (peer_id, username, department, software_version, mac_address, avatar_path, avatar_hash, avatar_updated_at, ip, port, is_online, first_seen_at, last_seen_at)
             VALUES (?, 'Alice', 'Ops', '', '', '', '', 0, ?, ?, 1, ?, ?)",
        )
        .bind("10.0.0.9:9527")
        .bind("10.0.0.9")
        .bind(9527_i64)
        .bind(&now)
        .bind(&now)
        .execute(&db.pool)
        .await
        .expect("valid legacy peer should insert");
        db.add_recent_contact("10.0.0.8:9527")
            .await
            .expect("dirty recent should insert");
        db.add_recent_contact("10.0.0.9:9527")
            .await
            .expect("valid recent should insert");
        drop(db);

        let db = Database::new(&db_path_str)
            .await
            .expect("database should reopen and clean legacy dirty contacts");
        let dirty_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM peers WHERE peer_id = ?")
            .bind("10.0.0.8:9527")
            .fetch_one(&db.pool)
            .await
            .expect("dirty peer count should load");
        assert_eq!(dirty_count, 0);

        let recent = db
            .list_recent_contacts()
            .await
            .expect("recent contacts should load");
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].peer_id, "10.0.0.9:9527");

        drop(db);
        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test]
    async fn empty_identity_updates_do_not_overwrite_existing_peer_identity() {
        let db_path = temp_db_path("preserve-contact");
        let db_path_str = db_path.to_string_lossy().to_string();
        let db = Database::new(&db_path_str)
            .await
            .expect("database should initialize");

        db.upsert_peer("peer-1", "Alice", "Ops", "10.0.0.4", 9527, true)
            .await
            .expect("named peer should persist");
        db.upsert_peer("peer-1", "", "", "10.0.0.5", 9527, false)
            .await
            .expect("empty identity update should preserve existing identity");
        db.upsert_peer("peer-1", "   ", "   ", "10.0.0.6", 9527, false)
            .await
            .expect("blank identity update should preserve existing identity");

        let stored = db
            .get_stored_peer("peer-1")
            .await
            .expect("stored peer should load")
            .expect("peer should still exist");
        assert_eq!(stored.username, "Alice");
        assert_eq!(stored.department, "Ops");
        assert_eq!(stored.ip, "10.0.0.6");
        assert!(!stored.is_online);

        drop(db);
        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test]
    async fn endpoint_peer_id_drift_migrates_to_current_endpoint() {
        let db_path = temp_db_path("endpoint-drift");
        let db_path_str = db_path.to_string_lossy().to_string();
        let db = Database::new(&db_path_str)
            .await
            .expect("database should initialize");

        db.upsert_peer_with_avatar(
            "10.0.0.2:9527",
            "Alice",
            "Ops",
            "1.0.0",
            "aa:bb:cc",
            "/tmp/alice.png",
            "avatar-old",
            10,
            "10.0.0.2",
            9527,
            true,
        )
        .await
        .expect("old endpoint should persist");
        db.add_recent_contact("10.0.0.2:9527")
            .await
            .expect("old endpoint should become recent");
        db.save_message(
            "me",
            "Me",
            "10.0.0.2:9527",
            "old endpoint message",
            "text",
            None,
            None,
            None,
            None,
        )
        .await
        .expect("message to old endpoint should persist");

        db.upsert_peer_with_avatar(
            "10.0.0.2:9527",
            "",
            "",
            "",
            "",
            "",
            "avatar-new",
            20,
            "10.0.0.9",
            9527,
            true,
        )
        .await
        .expect("drifted endpoint should migrate");

        assert!(db
            .get_stored_peer("10.0.0.2:9527")
            .await
            .expect("old peer lookup should succeed")
            .is_none());

        let migrated = db
            .get_stored_peer("10.0.0.9:9527")
            .await
            .expect("new peer lookup should succeed")
            .expect("new endpoint should exist");
        assert_eq!(migrated.username, "Alice");
        assert_eq!(migrated.department, "Ops");
        assert_eq!(migrated.software_version, "1.0.0");
        assert_eq!(migrated.mac_address, "aa:bb:cc");
        assert_eq!(migrated.avatar_path, "");
        assert_eq!(migrated.avatar_hash, "avatar-new");
        assert_eq!(migrated.avatar_updated_at, 20);
        assert_eq!(migrated.ip, "10.0.0.9");

        let recent = db
            .list_recent_contacts()
            .await
            .expect("recent contacts should load");
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].peer_id, "10.0.0.9:9527");

        let old_messages = db
            .get_conversation("10.0.0.2:9527", "me", Some(10))
            .await
            .expect("old endpoint messages should load");
        let new_messages = db
            .get_conversation("10.0.0.9:9527", "me", Some(10))
            .await
            .expect("new endpoint messages should load");
        assert!(old_messages.is_empty());
        assert_eq!(new_messages.len(), 1);
        assert_eq!(new_messages[0].content, "old endpoint message");

        drop(db);
        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test]
    async fn user_profile_endpoint_peer_id_drift_migrates_self_references() {
        let db_path = temp_db_path("profile-endpoint-drift");
        let db_path_str = db_path.to_string_lossy().to_string();
        let db = Database::new(&db_path_str)
            .await
            .expect("database should initialize");

        db.save_user_profile("10.0.0.2:9527", "Me", "Ops", "1.0.0", "aa:bb:cc")
            .await
            .expect("old profile should save");
        db.upsert_peer_with_avatar(
            "10.0.0.2:9527",
            "Me",
            "Ops",
            "1.0.0",
            "aa:bb:cc",
            "/tmp/me.png",
            "self-avatar",
            10,
            "10.0.0.2",
            9527,
            true,
        )
        .await
        .expect("legacy self peer row should persist");
        db.upsert_peer("10.0.0.3:9527", "Alice", "Ops", "10.0.0.3", 9527, true)
            .await
            .expect("peer should persist");
        db.save_message(
            "10.0.0.2:9527",
            "Me",
            "10.0.0.3:9527",
            "hello before ip change",
            "text",
            None,
            None,
            None,
            None,
        )
        .await
        .expect("self message should persist");
        db.create_group(
            "group-1",
            "Ops Group",
            "10.0.0.2:9527",
            &["10.0.0.2:9527".to_string(), "10.0.0.3:9527".to_string()],
        )
        .await
        .expect("group should persist");
        db.store_pending_group_msg(
            "group-1",
            "10.0.0.3:9527",
            "10.0.0.2:9527",
            "Me",
            "queued group message",
            "text",
            "2026-01-01T00:00:00Z",
        )
        .await
        .expect("legacy pending group message should persist");
        db.queue_pending_notification(
            "10.0.0.3:9527",
            "group_created",
            r#"{"sender_id":"10.0.0.2:9527","receiver_id":"10.0.0.3:9527","content":{"member_ids":["10.0.0.2:9527","10.0.0.3:9527"]}}"#,
        )
        .await
        .expect("pending notification should persist");
        db.queue_pending_file_transfer(
            "group-1",
            "10.0.0.3:9527",
            "10.0.0.2:9527",
            "Me",
            "Ops",
            9527,
            "/tmp/queued-file.txt",
            "queued-file.txt",
            1,
            "file",
            Some("client-queued-file"),
        )
        .await
        .expect("pending file transfer should persist");

        db.save_user_profile("10.0.0.9:9527", "Me", "Ops", "1.0.1", "aa:bb:cc")
            .await
            .expect("new profile should save and migrate references");

        let profile = db
            .get_user_profile()
            .await
            .expect("profile should load")
            .expect("profile should exist");
        assert_eq!(profile.peer_id, "10.0.0.9:9527");
        assert_eq!(profile.software_version, "1.0.1");

        assert!(db
            .get_stored_peer("10.0.0.2:9527")
            .await
            .expect("old self peer lookup should succeed")
            .is_none());
        assert!(db
            .get_stored_peer("10.0.0.9:9527")
            .await
            .expect("new self peer lookup should succeed")
            .is_none());

        let old_messages = db
            .get_conversation("10.0.0.3:9527", "10.0.0.2:9527", Some(10))
            .await
            .expect("old self conversation lookup should load");
        let new_messages = db
            .get_conversation("10.0.0.3:9527", "10.0.0.9:9527", Some(10))
            .await
            .expect("new self conversation lookup should load");
        assert!(old_messages.is_empty());
        assert_eq!(new_messages.len(), 1);
        assert_eq!(new_messages[0].sender_id, "10.0.0.9:9527");
        assert_eq!(new_messages[0].receiver_id, "10.0.0.3:9527");

        let groups = db
            .list_groups("10.0.0.9:9527")
            .await
            .expect("groups should load for new self id");
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].creator_id, "10.0.0.9:9527");

        let pending_group = db
            .get_pending_for_peer("10.0.0.3:9527")
            .await
            .expect("pending group messages should load");
        assert_eq!(pending_group.len(), 1);
        assert_eq!(pending_group[0].sender_id, "10.0.0.9:9527");

        let notifications = db
            .get_pending_notifications("10.0.0.3:9527")
            .await
            .expect("pending notifications should load");
        assert_eq!(notifications.len(), 1);
        assert!(notifications[0].2.contains("10.0.0.9:9527"));
        assert!(!notifications[0].2.contains("10.0.0.2:9527"));

        let pending_files = db
            .get_pending_file_transfers("10.0.0.3:9527")
            .await
            .expect("pending file transfers should load");
        assert_eq!(pending_files.len(), 1);
        assert_eq!(pending_files[0].sender_id, "10.0.0.9:9527");

        drop(db);
        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test]
    async fn same_identity_at_new_endpoint_stays_separate() {
        let db_path = temp_db_path("same-identity-new-endpoint");
        let db_path_str = db_path.to_string_lossy().to_string();
        let db = Database::new(&db_path_str)
            .await
            .expect("database should initialize");

        db.upsert_peer("10.0.0.2:9527", "Alice", "Ops", "10.0.0.2", 9527, true)
            .await
            .expect("first endpoint should persist");
        db.add_recent_contact("10.0.0.2:9527")
            .await
            .expect("first endpoint should become recent");
        db.save_message(
            "me",
            "Me",
            "10.0.0.2:9527",
            "old endpoint message",
            "text",
            None,
            None,
            None,
            None,
        )
        .await
        .expect("message to old endpoint should persist");

        db.upsert_peer("10.0.0.3:9527", "Alice", "Ops", "10.0.0.3", 9527, true)
            .await
            .expect("new endpoint should persist as a separate contact");

        let old_peer = db
            .get_stored_peer("10.0.0.2:9527")
            .await
            .expect("old peer lookup should succeed")
            .expect("old endpoint should remain");
        let new_peer = db
            .get_stored_peer("10.0.0.3:9527")
            .await
            .expect("new peer lookup should succeed")
            .expect("new endpoint should exist");
        assert_eq!(old_peer.username, "Alice");
        assert_eq!(new_peer.username, "Alice");

        let recent = db
            .list_recent_contacts()
            .await
            .expect("recent contacts should load");
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].peer_id, "10.0.0.2:9527");

        let old_messages = db
            .get_conversation("me", "10.0.0.2:9527", Some(10))
            .await
            .expect("old endpoint messages should load");
        let new_messages = db
            .get_conversation("me", "10.0.0.3:9527", Some(10))
            .await
            .expect("new endpoint messages should load");
        assert_eq!(old_messages.len(), 1);
        assert!(new_messages.is_empty());

        drop(db);
        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test]
    async fn conversation_resolves_messages_across_peer_aliases() {
        let db_path = temp_db_path("node-alias-conversation");
        let db_path_str = db_path.to_string_lossy().to_string();
        let db = Database::new(&db_path_str)
            .await
            .expect("database should initialize");

        db.upsert_peer_with_node_id_avatar(
            "10.0.0.2:9527",
            "node-alice",
            "Alice",
            "Ops",
            "1.0.0",
            "aa:bb:cc",
            "",
            "",
            0,
            "10.0.0.2",
            9527,
            true,
        )
        .await
        .expect("old endpoint should persist with node_id");
        db.save_message(
            "me",
            "Me",
            "10.0.0.2:9527",
            "message before DHCP change",
            "text",
            None,
            None,
            None,
            None,
        )
        .await
        .expect("message to old endpoint should save");

        db.upsert_peer_with_node_id_avatar(
            "10.0.0.9:9527",
            "node-alice",
            "Alice",
            "Ops",
            "1.0.1",
            "aa:bb:cc",
            "",
            "",
            0,
            "10.0.0.9",
            9527,
            true,
        )
        .await
        .expect("new endpoint should update same node_id");

        let messages = db
            .get_conversation("10.0.0.9:9527", "me", Some(10))
            .await
            .expect("new endpoint conversation should include old alias messages");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, "message before DHCP change");

        let aliases = db
            .list_peer_aliases("node-alice")
            .await
            .expect("aliases should load");
        assert!(aliases.iter().any(|alias| alias == "10.0.0.2:9527"));
        assert!(aliases.iter().any(|alias| alias == "10.0.0.9:9527"));

        drop(db);
        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test]
    async fn alias_identity_covers_history_search_and_read_state() {
        let db_path = temp_db_path("node-alias-history-search-read");
        let db_path_str = db_path.to_string_lossy().to_string();
        let db = Database::new(&db_path_str)
            .await
            .expect("database should initialize");

        db.upsert_peer_with_node_id_avatar(
            "10.0.0.2:9527",
            "node-alice",
            "Alice",
            "Ops",
            "1.0.0",
            "aa:bb:cc",
            "",
            "",
            0,
            "10.0.0.2",
            9527,
            true,
        )
        .await
        .expect("old endpoint should persist with node_id");
        db.save_message(
            "10.0.0.2:9527",
            "Alice",
            "me",
            "searchable message before DHCP change",
            "text",
            None,
            None,
            None,
            None,
        )
        .await
        .expect("old endpoint message should save");

        db.upsert_peer_with_node_id_avatar(
            "10.0.0.9:9527",
            "node-alice",
            "Alice",
            "Ops",
            "1.0.1",
            "aa:bb:cc",
            "",
            "",
            0,
            "10.0.0.9",
            9527,
            true,
        )
        .await
        .expect("new endpoint should update same node_id");

        let history = db
            .get_conversation_history("10.0.0.9:9527", "me", None, Some(10), None, None, None)
            .await
            .expect("history should resolve aliases");
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].content, "searchable message before DHCP change");

        let conversation_search = db
            .search_conversation_messages("10.0.0.9:9527", "me", "DHCP", Some(10), None, None, None)
            .await
            .expect("conversation search should resolve aliases");
        assert_eq!(conversation_search.len(), 1);

        let global_search = db
            .search_messages("me", "searchable")
            .await
            .expect("global search should include messages to current self id");
        assert_eq!(global_search.len(), 1);

        let unread = db
            .get_unread_counts("me")
            .await
            .expect("unread counts should resolve aliases");
        assert_eq!(unread.len(), 1);
        assert_eq!(unread[0].peer_id, "10.0.0.9:9527");
        assert_eq!(unread[0].count, 1);

        db.mark_read("10.0.0.9:9527", "me")
            .await
            .expect("mark_read should resolve aliases");
        let unread_after = db
            .get_unread_counts("me")
            .await
            .expect("unread counts should reload");
        assert!(unread_after.is_empty());

        drop(db);
        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test]
    async fn user_profile_persists_node_id_across_peer_id_changes() {
        let db_path = temp_db_path("profile-node-id");
        let db_path_str = db_path.to_string_lossy().to_string();
        let db = Database::new(&db_path_str)
            .await
            .expect("database should initialize");

        db.save_user_profile("10.0.0.2:9527", "Me", "Ops", "1.0.0", "aa:bb:cc")
            .await
            .expect("profile should save");
        let first_node_id = db
            .ensure_user_node_id()
            .await
            .expect("node_id should be generated");

        db.save_user_profile("10.0.0.9:9527", "Me", "Ops", "1.0.1", "aa:bb:cc")
            .await
            .expect("profile peer_id change should save");
        let profile = db
            .get_user_profile()
            .await
            .expect("profile should load")
            .expect("profile should exist");

        assert_eq!(profile.node_id, first_node_id);
        assert_eq!(profile.peer_id, "10.0.0.9:9527");

        drop(db);
        let _ = std::fs::remove_file(db_path);
    }
}
