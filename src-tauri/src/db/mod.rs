use anyhow::{Context, Result};
use chrono::Utc;
use log::info;
use serde::{Deserialize, Serialize};
use sqlx::{sqlite::SqlitePoolOptions, Pool, Row, Sqlite};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserProfile {
    pub peer_id: String,
    pub username: String,
    pub department: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredPeer {
    pub peer_id: String,
    pub username: String,
    pub department: String,
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
}

pub struct Database {
    pool: Pool<Sqlite>,
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
                username TEXT NOT NULL,
                department TEXT NOT NULL
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

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS peers (
                peer_id TEXT PRIMARY KEY,
                username TEXT NOT NULL,
                department TEXT NOT NULL,
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
            "DELETE FROM peers
             WHERE rowid NOT IN (
                SELECT MAX(rowid) FROM peers GROUP BY ip, port
             )",
        )
        .execute(&self.pool)
        .await
        .context("Failed to clean duplicate peer endpoints")?;

        info!("Database initialized successfully.");
        Ok(())
    }

    pub async fn get_user_profile(&self) -> Result<Option<UserProfile>> {
        let row = sqlx::query("SELECT peer_id, username, department FROM user_profile WHERE id = 1")
            .fetch_optional(&self.pool)
            .await
            .context("Failed to load user profile")?;

        Ok(row.map(|row| UserProfile {
            peer_id: row.try_get::<Option<String>, _>("peer_id").ok().flatten().unwrap_or_default(),
            username: row.get("username"),
            department: row.get("department"),
        }))
    }

    pub async fn save_user_profile(&self, peer_id: &str, username: &str, department: &str) -> Result<()> {
        sqlx::query(
            "INSERT INTO user_profile (id, peer_id, username, department)
             VALUES (1, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                peer_id = excluded.peer_id,
                username = excluded.username,
                department = excluded.department",
        )
        .bind(peer_id)
        .bind(username)
        .bind(department)
        .execute(&self.pool)
        .await
        .context("Failed to save user profile")?;

        Ok(())
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
        let now = Utc::now().to_rfc3339();

        sqlx::query("DELETE FROM peers WHERE (ip = ? AND port = ?) OR (username = ? AND department = ? AND peer_id <> ?)")
            .bind(ip)
            .bind(port as i64)
            .bind(username)
            .bind(department)
            .bind(peer_id)
            .execute(&self.pool)
            .await
            .context("Failed to remove duplicate peer endpoints")?;

        sqlx::query(
            "INSERT INTO peers (peer_id, username, department, ip, port, is_online, first_seen_at, last_seen_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(peer_id) DO UPDATE SET
                username = excluded.username,
                department = excluded.department,
                ip = excluded.ip,
                port = excluded.port,
                is_online = excluded.is_online,
                last_seen_at = excluded.last_seen_at",
        )
        .bind(peer_id)
        .bind(username)
        .bind(department)
        .bind(ip)
        .bind(port as i64)
        .bind(if is_online { 1 } else { 0 })
        .bind(&now)
        .bind(&now)
        .execute(&self.pool)
        .await
        .context("Failed to upsert peer")?;

        Ok(())
    }

    #[allow(dead_code)]
    pub async fn mark_peer_offline(&self, peer_id: &str) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query("UPDATE peers SET is_online = 0, last_seen_at = ? WHERE peer_id = ?")
            .bind(now)
            .bind(peer_id)
            .execute(&self.pool)
            .await
            .context("Failed to mark peer offline")?;
        Ok(())
    }

    pub async fn list_stored_peers(&self) -> Result<Vec<StoredPeer>> {
        let rows = sqlx::query(
            "SELECT peer_id, username, department, ip, port, is_online, first_seen_at, last_seen_at
             FROM peers
             ORDER BY is_online DESC, last_seen_at DESC",
        )
        .fetch_all(&self.pool)
        .await
        .context("Failed to list stored peers")?;

        Ok(rows
            .iter()
            .map(|row| StoredPeer {
                peer_id: row.get("peer_id"),
                username: row.get("username"),
                department: row.get("department"),
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
            "SELECT peer_id, username, department, ip, port, is_online, first_seen_at, last_seen_at
             FROM peers
             WHERE peer_id = ?",
        )
        .bind(peer_id)
        .fetch_optional(&self.pool)
        .await
        .context("Failed to get stored peer")?;

        Ok(row.map(|row| StoredPeer {
            peer_id: row.get("peer_id"),
            username: row.get("username"),
            department: row.get("department"),
            ip: row.get("ip"),
            port: row.get::<i64, _>("port") as u16,
            is_online: row.get::<bool, _>("is_online"),
            first_seen_at: row.get("first_seen_at"),
            last_seen_at: row.get("last_seen_at"),
        }))
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
    ) -> Result<ChatMessage> {
        let timestamp = Utc::now().to_rfc3339();

        let result = sqlx::query(
            "INSERT INTO messages (sender_id, sender_name, receiver_id, content, msg_type, file_path, file_name, file_size, timestamp, is_read)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 0)"
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
        .execute(&self.pool)
        .await
        .context("Failed to save message")?;

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
        })
    }

    pub async fn get_conversation(&self, peer_id: &str, my_id: &str) -> Result<Vec<ChatMessage>> {
        log::info!("get_conversation: peer_id={}, my_id={}", peer_id, my_id);
        let rows = sqlx::query(
            "SELECT id, sender_id, sender_name, receiver_id, content, msg_type, file_path, file_name, file_size, timestamp, is_read
             FROM messages
             WHERE (sender_id = ? AND receiver_id = ?) OR (sender_id = ? AND receiver_id = ?)
             ORDER BY id ASC"
        )
        .bind(my_id)
        .bind(peer_id)
        .bind(peer_id)
        .bind(my_id)
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
            })
            .collect();

        Ok(messages)
    }

    pub async fn mark_read(&self, sender_id: &str, receiver_id: &str) -> Result<()> {
        sqlx::query("UPDATE messages SET is_read = 1 WHERE sender_id = ? AND receiver_id = ?")
            .bind(sender_id)
            .bind(receiver_id)
            .execute(&self.pool)
            .await
            .context("Failed to mark messages as read")?;
        Ok(())
    }

    pub async fn get_unread_counts(&self, my_id: &str) -> Result<Vec<UnreadCount>> {
        let rows = sqlx::query(
            "SELECT sender_id, COUNT(*) as cnt
             FROM messages
             WHERE receiver_id = ? AND is_read = 0
             GROUP BY sender_id",
        )
        .bind(my_id)
        .fetch_all(&self.pool)
        .await
        .context("Failed to get unread counts")?;

        Ok(rows
            .iter()
            .map(|row| UnreadCount {
                peer_id: row.get("sender_id"),
                count: row.get::<i64, _>("cnt") as u32,
            })
            .collect())
    }
}