pub mod agents;
pub mod channels;
pub mod knowledge;
pub mod messages;
pub mod tasks;
pub mod teams;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::Result;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

pub use agents::AgentRecordUpsert;
pub use agents::{Agent, AgentConfig, AgentEnvVar, AgentStatus, Human};
pub use channels::{Channel, ChannelListParams, ChannelMember, ChannelMemberProfile, ChannelType};
pub use knowledge::{
    KnowledgeEntry, RecallQuery, RecallResponse, RememberRequest, RememberResponse,
};
pub use messages::{
    ActivityMessage, AttachmentRef, ForwardedFrom, HistoryMessage, Message, ReceivedMessage,
    SenderType,
};
pub use tasks::{ClaimResult, Task, TaskInfo, TaskStatus};
pub use teams::{Team, TeamMember, TeamMembership};

// ── Types that live in store/mod.rs ──

/// Binary upload metadata persisted in SQLite and on disk under `attachments/`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    /// Random UUID primary key referenced by messages.
    pub id: String,
    /// Original client filename.
    pub filename: String,
    /// MIME type reported at upload.
    pub mime_type: String,
    /// Byte length on disk.
    pub size_bytes: i64,
    /// Path relative to the server data dir where the file is stored.
    pub stored_path: String,
    /// When the row was created.
    pub uploaded_at: chrono::DateTime<chrono::Utc>,
}

/// SQLite-backed persistence and pub/sub for new messages.
pub struct Store {
    /// Serialized access to the rusqlite connection.
    conn: Mutex<Connection>,
    /// Broadcast channel: `(channel_id, message_id)` for wake / notify.
    msg_tx: broadcast::Sender<(String, String)>,
    /// Root data directory (db parent, attachments, agents, teams).
    data_dir: PathBuf,
}

impl Store {
    pub const DEFAULT_SYSTEM_CHANNEL: &'static str = "all";
    pub const DEFAULT_SYSTEM_CHANNEL_DESCRIPTION: &'static str = "All members";
    pub const SHARED_MEMORY_CHANNEL: &'static str = "shared-memory";
    pub const SHARED_MEMORY_DESCRIPTION: &'static str =
        "Agent group memory — breadcrumbs posted here by mcp_chat_remember";

