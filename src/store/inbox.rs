use std::collections::HashMap;

use anyhow::{anyhow, Result};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};

use super::channels::Channel;
use super::events::NewEvent;
use super::Store;

/// Explicit read-model row for per-member conversation state while inbox
/// ownership is still projected from channel membership storage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboxConversationStateView {
    /// Owning conversation UUID.
    pub conversation_id: String,
    /// Conversation slug.
    pub conversation_name: String,
    /// Conversation API kind (`channel`, `dm`, `team`, `system`).
    pub conversation_type: String,
    /// Member handle.
    pub member_name: String,
    /// `human` or `agent`.
    pub member_type: String,
    /// Latest read per-conversation sequence.
    pub last_read_seq: i64,
    /// Message id at `last_read_seq` when it exists.
    pub last_read_message_id: Option<String>,
    /// Count of unread conversation messages after `last_read_seq`.
    /// Humans count all thread replies in the parent conversation badge.
    /// Agents only count thread replies they can access.
    pub unread_count: i64,
}

/// Absolute notification snapshot for one conversation, suitable for inbox
/// bootstrap and sidebar badge state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboxConversationNotificationView {
    /// Owning conversation UUID.
    pub conversation_id: String,
    /// Conversation slug.
    pub conversation_name: String,
    /// Conversation API kind (`channel`, `dm`, `team`, `system`).
    pub conversation_type: String,
    /// Latest committed conversation sequence.
    pub latest_seq: i64,
    /// Latest read per-conversation sequence.
    pub last_read_seq: i64,
    /// Count of unread conversation messages after `last_read_seq`.
    pub unread_count: i64,
    /// Most recent message id when present.
    pub last_message_id: Option<String>,
    /// Most recent message timestamp when present.
    pub last_message_at: Option<String>,
}

/// Derived per-member notification state for one thread using dedicated thread
/// read cursors.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadNotificationStateView {
    /// Owning conversation UUID.
    pub conversation_id: String,
    /// Parent message id that anchors the thread.
    pub thread_parent_id: String,
    /// Latest reply sequence in the conversation stream.
    pub latest_seq: i64,
    /// Highest reply sequence the member has explicitly read in this thread.
    pub last_read_seq: i64,
    /// Replies newer than `last_read_seq`.
    pub unread_count: i64,
    /// Most recent reply id when present.
    pub last_reply_message_id: Option<String>,
    /// Most recent reply timestamp when present.
    pub last_reply_at: Option<String>,
}

impl InboxConversationStateView {
    fn from_projection_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            conversation_id: row.get("conversation_id")?,
            conversation_name: row.get("conversation_name")?,
            conversation_type: row.get("conversation_type")?,
            member_name: row.get("member_name")?,
            member_type: row.get("member_type")?,
            last_read_seq: row.get("last_read_seq")?,
            last_read_message_id: row.get("last_read_message_id")?,
            unread_count: row.get("unread_count")?,
        })
    }
}

