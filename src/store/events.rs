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
    /// Domain stream that owns this event.
    pub stream_id: String,
    /// Stream kind (`conversation`, `team`, `agent`, `inbox`, ...).
    pub stream_kind: String,
    /// Monotonic position within the owning stream.
    pub stream_pos: i64,
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

/// Validated subscription target tied to one owning stream plus the target-side
/// event filter needed for projection scopes like threads.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ResolvedSubscriptionTarget {
    pub target_id: String,
    pub stream_id: String,
    pub kind: SubscriptionTargetKind,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum SubscriptionTargetKind {
    Conversation { conversation_id: String },
    Thread { parent_message_id: String },
    Team { team_id: String },
    Agent { agent_name: String },
    Inbox { user_name: String },
    Workspace,
}

impl ResolvedSubscriptionTarget {
    pub fn matches_event(&self, event: &StoredEvent) -> bool {
        match &self.kind {
            SubscriptionTargetKind::Conversation { .. } => {
                event.stream_id == self.stream_id && event.scope_kind != "thread"
            }
            SubscriptionTargetKind::Thread { parent_message_id } => {
                event.stream_id == self.stream_id
                    && (event.thread_parent_id.as_deref() == Some(parent_message_id.as_str())
                        || event.scope_id == format!("thread:{parent_message_id}"))
            }
            SubscriptionTargetKind::Team { .. }
            | SubscriptionTargetKind::Agent { .. }
            | SubscriptionTargetKind::Inbox { .. }
            | SubscriptionTargetKind::Workspace => event.stream_id == self.stream_id,
        }
    }
}

impl StoredEvent {
    /// Message-arrival events are rendered as notification frames on the websocket.
    pub fn is_message_created(&self) -> bool {
        self.event_type == "message.created"
    }

    /// Conversation notification events carry absolute room state.
    pub fn is_conversation_state(&self) -> bool {
        self.event_type == "conversation.state"
    }

    /// Thread notification events carry absolute thread state.
    pub fn is_thread_state(&self) -> bool {
        self.event_type == "thread.state"
    }
}

