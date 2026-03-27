use std::collections::BTreeSet;

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::channels::{Channel, ChannelType};
use super::Store;

// ── Types owned by this module ──

/// Who authored a message or holds channel membership.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SenderType {
    /// Human user row from `humans`.
    Human,
    /// Agent row from `agents`.
    Agent,
}

impl SenderType {
    /// Value stored in `messages.sender_type` / `channel_members.member_type` and in JSON.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::Agent => "agent",
        }
    }

    /// Parse DB / wire string; unknown values default to [`Human`] (matches prior `parse_sender_type`).
    pub fn from_sender_type_str(s: &str) -> Self {
        match s {
            "agent" => Self::Agent,
            _ => Self::Human,
        }
    }
}

/// Provenance metadata attached to a forwarded message, capturing the origin
/// channel and the original sender so recipients can trace where it came from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForwardedFrom {
    /// Source channel slug (no `#`).
    pub channel_name: String,
    /// Original author handle.
    pub sender_name: String,
}

/// In-memory / store representation of one `messages` row plus attachment ids.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// UUID message id.
    pub id: String,
    /// Owning channel id.
    pub channel_id: String,
    /// When set, this message is a thread reply under the parent message id.
    pub thread_parent_id: Option<String>,
    /// Author handle.
    pub sender_name: String,
    /// Author kind.
    pub sender_type: SenderType,
    /// Markdown or plain text body.
    pub content: String,
    /// Wall-clock timestamp from SQLite.
    pub created_at: DateTime<Utc>,
    /// Monotonic per-channel ordering.
    pub seq: i64,
    /// Attachment UUIDs linked via `message_attachments`.
    pub attachment_ids: Vec<String>,
    /// Set when this message was forwarded from another channel.
    pub forwarded_from: Option<ForwardedFrom>,
}

/// Wire shape pushed to agent bridges on receive (names resolved for prompts).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceivedMessage {
    /// Same as `messages.id`.
    pub message_id: String,
    /// Target channel slug.
    pub channel_name: String,
    /// API string for channel kind (`channel`, `dm`, …).
    pub channel_type: String,
    /// Parent channel when this is a thread under another room.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_channel_name: Option<String>,
    /// Parent channel kind string when applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_channel_type: Option<String>,
    /// Author handle.
    pub sender_name: String,
    /// `human` or `agent` string for JSON consumers.
    pub sender_type: String,
    /// Message body.
    pub content: String,
    /// ISO-ish timestamp string for the bridge.
    pub timestamp: String,
    /// Inline attachment metadata when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<AttachmentRef>>,
    /// Forward provenance when this is a cross-post.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forwarded_from: Option<ForwardedFrom>,
}

/// Minimal attachment descriptor embedded in history / receive payloads.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentRef {
    /// Attachment UUID.
    pub id: String,
    /// Original filename for display.
    pub filename: String,
}

/// One message in paginated channel history for the UI.
#[derive(Debug, Serialize, Deserialize)]
pub struct HistoryMessage {
    /// Message UUID.
    pub id: String,
    /// Channel sequence number.
    pub seq: i64,
    /// Body text.
    pub content: String,
    /// Author handle.
    #[serde(rename = "senderName")]
    pub sender_name: String,
    /// `human` or `agent`.
    #[serde(rename = "senderType")]
    pub sender_type: String,
    /// ISO timestamp string.
    #[serde(rename = "createdAt")]
    pub created_at: String,
    /// True when the sender was soft-deleted (tombstone display).
    #[serde(rename = "senderDeleted")]
    pub sender_deleted: bool,
    /// Linked files when any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<AttachmentRef>>,
    /// Number of thread replies when loaded.
    #[serde(rename = "replyCount", skip_serializing_if = "Option::is_none")]
    pub reply_count: Option<i64>,
    /// Set when this message was forwarded from another channel (e.g. via @team mention).
    #[serde(rename = "forwardedFrom", skip_serializing_if = "Option::is_none")]
    pub forwarded_from: Option<ForwardedFrom>,
}

