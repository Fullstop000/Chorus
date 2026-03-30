use anyhow::{anyhow, ensure, Result};
use rusqlite::{params, OptionalExtension};

use crate::store::messages::*;
use crate::store::Store;

impl Store {
    /// Rejects HTTP/client `last_read_seq` values that cannot refer to a real row.
    /// All messages in a channel share one monotonic `seq` space; thread scope uses
    /// `max_seq` over replies with that `thread_parent_id` only.
    fn ensure_read_seq_in_range(last_read_seq: i64, max_seq: i64) -> Result<()> {
        ensure!(
            last_read_seq >= 0,
            "last_read_seq must be non-negative (got {})",
            last_read_seq
        );
        ensure!(
            last_read_seq <= max_seq,
            "last_read_seq {} is greater than latest message seq {}",
            last_read_seq,
            max_seq
        );
        Ok(())
    }

    /// Persists read progress from the browser (`POST .../read-cursor`).
    ///
    /// `last_read_seq` must satisfy `0 <= last_read_seq <= max_seq` where `max_seq` is
    /// the latest `messages.seq` in scope (whole channel, or one thread’s replies).
    /// It is merged with the stored cursor so we never move backward on valid data, but
    /// if the stored value is above `max_seq` (orphan after data changes), we replace it
    /// with this request’s `last_read_seq` only.
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
            // Latest reply seq in this thread (0 if there are no replies yet).
            let max_seq: i64 = tx.query_row(
                "SELECT COALESCE(MAX(seq), 0)
                 FROM messages
                 WHERE channel_id = ?1 AND thread_parent_id = ?2",
                params![channel.id, parent_id],
                |row| row.get(0),
            )?;
            Self::ensure_read_seq_in_range(last_read_seq, max_seq)?;
            let current_last_read: i64 = tx
                .query_row(
                    "SELECT last_read_seq
                     FROM inbox_thread_read_state
                     WHERE conversation_id = ?1 AND thread_parent_id = ?2 AND member_name = ?3",
                    params![channel.id, parent_id, member_name],
                    |row| row.get(0),
                )
                .optional()?
                .unwrap_or(0);
            // Normal path: monotonic advance, capped by max_seq. Orphan path: stored
            // last_read_seq is past any existing reply; accept client seq as the new truth.
            let final_read = if current_last_read > max_seq {
                last_read_seq
            } else {
                last_read_seq.max(current_last_read).min(max_seq)
            };

            // Message id at `final_read` in this thread, if that seq exists.
            let last_read_message_id = tx
                .query_row(
                    "SELECT id
                     FROM messages
                     WHERE channel_id = ?1 AND thread_parent_id = ?2 AND seq = ?3
                     LIMIT 1",
                    params![channel.id, parent_id, final_read],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            Self::set_thread_read_cursor_tx(
                &tx,
                &channel,
                parent_id,
                member_name,
                member_type.as_str(),
                final_read,
                last_read_message_id.as_deref(),
            )?;
        } else {
            // Whole-channel seq space (root messages and thread replies share one counter).
            let max_seq: i64 = tx.query_row(
                "SELECT COALESCE(MAX(seq), 0) FROM messages WHERE channel_id = ?1",
                params![channel.id],
                |row| row.get(0),
            )?;
            Self::ensure_read_seq_in_range(last_read_seq, max_seq)?;
            let current_last_read: i64 = tx
                .query_row(
                    "SELECT last_read_seq
                     FROM inbox_read_state
                     WHERE conversation_id = ?1 AND member_name = ?2",
                    params![channel.id, member_name],
                    |row| row.get(0),
                )
                .optional()?
                .unwrap_or(0);
            let final_read = if current_last_read > max_seq {
                last_read_seq
            } else {
                last_read_seq.max(current_last_read).min(max_seq)
            };

            // Only top-level rows get a message id here; final_read may be a thread reply seq.
            let last_read_message_id = tx
                .query_row(
                    "SELECT id
                     FROM messages
                     WHERE channel_id = ?1 AND thread_parent_id IS NULL AND seq = ?2
                     LIMIT 1",
                    params![channel.id, final_read],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            Self::set_inbox_read_cursor_tx(
                &tx,
                &channel,
                member_name,
                member_type.as_str(),
                final_read,
                last_read_message_id.as_deref(),
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}
