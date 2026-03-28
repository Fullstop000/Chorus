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
    /// Count of unread top-level messages after `last_read_seq`.
    pub unread_count: i64,
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
                FROM messages m
                WHERE m.channel_id = cm.channel_id
                  AND m.thread_parent_id IS NULL
                  AND m.seq > COALESCE(irs.last_read_seq, 0)
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
                    "lastReadSeq": last_read_seq,
                    "lastReadMessageId": last_read_message_id,
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
}
