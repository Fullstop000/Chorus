pub mod agents;
pub mod attachments;
pub mod channels;
pub mod decisions;
pub mod humans;
pub mod inbox;
pub mod messages;
pub mod sessions;
pub mod stream;
pub mod tasks;
pub mod teams;
pub mod trace_writer;
pub mod workspaces;

use std::sync::Mutex;

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

use crate::utils::parse_datetime;

pub use agents::AgentRecordUpsert;
pub use agents::{Agent, AgentEnvVar};
pub use attachments::Attachment;
pub use channels::{Channel, ChannelListParams, ChannelMember, ChannelMemberProfile, ChannelType};
pub use decisions::{DecisionRow, DecisionStatus};
pub use humans::Human;
pub use inbox::{InboxConversationNotificationView, InboxConversationStateView};
pub use messages::{
    ActivityMessage, AttachmentRef, ConversationMessageView, ForwardedFrom, HistoryMessage,
    HistorySnapshot, Message, ReceivedMessage, SenderType,
};
pub use sessions::AgentSession;
pub use stream::StreamEvent;
pub use tasks::{ClaimResult, TaskInfo, TaskStatus};
pub use teams::{Team, TeamMember, TeamMembership};
pub use workspaces::{Workspace, WorkspaceCounts, WorkspaceMode};

/// SQLite-backed persistence layer.
pub struct Store {
    /// Serialized access to the rusqlite connection.
    conn: Mutex<Connection>,
}

impl Store {
    pub const DEFAULT_SYSTEM_CHANNEL: &'static str = "all";
    pub const DEFAULT_SYSTEM_CHANNEL_DESCRIPTION: &'static str = "All members";

    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        // Validate BEFORE init_schema so the validator inspects the
        // user's actual on-disk shape — not a freshly-minted shape that
        // `CREATE TABLE IF NOT EXISTS` would write into a half-empty
        // legacy DB and then trivially pass.
        Self::validate_schema_shape(&conn, path)?;
        Self::init_schema(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Open the store for the bridge runtime: `foreign_keys=OFF` so the
    /// bridge can write `agent_sessions` rows without a corresponding
    /// `agents` row. The bridge's local DB is a runtime-state cache, not
    /// a normalized model — agent records live in-memory in the
    /// reconcile loop's `TargetCache`.
    pub fn open_for_bridge(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=OFF;")?;
        Self::validate_schema_shape(&conn, path)?;
        Self::init_schema(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
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

    /// Catch DBs that pre-date the always-set-machine_id invariant. The
    /// runtime ALTER shims that used to fix these in place were dropped
    /// once every code path stopped writing NULL — but `CREATE TABLE IF
    /// NOT EXISTS` is a no-op on existing tables, so opening an old DB
    /// would otherwise succeed here and surface a cryptic SQLite error
    /// later when the first INSERT trips `NOT NULL`. Fail loudly with a
    /// "delete this file" hint instead.
    ///
    /// Runs BEFORE `init_schema` so the check inspects the user's
    /// on-disk shape, not a freshly-minted-by-CREATE-IF-NOT-EXISTS one.
    /// A completely fresh DB (no tables yet) passes through to
    /// `init_schema`; only DBs that already have an `agents` table get
    /// the shape check.
    fn validate_schema_shape(conn: &Connection, path: &str) -> Result<()> {
        // No tables yet → fresh install. Skip and let init_schema
        // create the canonical shape.
        let table_count: i64 = conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table'",
            [],
            |row| row.get(0),
        )?;
        if table_count == 0 {
            return Ok(());
        }

        // Only check `agents.machine_id`: it's the marker column for
        // every recent schema phase (added in #150, made NOT NULL in
        // this cleanup). Older shapes are caught by either the column
        // being absent or by `notnull = 0`.
        let mut stmt = conn.prepare("PRAGMA table_info(agents)")?;
        let mut machine_id_notnull: Option<i64> = None;
        for row in stmt.query_map([], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, i64>(3)?))
        })? {
            let (name, notnull) = row?;
            if name == "machine_id" {
                machine_id_notnull = Some(notnull);
                break;
            }
        }
        match machine_id_notnull {
            None => anyhow::bail!(
                "incompatible database schema at {path}: \
                 `agents.machine_id` is missing. Delete this file and \
                 restart to recreate it from scratch."
            ),
            Some(0) => anyhow::bail!(
                "incompatible database schema at {path}: \
                 `agents.machine_id` is nullable but the current schema \
                 declares it NOT NULL. Delete this file and restart to \
                 recreate it from scratch."
            ),
            Some(_) => Ok(()),
        }
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
        agent_id: &str,
        viewer_id: &str,
        viewer_type: &str,
        limit: usize,
    ) -> Result<Vec<serde_json::Value>> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare(
            "SELECT m.id, m.run_id, m.trace_summary, m.created_at FROM messages m
             JOIN channel_members cm ON cm.channel_id = m.channel_id
               AND cm.member_id = ?2 AND cm.member_type = ?3
             WHERE m.sender_id = ?1 AND m.run_id IS NOT NULL AND m.trace_summary IS NOT NULL
             GROUP BY m.run_id
             ORDER BY m.created_at DESC LIMIT ?4",
        )?;
        let runs: Vec<serde_json::Value> = stmt
            .query_map(params![agent_id, viewer_id, viewer_type, limit as i64], |row| {
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

    pub fn lookup_sender_type(&self, id: &str) -> Result<Option<SenderType>> {
        let conn = self.lock_conn();
        Self::lookup_sender_type_inner(&conn, id)
    }

    /// Resolve a stable sender name to the canonical (id, SenderType) pair.
    ///
    /// Used by API handlers that explicitly accept human-friendly names from
    /// clients but persist immutable ids.
    pub fn lookup_sender_by_name(&self, name: &str) -> Result<Option<(String, SenderType)>> {
        let conn = self.lock_conn();
        Self::lookup_sender_by_name_inner(&conn, name)
    }

    fn lookup_sender_by_name_inner(
        conn: &Connection,
        name: &str,
    ) -> Result<Option<(String, SenderType)>> {
        if let Some(id) = conn
            .query_row(
                "SELECT id FROM humans WHERE name = ?1",
                params![name],
                |row| row.get::<_, String>(0),
            )
            .optional()?
        {
            return Ok(Some((id, SenderType::Human)));
        }
        if let Some(id) = conn
            .query_row(
                "SELECT id FROM agents WHERE name = ?1",
                params![name],
                |row| row.get::<_, String>(0),
            )
            .optional()?
        {
            return Ok(Some((id, SenderType::Agent)));
        }
        Ok(None)
    }

    fn lookup_sender_type_inner(conn: &Connection, id: &str) -> Result<Option<SenderType>> {
        let agent_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM agents WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )?;
        if agent_count > 0 {
            return Ok(Some(SenderType::Agent));
        }
        let human_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM humans WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )?;
        if human_count > 0 {
            return Ok(Some(SenderType::Human));
        }
        Ok(None)
    }
}