/// Compact message row for activity / cross-channel feeds.
#[derive(Debug, Serialize, Deserialize)]
pub struct ActivityMessage {
    /// Message UUID.
    pub id: String,
    /// Channel sequence number.
    pub seq: i64,
    /// Body text.
    pub content: String,
    /// Channel slug where the message lives.
    #[serde(rename = "channelName")]
    pub channel_name: String,
    /// ISO timestamp string.
    #[serde(rename = "createdAt")]
    pub created_at: String,
}

impl Store {
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
        let conn = self.conn.lock().unwrap();

        let seq: i64 = conn.query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM messages WHERE channel_id = ?1",
            params![channel_id],
            |row| row.get(0),
        )?;

        let msg_id = Uuid::new_v4().to_string();
        let st = sender_type.as_str();
        let forwarded_from_json = forwarded_from
            .map(|value| serde_json::to_string(&value))
            .transpose()?;

        conn.execute(
            "INSERT INTO messages (
                id, channel_id, thread_parent_id, sender_name, sender_type, sender_deleted, content, seq, forwarded_from
             ) VALUES (?1, ?2, NULL, ?3, ?4, 0, ?5, ?6, ?7)",
            params![
                msg_id,
                channel_id,
                sender_name,
                st,
                content,
                seq,
                forwarded_from_json
            ],
        )?;

        for att_id in attachment_ids {
            conn.execute(
                "INSERT INTO message_attachments (message_id, attachment_id) VALUES (?1, ?2)",
                params![msg_id, att_id],
            )?;
        }

        let _ = self.msg_tx.send((channel_id.to_string(), msg_id.clone()));
        Ok(msg_id)
    }

    /// Post a server-authored message into a channel.
    pub fn post_system_message(&self, channel_id: &str, content: &str) -> Result<String> {
        self.post_message_with_forwarded_from(
            channel_id,
            "system",
            SenderType::Human,
            content,
            &[],
            None,
        )
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
        let conn = self.conn.lock().unwrap();
        let channel = Self::find_channel_by_name_inner(&conn, channel_name)?
            .ok_or_else(|| anyhow!("channel not found: {}", channel_name))?;

        let seq: i64 = conn.query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM messages WHERE channel_id = ?1",
            params![channel.id],
            |row| row.get(0),
        )?;
        let msg_id = Uuid::new_v4().to_string();
        let st = sender_type.as_str();
        conn.execute(
            "INSERT INTO messages (
                id, channel_id, thread_parent_id, sender_name, sender_type, sender_deleted, content, seq
             ) VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, ?7)",
            params![msg_id, channel.id, thread_parent_id, sender_name, st, content, seq],
        )?;
        for att_id in attachment_ids {
            conn.execute(
                "INSERT INTO message_attachments (message_id, attachment_id) VALUES (?1, ?2)",
                params![msg_id, att_id],
            )?;
        }
        // Treat the sender's own newly-created message as already read so it
        // does not come back later through get_messages_for_agent() as unread.
        conn.execute(
            "UPDATE channel_members SET last_read_seq = MAX(last_read_seq, ?1) WHERE channel_id = ?2 AND member_name = ?3",
            params![seq, channel.id, sender_name],
        )?;
        let _ = self.msg_tx.send((channel.id.clone(), msg_id.clone()));
        Ok(msg_id)
    }

    pub fn get_messages_for_agent(
        &self,
        agent_name: &str,
        update_read_pos: bool,
    ) -> Result<Vec<ReceivedMessage>> {
        let conn = self.conn.lock().unwrap();
        let memberships: Vec<(String, i64)> = conn
            .prepare("SELECT cm.channel_id, cm.last_read_seq FROM channel_members cm WHERE cm.member_name = ?1")?
            .query_map(params![agent_name], |row| Ok((row.get(0)?, row.get(1)?)))?
            .filter_map(|r| r.ok())
            .collect();

        let mut result = Vec::new();

        for (channel_id, last_read_seq) in &memberships {
            let channel = Self::find_channel_by_id_inner(&conn, channel_id)?
                .ok_or_else(|| anyhow!("channel not found by id"))?;

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

            let mut max_seq = *last_read_seq;
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
                if *seq > max_seq {
                    max_seq = *seq;
                }

                if let Some(parent_id) = thread_parent_id {
                    if !Self::agent_can_access_thread_inner(
                        &conn, channel_id, parent_id, agent_name,
                    )? {
                        continue;
                    }
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

                let forwarded_from = forwarded_from_raw
                    .as_deref()
                    .and_then(|s| serde_json::from_str::<ForwardedFrom>(s).ok());

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

            if update_read_pos && max_seq > *last_read_seq {
                conn.execute(
                    "UPDATE channel_members SET last_read_seq = ?1 WHERE channel_id = ?2 AND member_name = ?3",
                    params![max_seq, channel_id, agent_name],
                )?;
            }
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

    pub fn get_history(
        &self,
        channel_name: &str,
        thread_parent_id: Option<&str>,
        limit: i64,
        before: Option<i64>,
        after: Option<i64>,
    ) -> Result<(Vec<HistoryMessage>, bool)> {
        let conn = self.conn.lock().unwrap();
        let channel = Self::find_channel_by_name_inner(&conn, channel_name)?
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
            format!("AND thread_parent_id = {thread_param_num}")
        } else {
            "AND thread_parent_id IS NULL".to_string()
        };
        let (cursor_clause, order, needs_reverse) = if before.is_some() {
            (format!("AND seq < {cursor_param}"), "DESC", true)
        } else if after.is_some() {
            (format!("AND seq > {cursor_param}"), "ASC", false)
        } else {
            (String::new(), "DESC", true)
        };
        if before.is_some() && after.is_some() {
            return Err(anyhow!("cannot specify both before and after"));
        }

        let sql = format!(
            "SELECT id, seq, content, sender_name, sender_type, sender_deleted, created_at, forwarded_from \
             FROM messages WHERE channel_id = ?1 {thread_clause} {cursor_clause} \
             ORDER BY seq {order} LIMIT {fetch_limit}"
        );

        let cursor_val = before.or(after).unwrap_or(0);
        let thread_val = thread_parent_id.unwrap_or("");
        let mut stmt = conn.prepare(&sql)?;

        let map_row = |row: &rusqlite::Row| -> rusqlite::Result<HistoryMessage> {
            let forwarded_from_raw: Option<String> = row.get(7)?;
            let forwarded_from = forwarded_from_raw
                .as_deref()
                .and_then(|s| serde_json::from_str::<ForwardedFrom>(s).map_err(|e| {
                    tracing::warn!(raw = s, err = %e, "failed to parse forwarded_from JSON in history");
                    e
                }).ok());
            Ok(HistoryMessage {
                id: row.get(0)?,
                seq: row.get(1)?,
                content: row.get(2)?,
                sender_name: row.get(3)?,
                sender_type: row.get(4)?,
                sender_deleted: row.get::<_, i64>(5)? > 0,
                created_at: row.get(6)?,
                attachments: None,
                reply_count: None,
                forwarded_from,
            })
        };

        // Bind exactly the parameters the SQL expects: ?1=channel_id, optionally ?2=cursor, optionally ?3=thread
        let rows: Vec<HistoryMessage> = match (has_cursor, thread_parent_id.is_some()) {
            (true, true) => stmt.query_map(params![channel.id, cursor_val, thread_val], map_row)?,
            (true, false) => stmt.query_map(params![channel.id, cursor_val], map_row)?,
            (false, true) => stmt.query_map(params![channel.id, thread_val], map_row)?,
            (false, false) => stmt.query_map(params![channel.id], map_row)?,
        }
        .filter_map(|r| r.ok())
        .collect();

        let has_more = rows.len() as i64 > limit;
        let mut msgs: Vec<HistoryMessage> = rows.into_iter().take(limit as usize).collect();

        for msg in &mut msgs {
            let atts = Self::get_message_attachments(&conn, &msg.id)?;
            if !atts.is_empty() {
                msg.attachments = Some(atts);
            }
        }

        if thread_parent_id.is_none() {
            for msg in &mut msgs {
                let count: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM messages WHERE channel_id = ?1 AND thread_parent_id = ?2",
                    params![channel.id, msg.id],
                    |row| row.get(0),
                ).unwrap_or(0);
                if count > 0 {
                    msg.reply_count = Some(count);
                }
            }
        }

        if needs_reverse {
            msgs.reverse();
        }

        Ok((msgs, has_more))
    }

    pub fn mark_agent_messages_deleted(&self, name: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE messages SET sender_deleted = 1 WHERE sender_type = 'agent' AND sender_name = ?1",
            params![name],
        )?;
        Ok(())
    }

    pub fn get_last_read_seq(&self, channel_name: &str, member_name: &str) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        let channel = Self::find_channel_by_name_inner(&conn, channel_name)?
            .ok_or_else(|| anyhow!("channel not found: {}", channel_name))?;
        let seq: i64 = conn.query_row(
            "SELECT last_read_seq FROM channel_members WHERE channel_id = ?1 AND member_name = ?2",
            params![channel.id, member_name],
            |row| row.get(0),
        )?;
        Ok(seq)
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

    /// Resolve a `#channel`, `#channel:msgid`, `dm:@name`, or `dm:@name:msgid` target
    /// into `(channel_id, thread_parent_id)`.
    pub fn resolve_target(
        &self,
        target: &str,
        sender_name: &str,
    ) -> Result<(String, Option<String>)> {
        let conn = self.conn.lock().unwrap();

        if let Some(rest) = target.strip_prefix("dm:@") {
            let parts: Vec<&str> = rest.splitn(2, ':').collect();
            let other_name = parts[0];
            let thread_short = parts.get(1).copied();

            let mut names = [sender_name.to_string(), other_name.to_string()];
            names.sort();
            let dm_name = format!("dm-{}-{}", names[0], names[1]);

            let channel = match Self::find_channel_by_name_inner(&conn, &dm_name)? {
                Some(ch) => ch,
                None => {
                    let id = Uuid::new_v4().to_string();
                    conn.execute(
                        "INSERT INTO channels (id, name, channel_type) VALUES (?1, ?2, 'dm')",
                        params![id, dm_name],
                    )?;
                    let sender_mt = Self::lookup_sender_type_inner(&conn, sender_name)?
                        .map(SenderType::as_str)
                        .unwrap_or("agent");
                    let other_mt = Self::lookup_sender_type_inner(&conn, other_name)?
                        .map(SenderType::as_str)
                        .unwrap_or("human");
                    conn.execute(
                        "INSERT OR IGNORE INTO channel_members (channel_id, member_name, member_type, last_read_seq) VALUES (?1, ?2, ?3, 0)",
                        params![id, sender_name, sender_mt],
                    )?;
                    conn.execute(
                        "INSERT OR IGNORE INTO channel_members (channel_id, member_name, member_type, last_read_seq) VALUES (?1, ?2, ?3, 0)",
                        params![id, other_name, other_mt],
                    )?;
                    Channel {
                        id,
                        name: dm_name,
                        description: None,
                        channel_type: ChannelType::Dm,
                        created_at: chrono::Utc::now(),
                    }
                }
            };

            let thread_parent_id = thread_short.and_then(|short| {
                conn.query_row(
                    "SELECT id FROM messages WHERE channel_id = ?1 AND id LIKE ?2",
                    params![channel.id, format!("{}%", short)],
                    |row| row.get(0),
                )
                .ok()
            });

            Ok((channel.id, thread_parent_id))
        } else if let Some(rest) = target.strip_prefix('#') {
            let parts: Vec<&str> = rest.splitn(2, ':').collect();
            let channel_name = parts[0];
            let thread_short = parts.get(1).copied();

            let channel = Self::find_channel_by_name_inner(&conn, channel_name)?
                .ok_or_else(|| anyhow!("channel not found: {}", channel_name))?;

            let thread_parent_id = thread_short.and_then(|short| {
                conn.query_row(
                    "SELECT id FROM messages WHERE channel_id = ?1 AND id LIKE ?2",
                    params![channel.id, format!("{}%", short)],
                    |row| row.get(0),
                )
                .ok()
            });

            Ok((channel.id, thread_parent_id))
        } else {
            Err(anyhow!("invalid target format: {}", target))
        }
    }

    fn get_message_attachments(conn: &Connection, message_id: &str) -> Result<Vec<AttachmentRef>> {
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

    /// Load all agent members for a channel as plain names so delivery policy
    /// can filter on top without re-querying membership repeatedly.
    fn get_channel_agent_members_inner(conn: &Connection, channel_id: &str) -> Result<Vec<String>> {
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
    fn get_thread_agent_recipients_inner(
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
    fn agent_can_access_thread_inner(
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
