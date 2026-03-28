use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{parse_datetime, Store};

/// One durable IM event persisted for replay and realtime fanout.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredEvent {
    /// Monotonic workspace-global event cursor.
    pub event_id: i64,
    /// Stable event name such as `message.created`.
    pub event_type: String,
    /// Subscription scope discriminator (`channel`, `dm`, `thread`, ...).
    pub scope_kind: String,
    /// Stable scope identity string such as `channel:<id>`.
    pub scope_id: String,
    /// Optional owning conversation id when the event belongs to a channel/DM.
    pub channel_id: Option<String>,
    /// Optional owning conversation name for debugging and bootstrap helpers.
    pub channel_name: Option<String>,
    /// Parent message id when the event belongs to a thread.
    pub thread_parent_id: Option<String>,
    /// Originating actor name when one exists.
    pub actor_name: Option<String>,
    /// Originating actor type (`human`, `agent`) when known.
    pub actor_type: Option<String>,
    /// Command path that produced the event.
    pub caused_by_kind: Option<String>,
    /// Event-specific fields.
    pub payload: Value,
    /// Wall-clock timestamp from SQLite.
    pub created_at: DateTime<Utc>,
}

/// Mutable event content written in the same transaction as the domain change.
#[derive(Debug)]
pub(crate) struct NewEvent<'a> {
    pub event_type: &'a str,
    pub scope_kind: &'a str,
    pub scope_id: String,
    pub channel_id: Option<&'a str>,
    pub channel_name: Option<&'a str>,
    pub thread_parent_id: Option<&'a str>,
    pub actor_name: Option<&'a str>,
    pub actor_type: Option<&'a str>,
    pub caused_by_kind: Option<&'a str>,
    pub payload: Value,
}

impl Store {
    /// Return the latest committed global event cursor.
    pub fn latest_event_id(&self) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        let latest = conn.query_row(
            "SELECT COALESCE(MAX(event_id), 0) FROM events",
            [],
            |row| row.get(0),
        )?;
        Ok(latest)
    }

    /// List persisted events ordered by global cursor, optionally starting after
    /// a previously-seen cursor.
    pub fn list_events(&self, after_event_id: Option<i64>, limit: i64) -> Result<Vec<StoredEvent>> {
        let conn = self.conn.lock().unwrap();
        Self::list_events_inner(&conn, after_event_id, limit)
    }

    fn list_events_inner(
        conn: &Connection,
        after_event_id: Option<i64>,
        limit: i64,
    ) -> Result<Vec<StoredEvent>> {
        let mut stmt = if after_event_id.is_some() {
            conn.prepare(
                "SELECT event_id, event_type, scope_kind, scope_id, channel_id, channel_name,
                        thread_parent_id, actor_name, actor_type, caused_by_kind, payload, created_at
                 FROM events
                 WHERE event_id > ?1
                 ORDER BY event_id ASC
                 LIMIT ?2",
            )?
        } else {
            conn.prepare(
                "SELECT event_id, event_type, scope_kind, scope_id, channel_id, channel_name,
                        thread_parent_id, actor_name, actor_type, caused_by_kind, payload, created_at
                 FROM events
                 ORDER BY event_id ASC
                 LIMIT ?1",
            )?
        };

        let rows = if let Some(after) = after_event_id {
            stmt.query_map(params![after, limit], Self::map_stored_event)?
        } else {
            stmt.query_map(params![limit], Self::map_stored_event)?
        };

        Ok(rows.filter_map(|row| row.ok()).collect())
    }

    fn map_stored_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredEvent> {
        let payload_raw: String = row.get(10)?;
        let payload = serde_json::from_str(&payload_raw).unwrap_or(Value::Null);
        Ok(StoredEvent {
            event_id: row.get(0)?,
            event_type: row.get(1)?,
            scope_kind: row.get(2)?,
            scope_id: row.get(3)?,
            channel_id: row.get(4)?,
            channel_name: row.get(5)?,
            thread_parent_id: row.get(6)?,
            actor_name: row.get(7)?,
            actor_type: row.get(8)?,
            caused_by_kind: row.get(9)?,
            payload,
            created_at: parse_datetime(&row.get::<_, String>(11)?),
        })
    }

    /// Validate whether the named viewer can subscribe to the requested event scope.
    pub fn can_access_event_scope(
        &self,
        viewer_name: &str,
        scope_kind: &str,
        scope_id: &str,
    ) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        Self::can_access_event_scope_inner(&conn, viewer_name, scope_kind, scope_id)
    }

    fn can_access_event_scope_inner(
        conn: &Connection,
        viewer_name: &str,
        scope_kind: &str,
        scope_id: &str,
    ) -> Result<bool> {
        if Self::lookup_sender_type_inner(conn, viewer_name)?.is_none() {
            return Ok(false);
        }

        match scope_kind {
            "workspace" => Ok(true),
            "user" => Ok(scope_id == format!("user:{viewer_name}")),
            "channel" | "dm" => {
                let Some(channel_id) = scope_id.strip_prefix(&format!("{scope_kind}:")) else {
                    return Ok(false);
                };
                let membership_count: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM channel_members WHERE channel_id = ?1 AND member_name = ?2",
                    params![channel_id, viewer_name],
                    |row| row.get(0),
                )?;
                Ok(membership_count > 0)
            }
            "thread" => {
                let Some(parent_message_id) = scope_id.strip_prefix("thread:") else {
                    return Ok(false);
                };
                let membership_count: i64 = conn.query_row(
                    "SELECT COUNT(*)
                     FROM messages m
                     JOIN channel_members cm ON cm.channel_id = m.channel_id
                     WHERE m.id = ?1 AND cm.member_name = ?2",
                    params![parent_message_id, viewer_name],
                    |row| row.get(0),
                )?;
                Ok(membership_count > 0)
            }
            "agent" => Ok(scope_id == format!("agent:{viewer_name}")),
            _ => Ok(false),
        }
    }

    pub(crate) fn append_event_tx(tx: &Transaction<'_>, event: NewEvent<'_>) -> Result<i64> {
        let payload_json = serde_json::to_string(&event.payload)?;
        tx.execute(
            "INSERT INTO events (
                event_type, scope_kind, scope_id, channel_id, channel_name, thread_parent_id,
                actor_name, actor_type, caused_by_kind, payload
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                event.event_type,
                event.scope_kind,
                event.scope_id,
                event.channel_id,
                event.channel_name,
                event.thread_parent_id,
                event.actor_name,
                event.actor_type,
                event.caused_by_kind,
                payload_json,
            ],
        )?;
        Ok(tx.last_insert_rowid())
    }
}
