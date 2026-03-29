use anyhow::{anyhow, Result};
use rusqlite::{params, Transaction};
use serde_json::json;
use uuid::Uuid;

use crate::store::channels::{Channel, ChannelType};
use crate::store::events::NewEvent;
use crate::store::messages::*;
use crate::store::Store;

impl Store {
    pub(crate) fn conversation_scope_for(channel: &Channel) -> (&'static str, String) {
        let scope_kind = if channel.channel_type == ChannelType::Dm {
            "dm"
        } else {
            "channel"
        };
        (scope_kind, format!("{scope_kind}:{}", channel.id))
    }

    pub(crate) fn message_scope_for(
        channel: &Channel,
        thread_parent_id: Option<&str>,
    ) -> (&'static str, String) {
        match thread_parent_id {
            Some(parent_id) => ("thread", format!("thread:{parent_id}")),
            None => Self::conversation_scope_for(channel),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn insert_message_tx(
        tx: &Transaction<'_>,
        channel: &Channel,
        thread_parent_id: Option<&str>,
        sender_name: &str,
        sender_type: SenderType,
        content: &str,
        attachment_ids: &[String],
        forwarded_from: Option<&ForwardedFrom>,
    ) -> Result<InsertedMessage> {
        let seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM messages WHERE channel_id = ?1",
            params![channel.id],
            |row| row.get(0),
        )?;
        let msg_id = Uuid::new_v4().to_string();
        let forwarded_from_json = forwarded_from.map(serde_json::to_string).transpose()?;
        tx.execute(
            "INSERT INTO messages (
                id, channel_id, thread_parent_id, sender_name, sender_type, sender_deleted, content, seq, forwarded_from
             ) VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, ?7, ?8)",
            params![
                msg_id,
                channel.id,
                thread_parent_id,
                sender_name,
                sender_type.as_str(),
                content,
                seq,
                forwarded_from_json
            ],
        )?;
        for att_id in attachment_ids {
            tx.execute(
                "INSERT INTO message_attachments (message_id, attachment_id) VALUES (?1, ?2)",
                params![msg_id, att_id],
            )?;
        }

