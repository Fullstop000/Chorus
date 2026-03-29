use anyhow::{anyhow, Result};
use rusqlite::{params, Transaction};
use serde_json::json;
use uuid::Uuid;

use crate::store::channels::{Channel, ChannelType};
use crate::store::events::NewEvent;
use crate::store::messages::*;
use crate::store::stream::StreamEvent;
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

    /// Insert a message row directly by channel id, optionally attaching
    /// provenance metadata for forwarded copies.
    pub fn create_message_with_forwarded_from(
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
        let channel = Self::get_channel_by_id_inner(&tx, channel_id)?
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
        let _ = Self::append_message_created_event_tx(
            &tx,
            &channel,
            None,
            sender_name,
            sender_type,
            &inserted,
            "post_message_with_forwarded_from",
        )?;
        tx.commit()?;

        let event_payload = json!({
            "messageId": inserted.id.as_str(),
            "conversationId": channel.id.as_str(),
            "conversationType": channel.channel_type.as_api_str(),
            "threadParentId": null,
        });
        let stream_event = StreamEvent::new(channel.id.clone(), inserted.seq, event_payload);
        let _ = self.stream_tx.send(stream_event);
        Ok(inserted.id)
    }

    /// Post a server-authored message into a channel.
    pub fn create_system_message(&self, channel_id: &str, content: &str) -> Result<String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let channel = Self::get_channel_by_id_inner(&tx, channel_id)?
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
        tx.commit()?;

        let event_payload = json!({
            "messageId": inserted.id.as_str(),
            "conversationId": channel.id.as_str(),
            "conversationType": channel.channel_type.as_api_str(),
            "threadParentId": null,
        });
        let stream_event = StreamEvent::new(channel.id.clone(), inserted.seq, event_payload);
        let _ = self.stream_tx.send(stream_event);
        Ok(inserted.id)
    }

    pub fn create_message(
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
        let channel = Self::get_channel_by_name_inner(&tx, channel_name)?
            .ok_or_else(|| anyhow!("channel not found: {}", channel_name))?;
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
        let _ = Self::append_message_created_event_tx(
            &tx,
            &channel,
            thread_parent_id,
            sender_name,
            sender_type,
            &inserted,
            "send_message",
        )?;
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

        let event_payload = json!({
            "messageId": inserted.id.as_str(),
            "conversationId": channel.id.as_str(),
            "conversationType": channel.channel_type.as_api_str(),
            "threadParentId": thread_parent_id,
        });
        let stream_event = StreamEvent::new(channel.id.clone(), inserted.seq, event_payload);
        let _ = self.stream_tx.send(stream_event);
        Ok(inserted.id)
    }
}
