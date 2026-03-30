//! Agent **receive** path: unread rows from SQLite, shaping into [`ReceivedMessage`], optional read-cursor updates.

use std::collections::BTreeMap;

use anyhow::{anyhow, Result};
use rusqlite::{params, Connection};
use tracing::warn;

use crate::store::channels::{Channel, ChannelType};
use crate::store::messages::*;
use crate::store::Store;

// ── Row / pipeline state (private to this module) ─────────────────────────

/// `messages` rows strictly after the conversation read cursor, `seq` ascending.
struct AgentUnreadMessageRow {
    message_id: String,
    sender_name: String,
    sender_type: String,
    content: String,
    created_at: String,
    seq: i64,
    thread_parent_id: Option<String>,
    forwarded_from_raw: Option<String>,
}

struct AgentChannelUnreadScan {
    received: Vec<ReceivedMessage>,
    max_conversation_seq: i64,
    last_conversation_message_id: Option<String>,
    thread_read_updates: BTreeMap<String, (i64, String)>,
}

/// Per-channel snapshot before scanning: channel row, thread cursors, unread rows, DM display peer.
struct AgentInboxChannelContext {
    channel: Channel,
    thread_last_read: BTreeMap<String, i64>,
    unread_rows: Vec<AgentUnreadMessageRow>,
    dm_peer_name: Option<String>,
}

fn channel_type_wire_label(kind: ChannelType) -> &'static str {
    match kind {
        ChannelType::Channel | ChannelType::System | ChannelType::Team => "channel",
        ChannelType::Dm => "dm",
    }
}

// ── `Store` ────────────────────────────────────────────────────────────────

impl Store {
    pub fn get_messages_for_agent(
        &self,
        agent_name: &str,
        update_read_pos: bool,
    ) -> Result<Vec<ReceivedMessage>> {
        let mut conn = self.conn.lock().unwrap();
        let memberships = Self::load_agent_channel_memberships(&conn, agent_name)?;

        let mut out = Vec::new();
        for (channel_id, member_type, last_read_seq) in &memberships {
            let ctx =
                Self::load_agent_inbox_channel_context(&conn, channel_id, agent_name, *last_read_seq)?;
            let scan = Self::scan_agent_inbox_channel(&conn, &ctx, agent_name, *last_read_seq)?;
            Self::persist_agent_inbox_read_cursors(
                &mut conn,
                &ctx.channel,
                agent_name,
                member_type,
                *last_read_seq,
                update_read_pos,
                &scan,
            )?;
            out.extend(scan.received);
        }
        Ok(out)
    }

    /// Same shaping as the normal receive path (`get_messages_for_agent`), for wake-up prompts.
    pub fn get_received_message_for_agent(
        &self,
        agent_name: &str,
        message_id: &str,
    ) -> Result<Option<ReceivedMessage>> {
        let unread = self.get_messages_for_agent(agent_name, false)?;
        Ok(unread
            .into_iter()
            .find(|message| message.message_id == message_id))
    }

