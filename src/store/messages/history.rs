use anyhow::{anyhow, Result};
use rusqlite::{params, Connection};
use serde_json::Value;

use crate::store::messages::*;
use crate::store::Store;

impl Store {
    pub fn get_history(
        &self,
        channel_name: &str,
        thread_parent_id: Option<&str>,
        limit: i64,
        before: Option<i64>,
        after: Option<i64>,
    ) -> Result<(Vec<HistoryMessage>, bool)> {
        let conn = self.conn.lock().unwrap();
        let (messages, has_more) = Self::get_conversation_history_view_inner(
            &conn,
            channel_name,
            thread_parent_id,
            limit,
            before,
            after,
        )?;
        Ok((
            messages
                .iter()
                .map(ConversationMessageView::to_history_message)
                .collect(),
            has_more,
        ))
    }

    /// Read a history page and durable event cursor together so reconnecting
    /// clients can resume from a cursor that matches the returned snapshot.
    pub fn get_history_snapshot(
        &self,
        channel_name: &str,
        member_name: &str,
        thread_parent_id: Option<&str>,
        limit: i64,
        before: Option<i64>,
        after: Option<i64>,
    ) -> Result<HistorySnapshot> {
        let conn = self.conn.lock().unwrap();
        let (message_views, has_more) = Self::get_conversation_history_view_inner(
            &conn,
            channel_name,
            thread_parent_id,
            limit,
            before,
            after,
        )?;
        let channel = Self::find_channel_by_name_inner(&conn, channel_name)?
            .ok_or_else(|| anyhow!("channel not found: {}", channel_name))?;
        let last_read_seq = if let Some(parent_id) = thread_parent_id {
            Self::get_thread_notification_state_by_channel_id_inner(
                &conn,
                &channel.id,
                parent_id,
                member_name,
            )?
            .map(|state| state.last_read_seq)
            .unwrap_or(0)
        } else {
            Self::get_inbox_conversation_state_by_channel_id_inner(&conn, &channel.id, member_name)?
                .map(|state| state.last_read_seq)
                .unwrap_or(0)
        };
        let latest_event_id: i64 =
            conn.query_row("SELECT COALESCE(MAX(event_id), 0) FROM events", [], |row| {
                row.get(0)
            })?;
        let stream_id = format!("conversation:{}", channel.id);
        let stream_pos: i64 = conn
            .query_row(
                "SELECT current_pos FROM streams WHERE stream_id = ?1",
                params![stream_id.as_str()],
                |row| row.get(0),
            )
            .unwrap_or(0);
        Ok(HistorySnapshot {
            messages: message_views
                .iter()
                .map(ConversationMessageView::to_history_message)
                .collect(),
            has_more,
            last_read_seq,
            latest_event_id,
            stream_id,
            stream_pos,
        })
    }

    fn get_conversation_history_view_inner(
        conn: &Connection,
        channel_name: &str,
        thread_parent_id: Option<&str>,
        limit: i64,
        before: Option<i64>,
        after: Option<i64>,
    ) -> Result<(Vec<ConversationMessageView>, bool)> {
        let channel = Self::find_channel_by_name_inner(conn, channel_name)?
            .ok_or_else(|| anyhow!("channel not found: {}", channel_name))?;

        let fetch_limit = limit + 1;

        // Build SQL with correct positional params depending on which optional args are present.
        // Always: ?1 = channel_id
        // If has_cursor: ?2 = cursor (before or after value)
        // If has_thread: ?N = thread_parent_id (N=2 when no cursor, N=3 when cursor)
        let has_cursor = before.is_some() || after.is_some();
        let cursor_param = if has_cursor { "?2" } else { "" };
        let thread_param_num = if has_cursor { "?3" } else { "?2" };

        let thread_clause = if thread_parent_id.is_some() {
            format!("AND conversation_messages_view.thread_parent_id = {thread_param_num}")
        } else {
            "AND conversation_messages_view.thread_parent_id IS NULL".to_string()
        };
        let (cursor_clause, order, needs_reverse) = if before.is_some() {
            (
                format!("AND conversation_messages_view.seq < {cursor_param}"),
                "DESC",
                true,
            )
        } else if after.is_some() {
            (
                format!("AND conversation_messages_view.seq > {cursor_param}"),
                "ASC",
                false,
            )
        } else {
            (String::new(), "DESC", true)
        };
        if before.is_some() && after.is_some() {
            return Err(anyhow!("cannot specify both before and after"));
        }

        let sql = format!(
            "SELECT conversation_messages_view.message_id AS message_id,
                    conversation_messages_view.conversation_id AS conversation_id,
                    conversation_messages_view.conversation_name AS conversation_name,
                    conversation_messages_view.conversation_type AS conversation_type,
                    conversation_messages_view.thread_parent_id AS thread_parent_id,
                    conversation_messages_view.sender_name AS sender_name,
                    conversation_messages_view.sender_type AS sender_type,
                    conversation_messages_view.sender_deleted AS sender_deleted,
                    conversation_messages_view.content AS content,
                    conversation_messages_view.created_at AS created_at,
                    conversation_messages_view.seq AS seq,
                    conversation_messages_view.forwarded_from AS forwarded_from,
                    thread_summaries_view.reply_count AS reply_count
             FROM conversation_messages_view
             LEFT JOIN thread_summaries_view
               ON thread_summaries_view.conversation_id = conversation_messages_view.conversation_id
              AND thread_summaries_view.parent_message_id = conversation_messages_view.message_id
             WHERE conversation_messages_view.conversation_id = ?1 {thread_clause} {cursor_clause} \
             ORDER BY conversation_messages_view.seq {order} LIMIT {fetch_limit}"
        );

        let cursor_val = before.or(after).unwrap_or(0);
        let thread_val = thread_parent_id.unwrap_or("");
        let mut stmt = conn.prepare(&sql)?;

        // Bind exactly the parameters the SQL expects: ?1=channel_id, optionally ?2=cursor, optionally ?3=thread
        let rows: Vec<ConversationMessageView> = match (has_cursor, thread_parent_id.is_some()) {
            (true, true) => stmt.query_map(
                params![channel.id, cursor_val, thread_val],
                ConversationMessageView::from_projection_row,
            )?,
            (true, false) => stmt.query_map(
                params![channel.id, cursor_val],
                ConversationMessageView::from_projection_row,
            )?,
            (false, true) => stmt.query_map(
                params![channel.id, thread_val],
                ConversationMessageView::from_projection_row,
            )?,
            (false, false) => stmt.query_map(
                params![channel.id],
                ConversationMessageView::from_projection_row,
            )?,
        }
        .filter_map(|r| r.ok())
        .collect();

        let has_more = rows.len() as i64 > limit;
        let mut msgs: Vec<ConversationMessageView> =
            rows.into_iter().take(limit as usize).collect();

        for msg in &mut msgs {
            Self::hydrate_conversation_message_view(conn, msg)?;
        }

        if needs_reverse {
            msgs.reverse();
        }

        Ok((msgs, has_more))
    }

