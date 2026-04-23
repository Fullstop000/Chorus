use anyhow::{anyhow, Result};
use rusqlite::{params, Transaction};
use uuid::Uuid;

use crate::store::channels::Channel;
use crate::store::messages::*;
use crate::store::stream::StreamEvent;
use crate::store::Store;

pub struct CreateMessage<'a> {
    pub channel_name: &'a str,
    pub sender_name: &'a str,
    pub sender_type: SenderType,
    pub content: &'a str,
    pub attachment_ids: &'a [String],
    pub suppress_event: bool,
    pub run_id: Option<&'a str>,
}

impl Store {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn insert_message_tx(
        tx: &Transaction<'_>,
        channel: &Channel,
        sender_name: &str,
        sender_type: SenderType,
        content: &str,
        attachment_ids: &[String],
        forwarded_from: Option<&ForwardedFrom>,
        run_id: Option<&str>,
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
                id, channel_id, sender_name, sender_type, sender_deleted, content, seq, forwarded_from, run_id
             ) VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6, ?7, ?8)",
            params![
                msg_id,
                channel.id,
                sender_name,
                sender_type.as_str(),
                content,
                seq,
                forwarded_from_json,
                run_id
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
            sender_name,
            sender_type,
            content,
            attachment_ids,
            forwarded_from.as_ref(),
            None,
        )?;
        tx.commit()?;

        let payload = inserted.to_event_payload(
            channel.id.as_str(),
            channel.channel_type.as_api_str(),
            sender_name,
            sender_type.as_str(),
            content,
        );
        let stream_event = StreamEvent::new(
            channel.id.clone(),
            inserted.seq,
            serde_json::to_value(payload)?,
        );
        let _ = self.stream_tx.send(stream_event);
        Ok(inserted.id)
    }

    /// Insert a `sender_type = 'system'` message inside an existing transaction.
    /// Callers doing multi-statement mutations (e.g. task claim → status flip
    /// → event message) wrap the whole sequence in their own transaction and
    /// call this helper. Use the public [`Store::create_system_message`] for
    /// standalone posts.
    ///
    /// Returns the [`InsertedMessage`] so callers can emit the stream event
    /// after the outer transaction commits.
    pub(crate) fn create_system_message_tx(
        tx: &Transaction<'_>,
        channel_id: &str,
        content: &str,
    ) -> Result<InsertedMessage> {
        let channel = Self::get_channel_by_id_inner(tx, channel_id)?
            .ok_or_else(|| anyhow!("channel not found by id"))?;
        Self::insert_message_tx(
            tx,
            &channel,
            "system",
            SenderType::System,
            content,
            &[],
            None,
            None,
        )
    }

    /// Emit `message.created` stream events for system messages that were
    /// inserted via `create_system_message_tx` and committed by the caller.
    /// Each `(inserted, content)` tuple produced inside the transaction maps
    /// to one WebSocket event. Best-effort: send errors are dropped because
    /// the DB rows are the source of truth.
    pub(crate) fn emit_system_stream_events(
        &self,
        channel: &Channel,
        pending: Vec<(InsertedMessage, String)>,
    ) -> Result<()> {
        for (inserted, content) in pending {
            let payload = inserted.to_event_payload(
                channel.id.as_str(),
                channel.channel_type.as_api_str(),
                "system",
                SenderType::System.as_str(),
                &content,
            );
            let stream_event = StreamEvent::new(
                channel.id.clone(),
                inserted.seq,
                serde_json::to_value(payload)?,
            );
            let _ = self.stream_tx.send(stream_event);
        }
        Ok(())
    }

    /// Post a server-authored message into a channel.
    pub fn create_system_message(&self, channel_id: &str, content: &str) -> Result<String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let channel = Self::get_channel_by_id_inner(&tx, channel_id)?
            .ok_or_else(|| anyhow!("channel not found by id"))?;
        let inserted = Self::create_system_message_tx(&tx, channel_id, content)?;
        let message_id = inserted.id.clone();
        tx.commit()?;
        drop(conn); // release the guard before fanout to avoid holding the mutex

        self.emit_system_stream_events(&channel, vec![(inserted, content.to_string())])?;
        Ok(message_id)
    }

    pub fn create_message(&self, message: CreateMessage<'_>) -> Result<String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let channel = Self::get_channel_by_name_inner(&tx, message.channel_name)?
            .ok_or_else(|| anyhow!("channel not found: {}", message.channel_name))?;
        let inserted = Self::insert_message_tx(
            &tx,
            &channel,
            message.sender_name,
            message.sender_type,
            message.content,
            message.attachment_ids,
            None,
            message.run_id,
        )?;
        Self::set_inbox_read_cursor_tx(
            &tx,
            &channel,
            message.sender_name,
            message.sender_type.as_str(),
            inserted.seq,
            Some(&inserted.id),
        )?;
        tx.commit()?;

        let payload = inserted.to_event_payload(
            channel.id.as_str(),
            channel.channel_type.as_api_str(),
            message.sender_name,
            message.sender_type.as_str(),
            message.content,
        );
        let stream_event = StreamEvent::new(
            channel.id.clone(),
            inserted.seq,
            serde_json::to_value(payload)?,
        );
        if !message.suppress_event {
            let _ = self.stream_tx.send(stream_event);
        }
        Ok(inserted.id)
    }
}

#[cfg(test)]
mod tests {
    use crate::store::channels::ChannelType;
    use crate::store::Store;
    use rusqlite::params;

    fn make_store() -> Store {
        // `Store::open` takes `&str`; in-memory matches what `tests/e2e_tests.rs`
        // does for the test harness and avoids any path-to-string juggling.
        Store::open(":memory:").unwrap()
    }

    #[test]
    fn create_system_message_tx_inserts_within_caller_transaction() {
        let store = make_store();
        let channel_id = store
            .create_channel("eng", None, ChannelType::Channel, None)
            .unwrap();

        let msg_id = {
            let mut conn = store.conn_for_test();
            let tx = conn.transaction().unwrap();
            let inserted = Store::create_system_message_tx(&tx, &channel_id, "hello").unwrap();
            tx.commit().unwrap();
            inserted.id
        };

        // Verify ALL invariants the helper owns, not just content.
        let (content, sender_type, sender_name, seq): (String, String, String, i64) = store
            .conn_for_test()
            .query_row(
                "SELECT content, sender_type, sender_name, seq FROM messages WHERE id = ?1",
                params![msg_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(content, "hello");
        assert_eq!(sender_type, "system");
        assert_eq!(sender_name, "system");
        assert_eq!(seq, 1);
    }

    #[test]
    fn create_system_message_tx_respects_caller_rollback() {
        // The whole point of a tx-scoped API: if the caller rolls back, the
        // insert must not persist. Pins the "no inner transaction" invariant.
        use rusqlite::OptionalExtension;

        let store = make_store();
        let channel_id = store
            .create_channel("eng", None, ChannelType::Channel, None)
            .unwrap();

        let msg_id = {
            let mut conn = store.conn_for_test();
            let tx = conn.transaction().unwrap();
            let inserted = Store::create_system_message_tx(&tx, &channel_id, "discard me").unwrap();
            // Drop the tx without committing — implicit rollback.
            drop(tx);
            inserted.id
        };

        let row: Option<String> = store
            .conn_for_test()
            .query_row("SELECT id FROM messages WHERE id = ?1", params![msg_id], |r| r.get(0))
            .optional()
            .unwrap();
        assert!(
            row.is_none(),
            "tx-scoped insert must not persist when caller rolls back"
        );
    }
}