        Ok(InsertedMessage { id: msg_id, seq })
    }

    pub(crate) fn append_message_created_event_tx(
        tx: &Transaction<'_>,
        channel: &Channel,
        thread_parent_id: Option<&str>,
        sender_name: &str,
        sender_type: SenderType,
        inserted: &InsertedMessage,
        caused_by_kind: &'static str,
    ) -> Result<i64> {
        let (scope_kind, scope_id) = Self::message_scope_for(channel, thread_parent_id);
        Self::append_event_tx(
            tx,
            NewEvent {
                event_type: "message.created",
                scope_kind,
                scope_id,
                channel_id: Some(&channel.id),
                channel_name: Some(&channel.name),
                thread_parent_id,
                actor_name: Some(sender_name),
                actor_type: Some(sender_type.as_str()),
                caused_by_kind: Some(caused_by_kind),
                payload: json!({
                    "messageId": inserted.id.as_str(),
                    "conversationId": channel.id.as_str(),
                    "conversationType": channel.channel_type.as_api_str(),
                    "threadParentId": thread_parent_id,
                }),
            },
        )
    }

    pub(crate) fn append_conversation_state_event_tx(
        tx: &Transaction<'_>,
        channel: &Channel,
        thread_parent_id: Option<&str>,
        sender_name: &str,
        sender_type: SenderType,
        inserted: &InsertedMessage,
        caused_by_kind: &'static str,
    ) -> Result<i64> {
        let (scope_kind, scope_id) = Self::conversation_scope_for(channel);
        Self::append_event_tx(
            tx,
            NewEvent {
                event_type: "conversation.state",
                scope_kind,
                scope_id,
                channel_id: Some(&channel.id),
                channel_name: Some(&channel.name),
                thread_parent_id,
                actor_name: Some(sender_name),
                actor_type: Some(sender_type.as_str()),
                caused_by_kind: Some(caused_by_kind),
                payload: json!({
                    "conversationId": channel.id.as_str(),
                    "conversationName": channel.name.as_str(),
                    "conversationType": channel.channel_type.as_api_str(),
                    "messageId": inserted.id.as_str(),
                    "latestSeq": inserted.seq,
                    "threadParentId": thread_parent_id,
                }),
            },
        )
    }

    pub(crate) fn append_system_notice_event_tx(
        tx: &Transaction<'_>,
        channel: &Channel,
        inserted: &InsertedMessage,
    ) -> Result<i64> {
        let (scope_kind, scope_id) = Self::conversation_scope_for(channel);
        Self::append_event_tx(
            tx,
            NewEvent {
                event_type: "system.notice_posted",
                scope_kind,
                scope_id,
                channel_id: Some(&channel.id),
                channel_name: Some(&channel.name),
                thread_parent_id: None,
                actor_name: Some("system"),
                actor_type: None,
                caused_by_kind: Some("post_system_message"),
                payload: json!({
                    "messageId": inserted.id.as_str(),
                    "conversationId": channel.id.as_str(),
                    "noticeKind": "system_message",
                }),
            },
        )
    }

    pub(crate) fn append_thread_events_tx(
        tx: &Transaction<'_>,
        channel: &Channel,
        parent_id: &str,
        sender_name: &str,
        sender_type: SenderType,
        sender_was_participant_before: bool,
    ) -> Result<i64> {
        let (conversation_scope_kind, conversation_scope_id) =
            Self::conversation_scope_for(channel);
        let summary = tx.query_row(
            "SELECT conversation_id, parent_message_id, reply_count,
                    last_reply_message_id, last_reply_at, participant_count
             FROM thread_summaries_view
             WHERE conversation_id = ?1 AND parent_message_id = ?2",
            params![channel.id, parent_id],
            ThreadSummaryView::from_projection_row,
        )?;
        let _ = Self::append_event_tx(
            tx,
            NewEvent {
                event_type: "thread.reply_count_changed",
                scope_kind: conversation_scope_kind,
                scope_id: conversation_scope_id,
                channel_id: Some(&channel.id),
                channel_name: Some(&channel.name),
                thread_parent_id: Some(parent_id),
                actor_name: Some(sender_name),
                actor_type: Some(sender_type.as_str()),
                caused_by_kind: Some("send_message"),
                payload: json!({
                    "parentMessageId": parent_id,
                    "conversationId": channel.id.as_str(),
                    "replyCount": summary.reply_count,
                }),
            },
        )?;

        let thread_scope_id = format!("thread:{parent_id}");
        let mut last_event_id = Self::append_event_tx(
            tx,
            NewEvent {
                event_type: "thread.activity_bumped",
                scope_kind: "thread",
                scope_id: thread_scope_id.clone(),
                channel_id: Some(&channel.id),
                channel_name: Some(&channel.name),
                thread_parent_id: Some(parent_id),
                actor_name: Some(sender_name),
                actor_type: Some(sender_type.as_str()),
                caused_by_kind: Some("send_message"),
                payload: json!({
                    "parentMessageId": parent_id,
                    "lastReplyAt": summary.last_reply_at.as_deref(),
                    "lastReplyMessageId": summary.last_reply_message_id.as_deref(),
                }),
            },
        )?;

        if !sender_was_participant_before {
            last_event_id = Self::append_event_tx(
                tx,
                NewEvent {
                    event_type: "thread.participant_added",
                    scope_kind: "thread",
                    scope_id: thread_scope_id,
                    channel_id: Some(&channel.id),
                    channel_name: Some(&channel.name),
                    thread_parent_id: Some(parent_id),
                    actor_name: Some(sender_name),
                    actor_type: Some(sender_type.as_str()),
                    caused_by_kind: Some("send_message"),
                    payload: json!({
                        "parentMessageId": parent_id,
                        "participant": {
                            "name": sender_name,
                            "type": sender_type.as_str(),
                        },
                        "reason": "reply_sent",
                    }),
                },
            )?;
        }

        Ok(last_event_id)
    }

    pub(crate) fn append_thread_state_event_tx(
        tx: &Transaction<'_>,
        channel: &Channel,
        parent_id: &str,
        sender_name: &str,
        sender_type: SenderType,
        inserted: &InsertedMessage,
    ) -> Result<i64> {
        let summary = tx.query_row(
            "SELECT conversation_id, parent_message_id, reply_count,
                    last_reply_message_id, last_reply_at, participant_count
             FROM thread_summaries_view
             WHERE conversation_id = ?1 AND parent_message_id = ?2",
            params![channel.id, parent_id],
            ThreadSummaryView::from_projection_row,
        )?;
        Self::append_event_tx(
            tx,
            NewEvent {
                event_type: "thread.state",
                scope_kind: "thread",
                scope_id: format!("thread:{parent_id}"),
                channel_id: Some(&channel.id),
                channel_name: Some(&channel.name),
                thread_parent_id: Some(parent_id),
                actor_name: Some(sender_name),
                actor_type: Some(sender_type.as_str()),
                caused_by_kind: Some("send_message"),
                payload: json!({
                    "conversationId": channel.id.as_str(),
                    "conversationType": channel.channel_type.as_api_str(),
                    "threadParentId": parent_id,
                    "messageId": inserted.id.as_str(),
                    "latestSeq": inserted.seq,
                    "replyCount": summary.reply_count,
                    "lastReplyMessageId": summary.last_reply_message_id.as_deref(),
                    "lastReplyAt": summary.last_reply_at.as_deref(),
                }),
            },
        )
    }

    pub(crate) fn append_tombstone_changed_event_tx(
        tx: &Transaction<'_>,
        channel: &Channel,
        thread_parent_id: Option<&str>,
        message_id: &str,
        caused_by_kind: &'static str,
    ) -> Result<i64> {
        let (scope_kind, scope_id) = Self::message_scope_for(channel, thread_parent_id);
        Self::append_event_tx(
            tx,
            NewEvent {
                event_type: "message.tombstone_changed",
                scope_kind,
                scope_id,
                channel_id: Some(&channel.id),
                channel_name: Some(&channel.name),
                thread_parent_id,
                actor_name: None,
                actor_type: None,
                caused_by_kind: Some(caused_by_kind),
                payload: json!({
                    "messageId": message_id,
                    "conversationId": channel.id.as_str(),
                    "threadParentId": thread_parent_id,
                    "senderDeleted": true,
                }),
            },
        )
    }

    /// Insert a message row directly by channel id, optionally attaching
    /// provenance metadata for forwarded copies.
    pub fn post_message_with_forwarded_from(
        &self,
        channel_id: &str,
        sender_name: &str,
        sender_type: SenderType,
        content: &str,
        attachment_ids: &[String],
        forwarded_from: Option<ForwardedFrom>,
    ) -> Result<String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let channel = Self::find_channel_by_id_inner(&tx, channel_id)?
            .ok_or_else(|| anyhow!("channel not found by id"))?;
        let inserted = Self::insert_message_tx(
            &tx,
            &channel,
            None,
            sender_name,
            sender_type,
            content,
            attachment_ids,
            forwarded_from.as_ref(),
        )?;
        let last_event_id = Self::append_message_created_event_tx(
            &tx,
            &channel,
            None,
            sender_name,
            sender_type,
            &inserted,
            "post_message_with_forwarded_from",
        )?;
        tx.commit()?;

        let _ = self
            .msg_tx
            .send((channel_id.to_string(), inserted.id.clone()));
        let _ = self.event_tx.send(last_event_id);
        Ok(inserted.id)
    }

    /// Post a server-authored message into a channel.
    pub fn post_system_message(&self, channel_id: &str, content: &str) -> Result<String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let channel = Self::find_channel_by_id_inner(&tx, channel_id)?
            .ok_or_else(|| anyhow!("channel not found by id"))?;
        let inserted = Self::insert_message_tx(
            &tx,
            &channel,
            None,
            "system",
            SenderType::Human,
            content,
            &[],
            None,
        )?;
        let _ = Self::append_message_created_event_tx(
            &tx,
            &channel,
            None,
            "system",
            SenderType::Human,
            &inserted,
            "post_system_message",
        )?;
        let last_event_id = Self::append_system_notice_event_tx(&tx, &channel, &inserted)?;
        tx.commit()?;

        let _ = self
            .msg_tx
            .send((channel_id.to_string(), inserted.id.clone()));
        let _ = self.event_tx.send(last_event_id);
        Ok(inserted.id)
    }

    pub fn send_message(
        &self,
        channel_name: &str,
        thread_parent_id: Option<&str>,
        sender_name: &str,
        sender_type: SenderType,
        content: &str,
        attachment_ids: &[String],
    ) -> Result<String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let channel = Self::find_channel_by_name_inner(&tx, channel_name)?
            .ok_or_else(|| anyhow!("channel not found: {}", channel_name))?;
        let sender_was_participant_before = match thread_parent_id {
            Some(parent_id) => {
                Self::thread_participant_exists_before(&tx, &channel.id, parent_id, sender_name)?
            }
            None => false,
        };
        let inserted = Self::insert_message_tx(
            &tx,
            &channel,
            thread_parent_id,
            sender_name,
            sender_type,
            content,
            attachment_ids,
            None,
        )?;
        let mut last_event_id = Self::append_conversation_state_event_tx(
            &tx,
            &channel,
            thread_parent_id,
            sender_name,
            sender_type,
            &inserted,
            "send_message",
        )?;
        if let Some(parent_id) = thread_parent_id {
            let _ = Self::append_thread_state_event_tx(
                &tx,
                &channel,
                parent_id,
                sender_name,
                sender_type,
                &inserted,
            )?;
            last_event_id = Self::append_thread_events_tx(
                &tx,
                &channel,
                parent_id,
                sender_name,
                sender_type,
                sender_was_participant_before,
            )?;
        }
        // Treat the sender's own newly-created message as already read in the
        // surface where it was composed so it does not come back later as unread.
        if let Some(parent_id) = thread_parent_id {
            Self::set_thread_read_cursor_tx(
                &tx,
                &channel,
                parent_id,
                sender_name,
                sender_type.as_str(),
                inserted.seq,
                Some(&inserted.id),
                false,
                "send_message",
            )?;
        } else {
            Self::set_inbox_read_cursor_tx(
                &tx,
                &channel,
                sender_name,
                sender_type.as_str(),
                inserted.seq,
                Some(&inserted.id),
                false,
                "send_message",
            )?;
        }
        tx.commit()?;

        let _ = self.msg_tx.send((channel.id.clone(), inserted.id.clone()));
        let _ = self.event_tx.send(last_event_id);
        Ok(inserted.id)
    }
}
