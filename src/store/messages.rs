use anyhow::{anyhow, Result};
use rusqlite::{params, Connection};
use uuid::Uuid;

use crate::models::*;
use super::{Store, channel_from_row, sender_type_str};

impl Store {
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
        let st = sender_type_str(sender_type);

        conn.execute(
            "INSERT INTO messages (id, channel_id, thread_parent_id, sender_name, sender_type, content, seq) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![msg_id, channel.id, thread_parent_id, sender_name, st, content, seq],
        )?;

        for att_id in attachment_ids {
            conn.execute(
                "INSERT INTO message_attachments (message_id, attachment_id) VALUES (?1, ?2)",
                params![msg_id, att_id],
            )?;
        }

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

            let msgs: Vec<(String, String, String, String, String, i64, Option<String>)> = conn
                .prepare(
                    "SELECT m.id, m.sender_name, m.sender_type, m.content, m.created_at, m.seq, m.thread_parent_id
                     FROM messages m WHERE m.channel_id = ?1 AND m.seq > ?2 ORDER BY m.seq ASC",
                )?
                .query_map(params![channel_id, last_read_seq], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?))
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
            for (msg_id, sender_name, sender_type, content, created_at, seq, thread_parent_id) in &msgs {
                if *seq > max_seq {
                    max_seq = *seq;
                }

                let attachments = Self::get_message_attachments(&conn, msg_id)?;
                let atts = if attachments.is_empty() { None } else { Some(attachments) };

                let effective_channel_name = match &dm_peer_name {
                    Some(peer) => peer.clone(),
                    None => channel.name.clone(),
                };

                let (msg_channel_name, msg_channel_type, parent_channel_name, parent_channel_type) =
                    if let Some(parent_id) = thread_parent_id {
                        let short = if parent_id.len() >= 8 { &parent_id[..8] } else { parent_id.as_str() };
                        let parent_type = match channel.channel_type {
                            ChannelType::Channel => "channel",
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
                                ChannelType::Channel => "channel".to_string(),
                                ChannelType::Dm => "dm".to_string(),
                            },
                            None,
                            None,
                        )
                    };

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
            "SELECT id, seq, content, sender_name, sender_type, created_at \
             FROM messages WHERE channel_id = ?1 {thread_clause} {cursor_clause} \
             ORDER BY seq {order} LIMIT {fetch_limit}"
        );

        let cursor_val = before.or(after).unwrap_or(0);
        let thread_val = thread_parent_id.unwrap_or("");
        let mut stmt = conn.prepare(&sql)?;

        let map_row = |row: &rusqlite::Row| -> rusqlite::Result<HistoryMessage> {
            Ok(HistoryMessage {
                id: row.get(0)?,
                seq: row.get(1)?,
                content: row.get(2)?,
                sender_name: row.get(3)?,
                sender_type: row.get(4)?,
                created_at: row.get(5)?,
                attachments: None,
                reply_count: None,
            })
        };

        // Bind exactly the parameters the SQL expects: ?1=channel_id, optionally ?2=cursor, optionally ?3=thread
        let rows: Vec<HistoryMessage> = match (has_cursor, thread_parent_id.is_some()) {
            (true, true)  => stmt.query_map(params![channel.id, cursor_val, thread_val], map_row)?,
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

            let mut names = vec![sender_name.to_string(), other_name.to_string()];
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
                        .map(|st| sender_type_str(st))
                        .unwrap_or("agent");
                    let other_mt = Self::lookup_sender_type_inner(&conn, other_name)?
                        .map(|st| sender_type_str(st))
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

            let thread_parent_id = thread_short
                .and_then(|short| {
                    conn.query_row(
                        "SELECT id FROM messages WHERE channel_id = ?1 AND id LIKE ?2",
                        params![channel.id, format!("{}%", short)],
                        |row| row.get(0),
                    ).ok()
                });

            Ok((channel.id, thread_parent_id))
        } else if let Some(rest) = target.strip_prefix('#') {
            let parts: Vec<&str> = rest.splitn(2, ':').collect();
            let channel_name = parts[0];
            let thread_short = parts.get(1).copied();

            let channel = Self::find_channel_by_name_inner(&conn, channel_name)?
                .ok_or_else(|| anyhow!("channel not found: {}", channel_name))?;

            let thread_parent_id = thread_short
                .and_then(|short| {
                    conn.query_row(
                        "SELECT id FROM messages WHERE channel_id = ?1 AND id LIKE ?2",
                        params![channel.id, format!("{}%", short)],
                        |row| row.get(0),
                    ).ok()
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
                Ok(AttachmentRef { id: row.get(0)?, filename: row.get(1)? })
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }
}
