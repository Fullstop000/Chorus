use anyhow::{anyhow, Result};
use rusqlite::{params, Connection};

use crate::store::messages::*;
use crate::store::Store;

impl Store {
    /// Load the thread inbox for one member scoped to a single conversation.
    pub fn get_channel_thread_inbox(
        &self,
        channel_name: &str,
        member_name: &str,
    ) -> Result<ChannelThreadInbox> {
        let conn = self.conn.lock().unwrap();
        let channel = Self::get_channel_by_name_inner(&conn, channel_name)?
            .ok_or_else(|| anyhow!("channel not found: {}", channel_name))?;
        let mut threads = Vec::new();

        for summary in Self::list_thread_summary_views_by_channel_id_inner(&conn, &channel.id)? {
            let Some(parent_message) =
                Self::get_conversation_message_view_inner(&conn, &summary.parent_message_id)?
            else {
                continue;
            };
            let Some(thread_state) = Self::get_thread_notification_state_by_channel_id_inner(
                &conn,
                &channel.id,
                &summary.parent_message_id,
                member_name,
            )?
            else {
                continue;
            };

            threads.push(ChannelThreadInboxEntry {
                conversation_id: channel.id.clone(),
                thread_parent_id: summary.parent_message_id.clone(),
                parent_seq: parent_message.seq,
                parent_sender_name: parent_message.sender_name,
                parent_sender_type: parent_message.sender_type,
                parent_content: parent_message.content,
                parent_created_at: parent_message.created_at,
                reply_count: summary.reply_count,
                participant_count: summary.participant_count,
                latest_seq: thread_state.latest_seq,
                last_read_seq: thread_state.last_read_seq,
                unread_count: thread_state.unread_count,
                last_reply_message_id: thread_state.last_reply_message_id,
                last_reply_at: thread_state.last_reply_at,
            });
        }

        threads.sort_by(|left, right| {
            right
                .latest_seq
                .cmp(&left.latest_seq)
                .then_with(|| right.parent_seq.cmp(&left.parent_seq))
        });

        let unread_count = threads.iter().map(|thread| thread.unread_count).sum();
        Ok(ChannelThreadInbox {
            unread_count,
            threads,
        })
    }

    /// Load one projected thread summary row for a top-level parent message.
    pub fn get_thread_summary_view(
        &self,
        parent_message_id: &str,
    ) -> Result<Option<ThreadSummaryView>> {
        let conn = self.conn.lock().unwrap();
        Self::get_thread_summary_view_inner(&conn, parent_message_id)
    }

    fn get_thread_summary_view_inner(
        conn: &Connection,
        parent_message_id: &str,
    ) -> Result<Option<ThreadSummaryView>> {
        Ok(conn
            .query_row(
                "SELECT conversation_id, parent_message_id, reply_count,
                        last_reply_message_id, last_reply_at, participant_count
                 FROM thread_summaries_view
                 WHERE parent_message_id = ?1",
                params![parent_message_id],
                ThreadSummaryView::from_projection_row,
            )
            .ok())
    }

    fn list_thread_summary_views_by_channel_id_inner(
        conn: &Connection,
        conversation_id: &str,
    ) -> Result<Vec<ThreadSummaryView>> {
        let mut stmt = conn.prepare(
            "SELECT conversation_id, parent_message_id, reply_count,
                    last_reply_message_id, last_reply_at, participant_count
             FROM thread_summaries_view
             WHERE conversation_id = ?1",
        )?;
        let rows = stmt.query_map(
            params![conversation_id],
            ThreadSummaryView::from_projection_row,
        )?;
        Ok(rows.filter_map(|row| row.ok()).collect())
    }

    // pub(crate) fn thread_participant_exists_before(
    //     conn: &Connection,
    //     channel_id: &str,
    //     parent_id: &str,
    //     member_name: &str,
    // ) -> Result<bool> {
    //     let parent_author_matches: i64 = conn.query_row(
    //         "SELECT COUNT(*) FROM messages
    //          WHERE id = ?1 AND channel_id = ?2 AND sender_name = ?3",
    //         params![parent_id, channel_id, member_name],
    //         |row| row.get(0),
    //     )?;
    //     if parent_author_matches > 0 {
    //         return Ok(true);
    //     }

    //     let prior_reply_matches: i64 = conn.query_row(
    //         "SELECT COUNT(*) FROM messages
    //          WHERE channel_id = ?1 AND thread_parent_id = ?2 AND sender_name = ?3",
    //         params![channel_id, parent_id, member_name],
    //         |row| row.get(0),
    //     )?;
    //     Ok(prior_reply_matches > 0)
    // }
}