pub(super) fn refresh_inbox_conversation_state_view(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        DROP VIEW IF EXISTS inbox_conversation_state_view;
        CREATE VIEW inbox_conversation_state_view AS
        SELECT
            cm.channel_id AS conversation_id,
            c.name AS conversation_name,
            c.channel_type AS conversation_type,
            cm.member_name AS member_name,
            cm.member_type AS member_type,
            COALESCE(irs.last_read_seq, 0) AS last_read_seq,
            irs.last_read_message_id AS last_read_message_id,
            (
                SELECT COUNT(*)
                FROM messages top_level
                WHERE top_level.channel_id = cm.channel_id
                  AND top_level.thread_parent_id IS NULL
                  AND top_level.seq > COALESCE(irs.last_read_seq, 0)
            ) + (
                SELECT COUNT(*)
                FROM messages reply
                LEFT JOIN inbox_thread_read_state itrs
                  ON itrs.conversation_id = reply.channel_id
                 AND itrs.thread_parent_id = reply.thread_parent_id
                 AND itrs.member_name = cm.member_name
                WHERE reply.channel_id = cm.channel_id
                  AND reply.thread_parent_id IS NOT NULL
                  AND reply.seq > COALESCE(itrs.last_read_seq, 0)
                  AND (
                    cm.member_type != 'agent'
                    OR EXISTS (
                        SELECT 1
                        FROM messages parent
                        WHERE parent.id = reply.thread_parent_id
                          AND parent.channel_id = cm.channel_id
                          AND parent.sender_type = 'agent'
                          AND parent.sender_name = cm.member_name
                    )
                    OR EXISTS (
                        SELECT 1
                        FROM messages prior
                        WHERE prior.channel_id = cm.channel_id
                          AND prior.thread_parent_id = reply.thread_parent_id
                          AND prior.sender_type = 'agent'
                          AND prior.sender_name = cm.member_name
                          AND prior.seq < reply.seq
                    )
                  )
            ) AS unread_count
        FROM channel_members cm
        JOIN channels c ON c.id = cm.channel_id
        LEFT JOIN inbox_read_state irs
          ON irs.conversation_id = cm.channel_id
         AND irs.member_name = cm.member_name;
        ",
    )?;
    Ok(())
}

pub(super) fn migrate_inbox_read_state(conn: &Connection) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO inbox_read_state (
            conversation_id, member_name, member_type, last_read_seq, last_read_message_id
         )
         SELECT
            cm.channel_id,
            cm.member_name,
            cm.member_type,
            cm.last_read_seq,
            (
                SELECT m.id
                FROM messages m
                WHERE m.channel_id = cm.channel_id
                  AND m.seq = cm.last_read_seq
                LIMIT 1
            )
         FROM channel_members cm",
        [],
    )?;
    Ok(())
}