impl Store {
    /// Return the latest committed global event cursor.
    pub fn latest_event_id(&self) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        let latest =
            conn.query_row("SELECT COALESCE(MAX(event_id), 0) FROM events", [], |row| {
                row.get(0)
            })?;
        Ok(latest)
    }

    /// List persisted events ordered by global cursor, optionally starting after
    /// a previously-seen cursor.
    pub fn list_events(&self, after_event_id: Option<i64>, limit: i64) -> Result<Vec<StoredEvent>> {
        let conn = self.conn.lock().unwrap();
        Self::list_events_inner(&conn, after_event_id, limit)
    }

    /// List persisted events for a single stream ordered by stream position.
    pub fn list_events_for_stream(
        &self,
        stream_id: &str,
        after_stream_pos: Option<i64>,
        limit: i64,
    ) -> Result<Vec<StoredEvent>> {
        let conn = self.conn.lock().unwrap();
        Self::list_events_for_stream_inner(&conn, stream_id, after_stream_pos, limit)
    }

    fn list_events_inner(
        conn: &Connection,
        after_event_id: Option<i64>,
        limit: i64,
    ) -> Result<Vec<StoredEvent>> {
        let mut stmt = if after_event_id.is_some() {
            conn.prepare(
                "SELECT event_id, stream_id, stream_kind, stream_pos, event_type, scope_kind, scope_id, channel_id, channel_name,
                        thread_parent_id, actor_name, actor_type, caused_by_kind, payload, created_at
                 FROM events
                 WHERE event_id > ?1
                 ORDER BY event_id ASC
                 LIMIT ?2",
            )?
        } else {
            conn.prepare(
                "SELECT event_id, stream_id, stream_kind, stream_pos, event_type, scope_kind, scope_id, channel_id, channel_name,
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

    fn list_events_for_stream_inner(
        conn: &Connection,
        stream_id: &str,
        after_stream_pos: Option<i64>,
        limit: i64,
    ) -> Result<Vec<StoredEvent>> {
        let mut stmt = if after_stream_pos.is_some() {
            conn.prepare(
                "SELECT event_id, stream_id, stream_kind, stream_pos, event_type, scope_kind, scope_id, channel_id, channel_name,
                        thread_parent_id, actor_name, actor_type, caused_by_kind, payload, created_at
                 FROM events
                 WHERE stream_id = ?1 AND stream_pos > ?2
                 ORDER BY stream_pos ASC
                 LIMIT ?3",
            )?
        } else {
            conn.prepare(
                "SELECT event_id, stream_id, stream_kind, stream_pos, event_type, scope_kind, scope_id, channel_id, channel_name,
                        thread_parent_id, actor_name, actor_type, caused_by_kind, payload, created_at
                 FROM events
                 WHERE stream_id = ?1
                 ORDER BY stream_pos ASC
                 LIMIT ?2",
            )?
        };

        let rows = if let Some(after) = after_stream_pos {
            stmt.query_map(params![stream_id, after, limit], Self::map_stored_event)?
        } else {
            stmt.query_map(params![stream_id, limit], Self::map_stored_event)?
        };

        Ok(rows.filter_map(|row| row.ok()).collect())
    }

    fn map_stored_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredEvent> {
        let payload_raw: String = row.get(13)?;
        let payload = serde_json::from_str(&payload_raw).unwrap_or(Value::Null);
        Ok(StoredEvent {
            event_id: row.get(0)?,
            stream_id: row.get(1)?,
            stream_kind: row.get(2)?,
            stream_pos: row.get(3)?,
            event_type: row.get(4)?,
            scope_kind: row.get(5)?,
            scope_id: row.get(6)?,
            channel_id: row.get(7)?,
            channel_name: row.get(8)?,
            thread_parent_id: row.get(9)?,
            actor_name: row.get(10)?,
            actor_type: row.get(11)?,
            caused_by_kind: row.get(12)?,
            payload,
            created_at: parse_datetime(&row.get::<_, String>(14)?),
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
            "team" => {
                let Some(team_id) = scope_id.strip_prefix("team:") else {
                    return Ok(false);
                };
                let team_membership_count: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM team_members WHERE team_id = ?1 AND member_name = ?2",
                    params![team_id, viewer_name],
                    |row| row.get(0),
                )?;
                if team_membership_count > 0 {
                    return Ok(true);
                }
                let room_membership_count: i64 = conn.query_row(
                    "SELECT COUNT(*)
                     FROM teams t
                     JOIN channels c ON c.name = t.name AND c.channel_type = 'team' AND c.archived = 0
                     JOIN channel_members cm ON cm.channel_id = c.id
                     WHERE t.id = ?1 AND cm.member_name = ?2",
                    params![team_id, viewer_name],
                    |row| row.get(0),
                )?;
                Ok(room_membership_count > 0)
            }
            _ => Ok(false),
        }
    }

    pub(crate) fn append_event_tx(tx: &Transaction<'_>, event: NewEvent<'_>) -> Result<i64> {
        let (stream_id, stream_kind, aggregate_id) =
            derive_stream_identity(event.scope_kind, &event.scope_id, event.channel_id);
        tx.execute(
            "INSERT OR IGNORE INTO streams (stream_id, stream_kind, aggregate_id, current_pos)
             VALUES (?1, ?2, ?3, 0)",
            params![
                stream_id.as_str(),
                stream_kind.as_str(),
                aggregate_id.as_str()
            ],
        )?;
        tx.execute(
            "UPDATE streams SET current_pos = current_pos + 1 WHERE stream_id = ?1",
            params![stream_id.as_str()],
        )?;
        let stream_pos: i64 = tx.query_row(
            "SELECT current_pos FROM streams WHERE stream_id = ?1",
            params![stream_id.as_str()],
            |row| row.get(0),
        )?;
        let payload_json = serde_json::to_string(&event.payload)?;
        tx.execute(
            "INSERT INTO events (
                stream_id, stream_kind, stream_pos, event_type, scope_kind, scope_id, channel_id, channel_name, thread_parent_id,
                actor_name, actor_type, caused_by_kind, payload
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                stream_id.as_str(),
                stream_kind.as_str(),
                stream_pos,
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

    /// Resolve the owning stream for a single subscription scope.
    pub fn stream_id_for_scope(&self, scope_kind: &str, scope_id: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        Self::stream_id_for_scope_inner(&conn, scope_kind, scope_id)
    }

    /// If all scopes map to the same owning stream, return it. Otherwise return None.
    pub fn shared_stream_id_for_scopes(
        &self,
        scopes: &[(String, String)],
    ) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        let mut shared_stream_id: Option<String> = None;
        for (scope_kind, scope_id) in scopes {
            let Some(stream_id) = Self::stream_id_for_scope_inner(&conn, scope_kind, scope_id)?
            else {
                return Ok(None);
            };
            match &shared_stream_id {
                Some(existing) if existing != &stream_id => return Ok(None),
                Some(_) => {}
                None => shared_stream_id = Some(stream_id),
            }
        }
        Ok(shared_stream_id)
    }

    pub fn resolve_subscription_target(
        &self,
        viewer_name: &str,
        target_id: &str,
    ) -> Result<Option<ResolvedSubscriptionTarget>> {
        let conn = self.conn.lock().unwrap();
        Self::resolve_subscription_target_inner(&conn, viewer_name, target_id)
    }

    pub fn resolve_scope_subscription_target(
        &self,
        viewer_name: &str,
        scope_kind: &str,
        scope_id: &str,
    ) -> Result<Option<ResolvedSubscriptionTarget>> {
        let conn = self.conn.lock().unwrap();
        let Some(target_id) = Self::target_id_for_scope_inner(&conn, scope_kind, scope_id)? else {
            return Ok(None);
        };
        Self::resolve_subscription_target_inner(&conn, viewer_name, &target_id)
    }

    pub fn shared_stream_id_for_targets(
        &self,
        targets: &[ResolvedSubscriptionTarget],
    ) -> Result<Option<String>> {
        let mut shared_stream_id: Option<String> = None;
        for target in targets {
            match &shared_stream_id {
                Some(existing) if existing != &target.stream_id => return Ok(None),
                Some(_) => {}
                None => shared_stream_id = Some(target.stream_id.clone()),
            }
        }
        Ok(shared_stream_id)
    }

    fn stream_id_for_scope_inner(
        conn: &Connection,
        scope_kind: &str,
        scope_id: &str,
    ) -> Result<Option<String>> {
        match scope_kind {
            "channel" | "dm" => Ok(scope_id
                .strip_prefix(&format!("{scope_kind}:"))
                .map(|channel_id| format!("conversation:{channel_id}"))),
            "thread" => {
                let Some(parent_message_id) = scope_id.strip_prefix("thread:") else {
                    return Ok(None);
                };
                let channel_id: Option<String> = conn
                    .query_row(
                        "SELECT channel_id FROM messages WHERE id = ?1",
                        params![parent_message_id],
                        |row| row.get(0),
                    )
                    .ok();
                Ok(channel_id.map(|channel_id| format!("conversation:{channel_id}")))
            }
            "agent" => Ok(Some(scope_id.to_string())),
            "team" => Ok(scope_id
                .strip_prefix("team:")
                .map(|team_id| format!("team:{team_id}"))),
            "user" => Ok(scope_id
                .strip_prefix("user:")
                .map(|user_name| format!("inbox:{user_name}"))),
            "workspace" => Ok(Some("workspace:default".to_string())),
            _ => Ok(None),
        }
    }

    fn target_id_for_scope_inner(
        conn: &Connection,
        scope_kind: &str,
        scope_id: &str,
    ) -> Result<Option<String>> {
        match scope_kind {
            "channel" | "dm" => Ok(scope_id
                .strip_prefix(&format!("{scope_kind}:"))
                .map(|channel_id| format!("conversation:{channel_id}"))),
            "thread" => Ok(scope_id
                .strip_prefix("thread:")
                .map(|parent_message_id| format!("thread:{parent_message_id}"))),
            "team" => Ok(scope_id
                .strip_prefix("team:")
                .map(|team_id| format!("team:{team_id}"))),
            "agent" => Ok(scope_id
                .strip_prefix("agent:")
                .map(|agent_name| format!("agent:{agent_name}"))),
            "user" => Ok(scope_id
                .strip_prefix("user:")
                .map(|user_name| format!("inbox:{user_name}"))),
            "workspace" => Ok(Some("workspace:default".to_string())),
            _ => {
                let Some(stream_id) = Self::stream_id_for_scope_inner(conn, scope_kind, scope_id)?
                else {
                    return Ok(None);
                };
                Ok(Some(stream_id))
            }
        }
    }

    fn resolve_subscription_target_inner(
        conn: &Connection,
        viewer_name: &str,
        target_id: &str,
    ) -> Result<Option<ResolvedSubscriptionTarget>> {
        if Self::lookup_sender_type_inner(conn, viewer_name)?.is_none() {
            return Ok(None);
        }

        if let Some(conversation_id) = target_id.strip_prefix("conversation:") {
            let membership_count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM channel_members WHERE channel_id = ?1 AND member_name = ?2",
                params![conversation_id, viewer_name],
                |row| row.get(0),
            )?;
            if membership_count == 0 {
                return Ok(None);
            }
            return Ok(Some(ResolvedSubscriptionTarget {
                target_id: target_id.to_string(),
                stream_id: format!("conversation:{conversation_id}"),
                kind: SubscriptionTargetKind::Conversation {
                    conversation_id: conversation_id.to_string(),
                },
            }));
        }

        if let Some(parent_message_id) = target_id.strip_prefix("thread:") {
            let channel_id: Option<String> = conn
                .query_row(
                    "SELECT channel_id FROM messages WHERE id = ?1",
                    params![parent_message_id],
                    |row| row.get(0),
                )
                .ok();
            let Some(channel_id) = channel_id else {
                return Ok(None);
            };
            let membership_count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM channel_members WHERE channel_id = ?1 AND member_name = ?2",
                params![channel_id, viewer_name],
                |row| row.get(0),
            )?;
            if membership_count == 0 {
                return Ok(None);
            }
            return Ok(Some(ResolvedSubscriptionTarget {
                target_id: target_id.to_string(),
                stream_id: format!("conversation:{channel_id}"),
                kind: SubscriptionTargetKind::Thread {
                    parent_message_id: parent_message_id.to_string(),
                },
            }));
        }

        if let Some(team_id) = target_id.strip_prefix("team:") {
            let team_membership_count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM team_members WHERE team_id = ?1 AND member_name = ?2",
                params![team_id, viewer_name],
                |row| row.get(0),
            )?;
            let room_membership_count: i64 = conn.query_row(
                "SELECT COUNT(*)
                 FROM teams t
                 JOIN channels c ON c.name = t.name AND c.channel_type = 'team' AND c.archived = 0
                 JOIN channel_members cm ON cm.channel_id = c.id
                 WHERE t.id = ?1 AND cm.member_name = ?2",
                params![team_id, viewer_name],
                |row| row.get(0),
            )?;
            if team_membership_count == 0 && room_membership_count == 0 {
                return Ok(None);
            }
            return Ok(Some(ResolvedSubscriptionTarget {
                target_id: target_id.to_string(),
                stream_id: format!("team:{team_id}"),
                kind: SubscriptionTargetKind::Team {
                    team_id: team_id.to_string(),
                },
            }));
        }

        if let Some(agent_name) = target_id.strip_prefix("agent:") {
            if agent_name != viewer_name {
                return Ok(None);
            }
            return Ok(Some(ResolvedSubscriptionTarget {
                target_id: target_id.to_string(),
                stream_id: format!("agent:{agent_name}"),
                kind: SubscriptionTargetKind::Agent {
                    agent_name: agent_name.to_string(),
                },
            }));
        }

        if let Some(user_name) = target_id.strip_prefix("inbox:") {
            if user_name != viewer_name {
                return Ok(None);
            }
            return Ok(Some(ResolvedSubscriptionTarget {
                target_id: target_id.to_string(),
                stream_id: format!("inbox:{user_name}"),
                kind: SubscriptionTargetKind::Inbox {
                    user_name: user_name.to_string(),
                },
            }));
        }

        if target_id == "workspace:default" {
            return Ok(Some(ResolvedSubscriptionTarget {
                target_id: target_id.to_string(),
                stream_id: "workspace:default".to_string(),
                kind: SubscriptionTargetKind::Workspace,
            }));
        }

        Ok(None)
    }
}

pub(crate) fn derive_stream_identity(
    scope_kind: &str,
    scope_id: &str,
    channel_id: Option<&str>,
) -> (String, String, String) {
    if let Some(user_name) = scope_id.strip_prefix("user:") {
        return (
            format!("inbox:{user_name}"),
            "inbox".to_string(),
            user_name.to_string(),
        );
    }

    if let Some(agent_name) = scope_id.strip_prefix("agent:") {
        return (
            format!("agent:{agent_name}"),
            "agent".to_string(),
            agent_name.to_string(),
        );
    }

    if let Some(team_id) = scope_id.strip_prefix("team:") {
        return (
            format!("team:{team_id}"),
            "team".to_string(),
            team_id.to_string(),
        );
    }

    if let Some(channel_id) = channel_id {
        return (
            format!("conversation:{channel_id}"),
            "conversation".to_string(),
            channel_id.to_string(),
        );
    }

    if scope_kind == "workspace" {
        return (
            "workspace:default".to_string(),
            "workspace".to_string(),
            "default".to_string(),
        );
    }

    (
        scope_id.to_string(),
        scope_kind.to_string(),
        scope_id.to_string(),
    )
}
