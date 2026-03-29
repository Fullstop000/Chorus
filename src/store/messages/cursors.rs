use anyhow::{anyhow, Result};
use rusqlite::{params, OptionalExtension};

use crate::store::messages::*;
use crate::store::Store;

impl Store {
    pub fn set_history_read_cursor(
        &self,
        channel_name: &str,
        member_name: &str,
        member_type: SenderType,
        thread_parent_id: Option<&str>,
        last_read_seq: i64,
    ) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let channel = Self::get_channel_by_name_inner(&tx, channel_name)?
            .ok_or_else(|| anyhow!("channel not found: {}", channel_name))?;

        if let Some(parent_id) = thread_parent_id {
            let last_read_message_id = tx
                .query_row(
                    "SELECT id
                     FROM messages
                     WHERE channel_id = ?1 AND thread_parent_id = ?2 AND seq = ?3
                     LIMIT 1",
                    params![channel.id, parent_id, last_read_seq],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            Self::set_thread_read_cursor_tx(
                &tx,
                &channel,
                parent_id,
                member_name,
                member_type.as_str(),
                last_read_seq,
                last_read_message_id.as_deref(),
                false,
                "set_history_read_cursor",
            )?;
        } else {
            let last_read_message_id = tx
                .query_row(
                    "SELECT id
                     FROM messages
                     WHERE channel_id = ?1 AND thread_parent_id IS NULL AND seq = ?2
                     LIMIT 1",
                    params![channel.id, last_read_seq],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            Self::set_inbox_read_cursor_tx(
                &tx,
                &channel,
                member_name,
                member_type.as_str(),
                last_read_seq,
                last_read_message_id.as_deref(),
                false,
                "set_history_read_cursor",
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}
