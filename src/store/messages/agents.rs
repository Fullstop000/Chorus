use std::collections::BTreeSet;

use anyhow::{anyhow, Result};
use rusqlite::{params, Connection};

use crate::store::channels::{Channel, ChannelType};
use crate::store::messages::*;
use crate::store::Store;

impl Store {
    pub fn get_messages_for_agent(
        &self,
        agent_name: &str,
        update_read_pos: bool,
    ) -> Result<Vec<ReceivedMessage>> {
        let mut conn = self.conn.lock().unwrap();
        let memberships: Vec<(String, String, i64)> = conn
            .prepare(
                "SELECT cm.channel_id,
                        cm.member_type,
                        COALESCE(irs.last_read_seq, 0)
                 FROM channel_members cm
                 LEFT JOIN inbox_read_state irs
                   ON irs.conversation_id = cm.channel_id
                  AND irs.member_name = cm.member_name
                 WHERE cm.member_name = ?1",
            )?
            .query_map(params![agent_name], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?
            .filter_map(|r| r.ok())
            .collect();

        let mut result = Vec::new();
        let mut last_event_id = None;

        for (channel_id, member_type, last_read_seq) in &memberships {
            let channel = Self::get_channel_by_id_inner(&conn, channel_id)?
                .ok_or_else(|| anyhow!("channel not found by id"))?;
            let thread_last_read: std::collections::BTreeMap<String, i64> = conn
                .prepare(
                    "SELECT thread_parent_id, last_read_seq
                     FROM inbox_thread_read_state
                     WHERE conversation_id = ?1 AND member_name = ?2",
                )?
                .query_map(params![channel_id, agent_name], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                })?
                .filter_map(|row| row.ok())
                .collect();

            #[allow(clippy::type_complexity)]
            let msgs: Vec<(String, String, String, String, String, i64, Option<String>, Option<String>)> = conn
                .prepare(
                    "SELECT m.id, m.sender_name, m.sender_type, m.content, m.created_at, m.seq, m.thread_parent_id, m.forwarded_from
                     FROM messages m WHERE m.channel_id = ?1 AND m.seq > ?2 ORDER BY m.seq ASC",
                )?
                .query_map(params![channel_id, last_read_seq], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?, row.get(7)?))
                })?
                .filter_map(|r| r.ok())
                .collect();

            // For DM channels, resolve the peer's name so agents see "dm:@peer" not "dm:@dm-a-b"
            let dm_peer_name: Option<String> = if channel.channel_type == ChannelType::Dm {
                conn.prepare(
                    "SELECT member_name FROM channel_members WHERE channel_id = ?1 AND member_name != ?2 LIMIT 1",
                )?
                .query_row(params![channel_id, agent_name], |row| row.get(0))
                .ok()
            } else {
                None
            };

            let mut max_conversation_seq = *last_read_seq;
            let mut last_conversation_message_id = None::<String>;
            let mut thread_read_updates =
                std::collections::BTreeMap::<String, (i64, String)>::new();
            for (
                msg_id,
                sender_name,
                sender_type,
                content,
                created_at,
                seq,
                thread_parent_id,
                forwarded_from_raw,
            ) in &msgs
            {
                if let Some(parent_id) = thread_parent_id {
                    if *seq <= thread_last_read.get(parent_id).copied().unwrap_or(0) {
                        continue;
                    }
                    if !Self::agent_can_access_thread_inner(
                        &conn, channel_id, parent_id, agent_name,
                    )? {
                        continue;
                    }
                    let entry = thread_read_updates
                        .entry(parent_id.clone())
                        .or_insert((*seq, msg_id.clone()));
                    if *seq > entry.0 {
                        *entry = (*seq, msg_id.clone());
                    }
                } else if *seq > max_conversation_seq {
                    max_conversation_seq = *seq;
                    last_conversation_message_id = Some(msg_id.clone());
                }

                let attachments = Self::get_message_attachments(&conn, msg_id)?;
                let atts = if attachments.is_empty() {
                    None
                } else {
                    Some(attachments)
                };

                let effective_channel_name = match &dm_peer_name {
                    Some(peer) => peer.clone(),
                    None => channel.name.clone(),
                };

                let (msg_channel_name, msg_channel_type, parent_channel_name, parent_channel_type) =
                    if let Some(parent_id) = thread_parent_id {
                        let short = if parent_id.len() >= 8 {
                            &parent_id[..8]
                        } else {
                            parent_id.as_str()
                        };
                        let parent_type = match channel.channel_type {
                            ChannelType::Channel | ChannelType::System | ChannelType::Team => {
                                "channel"
                            }
                            ChannelType::Dm => "dm",
                        };
                        (
                            format!("thread-{}", short),
                            "thread".to_string(),
                            Some(effective_channel_name),
                            Some(parent_type.to_string()),
                        )
                    } else {
                        (
                            effective_channel_name,
                            match channel.channel_type {
                                ChannelType::Channel | ChannelType::System | ChannelType::Team => {
                                    "channel".to_string()
                                }
                                ChannelType::Dm => "dm".to_string(),
                            },
                            None,
                            None,
                        )
                    };

                let forwarded_from = Self::parse_forwarded_from_raw(forwarded_from_raw.as_deref());

                result.push(ReceivedMessage {
                    message_id: msg_id.clone(),
                    channel_name: msg_channel_name,
                    channel_type: msg_channel_type,
                    parent_channel_name,
                    parent_channel_type,
                    sender_name: sender_name.clone(),
                    sender_type: sender_type.clone(),
                    content: content.clone(),
                    timestamp: created_at.clone(),
                    attachments: atts,
                    forwarded_from,
                });
            }

            if update_read_pos && max_conversation_seq > *last_read_seq {
                let tx = conn.transaction()?;
                let conversation_event_id = Self::set_inbox_read_cursor_tx(
                    &tx,
                    &channel,
                    agent_name,
                    member_type,
                    max_conversation_seq,
                    last_conversation_message_id.as_deref(),
                    true,
                    "get_messages_for_agent",
                )?;
                let mut newest_event_id = conversation_event_id;
                for (parent_id, (seq, message_id)) in &thread_read_updates {
                    if let Some(event_id) = Self::set_thread_read_cursor_tx(
                        &tx,
                        &channel,
                        parent_id,
                        agent_name,
                        member_type,
                        *seq,
                        Some(message_id.as_str()),
                        true,
                        "get_messages_for_agent",
                    )? {
                        newest_event_id = Some(event_id);
                    }
                }
                tx.commit()?;
                if let Some(event_id) = newest_event_id {
                    last_event_id = Some(event_id);
                }
            } else if update_read_pos && !thread_read_updates.is_empty() {
                let tx = conn.transaction()?;
                let mut newest_event_id = None;
                for (parent_id, (seq, message_id)) in &thread_read_updates {
                    if let Some(event_id) = Self::set_thread_read_cursor_tx(
                        &tx,
                        &channel,
                        parent_id,
                        agent_name,
                        member_type,
                        *seq,
                        Some(message_id.as_str()),
                        true,
                        "get_messages_for_agent",
                    )? {
                        newest_event_id = Some(event_id);
                    }
                }
                tx.commit()?;
                if let Some(event_id) = newest_event_id {
                    last_event_id = Some(event_id);
                }
            }
        }

        if let Some(last_event_id) = last_event_id {
            let _ = self.event_tx.send(last_event_id);
        }

        Ok(result)
    }

    /// Resolve a specific unread message using the same shaping logic the normal
    /// receive path uses, so wake-up prompts match what the agent will later see
    /// from `check_messages()` or `wait_for_message()`.
    pub fn get_received_message_for_agent(
        &self,
        agent_name: &str,
        message_id: &str,
    ) -> Result<Option<ReceivedMessage>> {
        let unread_messages = self.get_messages_for_agent(agent_name, false)?;
        Ok(unread_messages
            .into_iter()
            .find(|message| message.message_id == message_id))
    }

    /// Resolve which agent recipients should receive delivery for a specific
    /// message. Top-level channel and DM messages still fan out to every agent
    /// member except the sender. Thread messages are scoped to the parent agent
    /// author plus agents that have already replied in that same thread.
    pub fn get_agent_message_recipients(
        &self,
        channel_id: &str,
        message_id: &str,
        sender_name: &str,
    ) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let thread_parent_id: Option<String> = conn.query_row(
            "SELECT thread_parent_id FROM messages WHERE id = ?1 AND channel_id = ?2",
            params![message_id, channel_id],
            |row| row.get(0),
        )?;

        if let Some(parent_id) = thread_parent_id {
            return Self::get_thread_agent_recipients_inner(
                &conn,
                channel_id,
                &parent_id,
                sender_name,
            );
        }

        let recipients = Self::get_channel_agent_members_inner(&conn, channel_id)?
            .into_iter()
            .filter(|member_name| member_name != sender_name)
            .collect();
        Ok(recipients)
    }

    pub fn mark_agent_messages_deleted(&self, name: &str) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let impacted_messages: Vec<(String, Channel, Option<String>)> = tx
            .prepare(
                "SELECT m.id, c.id, c.name, c.description, c.channel_type, c.created_at, m.thread_parent_id
                 FROM messages m
                 JOIN channels c ON c.id = m.channel_id
                 WHERE m.sender_type = 'agent' AND m.sender_name = ?1 AND m.sender_deleted = 0",
            )?
            .query_map(params![name], |row| {
                Ok((
                    row.get(0)?,
                    Channel {
                        id: row.get(1)?,
                        name: row.get(2)?,
                        description: row.get(3)?,
                        channel_type: match row.get::<_, String>(4)?.as_str() {
                            "team" => ChannelType::Team,
                            "dm" => ChannelType::Dm,
                            "system" => ChannelType::System,
                            _ => ChannelType::Channel,
                        },
                        created_at: crate::utils::parse_datetime(&row.get::<_, String>(5)?),
                    },
                    row.get(6)?,
                ))
            })?
            .filter_map(|row| row.ok())
            .collect();
        tx.execute(
            "UPDATE messages SET sender_deleted = 1
             WHERE sender_type = 'agent' AND sender_name = ?1 AND sender_deleted = 0",
            params![name],
        )?;
        let mut last_event_id = None;
        for (message_id, channel, thread_parent_id) in impacted_messages {
            last_event_id = Some(Self::append_tombstone_changed_event_tx(
                &tx,
                &channel,
                thread_parent_id.as_deref(),
                &message_id,
                "mark_agent_messages_deleted",
            )?);
        }
        tx.commit()?;
        if let Some(last_event_id) = last_event_id {
            let _ = self.event_tx.send(last_event_id);
        }
        Ok(())
    }

    pub fn get_agent_activity(&self, agent_name: &str, limit: i64) -> Result<Vec<ActivityMessage>> {
        let conn = self.conn.lock().unwrap();
        let rows = conn
            .prepare(
                "SELECT m.id, m.seq, m.content, c.name, m.created_at
                 FROM messages m JOIN channels c ON c.id = m.channel_id
                 WHERE m.sender_name = ?1 ORDER BY m.created_at DESC LIMIT ?2",
            )?
            .query_map(params![agent_name, limit], |row| {
                Ok(ActivityMessage {
                    id: row.get(0)?,
                    seq: row.get(1)?,
                    content: row.get(2)?,
                    channel_name: row.get(3)?,
                    created_at: row.get(4)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    /// Load all agent members for a channel as plain names so delivery policy
    /// can filter on top without re-querying membership repeatedly.
    pub(crate) fn get_channel_agent_members_inner(
        conn: &Connection,
        channel_id: &str,
    ) -> Result<Vec<String>> {
        let rows = conn
            .prepare(
                "SELECT member_name FROM channel_members WHERE channel_id = ?1 AND member_type = 'agent'",
            )?
            .query_map(params![channel_id], |row| row.get(0))?
            .filter_map(|row| row.ok())
            .collect();
        Ok(rows)
    }

    /// Derive implicit thread participants from the parent author and prior
    /// thread replies because the product does not yet have an explicit invite
    /// mechanism for threads.
    pub(crate) fn get_thread_agent_recipients_inner(
        conn: &Connection,
        channel_id: &str,
        parent_id: &str,
        sender_name: &str,
    ) -> Result<Vec<String>> {
        let channel_agents = Self::get_channel_agent_members_inner(conn, channel_id)?;
        let channel_agent_set: BTreeSet<String> = channel_agents.into_iter().collect();

        let mut recipients = BTreeSet::new();
        for agent_name in channel_agent_set {
            if agent_name == sender_name {
                continue;
            }
            if Self::agent_can_access_thread_inner(conn, channel_id, parent_id, &agent_name)? {
                recipients.insert(agent_name);
            }
        }

        Ok(recipients.into_iter().collect())
    }

    /// Thread membership is implicit: an agent can access the thread if it
    /// authored the parent message or has already sent at least one reply in
    /// that thread.
    pub(crate) fn agent_can_access_thread_inner(
        conn: &Connection,
        channel_id: &str,
        parent_id: &str,
        agent_name: &str,
    ) -> Result<bool> {
        let parent_author_matches: i64 = conn.query_row(
            "SELECT COUNT(*)
             FROM messages
             WHERE id = ?1 AND channel_id = ?2 AND sender_type = 'agent' AND sender_name = ?3",
            params![parent_id, channel_id, agent_name],
            |row| row.get(0),
        )?;
        if parent_author_matches > 0 {
            return Ok(true);
        }

        let prior_reply_matches: i64 = conn.query_row(
            "SELECT COUNT(*)
             FROM messages
             WHERE channel_id = ?1 AND thread_parent_id = ?2 AND sender_type = 'agent' AND sender_name = ?3",
            params![channel_id, parent_id, agent_name],
            |row| row.get(0),
        )?;
        Ok(prior_reply_matches > 0)
    }
}
