pub mod agents;
pub mod attachments;
pub mod channels;
pub mod events;
pub mod inbox;
pub mod knowledge;
pub mod messages;
pub mod migrations;
pub mod tasks;
pub mod teams;

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::Result;
use rusqlite::{params, Connection};
use tokio::sync::broadcast;

pub use agents::AgentRecordUpsert;
pub use agents::{Agent, AgentConfig, AgentEnvVar, AgentStatus, Human};
pub use attachments::Attachment;
pub use channels::{Channel, ChannelListParams, ChannelMember, ChannelMemberProfile, ChannelType};
pub use events::{ResolvedSubscriptionTarget, StoredEvent, SubscriptionTargetKind};
pub use inbox::{InboxConversationNotificationView, InboxConversationStateView};
pub use knowledge::{
    KnowledgeEntry, RecallQuery, RecallResponse, RememberRequest, RememberResponse,
};
pub use messages::{
    ActivityMessage, AttachmentRef, ChannelThreadInbox, ChannelThreadInboxEntry,
    ConversationMessageView, ForwardedFrom, HistoryMessage, HistorySnapshot, Message,
    ReceivedMessage, SenderType, ThreadSummaryView,
};
pub use tasks::{ClaimResult, Task, TaskInfo, TaskStatus};
pub use teams::{Team, TeamMember, TeamMembership};

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
        migrations::run_migrations(&conn)?;
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
        let schema = include_str!("schema.sql");
        conn.execute_batch(schema)?;
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
