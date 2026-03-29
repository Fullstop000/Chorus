pub mod agents;
pub mod channels;
pub mod events;
pub mod inbox;
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
pub use events::{ResolvedSubscriptionTarget, StoredEvent, SubscriptionTargetKind};
pub use inbox::{InboxConversationNotificationView, InboxConversationStateView};
pub use knowledge::{
    KnowledgeEntry, RecallQuery, RecallResponse, RememberRequest, RememberResponse,
};
pub use messages::{
    ActivityMessage, AttachmentRef, ConversationMessageView, ForwardedFrom, HistoryMessage,
    HistorySnapshot, Message, ReceivedMessage, SenderType, ThreadSummaryView,
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
    /// Broadcast channel: latest committed durable event id for realtime wake / replay.
    event_tx: broadcast::Sender<i64>,
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
        Self::migrate_event_stream_identity(&conn)?;
        let (msg_tx, _) = broadcast::channel(256);
        let (event_tx, _) = broadcast::channel(256);
        Ok(Self {
            conn: Mutex::new(conn),
            msg_tx,
            event_tx,
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
            CREATE TABLE IF NOT EXISTS inbox_read_state (
                conversation_id TEXT NOT NULL,
                member_name TEXT NOT NULL,
                member_type TEXT NOT NULL,
                last_read_seq INTEGER NOT NULL DEFAULT 0,
                last_read_message_id TEXT,
                updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                PRIMARY KEY (conversation_id, member_name)
            );
            CREATE TABLE IF NOT EXISTS inbox_thread_read_state (
                conversation_id TEXT NOT NULL,
                thread_parent_id TEXT NOT NULL,
                member_name TEXT NOT NULL,
                member_type TEXT NOT NULL,
                last_read_seq INTEGER NOT NULL DEFAULT 0,
                last_read_message_id TEXT,
                updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                PRIMARY KEY (conversation_id, thread_parent_id, member_name)
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
            CREATE TABLE IF NOT EXISTS events (
                event_id INTEGER PRIMARY KEY AUTOINCREMENT,
                stream_id TEXT,
                stream_kind TEXT,
                stream_pos INTEGER,
                event_type TEXT NOT NULL,
                scope_kind TEXT NOT NULL,
                scope_id TEXT NOT NULL,
                channel_id TEXT,
                channel_name TEXT,
                thread_parent_id TEXT,
                actor_name TEXT,
                actor_type TEXT,
                caused_by_kind TEXT,
                payload TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE INDEX IF NOT EXISTS events_scope_event_id
                ON events(scope_kind, scope_id, event_id);
            CREATE INDEX IF NOT EXISTS events_stream_event_id
                ON events(stream_id, stream_pos);
            CREATE TABLE IF NOT EXISTS streams (
                stream_id TEXT PRIMARY KEY,
                stream_kind TEXT NOT NULL,
                aggregate_id TEXT NOT NULL,
                current_pos INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
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
        conn.execute("ALTER TABLE events ADD COLUMN stream_id TEXT", [])
            .ok();
        conn.execute("ALTER TABLE events ADD COLUMN stream_kind TEXT", [])
            .ok();
        conn.execute("ALTER TABLE events ADD COLUMN stream_pos INTEGER", [])
            .ok();
        inbox::migrate_inbox_read_state(conn)?;
        Self::refresh_conversation_messages_view(conn)?;
        Self::refresh_thread_summaries_view(conn)?;
        inbox::refresh_inbox_conversation_state_view(conn)?;
        Ok(())
    }

    /// Keep the explicit conversation history read model aligned with the
    /// current backing tables while `messages` remains transitional storage.
    fn refresh_conversation_messages_view(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "
            DROP VIEW IF EXISTS conversation_messages_view;
            CREATE VIEW conversation_messages_view AS
            SELECT
                m.id AS message_id,
                m.channel_id AS conversation_id,
                c.name AS conversation_name,
                c.channel_type AS conversation_type,
                m.thread_parent_id AS thread_parent_id,
                m.sender_name AS sender_name,
                m.sender_type AS sender_type,
                m.sender_deleted AS sender_deleted,
                m.content AS content,
                m.created_at AS created_at,
                m.seq AS seq,
                m.forwarded_from AS forwarded_from
            FROM messages m
            JOIN channels c ON c.id = m.channel_id;
            ",
        )?;
        Ok(())
    }

    /// Keep thread summary reads behind an explicit projection view while
    /// replies continue to live in the conversation message stream.
    fn refresh_thread_summaries_view(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "
            DROP VIEW IF EXISTS thread_summaries_view;
            CREATE VIEW thread_summaries_view AS
            SELECT
                parent.channel_id AS conversation_id,
                parent.id AS parent_message_id,
                COUNT(reply.id) AS reply_count,
                (
                    SELECT reply_last.id
                    FROM messages reply_last
                    WHERE reply_last.channel_id = parent.channel_id
                      AND reply_last.thread_parent_id = parent.id
                    ORDER BY reply_last.seq DESC
                    LIMIT 1
                ) AS last_reply_message_id,
                (
                    SELECT reply_last.created_at
                    FROM messages reply_last
                    WHERE reply_last.channel_id = parent.channel_id
                      AND reply_last.thread_parent_id = parent.id
                    ORDER BY reply_last.seq DESC
                    LIMIT 1
                ) AS last_reply_at,
                (
                    SELECT COUNT(*)
                    FROM (
                        SELECT parent.sender_name AS participant_name
                        UNION
                        SELECT reply_participant.sender_name
                        FROM messages reply_participant
                        WHERE reply_participant.channel_id = parent.channel_id
                          AND reply_participant.thread_parent_id = parent.id
                    )
                ) AS participant_count
            FROM messages parent
            LEFT JOIN messages reply
              ON reply.channel_id = parent.channel_id
             AND reply.thread_parent_id = parent.id
            WHERE parent.thread_parent_id IS NULL
            GROUP BY parent.channel_id, parent.id;
            ",
        )?;
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

    /// Backfill per-domain stream identity onto existing durable events and
    /// ensure the lightweight stream cursor table reflects the latest position
    /// for every known stream. Safe to run on every startup.
    fn migrate_event_stream_identity(conn: &Connection) -> Result<()> {
        let rows: Vec<(i64, String, String, Option<String>)> = conn
            .prepare(
                "SELECT event_id, scope_kind, scope_id, channel_id
                 FROM events
                 ORDER BY event_id ASC",
            )?
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })?
            .filter_map(|row| row.ok())
            .collect();

        let mut stream_positions: HashMap<String, i64> = HashMap::new();
        let mut stream_meta: HashMap<String, (String, String, i64)> = HashMap::new();

        for (event_id, scope_kind, scope_id, channel_id) in rows {
            let (stream_id, stream_kind, aggregate_id) =
                crate::store::events::derive_stream_identity(
                    &scope_kind,
                    &scope_id,
                    channel_id.as_deref(),
                );
            let next_pos = stream_positions.get(&stream_id).copied().unwrap_or(0) + 1;
            stream_positions.insert(stream_id.clone(), next_pos);
            stream_meta.insert(
                stream_id.clone(),
                (stream_kind.clone(), aggregate_id.clone(), next_pos),
            );
            conn.execute(
                "UPDATE events
                 SET stream_id = ?1, stream_kind = ?2, stream_pos = ?3
                 WHERE event_id = ?4",
                params![stream_id, stream_kind, next_pos, event_id],
            )?;
        }

        for (stream_id, (stream_kind, aggregate_id, current_pos)) in stream_meta {
            conn.execute(
                "INSERT INTO streams (stream_id, stream_kind, aggregate_id, current_pos)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(stream_id) DO UPDATE SET
                     stream_kind = excluded.stream_kind,
                     aggregate_id = excluded.aggregate_id,
                     current_pos = excluded.current_pos",
                params![stream_id, stream_kind, aggregate_id, current_pos],
            )?;
        }

        Ok(())
    }

    pub fn subscribe(&self) -> broadcast::Receiver<(String, String)> {
        self.msg_tx.subscribe()
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<i64> {
        self.event_tx.subscribe()
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
}

/// Derive the server data directory from the SQLite path.
fn derive_data_dir(path: &str) -> PathBuf {
    if path == ":memory:" {
        return std::env::temp_dir().join(format!("chorus-memory-{}", uuid::Uuid::new_v4()));
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
