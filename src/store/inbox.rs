use std::collections::HashMap;

use anyhow::{anyhow, Result};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};

use super::channels::Channel;
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

impl Store {
    /// Persists `last_read_seq` (and optional `last_read_message_id`) for inbox or thread scope.
    ///
    /// Usually only advances: if `last_read_seq <=` stored value, the write is skipped.
    /// Exception: when the stored cursor is **above** `MAX(messages.seq)` in the same scope
    /// (orphan row), we still apply the write so `set_history_read_cursor` can correct it.
    ///
    /// `thread_parent_id`: `None` → `inbox_read_state`; `Some` → `inbox_thread_read_state`.
    pub(crate) fn set_read_cursor_tx(
        tx: &Transaction<'_>,
        channel: &Channel,
        thread_parent_id: Option<&str>,
        member_name: &str,
        member_type: &str,
        last_read_seq: i64,
        last_read_message_id: Option<&str>,
    ) -> Result<()> {
        let current_last_read_seq = match thread_parent_id {
            None => tx
                .query_row(
                    "SELECT last_read_seq
                     FROM inbox_read_state
                     WHERE conversation_id = ?1 AND member_name = ?2",
                    params![channel.id, member_name],
                    |row| row.get::<_, i64>(0),
                )
                .optional()?
                .unwrap_or(0),
            Some(parent_id) => tx
                .query_row(
                    "SELECT last_read_seq
                     FROM inbox_thread_read_state
                     WHERE conversation_id = ?1 AND thread_parent_id = ?2 AND member_name = ?3",
                    params![channel.id, parent_id, member_name],
                    |row| row.get::<_, i64>(0),
                )
                .optional()?
                .unwrap_or(0),
        };
        // Monotonic default: do not move `last_read_seq` backward.
        if last_read_seq <= current_last_read_seq {
            // Unless the stored value is impossible vs current messages — then allow overwrite.
            let max_seq: i64 = match thread_parent_id {
                None => tx.query_row(
                    "SELECT COALESCE(MAX(seq), 0) FROM messages WHERE channel_id = ?1",
                    params![channel.id],
                    |row| row.get(0),
                )?,
                Some(parent_id) => tx.query_row(
                    "SELECT COALESCE(MAX(seq), 0)
                     FROM messages
                     WHERE channel_id = ?1 AND thread_parent_id = ?2",
                    params![channel.id, parent_id],
                    |row| row.get(0),
                )?,
            };
            if current_last_read_seq <= max_seq {
                return Ok(());
            }
        }

        match thread_parent_id {
            None => {
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
            }
            Some(thread_parent_id) => {
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
            }
        }
        Ok(())
    }

    /// Conversation-level `last_read_seq` (see [`Self::set_read_cursor_tx`]).
    pub(crate) fn set_inbox_read_cursor_tx(
        tx: &Transaction<'_>,
        channel: &Channel,
        member_name: &str,
        member_type: &str,
        last_read_seq: i64,
        last_read_message_id: Option<&str>,
    ) -> Result<()> {
        Self::set_read_cursor_tx(
            tx,
            channel,
            None,
            member_name,
            member_type,
            last_read_seq,
            last_read_message_id,
        )
    }

    /// Per-thread `last_read_seq` for replies under `thread_parent_id`.
    pub(crate) fn set_thread_read_cursor_tx(
        tx: &Transaction<'_>,
        channel: &Channel,
        thread_parent_id: &str,
        member_name: &str,
        member_type: &str,
        last_read_seq: i64,
        last_read_message_id: Option<&str>,
    ) -> Result<()> {
        Self::set_read_cursor_tx(
            tx,
            channel,
            Some(thread_parent_id),
            member_name,
            member_type,
            last_read_seq,
            last_read_message_id,
        )
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
        let channel = Self::get_channel_by_name_inner(conn, channel_name)?
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
        let channel = Self::get_channel_by_name_inner(&conn, channel_name)?
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
    pub fn get_inbox_conversation_notifications(
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
        let channel = Self::get_channel_by_name_inner(&conn, channel_name)?
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