    /// Load one projected conversation message row from the explicit history
    /// read model that backs both UI history and websocket rehydration.
    pub fn get_conversation_message_view(
        &self,
        message_id: &str,
    ) -> Result<Option<ConversationMessageView>> {
        let conn = self.conn.lock().unwrap();
        Self::get_conversation_message_view_inner(&conn, message_id)
    }

    pub(crate) fn get_conversation_message_view_inner(
        conn: &Connection,
        message_id: &str,
    ) -> Result<Option<ConversationMessageView>> {
        let message = conn
            .query_row(
                "SELECT conversation_messages_view.message_id AS message_id,
                        conversation_messages_view.conversation_id AS conversation_id,
                        conversation_messages_view.conversation_name AS conversation_name,
                        conversation_messages_view.conversation_type AS conversation_type,
                        conversation_messages_view.thread_parent_id AS thread_parent_id,
                        conversation_messages_view.sender_name AS sender_name,
                        conversation_messages_view.sender_type AS sender_type,
                        conversation_messages_view.sender_deleted AS sender_deleted,
                        conversation_messages_view.content AS content,
                        conversation_messages_view.created_at AS created_at,
                        conversation_messages_view.seq AS seq,
                        conversation_messages_view.forwarded_from AS forwarded_from,
                        thread_summaries_view.reply_count AS reply_count
                 FROM conversation_messages_view
                 LEFT JOIN thread_summaries_view
                   ON thread_summaries_view.conversation_id = conversation_messages_view.conversation_id
                  AND thread_summaries_view.parent_message_id = conversation_messages_view.message_id
                 WHERE conversation_messages_view.message_id = ?1",
                params![message_id],
                ConversationMessageView::from_projection_row,
            )
            .ok();

        let Some(mut message) = message else {
            return Ok(None);
        };

        Self::hydrate_conversation_message_view(conn, &mut message)?;
        Ok(Some(message))
    }

    pub(crate) fn hydrate_conversation_message_view(
        conn: &Connection,
        message: &mut ConversationMessageView,
    ) -> Result<()> {
        message.attachments = Self::get_message_attachments(conn, &message.message_id)?;
        Ok(())
    }

    /// Rehydrate the canonical message projection for websocket transport so
    /// `message.created` converges with history reads even if the stored event
    /// payload becomes stale.
    pub fn get_message_event_payload(&self, message_id: &str) -> Result<Option<Value>> {
        let conn = self.conn.lock().unwrap();
        Self::get_message_event_payload_inner(&conn, message_id)
    }

    fn get_message_event_payload_inner(
        conn: &Connection,
        message_id: &str,
    ) -> Result<Option<Value>> {
        Ok(Self::get_conversation_message_view_inner(conn, message_id)?
            .map(|message| message.to_transport_payload()))
    }

    pub(crate) fn parse_forwarded_from_raw(raw: Option<&str>) -> Option<ForwardedFrom> {
        raw.and_then(|value| {
            serde_json::from_str::<ForwardedFrom>(value)
                .map_err(|err| {
                    tracing::warn!(raw = value, err = %err, "failed to parse forwarded_from JSON");
                    err
                })
                .ok()
        })
    }

    pub(crate) fn get_message_attachments(
        conn: &Connection,
        message_id: &str,
    ) -> Result<Vec<AttachmentRef>> {
        let rows = conn
            .prepare(
                "SELECT a.id, a.filename FROM message_attachments ma \
                 JOIN attachments a ON ma.attachment_id = a.id WHERE ma.message_id = ?1",
            )?
            .query_map(params![message_id], |row| {
                Ok(AttachmentRef {
                    id: row.get(0)?,
                    filename: row.get(1)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }
}