impl Store {
    fn latest_conversation_message_inner(
        conn: &Connection,
        channel_id: &str,
    ) -> Result<Option<(i64, String, String)>> {
        Ok(conn
            .query_row(
                "SELECT seq, id, created_at
                 FROM messages
                 WHERE channel_id = ?1
                 ORDER BY seq DESC
                 LIMIT 1",
                params![channel_id],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?)
    }

    pub(crate) fn set_inbox_read_cursor_tx(
        tx: &Transaction<'_>,
        channel: &Channel,
        member_name: &str,
        member_type: &str,
        last_read_seq: i64,
        last_read_message_id: Option<&str>,
        emit_event: bool,
        caused_by_kind: &'static str,
    ) -> Result<Option<i64>> {
        let current_last_read_seq = tx
            .query_row(
                "SELECT last_read_seq
                 FROM inbox_read_state
                 WHERE conversation_id = ?1 AND member_name = ?2",
                params![channel.id, member_name],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .unwrap_or(0);
        if last_read_seq <= current_last_read_seq {
            return Ok(None);
        }

        tx.execute(
            "INSERT INTO inbox_read_state (
                conversation_id, member_name, member_type, last_read_seq, last_read_message_id
             ) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(conversation_id, member_name) DO UPDATE SET
                member_type = excluded.member_type,
                last_read_seq = excluded.last_read_seq,
                last_read_message_id = excluded.last_read_message_id,
                updated_at = datetime('now')",
            params![
                channel.id,
                member_name,
                member_type,
                last_read_seq,
                last_read_message_id,
            ],
        )?;

        if !emit_event {
            return Ok(None);
        }

        let conversation_state =
            Self::get_inbox_conversation_state_by_channel_id_inner(tx, &channel.id, member_name)?;
        let latest_message = Self::latest_conversation_message_inner(tx, &channel.id)?;
        let latest_seq = latest_message.as_ref().map(|(seq, _, _)| *seq).unwrap_or(0);
        let last_message_id = latest_message.as_ref().map(|(_, message_id, _)| message_id);
        let last_message_at = latest_message.as_ref().map(|(_, _, created_at)| created_at);
        let unread_count = conversation_state
            .as_ref()
            .map(|state| state.unread_count)
            .unwrap_or(0);

        let event_id = Self::append_event_tx(
            tx,
            NewEvent {
                event_type: "conversation.read_cursor_set",
                scope_kind: "user",
                scope_id: format!("user:{member_name}"),
                channel_id: Some(channel.id.as_str()),
                channel_name: Some(channel.name.as_str()),
                thread_parent_id: None,
                actor_name: Some(member_name),
                actor_type: Some(member_type),
                caused_by_kind: Some(caused_by_kind),
                payload: serde_json::json!({
                    "conversationId": channel.id,
                    "conversationName": channel.name,
                    "latestSeq": latest_seq,
                    "lastReadSeq": last_read_seq,
                    "unreadCount": unread_count,
                    "lastReadMessageId": last_read_message_id,
                    "lastMessageId": last_message_id,
                    "lastMessageAt": last_message_at,
                }),
            },
        )?;
        Ok(Some(event_id))
    }

    pub(crate) fn set_thread_read_cursor_tx(
        tx: &Transaction<'_>,
        channel: &Channel,
        thread_parent_id: &str,
        member_name: &str,
        member_type: &str,
        last_read_seq: i64,
        last_read_message_id: Option<&str>,
        emit_event: bool,
        caused_by_kind: &'static str,
    ) -> Result<Option<i64>> {
        let current_last_read_seq = tx
            .query_row(
                "SELECT last_read_seq
                 FROM inbox_thread_read_state
                 WHERE conversation_id = ?1 AND thread_parent_id = ?2 AND member_name = ?3",
                params![channel.id, thread_parent_id, member_name],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .unwrap_or(0);
        if last_read_seq <= current_last_read_seq {
            return Ok(None);
        }

        tx.execute(
            "INSERT INTO inbox_thread_read_state (
                conversation_id, thread_parent_id, member_name, member_type, last_read_seq, last_read_message_id
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(conversation_id, thread_parent_id, member_name) DO UPDATE SET
                member_type = excluded.member_type,
                last_read_seq = excluded.last_read_seq,
                last_read_message_id = excluded.last_read_message_id,
                updated_at = datetime('now')",
            params![
                channel.id,
                thread_parent_id,
                member_name,
                member_type,
                last_read_seq,
                last_read_message_id,
            ],
        )?;

        if !emit_event {
            return Ok(None);
        }

        let thread_state = Self::get_thread_notification_state_by_channel_id_inner(
            tx,
            &channel.id,
            thread_parent_id,
            member_name,
        )?;
        let latest_seq = thread_state
            .as_ref()
            .map(|state| state.latest_seq)
            .unwrap_or(0);
        let unread_count = thread_state
            .as_ref()
            .map(|state| state.unread_count)
            .unwrap_or(0);
        let last_reply_message_id = thread_state
            .as_ref()
            .and_then(|state| state.last_reply_message_id.as_deref());
        let last_reply_at = thread_state
            .as_ref()
            .and_then(|state| state.last_reply_at.as_deref());

        let event_id = Self::append_event_tx(
            tx,
            NewEvent {
                event_type: "thread.read_cursor_set",
                scope_kind: "user",
                scope_id: format!("user:{member_name}"),
                channel_id: Some(channel.id.as_str()),
                channel_name: Some(channel.name.as_str()),
                thread_parent_id: Some(thread_parent_id),
                actor_name: Some(member_name),
                actor_type: Some(member_type),
                caused_by_kind: Some(caused_by_kind),
                payload: serde_json::json!({
                    "conversationId": channel.id,
                    "conversationName": channel.name,
                    "threadParentId": thread_parent_id,
                    "latestSeq": latest_seq,
                    "lastReadSeq": last_read_seq,
                    "unreadCount": unread_count,
                    "lastReadMessageId": last_read_message_id,
                    "lastReplyMessageId": last_reply_message_id,
                    "lastReplyAt": last_reply_at,
                }),
            },
        )?;
        Ok(Some(event_id))
    }

    /// Load one projected inbox/read-state row for a specific member in a
    /// conversation.
    pub fn get_inbox_conversation_state(
        &self,
        channel_name: &str,
        member_name: &str,
    ) -> Result<Option<InboxConversationStateView>> {
        let conn = self.conn.lock().unwrap();
        Self::get_inbox_conversation_state_inner(&conn, channel_name, member_name)
    }

    pub(crate) fn get_inbox_conversation_state_inner(
        conn: &Connection,
        channel_name: &str,
        member_name: &str,
    ) -> Result<Option<InboxConversationStateView>> {
        let channel = Self::find_channel_by_name_inner(conn, channel_name)?
            .ok_or_else(|| anyhow!("channel not found: {}", channel_name))?;
        Self::get_inbox_conversation_state_by_channel_id_inner(conn, &channel.id, member_name)
    }

    pub(crate) fn get_inbox_conversation_state_by_channel_id_inner(
        conn: &Connection,
        channel_id: &str,
        member_name: &str,
    ) -> Result<Option<InboxConversationStateView>> {
        Ok(conn
            .query_row(
                "SELECT conversation_id, conversation_name, conversation_type,
                        member_name, member_type, last_read_seq,
                        last_read_message_id, unread_count
                 FROM inbox_conversation_state_view
                 WHERE conversation_id = ?1 AND member_name = ?2",
                params![channel_id, member_name],
                InboxConversationStateView::from_projection_row,
            )
            .ok())
    }

    pub fn get_last_read_seq(&self, channel_name: &str, member_name: &str) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        let channel = Self::find_channel_by_name_inner(&conn, channel_name)?
            .ok_or_else(|| anyhow!("channel not found: {}", channel_name))?;
        let seq: i64 = conn.query_row(
            "SELECT last_read_seq
             FROM inbox_conversation_state_view
             WHERE conversation_id = ?1 AND member_name = ?2",
            params![channel.id, member_name],
            |row| row.get(0),
        )?;
        Ok(seq)
    }

    pub fn get_unread_summary(&self, member_name: &str) -> Result<HashMap<String, i64>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT conversation_name, unread_count
             FROM inbox_conversation_state_view
             WHERE member_name = ?1 AND unread_count > 0",
        )?;
        let rows = stmt
            .query_map(params![member_name], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?
            .filter_map(|row| row.ok());

        let mut map = HashMap::new();
        for (conversation_name, unread_count) in rows {
            map.insert(conversation_name, unread_count);
        }
        Ok(map)
    }

