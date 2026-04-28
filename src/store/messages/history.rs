use anyhow::{anyhow, Result};
use rusqlite::{params, Connection};
use serde_json::Value;

use crate::store::messages::*;
use crate::store::Store;

/// Caller intent for history reads — drives the `payload.audience` filter.
/// Use `All` for human/UI paths (chips and ambient markers stay visible) and
/// `AgentVisibleOnly` for agent context (humans-only payloads are hidden).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HistoryAudience {
    All,
    AgentVisibleOnly,
}

impl Store {
    pub fn get_history(
        &self,
        channel_name: &str,
        limit: i64,
        before: Option<i64>,
        after: Option<i64>,
    ) -> Result<(Vec<HistoryMessage>, bool)> {
        let conn = self.conn.lock().unwrap();
        let (messages, has_more) = Self::get_conversation_history_view_inner(
            &conn,
            channel_name,
            limit,
            before,
            after,
            HistoryAudience::All,
        )?;
        Ok((
            messages
                .iter()
                .map(ConversationMessageView::to_history_message)
                .collect(),
            has_more,
        ))
    }

    /// Read a history page with read cursor for the requesting member.
    /// Includes ambient structured notices — used by the human UI which renders
    /// them as chip-rows.
    pub fn get_history_snapshot(
        &self,
        channel_name: &str,
        member_id: &str,
        limit: i64,
        before: Option<i64>,
        after: Option<i64>,
    ) -> Result<HistorySnapshot> {
        Self::get_history_snapshot_inner(
            self,
            channel_name,
            member_id,
            limit,
            before,
            after,
            HistoryAudience::All,
        )
    }

    /// Same as [`get_history_snapshot`] but excludes rows tagged for human
    /// audiences (`payload.audience == 'humans'`, e.g. member_joined chips).
    /// Used by agent-facing history reads so agents don't see ambient channel
    /// markers in their context window. Mirrors the filter applied to
    /// [`get_messages_for_agent_id`].
    pub fn get_history_snapshot_for_agent(
        &self,
        channel_name: &str,
        member_id: &str,
        limit: i64,
        before: Option<i64>,
        after: Option<i64>,
    ) -> Result<HistorySnapshot> {
        Self::get_history_snapshot_inner(
            self,
            channel_name,
            member_id,
            limit,
            before,
            after,
            HistoryAudience::AgentVisibleOnly,
        )
    }

    fn get_history_snapshot_inner(
        store: &Store,
        channel_name: &str,
        member_id: &str,
        limit: i64,
        before: Option<i64>,
        after: Option<i64>,
        audience: HistoryAudience,
    ) -> Result<HistorySnapshot> {
        let conn = store.conn.lock().unwrap();
        let (message_views, has_more) = Self::get_conversation_history_view_inner(
            &conn,
            channel_name,
            limit,
            before,
            after,
            audience,
        )?;
        let channel = Self::get_channel_by_name_inner(&conn, channel_name)?
            .ok_or_else(|| anyhow!("channel not found: {}", channel_name))?;
        let last_read_seq =
            Self::get_inbox_conversation_state_by_channel_id_inner(&conn, &channel.id, member_id)?
                .map(|state| state.last_read_seq)
                .unwrap_or(0);
        Ok(HistorySnapshot {
            messages: message_views
                .iter()
                .map(ConversationMessageView::to_history_message)
                .collect(),
            has_more,
            last_read_seq,
        })
    }

    fn get_conversation_history_view_inner(
        conn: &Connection,
        channel_name: &str,
        limit: i64,
        before: Option<i64>,
        after: Option<i64>,
        audience: HistoryAudience,
    ) -> Result<(Vec<ConversationMessageView>, bool)> {
        let channel = Self::get_channel_by_name_inner(conn, channel_name)?
            .ok_or_else(|| anyhow!("channel not found: {}", channel_name))?;

        if before.is_some() && after.is_some() {
            return Err(anyhow!("cannot specify both before and after"));
        }

        let fetch_limit = limit + 1;
        let (cursor_clause, order, needs_reverse) = if before.is_some() {
            ("AND seq < ?2", "DESC", true)
        } else if after.is_some() {
            ("AND seq > ?2", "ASC", false)
        } else {
            ("", "DESC", true)
        };

        // Structural audience filter via payload field — not a kind allowlist.
        // Rows with no `payload.audience` default to "all" and reach everyone.
        // Member-joined chips set `audience: "humans"` and are hidden here.
        let audience_clause = match audience {
            HistoryAudience::All => "",
            HistoryAudience::AgentVisibleOnly => {
                "AND COALESCE(json_extract(payload, '$.audience'), 'all') != 'humans'"
            }
        };

        let sql = format!(
            "SELECT message_id, conversation_id, conversation_name, conversation_type,
                    sender_name, sender_type, sender_deleted, content, created_at, seq,
                    forwarded_from, run_id, trace_summary, payload
             FROM conversation_messages_view
             WHERE conversation_id = ?1 {cursor_clause} {audience_clause}
             ORDER BY seq {order} LIMIT {fetch_limit}"
        );

        let mut stmt = conn.prepare(&sql)?;
        let rows: Vec<ConversationMessageView> = match before.or(after) {
            Some(cursor) => stmt.query_map(
                params![channel.id, cursor],
                ConversationMessageView::from_projection_row,
            )?,
            None => stmt.query_map(
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
                "SELECT message_id, conversation_id, conversation_name, conversation_type,
                        sender_name, sender_type, sender_deleted, content, created_at, seq,
                        forwarded_from, run_id, trace_summary, payload
                 FROM conversation_messages_view
                 WHERE message_id = ?1",
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