    /// Built-in system channels can be surfaced separately in the UI without
    /// necessarily being write-protected. Only protected channels should block
    /// direct human or agent posts.
    pub fn is_system_channel_read_only(name: &str) -> bool {
        name == Self::SHARED_MEMORY_CHANNEL
    }

    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        Self::init_schema(&conn)?;
        Self::migrate_remove_spurious_dm_members(&conn)?;
        let (msg_tx, _) = broadcast::channel(256);
        Ok(Self {
            conn: Mutex::new(conn),
            msg_tx,
            data_dir: derive_data_dir(path),
        })
    }

    /// Return the configured server data directory that owns the SQLite file.
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// Return the directory used to persist uploaded attachments.
    pub fn attachments_dir(&self) -> PathBuf {
        self.data_dir.join("attachments")
    }

    /// Return the directory that contains per-agent workspaces.
    pub fn agents_dir(&self) -> PathBuf {
        self.data_dir.join("agents")
    }

    /// Return the directory used to persist per-team workspaces.
    pub fn teams_dir(&self) -> PathBuf {
        self.data_dir.join("teams")
    }

    /// Expose the raw connection guard for use in integration tests only.
    /// Not intended for production use.
    pub fn conn_for_test(&self) -> std::sync::MutexGuard<'_, rusqlite::Connection> {
        self.conn.lock().unwrap()
    }

    fn init_schema(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS channels (
                id TEXT PRIMARY KEY,
                name TEXT UNIQUE NOT NULL,
                description TEXT,
                channel_type TEXT NOT NULL DEFAULT 'channel',
                archived INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE IF NOT EXISTS channel_members (
                channel_id TEXT NOT NULL,
                member_name TEXT NOT NULL,
                member_type TEXT NOT NULL,
                last_read_seq INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (channel_id, member_name)
            );
            CREATE TABLE IF NOT EXISTS messages (
                id TEXT PRIMARY KEY,
                channel_id TEXT NOT NULL,
                thread_parent_id TEXT,
                sender_name TEXT NOT NULL,
                sender_type TEXT NOT NULL,
                sender_deleted INTEGER NOT NULL DEFAULT 0,
                content TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                seq INTEGER NOT NULL,
                UNIQUE(channel_id, seq)
            );
            CREATE TABLE IF NOT EXISTS message_attachments (
                message_id TEXT NOT NULL,
                attachment_id TEXT NOT NULL,
                PRIMARY KEY (message_id, attachment_id)
            );
            CREATE TABLE IF NOT EXISTS agents (
                id TEXT PRIMARY KEY,
                name TEXT UNIQUE NOT NULL,
                display_name TEXT NOT NULL,
                description TEXT,
                runtime TEXT NOT NULL,
                model TEXT NOT NULL,
                reasoning_effort TEXT,
                status TEXT NOT NULL DEFAULT 'inactive',
                session_id TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE IF NOT EXISTS agent_env_vars (
                agent_name TEXT NOT NULL,
                key TEXT NOT NULL,
                value TEXT NOT NULL,
                position INTEGER NOT NULL,
                PRIMARY KEY (agent_name, key),
                FOREIGN KEY (agent_name) REFERENCES agents(name) ON DELETE CASCADE
            );
            CREATE TABLE IF NOT EXISTS humans (
                name TEXT PRIMARY KEY,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE IF NOT EXISTS tasks (
                id TEXT PRIMARY KEY,
                channel_id TEXT NOT NULL,
                task_number INTEGER NOT NULL,
                title TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'todo',
                claimed_by TEXT,
                created_by TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                UNIQUE(channel_id, task_number)
            );
            CREATE TABLE IF NOT EXISTS attachments (
                id TEXT PRIMARY KEY,
                filename TEXT NOT NULL,
                mime_type TEXT NOT NULL,
                size_bytes INTEGER NOT NULL,
                stored_path TEXT NOT NULL,
                uploaded_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE IF NOT EXISTS shared_knowledge (
                id TEXT PRIMARY KEY,
                key TEXT NOT NULL,
                value TEXT NOT NULL,
                tags TEXT NOT NULL DEFAULT '',
                author_agent_id TEXT NOT NULL,
                channel_context TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE INDEX IF NOT EXISTS shared_knowledge_author ON shared_knowledge(author_agent_id);
            CREATE VIRTUAL TABLE IF NOT EXISTS knowledge_fts USING fts5(
                key,
                value,
                tags,
                content='shared_knowledge',
                content_rowid='rowid'
            );
            CREATE TRIGGER IF NOT EXISTS knowledge_fts_insert AFTER INSERT ON shared_knowledge BEGIN
                INSERT INTO knowledge_fts(rowid, key, value, tags)
                VALUES (new.rowid, new.key, new.value, new.tags);
            END;
            CREATE TRIGGER IF NOT EXISTS knowledge_fts_delete BEFORE DELETE ON shared_knowledge BEGIN
                INSERT INTO knowledge_fts(knowledge_fts, rowid, key, value, tags)
                VALUES ('delete', old.rowid, old.key, old.value, old.tags);
            END;
            CREATE TABLE IF NOT EXISTS teams (
                id TEXT PRIMARY KEY,
                name TEXT UNIQUE NOT NULL,
                display_name TEXT NOT NULL,
                collaboration_model TEXT NOT NULL,
                leader_agent_name TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE IF NOT EXISTS team_members (
                team_id TEXT NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
                member_name TEXT NOT NULL,
                member_type TEXT NOT NULL,
                member_id TEXT NOT NULL,
                role TEXT NOT NULL,
                joined_at TEXT NOT NULL DEFAULT (datetime('now')),
                PRIMARY KEY (team_id, member_name)
            );
            CREATE TABLE IF NOT EXISTS team_task_signals (
                id TEXT PRIMARY KEY,
                team_id TEXT NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
                trigger_message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
                member_name TEXT NOT NULL,
                signal TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                UNIQUE (trigger_message_id, member_name)
            );
            CREATE TABLE IF NOT EXISTS team_task_quorum (
                trigger_message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
                team_id TEXT NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
                member_name TEXT NOT NULL,
                resolved_at TEXT,
                PRIMARY KEY (trigger_message_id, member_name)
            );
            ",
        )?;
        conn.execute(
            "ALTER TABLE messages ADD COLUMN sender_deleted INTEGER NOT NULL DEFAULT 0",
            [],
        )
        .ok();
        conn.execute(
            "ALTER TABLE channels ADD COLUMN archived INTEGER NOT NULL DEFAULT 0",
            [],
        )
        .ok();
        conn.execute("ALTER TABLE agents ADD COLUMN reasoning_effort TEXT", [])
            .ok();
        conn.execute("ALTER TABLE messages ADD COLUMN forwarded_from TEXT", [])
            .ok();
        Ok(())
    }

    /// Ensure a system channel with the given name exists. Idempotent — safe to call on every startup.
    pub fn ensure_system_channel(&self, name: &str, description: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        Self::ensure_system_channel_inner(&conn, name, description)?;
        Ok(())
    }

    /// Ensure built-in channels exist and upgrade legacy `#general` installs to
    /// the new writable `#all` system channel without changing its stable id.
    pub fn ensure_builtin_channels(&self, default_human: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let all_id = if let Some(existing) =
            Self::find_channel_by_name_inner(&conn, Self::DEFAULT_SYSTEM_CHANNEL)?
        {
            conn.execute(
                "UPDATE channels
                 SET description = ?1, channel_type = 'system', archived = 0
                 WHERE id = ?2",
                params![Self::DEFAULT_SYSTEM_CHANNEL_DESCRIPTION, existing.id],
            )?;
            existing.id
        } else if let Some(legacy) = Self::find_channel_by_name_inner(&conn, "general")? {
            conn.execute(
                "UPDATE channels
                 SET name = ?1, description = ?2, channel_type = 'system', archived = 0
                 WHERE id = ?3",
                params![
                    Self::DEFAULT_SYSTEM_CHANNEL,
                    Self::DEFAULT_SYSTEM_CHANNEL_DESCRIPTION,
                    legacy.id
                ],
            )?;
            tracing::info!("migrated built-in channel #general to #all");
            legacy.id
        } else {
            let id = uuid::Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO channels (id, name, description, channel_type)
                 VALUES (?1, ?2, ?3, 'system')",
                params![
                    id,
                    Self::DEFAULT_SYSTEM_CHANNEL,
                    Self::DEFAULT_SYSTEM_CHANNEL_DESCRIPTION
                ],
            )?;
            tracing::info!(
                channel = Self::DEFAULT_SYSTEM_CHANNEL,
                "created built-in system channel"
            );
            id
        };

        conn.execute(
            "INSERT OR IGNORE INTO channel_members (channel_id, member_name, member_type, last_read_seq)
             VALUES (?1, ?2, 'human', 0)",
            params![all_id, default_human],
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO channel_members (channel_id, member_name, member_type, last_read_seq)
             SELECT ?1, name, 'human', 0 FROM humans",
            params![all_id],
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO channel_members (channel_id, member_name, member_type, last_read_seq)
             SELECT ?1, name, 'agent', 0 FROM agents",
            params![all_id],
        )?;

        Self::ensure_system_channel_inner(
            &conn,
            Self::SHARED_MEMORY_CHANNEL,
            Self::SHARED_MEMORY_DESCRIPTION,
        )?;
        Ok(())
    }

    fn ensure_system_channel_inner(conn: &Connection, name: &str, description: &str) -> Result<()> {
        let exists: i64 = conn.query_row(
            "SELECT COUNT(*) FROM channels WHERE name = ?1 AND channel_type = 'system'",
            params![name],
            |row| row.get(0),
        )?;
        if exists == 0 {
            let id = uuid::Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO channels (id, name, description, channel_type) VALUES (?1, ?2, ?3, 'system')",
                params![id, name, description],
            )?;
            tracing::info!(channel = %name, "created system channel");
        }
        Ok(())
    }

    /// One-time migration: remove agents that were incorrectly added to DM channels
    /// via `handle_create_agent`. A DM channel `dm-X-Y` must have exactly two members
    /// whose sorted names form the channel name. All other members are spurious.
    fn migrate_remove_spurious_dm_members(conn: &Connection) -> Result<()> {
        let dm_channels: Vec<(String, String)> = conn
            .prepare("SELECT id, name FROM channels WHERE channel_type = 'dm'")?
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .filter_map(|r| r.ok())
            .collect();

        for (channel_id, channel_name) in dm_channels {
            let members: Vec<String> = conn
                .prepare("SELECT member_name FROM channel_members WHERE channel_id = ?1")?
                .query_map(params![channel_id], |row| row.get(0))?
                .filter_map(|r| r.ok())
                .collect();

            if members.len() <= 2 {
                continue;
            }

            // Find the pair (m1, m2) whose sorted join equals the channel name.
            let mut correct: Option<(String, String)> = None;
            'outer: for i in 0..members.len() {
                for j in (i + 1)..members.len() {
                    let mut pair = [members[i].as_str(), members[j].as_str()];
                    pair.sort_unstable();
                    if format!("dm-{}-{}", pair[0], pair[1]) == channel_name {
                        correct = Some((members[i].clone(), members[j].clone()));
                        break 'outer;
                    }
                }
            }

            if let Some((m1, m2)) = correct {
                let removed = conn.execute(
                    "DELETE FROM channel_members WHERE channel_id = ?1 AND member_name NOT IN (?2, ?3)",
                    params![channel_id, m1, m2],
                )?;
                if removed > 0 {
                    tracing::info!(channel = %channel_name, removed, "removed spurious members from DM channel");
                }
            }
        }
        Ok(())
    }

    pub fn subscribe(&self) -> broadcast::Receiver<(String, String)> {
        self.msg_tx.subscribe()
    }

    // ── Sender type lookup ──

    pub fn lookup_sender_type(&self, name: &str) -> Result<Option<SenderType>> {
        let conn = self.conn.lock().unwrap();
        Self::lookup_sender_type_inner(&conn, name)
    }

    fn lookup_sender_type_inner(conn: &Connection, name: &str) -> Result<Option<SenderType>> {
        let agent_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM agents WHERE name = ?1",
            params![name],
            |row| row.get(0),
        )?;
        if agent_count > 0 {
            return Ok(Some(SenderType::Agent));
        }
        let human_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM humans WHERE name = ?1",
            params![name],
            |row| row.get(0),
        )?;
        if human_count > 0 {
            return Ok(Some(SenderType::Human));
        }
        Ok(None)
    }

    // ── Unread summary ──

    pub fn get_unread_summary(&self, agent_name: &str) -> Result<HashMap<String, i64>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT c.name, COUNT(m.id)
             FROM channel_members cm
             JOIN channels c ON cm.channel_id = c.id
             JOIN messages m ON m.channel_id = cm.channel_id AND m.seq > cm.last_read_seq AND m.thread_parent_id IS NULL
             WHERE cm.member_name = ?1
             GROUP BY c.name",
        )?;
        let rows = stmt
            .query_map(params![agent_name], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?
            .filter_map(|r| r.ok());

        let mut map = HashMap::new();
        for (name, count) in rows {
            map.insert(name, count);
        }
        Ok(map)
    }
}

/// Derive the server data directory from the SQLite path.
fn derive_data_dir(path: &str) -> PathBuf {
    if path == ":memory:" {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        return PathBuf::from(home).join(".chorus");
    }

    Path::new(path)
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf()
}

// ── Helpers (shared across submodules) ──

pub(crate) fn parse_datetime(s: &str) -> chrono::DateTime<chrono::Utc> {
    chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
        .map(|dt| dt.and_utc())
        .unwrap_or_else(|_| chrono::Utc::now())
}

