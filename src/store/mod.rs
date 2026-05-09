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
        Self::validate_supported_identity_schema(&conn)?;
        Self::init_schema(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Open the store for the bridge runtime: `foreign_keys=OFF` so the
    /// bridge can write `agent_sessions` rows without a corresponding
    /// `agents` row. The bridge's local DB is a runtime-state cache, not
    /// a normalized model — agent records live in-memory in the
    /// reconcile loop's `TargetCache`. See #145 for the design.
    pub fn open_for_bridge(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=OFF;")?;
        Self::validate_supported_identity_schema(&conn)?;
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
        // The schema below uses `CREATE TABLE IF NOT EXISTS` so this is a
        // no-op on existing DBs; column-shape migrations run after.
        let schema = include_str!("schema.sql");
        conn.execute_batch(schema)?;
        // Idempotent `machine_id` migration: SQLite has no
        // `ALTER TABLE ... ADD COLUMN IF NOT EXISTS`. Attempt the ALTER
        // and ignore *only* the "duplicate column name" error; surface
        // anything else (locked DB, syntax errors, real schema drift)
        // so first-run failures aren't silent.
        match conn.execute("ALTER TABLE agents ADD COLUMN machine_id TEXT", []) {
            Ok(_) => {}
            Err(rusqlite::Error::SqliteFailure(_, Some(msg)))
                if msg.contains("duplicate column name") => {}
            Err(e) => return Err(e.into()),
        }
        // Migrate `agent_env_vars` from name-keyed to id-keyed (#142). The
        // table was created earlier with `CREATE TABLE IF NOT EXISTS`, so
        // pre-existing rows still have an `agent_name` column even after
        // the new schema text ran. Detect that shape and rewrite in place.
        if Self::schema_column_exists(conn, "agent_env_vars", "agent_name")? {
            Self::migrate_agent_env_vars_to_id(conn)?;
        }
        Ok(())
    }

    /// One-shot migration: rewrite `agent_env_vars` from `(agent_name, key, ...)`
    /// keyed-by-name to `(agent_id, key, ...)` keyed-by-id. SQLite cannot
    /// rename a column on a table with foreign keys without recreating it.
    /// Rows whose `agent_name` no longer matches an `agents.name` (orphans
    /// left over from manual deletes when FKs were OFF) are dropped — the
    /// previous FK declared `ON DELETE CASCADE` so they should not exist;
    /// if they do, the agent they refer to is already gone.
    fn migrate_agent_env_vars_to_id(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "BEGIN;
             CREATE TABLE agent_env_vars_new (
                 agent_id TEXT NOT NULL,
                 key TEXT NOT NULL,
                 value TEXT NOT NULL,
                 position INTEGER NOT NULL,
                 PRIMARY KEY (agent_id, key),
                 FOREIGN KEY (agent_id) REFERENCES agents(id) ON DELETE CASCADE
             );
             INSERT INTO agent_env_vars_new (agent_id, key, value, position)
                 SELECT a.id, e.key, e.value, e.position
                 FROM agent_env_vars e
                 JOIN agents a ON a.name = e.agent_name;
             DROP TABLE agent_env_vars;
             ALTER TABLE agent_env_vars_new RENAME TO agent_env_vars;
             COMMIT;",
        )?;
        Ok(())
    }

    fn validate_supported_identity_schema(conn: &Connection) -> Result<()> {
        for (table, column) in [
            ("humans", "display_name"),
            ("workspaces", "created_by_human"),
            ("workspace_members", "human_name"),
            ("channel_members", "member_name"),
            ("inbox_read_state", "member_name"),
            ("messages", "sender_name"),
            ("tasks", "created_by"),
            ("tasks", "claimed_by"),
            ("team_members", "member_name"),
        ] {
            if Self::schema_column_exists(conn, table, column)? {
                anyhow::bail!(Self::old_identity_schema_message(table, column));
            }
        }

        for (table, columns) in [
            ("humans", &["id", "name"][..]),
            ("workspaces", &["created_by_human_id"][..]),
            ("workspace_members", &["human_id"][..]),
            ("channel_members", &["member_id", "member_type"][..]),
            ("inbox_read_state", &["member_id", "member_type"][..]),
            ("messages", &["sender_id", "sender_type"][..]),
            (
                "tasks",
                &[
                    "created_by_id",
                    "created_by_type",
                    "claimed_by_id",
                    "claimed_by_type",
                ][..],
            ),
            ("team_members", &["member_id", "member_type"][..]),
        ] {
            if !Self::schema_table_exists(conn, table)? {
                continue;
            }
            for column in columns {
                if !Self::schema_column_exists(conn, table, column)? {
                    anyhow::bail!(Self::old_identity_schema_message(table, column));
                }
            }
        }

        Ok(())
    }

    fn old_identity_schema_message(table: &str, column: &str) -> String {
        format!(
            "local database uses an old identity schema ({table}.{column}); run with a fresh data directory or reset local data"
        )
    }

    fn schema_table_exists(conn: &Connection, table: &str) -> Result<bool> {
        Ok(conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
                params![table],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    fn schema_column_exists(conn: &Connection, table: &str, column: &str) -> Result<bool> {
        if !Self::schema_table_exists(conn, table)? {
            return Ok(false);
        }
        let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
        let exists = stmt
            .query_map([], |row| row.get::<_, String>(1))?
            .filter_map(Result::ok)
            .any(|name| name == column);
        Ok(exists)
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

#[cfg(test)]
mod migration_tests {
    use super::*;
    use rusqlite::Connection;

    /// Build a connection holding the *old* `agent_env_vars` shape (keyed by
    /// `agent_name`) plus a parent agents row so we can verify the migration
    /// preserves data when re-running `init_schema` against a legacy DB.
    fn legacy_env_vars_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE workspaces (
                 id TEXT PRIMARY KEY,
                 name TEXT NOT NULL,
                 slug TEXT NOT NULL UNIQUE,
                 mode TEXT NOT NULL DEFAULT 'local_only',
                 created_by_human_id TEXT,
                 created_at TEXT NOT NULL DEFAULT (datetime('now'))
             );
             CREATE TABLE agents (
                 id TEXT PRIMARY KEY,
                 workspace_id TEXT NOT NULL,
                 name TEXT UNIQUE NOT NULL,
                 display_name TEXT NOT NULL,
                 description TEXT,
                 system_prompt TEXT,
                 runtime TEXT NOT NULL,
                 model TEXT NOT NULL,
                 reasoning_effort TEXT,
                 created_at TEXT NOT NULL DEFAULT (datetime('now'))
             );
             CREATE TABLE agent_env_vars (
                 agent_name TEXT NOT NULL,
                 key TEXT NOT NULL,
                 value TEXT NOT NULL,
                 position INTEGER NOT NULL,
                 PRIMARY KEY (agent_name, key),
                 FOREIGN KEY (agent_name) REFERENCES agents(name) ON DELETE CASCADE
             );
             INSERT INTO workspaces (id, name, slug) VALUES ('w1', 'ws', 'ws');
             INSERT INTO agents (id, workspace_id, name, display_name, runtime, model)
                 VALUES ('a-uuid-1', 'w1', 'alice', 'Alice', 'codex', 'gpt-x');
             INSERT INTO agent_env_vars (agent_name, key, value, position)
                 VALUES ('alice', 'API_KEY', 'sk-test', 0),
                        ('alice', 'TIMEOUT_MS', '5000', 1);",
        )
        .unwrap();
        conn
    }

    #[test]
    fn migrate_agent_env_vars_to_id_rewrites_in_place_and_preserves_rows() {
        let conn = legacy_env_vars_db();
        // Sanity: legacy shape detected.
        assert!(Store::schema_column_exists(&conn, "agent_env_vars", "agent_name").unwrap());
        // Run the schema bootstrap (re-creates anything missing, then runs the migration).
        Store::init_schema(&conn).unwrap();
        // After migration: column flipped, rows preserved with id-keyed FK.
        assert!(
            !Store::schema_column_exists(&conn, "agent_env_vars", "agent_name").unwrap(),
            "agent_name column should be gone post-migration"
        );
        assert!(
            Store::schema_column_exists(&conn, "agent_env_vars", "agent_id").unwrap(),
            "agent_id column should exist post-migration"
        );
        let mut stmt = conn
            .prepare("SELECT agent_id, key, value, position FROM agent_env_vars ORDER BY position")
            .unwrap();
        let rows: Vec<(String, String, String, i64)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert_eq!(
            rows,
            vec![
                (
                    "a-uuid-1".to_string(),
                    "API_KEY".to_string(),
                    "sk-test".to_string(),
                    0
                ),
                (
                    "a-uuid-1".to_string(),
                    "TIMEOUT_MS".to_string(),
                    "5000".to_string(),
                    1
                ),
            ]
        );
    }

    #[test]
    fn migrate_agent_env_vars_to_id_is_idempotent_on_already_new_shape() {
        let conn = Connection::open_in_memory().unwrap();
        // First boot creates the new shape directly via init_schema.
        Store::init_schema(&conn).unwrap();
        // Second boot must not blow up; the migration block only fires when
        // the legacy column is detected, so re-running init_schema is safe.
        Store::init_schema(&conn).unwrap();
        assert!(
            Store::schema_column_exists(&conn, "agent_env_vars", "agent_id").unwrap(),
            "id-keyed column must remain after re-running init_schema"
        );
    }
}