    fn load_agent_channel_memberships(
        conn: &Connection,
        agent_name: &str,
    ) -> Result<Vec<(String, String, i64)>> {
        let mut list = Vec::new();
        let mut stmt = conn.prepare(
            "SELECT cm.channel_id,
                    cm.member_type,
                    COALESCE(irs.last_read_seq, 0)
             FROM channel_members cm
             LEFT JOIN inbox_read_state irs
               ON irs.conversation_id = cm.channel_id
              AND irs.member_name = cm.member_name
             WHERE cm.member_name = ?1",
        )?;
        for row in stmt.query_map(params![agent_name], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })? {
            match row {
                Ok(entry) => list.push(entry),
                Err(e) => warn!(
                    agent = %agent_name,
                    error = %e,
                    "get_messages_for_agent: skip bad membership row"
                ),
            }
        }
        Ok(list)
    }

    fn load_inbox_thread_read_state(
        conn: &Connection,
        channel_id: &str,
        agent_name: &str,
    ) -> Result<BTreeMap<String, i64>> {
        let mut map = BTreeMap::new();
        let mut stmt = conn.prepare(
            "SELECT thread_parent_id, last_read_seq
             FROM inbox_thread_read_state
             WHERE conversation_id = ?1 AND member_name = ?2",
        )?;
        for row in stmt.query_map(params![channel_id, agent_name], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })? {
            match row {
                Ok((parent_id, seq)) => {
                    map.insert(parent_id, seq);
                }
                Err(e) => warn!(
                    agent = %agent_name,
                    channel_id = %channel_id,
                    error = %e,
                    "get_messages_for_agent: skip bad thread read state row"
                ),
            }
        }
        Ok(map)
    }

    fn load_agent_unread_message_rows(
        conn: &Connection,
        channel_id: &str,
        after_seq: i64,
        agent_name: &str,
    ) -> Result<Vec<AgentUnreadMessageRow>> {
        let mut rows = Vec::new();
        let mut stmt = conn.prepare(
            "SELECT m.id, m.sender_name, m.sender_type, m.content, m.created_at, m.seq, m.thread_parent_id, m.forwarded_from
             FROM messages m
             WHERE m.channel_id = ?1
               AND m.seq > ?2
               AND NOT (m.sender_name = ?3 AND m.sender_type = 'agent')
             ORDER BY m.seq ASC",
        )?;
        for row in stmt.query_map(params![channel_id, after_seq, agent_name], |row| {
            Ok(AgentUnreadMessageRow {
                message_id: row.get(0)?,
                sender_name: row.get(1)?,
                sender_type: row.get(2)?,
                content: row.get(3)?,
                created_at: row.get(4)?,
                seq: row.get(5)?,
                thread_parent_id: row.get(6)?,
                forwarded_from_raw: row.get(7)?,
            })
        })? {
            match row {
                Ok(entry) => rows.push(entry),
                Err(e) => warn!(
                    agent = %agent_name,
                    channel_id = %channel_id,
                    error = %e,
                    "get_messages_for_agent: skip bad message row"
                ),
            }
        }
        Ok(rows)
    }

    /// DM: peer member name so the agent sees a stable handle instead of the synthetic DM channel slug.
    fn resolve_dm_peer_display_name(
        conn: &Connection,
        channel_id: &str,
        agent_name: &str,
        channel_type: ChannelType,
    ) -> Result<Option<String>> {
        if channel_type != ChannelType::Dm {
            return Ok(None);
        }
        let peer: Option<String> = conn
            .prepare(
                "SELECT member_name FROM channel_members WHERE channel_id = ?1 AND member_name != ?2 LIMIT 1",
            )?
            .query_row(params![channel_id, agent_name], |row| row.get(0))
            .ok();
        Ok(peer)
    }

    fn load_agent_inbox_channel_context(
        conn: &Connection,
        channel_id: &str,
        agent_name: &str,
        last_read_seq: i64,
    ) -> Result<AgentInboxChannelContext> {
        let channel = Self::get_channel_by_id_inner(conn, channel_id)?
            .ok_or_else(|| anyhow!("channel not found by id"))?;
        Ok(AgentInboxChannelContext {
            thread_last_read: Self::load_inbox_thread_read_state(conn, channel_id, agent_name)?,
            unread_rows: Self::load_agent_unread_message_rows(
                conn,
                channel_id,
                last_read_seq,
                agent_name,
            )?,
            dm_peer_name: Self::resolve_dm_peer_display_name(
                conn,
                channel_id,
                agent_name,
                channel.channel_type,
            )?,
            channel,
        })
    }

    fn shape_agent_received_message(
        conn: &Connection,
        channel: &Channel,
        dm_peer_name: &Option<String>,
        row: &AgentUnreadMessageRow,
    ) -> Result<ReceivedMessage> {
        let attachments = Self::get_message_attachments(conn, &row.message_id)?;
        let attachments = (!attachments.is_empty()).then_some(attachments);

        let effective_channel_name = match dm_peer_name {
            Some(peer) => peer.clone(),
            None => channel.name.clone(),
        };

        let parent_label = channel_type_wire_label(channel.channel_type);
        let (channel_name, channel_type, parent_channel_name, parent_channel_type) =
            if let Some(parent_id) = &row.thread_parent_id {
                let short = parent_id.get(..8).unwrap_or(parent_id.as_str());
                (
                    format!("thread-{}", short),
                    "thread".to_string(),
                    Some(effective_channel_name),
                    Some(parent_label.to_string()),
                )
            } else {
                (
                    effective_channel_name,
                    parent_label.to_string(),
                    None,
                    None,
                )
            };

        Ok(ReceivedMessage {
            message_id: row.message_id.clone(),
            channel_name,
            channel_type,
            parent_channel_name,
            parent_channel_type,
            sender_name: row.sender_name.clone(),
            sender_type: row.sender_type.clone(),
            content: row.content.clone(),
            timestamp: row.created_at.clone(),
            attachments,
            forwarded_from: Self::parse_forwarded_from_raw(row.forwarded_from_raw.as_deref()),
        })
    }

    fn scan_agent_inbox_channel(
        conn: &Connection,
        ctx: &AgentInboxChannelContext,
        agent_name: &str,
        last_read_seq: i64,
    ) -> Result<AgentChannelUnreadScan> {
        let channel_id = ctx.channel.id.as_str();
        let mut max_conversation_seq = last_read_seq;
        let mut last_conversation_message_id = None::<String>;
        let mut thread_read_updates = BTreeMap::<String, (i64, String)>::new();
        let mut received = Vec::new();

        for row in &ctx.unread_rows {
            if let Some(parent_id) = &row.thread_parent_id {
                if row.seq <= ctx.thread_last_read.get(parent_id).copied().unwrap_or(0) {
                    continue;
                }
                if !Self::agent_can_access_thread_inner(conn, channel_id, parent_id, agent_name)? {
                    continue;
                }
                let entry = thread_read_updates
                    .entry(parent_id.clone())
                    .or_insert((row.seq, row.message_id.clone()));
                if row.seq > entry.0 {
                    *entry = (row.seq, row.message_id.clone());
                }
            } else if row.seq > max_conversation_seq {
                max_conversation_seq = row.seq;
                last_conversation_message_id = Some(row.message_id.clone());
            }

            received.push(Self::shape_agent_received_message(
                conn,
                &ctx.channel,
                &ctx.dm_peer_name,
                row,
            )?);
        }

        Ok(AgentChannelUnreadScan {
            received,
            max_conversation_seq,
            last_conversation_message_id,
            thread_read_updates,
        })
    }

    fn persist_agent_inbox_read_cursors(
        conn: &mut Connection,
        channel: &Channel,
        agent_name: &str,
        member_type: &str,
        last_read_seq: i64,
        update_read_pos: bool,
        scan: &AgentChannelUnreadScan,
    ) -> Result<()> {
        if !update_read_pos {
            return Ok(());
        }
        let inbox_advanced = scan.max_conversation_seq > last_read_seq;
        if !inbox_advanced && scan.thread_read_updates.is_empty() {
            return Ok(());
        }

        let tx = conn.transaction()?;
        if inbox_advanced {
            Self::set_inbox_read_cursor_tx(
                &tx,
                channel,
                agent_name,
                member_type,
                scan.max_conversation_seq,
                scan.last_conversation_message_id.as_deref(),
            )?;
        }
        for (parent_id, (seq, message_id)) in &scan.thread_read_updates {
            Self::set_thread_read_cursor_tx(
                &tx,
                channel,
                parent_id,
                agent_name,
                member_type,
                *seq,
                Some(message_id.as_str()),
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}
