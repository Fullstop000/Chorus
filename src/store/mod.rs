pub mod agents;
pub mod attachments;
pub mod channels;
pub mod inbox;
pub mod messages;
pub mod migrations;
pub mod stream;
pub mod tasks;
pub mod teams;
pub mod trace_writer;

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use tokio::sync::broadcast;

use crate::utils::{derive_data_dir, parse_datetime};

pub use agents::AgentRecordUpsert;
pub use agents::{Agent, AgentEnvVar, AgentStatus, Human};
pub use attachments::Attachment;
pub use channels::{Channel, ChannelListParams, ChannelMember, ChannelMemberProfile, ChannelType};
pub use inbox::{InboxConversationNotificationView, InboxConversationStateView};
pub use messages::{
    ActivityMessage, AttachmentRef, ChannelThreadInbox, ChannelThreadInboxEntry,
    ConversationMessageView, ForwardedFrom, HistoryMessage, HistorySnapshot, Message,
    ReceivedMessage, SenderType, ThreadSummaryView,
};
pub use stream::StreamEvent;
pub use tasks::{ClaimResult, Task, TaskInfo, TaskStatus};
pub use teams::{Team, TeamMember, TeamMembership};

use crate::agent::trace::TraceEvent;

/// SQLite-backed persistence and pub/sub for new messages.
pub struct Store {
    /// Serialized access to the rusqlite connection.
    conn: Mutex<Connection>,
    /// Broadcast channel for stream events (message.created only).
    stream_tx: broadcast::Sender<StreamEvent>,
    /// Broadcast channel for agent trace events (tool calls, thinking, etc.).
    trace_tx: broadcast::Sender<TraceEvent>,
    /// Root data directory (db parent, attachments, teams).
    data_dir: PathBuf,
    /// Optional override for the per-agent workspace root. When set, takes
    /// precedence over `data_dir.join("agents")` so the server can keep
    /// agent workspaces at the chorus-root level while chorus.db sits in
    /// `<root>/data/`.
    agents_dir_override: Option<PathBuf>,
}

impl Store {
    pub const DEFAULT_SYSTEM_CHANNEL: &'static str = "all";
    pub const DEFAULT_SYSTEM_CHANNEL_DESCRIPTION: &'static str = "All members";

    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        Self::init_schema(&conn)?;
        migrations::run_migrations(&conn)?;
        let (stream_tx, _) = broadcast::channel(256);
        let (trace_tx, _) = broadcast::channel(1024);
        Ok(Self {
            conn: Mutex::new(conn),
            stream_tx,
            trace_tx,
            data_dir: derive_data_dir(path),
            agents_dir_override: None,
        })
    }

    /// Builder: override the per-agent workspace directory. Intended for the
    /// CLI `serve`/`run` path, which places agent workspaces at the chorus
    /// root instead of inside `data/`.
    pub fn with_agents_dir(mut self, dir: PathBuf) -> Self {
        self.agents_dir_override = Some(dir);
        self
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
        self.agents_dir_override
            .clone()
            .unwrap_or_else(|| self.data_dir.join("agents"))
    }

    /// Return the directory used to persist per-team workspaces.
    pub fn teams_dir(&self) -> PathBuf {
        self.data_dir.join("teams")
    }

    /// Lock the database connection, handling mutex poison gracefully.
    /// If the mutex is poisoned (from a previous panic), the lock is recovered
    /// and the operation proceeds. This avoids crashing the server on panics
    /// in other threads.
    fn lock_conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        match self.conn.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                tracing::warn!("Database mutex was poisoned, recovering");
                self.conn.clear_poison();
                poisoned.into_inner()
            }
        }
    }

    /// Expose the raw connection guard for use in integration tests only.
    /// Not intended for production use.
    pub fn conn_for_test(&self) -> std::sync::MutexGuard<'_, rusqlite::Connection> {
        // Tests may expect a clean state; panicking is acceptable here.
        self.conn.lock().unwrap()
    }

    fn init_schema(conn: &Connection) -> Result<()> {
        let schema = include_str!("schema.sql");
        conn.execute_batch(schema)?;
        Ok(())
    }

    pub fn subscribe(&self) -> broadcast::Receiver<StreamEvent> {
        self.stream_tx.subscribe()
    }

    pub fn subscribe_traces(&self) -> broadcast::Receiver<TraceEvent> {
        self.trace_tx.subscribe()
    }

    pub fn trace_sender(&self) -> broadcast::Sender<TraceEvent> {
        self.trace_tx.clone()
    }

    /// Return the path to the SQLite database file (for trace_writer's separate connection).
    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("chorus.db")
    }

    /// Look up the channel_id for a given run_id (from the first message in that run).
    pub fn get_run_channel_id(&self, run_id: &str) -> Result<Option<String>> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare("SELECT channel_id FROM messages WHERE run_id = ?1 LIMIT 1")?;
        let result = stmt
            .query_row(params![run_id], |row| row.get(0))
            .optional()?;
        Ok(result)
    }

    /// Retrieve ordered trace events for a given run_id.
    pub fn get_trace_events(&self, run_id: &str) -> Result<Vec<serde_json::Value>> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare(
            "SELECT run_id, seq, timestamp_ms, kind, data FROM trace_events WHERE run_id = ?1 ORDER BY seq ASC",
        )?;
        let events: Vec<serde_json::Value> = stmt
            .query_map(params![run_id], |row| {
                let run_id: String = row.get(0)?;
                let seq: i64 = row.get(1)?;
                let timestamp_ms: i64 = row.get(2)?;
                let kind: String = row.get(3)?;
                let data: String = row.get(4)?;
                Ok(serde_json::json!({
                    "runId": run_id,
                    "seq": seq,
                    "timestampMs": timestamp_ms,
                    "kind": kind,
                    "data": serde_json::from_str::<serde_json::Value>(&data).unwrap_or_default(),
                }))
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(events)
    }

    /// List recent runs for an agent, filtered to channels the viewer is a member of.
    pub fn get_agent_runs(
        &self,
        agent_name: &str,
        viewer: &str,
        limit: usize,
    ) -> Result<Vec<serde_json::Value>> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare(
            "SELECT m.id, m.run_id, m.trace_summary, m.created_at FROM messages m
             JOIN channel_members cm ON cm.channel_id = m.channel_id AND cm.member_name = ?2
             WHERE m.sender_name = ?1 AND m.run_id IS NOT NULL AND m.trace_summary IS NOT NULL
             GROUP BY m.run_id
             ORDER BY m.created_at DESC LIMIT ?3",
        )?;
        let runs: Vec<serde_json::Value> = stmt
            .query_map(params![agent_name, viewer, limit as i64], |row| {
                let id: String = row.get(0)?;
                let run_id: String = row.get(1)?;
                let summary: String = row.get(2)?;
                let created_at: String = row.get(3)?;
                Ok(serde_json::json!({
                    "messageId": id,
                    "runId": run_id,
                    "traceSummary": serde_json::from_str::<serde_json::Value>(&summary).unwrap_or_default(),
                    "createdAt": created_at,
                }))
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(runs)
    }

    // ── Sender type lookup ──

    pub fn lookup_sender_type(&self, name: &str) -> Result<Option<SenderType>> {
        let conn = self.lock_conn();
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