    /// Load absolute conversation notification state for sidebar/bootstrap use.
    pub fn list_inbox_conversation_notifications(
        &self,
        member_name: &str,
    ) -> Result<Vec<InboxConversationNotificationView>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT
                view.conversation_id,
                view.conversation_name,
                view.conversation_type,
                view.last_read_seq,
                view.unread_count,
                (
                    SELECT COALESCE(MAX(m.seq), 0)
                    FROM messages m
                    WHERE m.channel_id = view.conversation_id
                ) AS latest_seq,
                (
                    SELECT m.id
                    FROM messages m
                    WHERE m.channel_id = view.conversation_id
                    ORDER BY m.seq DESC
                    LIMIT 1
                ) AS last_message_id,
                (
                    SELECT m.created_at
                    FROM messages m
                    WHERE m.channel_id = view.conversation_id
                    ORDER BY m.seq DESC
                    LIMIT 1
                ) AS last_message_at
             FROM inbox_conversation_state_view view
             WHERE view.member_name = ?1
             ORDER BY view.conversation_name ASC",
        )?;
        let rows = stmt.query_map(params![member_name], |row| {
            Ok(InboxConversationNotificationView {
                conversation_id: row.get("conversation_id")?,
                conversation_name: row.get("conversation_name")?,
                conversation_type: row.get("conversation_type")?,
                latest_seq: row.get("latest_seq")?,
                last_read_seq: row.get("last_read_seq")?,
                unread_count: row.get("unread_count")?,
                last_message_id: row.get("last_message_id")?,
                last_message_at: row.get("last_message_at")?,
            })
        })?;
        Ok(rows.filter_map(|row| row.ok()).collect())
    }

    /// Load derived thread notification state for one member.
    pub fn get_thread_notification_state(
        &self,
        channel_name: &str,
        thread_parent_id: &str,
        member_name: &str,
    ) -> Result<Option<ThreadNotificationStateView>> {
        let conn = self.conn.lock().unwrap();
        let channel = Self::find_channel_by_name_inner(&conn, channel_name)?
            .ok_or_else(|| anyhow!("channel not found: {}", channel_name))?;
        Self::get_thread_notification_state_by_channel_id_inner(
            &conn,
            &channel.id,
            thread_parent_id,
            member_name,
        )
    }

    pub(crate) fn get_thread_notification_state_by_channel_id_inner(
        conn: &Connection,
        channel_id: &str,
        thread_parent_id: &str,
        member_name: &str,
    ) -> Result<Option<ThreadNotificationStateView>> {
        let latest_reply = conn
            .query_row(
                "SELECT seq, id, created_at
                 FROM messages
                 WHERE channel_id = ?1 AND thread_parent_id = ?2
                 ORDER BY seq DESC
                 LIMIT 1",
                params![channel_id, thread_parent_id],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?;
        let Some((latest_seq, last_reply_message_id, last_reply_at)) = latest_reply else {
            return Ok(None);
        };

        let member_type = conn
            .query_row(
                "SELECT member_type
                 FROM channel_members
                 WHERE channel_id = ?1 AND member_name = ?2",
                params![channel_id, member_name],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let Some(member_type) = member_type else {
            return Ok(None);
        };
        if member_type == "agent" {
            let parent_author_matches: i64 = conn.query_row(
                "SELECT COUNT(*)
                 FROM messages
                 WHERE id = ?1 AND channel_id = ?2 AND sender_type = 'agent' AND sender_name = ?3",
                params![thread_parent_id, channel_id, member_name],
                |row| row.get(0),
            )?;
            let prior_reply_matches: i64 = conn.query_row(
                "SELECT COUNT(*)
                 FROM messages
                 WHERE channel_id = ?1 AND thread_parent_id = ?2 AND sender_type = 'agent' AND sender_name = ?3",
                params![channel_id, thread_parent_id, member_name],
                |row| row.get(0),
            )?;
            if parent_author_matches == 0 && prior_reply_matches == 0 {
                return Ok(None);
            }
        }

        let last_read_seq = conn
            .query_row(
                "SELECT last_read_seq
                 FROM inbox_thread_read_state
                 WHERE conversation_id = ?1 AND thread_parent_id = ?2 AND member_name = ?3",
                params![channel_id, thread_parent_id, member_name],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .unwrap_or(0);
        let unread_count = conn.query_row(
            "SELECT COUNT(*)
             FROM messages
             WHERE channel_id = ?1 AND thread_parent_id = ?2 AND seq > ?3",
            params![channel_id, thread_parent_id, last_read_seq],
            |row| row.get::<_, i64>(0),
        )?;

        Ok(Some(ThreadNotificationStateView {
            conversation_id: channel_id.to_string(),
            thread_parent_id: thread_parent_id.to_string(),
            latest_seq,
            last_read_seq,
            unread_count,
            last_reply_message_id: Some(last_reply_message_id),
            last_reply_at: Some(last_reply_at),
        }))
    }
}
