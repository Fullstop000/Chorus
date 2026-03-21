use std::collections::HashMap;
use std::sync::Mutex;

use anyhow::{anyhow, Result};
use rusqlite::{params, Connection};
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::models::*;

pub struct Store {
    conn: Mutex<Connection>,
    msg_tx: broadcast::Sender<(String, String)>,
}

impl Store {
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        Self::init_schema(&conn)?;
        let (msg_tx, _) = broadcast::channel(256);
        Ok(Self {
            conn: Mutex::new(conn),
            msg_tx,
        })
    }

    fn init_schema(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS channels (
                id TEXT PRIMARY KEY,
                name TEXT UNIQUE NOT NULL,
                description TEXT,
                channel_type TEXT NOT NULL DEFAULT 'channel',
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE IF NOT EXISTS channel_members (
                channel_id TEXT NOT NULL,
                member_name TEXT NOT NULL,
                member_type TEXT NOT NULL,
                last_read_seq INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (channel_id, member_name)
            );
            CREATE TABLE IF NOT EXISTS messages (
                id TEXT PRIMARY KEY,
                channel_id TEXT NOT NULL,
                thread_parent_id TEXT,
                sender_name TEXT NOT NULL,
                sender_type TEXT NOT NULL,
                content TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                seq INTEGER NOT NULL,
                UNIQUE(channel_id, seq)
            );
            CREATE TABLE IF NOT EXISTS message_attachments (
                message_id TEXT NOT NULL,
                attachment_id TEXT NOT NULL,
                PRIMARY KEY (message_id, attachment_id)
            );
            CREATE TABLE IF NOT EXISTS agents (
                id TEXT PRIMARY KEY,
                name TEXT UNIQUE NOT NULL,
                display_name TEXT NOT NULL,
                description TEXT,
                runtime TEXT NOT NULL,
                model TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'inactive',
                session_id TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE IF NOT EXISTS humans (
                name TEXT PRIMARY KEY,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE IF NOT EXISTS tasks (
                id TEXT PRIMARY KEY,
                channel_id TEXT NOT NULL,
                task_number INTEGER NOT NULL,
                title TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'todo',
                claimed_by TEXT,
                created_by TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                UNIQUE(channel_id, task_number)
            );
            CREATE TABLE IF NOT EXISTS attachments (
                id TEXT PRIMARY KEY,
                filename TEXT NOT NULL,
                mime_type TEXT NOT NULL,
                size_bytes INTEGER NOT NULL,
                stored_path TEXT NOT NULL,
                uploaded_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            ",
        )?;
        Ok(())
    }

    pub fn subscribe(&self) -> broadcast::Receiver<(String, String)> {
        self.msg_tx.subscribe()
    }

    // ── Channels ──

    pub fn create_channel(
        &self,
        name: &str,
        description: Option<&str>,
        channel_type: ChannelType,
    ) -> Result<String> {
        let conn = self.conn.lock().unwrap();
        let id = Uuid::new_v4().to_string();
        let ct = match channel_type {
            ChannelType::Channel => "channel",
            ChannelType::Dm => "dm",
        };
        conn.execute(
            "INSERT INTO channels (id, name, description, channel_type) VALUES (?1, ?2, ?3, ?4)",
            params![id, name, description, ct],
        )?;
        Ok(id)
    }

    pub fn list_channels(&self) -> Result<Vec<Channel>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, description, channel_type, created_at FROM channels ORDER BY created_at",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Channel {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                channel_type: match row.get::<_, String>(3)?.as_str() {
                    "dm" => ChannelType::Dm,
                    _ => ChannelType::Channel,
                },
                created_at: parse_datetime(&row.get::<_, String>(4)?),
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn find_channel_by_name(&self, name: &str) -> Result<Option<Channel>> {
        let conn = self.conn.lock().unwrap();
        Self::find_channel_by_name_inner(&conn, name)
    }

    fn find_channel_by_name_inner(conn: &Connection, name: &str) -> Result<Option<Channel>> {
        let mut stmt = conn.prepare(
            "SELECT id, name, description, channel_type, created_at FROM channels WHERE name = ?1",
        )?;
        let mut rows = stmt.query_map(params![name], |row| {
            Ok(Channel {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                channel_type: match row.get::<_, String>(3)?.as_str() {
                    "dm" => ChannelType::Dm,
                    _ => ChannelType::Channel,
                },
                created_at: parse_datetime(&row.get::<_, String>(4)?),
            })
        })?;
        Ok(rows.next().transpose()?)
    }

    pub fn find_channel_by_id(&self, id: &str) -> Result<Option<Channel>> {
        let conn = self.conn.lock().unwrap();
        Self::find_channel_by_id_inner(&conn, id)
    }

    fn find_channel_by_id_inner(conn: &Connection, id: &str) -> Result<Option<Channel>> {
        let mut stmt = conn.prepare(
            "SELECT id, name, description, channel_type, created_at FROM channels WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map(params![id], |row| {
            Ok(Channel {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                channel_type: match row.get::<_, String>(3)?.as_str() {
                    "dm" => ChannelType::Dm,
                    _ => ChannelType::Channel,
                },
                created_at: parse_datetime(&row.get::<_, String>(4)?),
            })
        })?;
        Ok(rows.next().transpose()?)
    }

    pub fn join_channel(
        &self,
        channel_name: &str,
        member_name: &str,
        member_type: SenderType,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let channel = Self::find_channel_by_name_inner(&conn, channel_name)?
            .ok_or_else(|| anyhow!("channel not found: {}", channel_name))?;
        let mt = sender_type_str(member_type);
        conn.execute(
            "INSERT OR IGNORE INTO channel_members (channel_id, member_name, member_type, last_read_seq) VALUES (?1, ?2, ?3, 0)",
            params![channel.id, member_name, mt],
        )?;
        Ok(())
    }

    pub fn get_channel_members(&self, channel_id: &str) -> Result<Vec<ChannelMember>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT channel_id, member_name, member_type, last_read_seq FROM channel_members WHERE channel_id = ?1",
        )?;
        let rows = stmt.query_map(params![channel_id], |row| {
            Ok(ChannelMember {
                channel_id: row.get(0)?,
                member_name: row.get(1)?,
                member_type: parse_sender_type(&row.get::<_, String>(2)?),
                last_read_seq: row.get(3)?,
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn is_member(&self, channel_name: &str, member_name: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let channel = Self::find_channel_by_name_inner(&conn, channel_name)?;
        match channel {
            None => Ok(false),
            Some(ch) => {
                let count: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM channel_members WHERE channel_id = ?1 AND member_name = ?2",
                    params![ch.id, member_name],
                    |row| row.get(0),
                )?;
                Ok(count > 0)
            }
        }
    }

    // ── Messages ──

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

        // Notify subscribers (ignore errors — no subscribers is OK)
        let _ = self.msg_tx.send((channel.id.clone(), msg_id.clone()));

        Ok(msg_id)
    }

    pub fn get_messages_for_agent(
        &self,
        agent_name: &str,
        update_read_pos: bool,
    ) -> Result<Vec<ReceivedMessage>> {
        let conn = self.conn.lock().unwrap();
        // Get all channels the agent is a member of
        let mut mem_stmt = conn.prepare(
            "SELECT cm.channel_id, cm.last_read_seq FROM channel_members cm WHERE cm.member_name = ?1",
        )?;
        let memberships: Vec<(String, i64)> = mem_stmt
            .query_map(params![agent_name], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?
            .filter_map(|r| r.ok())
            .collect();

        let mut result = Vec::new();

        for (channel_id, last_read_seq) in &memberships {
            let channel = Self::find_channel_by_id_inner(&conn, channel_id)?
                .ok_or_else(|| anyhow!("channel not found by id"))?;

            let mut msg_stmt = conn.prepare(
                "SELECT m.id, m.sender_name, m.sender_type, m.content, m.created_at, m.seq
                 FROM messages m
                 WHERE m.channel_id = ?1 AND m.seq > ?2 AND m.thread_parent_id IS NULL
                 ORDER BY m.seq ASC",
            )?;

            let msgs: Vec<(String, String, String, String, String, i64)> = msg_stmt
                .query_map(params![channel_id, last_read_seq], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, i64>(5)?,
                    ))
                })?
                .filter_map(|r| r.ok())
                .collect();

            let mut max_seq = *last_read_seq;
            for (msg_id, sender_name, sender_type, content, created_at, seq) in &msgs {
                if *seq > max_seq {
                    max_seq = *seq;
                }

                // Get attachments for this message
                let attachments = Self::get_message_attachments(&conn, msg_id)?;
                let atts = if attachments.is_empty() {
                    None
                } else {
                    Some(attachments)
                };

                // Determine parent channel info for DMs
                let (parent_channel_name, parent_channel_type) =
                    if channel.channel_type == ChannelType::Dm {
                        (None, None)
                    } else {
                        (None, None)
                    };

                result.push(ReceivedMessage {
                    message_id: msg_id.clone(),
                    channel_name: channel.name.clone(),
                    channel_type: match channel.channel_type {
                        ChannelType::Channel => "channel".to_string(),
                        ChannelType::Dm => "dm".to_string(),
                    },
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

    fn get_message_attachments(
        conn: &Connection,
        message_id: &str,
    ) -> Result<Vec<AttachmentRef>> {
        let mut stmt = conn.prepare(
            "SELECT a.id, a.filename FROM message_attachments ma JOIN attachments a ON ma.attachment_id = a.id WHERE ma.message_id = ?1",
        )?;
        let rows = stmt
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

        let fetch_limit = limit + 1; // fetch one extra to check has_more

        let (sql, needs_reverse) = match (thread_parent_id, before, after) {
            (None, None, None) => (
                format!(
                    "SELECT id, seq, content, sender_name, sender_type, created_at FROM messages WHERE channel_id = ?1 AND thread_parent_id IS NULL ORDER BY seq DESC LIMIT {}",
                    fetch_limit
                ),
                true,
            ),
            (None, Some(_), None) => (
                format!(
                    "SELECT id, seq, content, sender_name, sender_type, created_at FROM messages WHERE channel_id = ?1 AND thread_parent_id IS NULL AND seq < ?2 ORDER BY seq DESC LIMIT {}",
                    fetch_limit
                ),
                true,
            ),
            (None, None, Some(_)) => (
                format!(
                    "SELECT id, seq, content, sender_name, sender_type, created_at FROM messages WHERE channel_id = ?1 AND thread_parent_id IS NULL AND seq > ?2 ORDER BY seq ASC LIMIT {}",
                    fetch_limit
                ),
                false,
            ),
            (Some(_), None, None) => (
                format!(
                    "SELECT id, seq, content, sender_name, sender_type, created_at FROM messages WHERE channel_id = ?1 AND thread_parent_id = ?3 ORDER BY seq DESC LIMIT {}",
                    fetch_limit
                ),
                true,
            ),
            (Some(_), Some(_), None) => (
                format!(
                    "SELECT id, seq, content, sender_name, sender_type, created_at FROM messages WHERE channel_id = ?1 AND thread_parent_id = ?3 AND seq < ?2 ORDER BY seq DESC LIMIT {}",
                    fetch_limit
                ),
                true,
            ),
            (Some(_), None, Some(_)) => (
                format!(
                    "SELECT id, seq, content, sender_name, sender_type, created_at FROM messages WHERE channel_id = ?1 AND thread_parent_id = ?3 AND seq > ?2 ORDER BY seq ASC LIMIT {}",
                    fetch_limit
                ),
                false,
            ),
            _ => return Err(anyhow!("cannot specify both before and after")),
        };

        let mut stmt = conn.prepare(&sql)?;

        let cursor_val = before.or(after).unwrap_or(0);
        let thread_val = thread_parent_id.unwrap_or("");

        let rows: Vec<HistoryMessage> = if before.is_some() || after.is_some() {
            if thread_parent_id.is_some() {
                stmt.query_map(params![channel.id, cursor_val, thread_val], |row| {
                    Ok(HistoryMessage {
                        id: row.get(0)?,
                        seq: row.get(1)?,
                        content: row.get(2)?,
                        sender_name: row.get(3)?,
                        sender_type: row.get(4)?,
                        created_at: row.get(5)?,
                        attachments: None,
                    })
                })?
                .filter_map(|r| r.ok())
                .collect()
            } else {
                stmt.query_map(params![channel.id, cursor_val], |row| {
                    Ok(HistoryMessage {
                        id: row.get(0)?,
                        seq: row.get(1)?,
                        content: row.get(2)?,
                        sender_name: row.get(3)?,
                        sender_type: row.get(4)?,
                        created_at: row.get(5)?,
                        attachments: None,
                    })
                })?
                .filter_map(|r| r.ok())
                .collect()
            }
        } else if thread_parent_id.is_some() {
            stmt.query_map(params![channel.id, cursor_val, thread_val], |row| {
                Ok(HistoryMessage {
                    id: row.get(0)?,
                    seq: row.get(1)?,
                    content: row.get(2)?,
                    sender_name: row.get(3)?,
                    sender_type: row.get(4)?,
                    created_at: row.get(5)?,
                    attachments: None,
                })
            })?
            .filter_map(|r| r.ok())
            .collect()
        } else {
            stmt.query_map(params![channel.id], |row| {
                Ok(HistoryMessage {
                    id: row.get(0)?,
                    seq: row.get(1)?,
                    content: row.get(2)?,
                    sender_name: row.get(3)?,
                    sender_type: row.get(4)?,
                    created_at: row.get(5)?,
                    attachments: None,
                })
            })?
            .filter_map(|r| r.ok())
            .collect()
        };

        let has_more = rows.len() as i64 > limit;
        let mut msgs: Vec<HistoryMessage> = rows.into_iter().take(limit as usize).collect();

        // Populate attachments for each message
        for msg in &mut msgs {
            let atts = Self::get_message_attachments(&conn, &msg.id)?;
            if !atts.is_empty() {
                msg.attachments = Some(atts);
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

    // ── Target resolution ──

    pub fn resolve_target(
        &self,
        target: &str,
        sender_name: &str,
    ) -> Result<(String, Option<String>)> {
        let conn = self.conn.lock().unwrap();

        if let Some(rest) = target.strip_prefix("dm:@") {
            // DM target: dm:@name or dm:@name:msgid
            let parts: Vec<&str> = rest.splitn(2, ':').collect();
            let other_name = parts[0];
            let thread_short = parts.get(1).copied();

            // Create or find DM channel
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
                    // Auto-add both parties
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

            let thread_parent_id = if let Some(short) = thread_short {
                let full_id: Option<String> = conn
                    .query_row(
                        "SELECT id FROM messages WHERE channel_id = ?1 AND id LIKE ?2",
                        params![channel.id, format!("{}%", short)],
                        |row| row.get(0),
                    )
                    .ok();
                full_id
            } else {
                None
            };

            Ok((channel.id, thread_parent_id))
        } else if let Some(rest) = target.strip_prefix('#') {
            // Channel target: #name or #name:msgid
            let parts: Vec<&str> = rest.splitn(2, ':').collect();
            let channel_name = parts[0];
            let thread_short = parts.get(1).copied();

            let channel = Self::find_channel_by_name_inner(&conn, channel_name)?
                .ok_or_else(|| anyhow!("channel not found: {}", channel_name))?;

            let thread_parent_id = if let Some(short) = thread_short {
                let full_id: Option<String> = conn
                    .query_row(
                        "SELECT id FROM messages WHERE channel_id = ?1 AND id LIKE ?2",
                        params![channel.id, format!("{}%", short)],
                        |row| row.get(0),
                    )
                    .ok();
                full_id
            } else {
                None
            };

            Ok((channel.id, thread_parent_id))
        } else {
            Err(anyhow!("invalid target format: {}", target))
        }
    }

    pub fn format_target(
        &self,
        channel_id: &str,
        thread_parent_id: Option<&str>,
        for_name: &str,
    ) -> Result<String> {
        let conn = self.conn.lock().unwrap();
        let channel = Self::find_channel_by_id_inner(&conn, channel_id)?
            .ok_or_else(|| anyhow!("channel not found by id: {}", channel_id))?;

        let base = match channel.channel_type {
            ChannelType::Channel => format!("#{}", channel.name),
            ChannelType::Dm => {
                // DM channel name is dm-{sorted_name1}-{sorted_name2}
                // Find the other party by looking at channel members
                let members: Vec<String> = conn
                    .prepare("SELECT member_name FROM channel_members WHERE channel_id = ?1")?
                    .query_map(params![channel_id], |row| row.get(0))?
                    .filter_map(|r| r.ok())
                    .collect();
                let other = members.iter()
                    .find(|m| m.as_str() != for_name)
                    .cloned()
                    .unwrap_or_else(|| channel.name.strip_prefix("dm-").unwrap_or(&channel.name).to_string());
                format!("dm:@{}", other)
            }
        };

        match thread_parent_id {
            Some(pid) => {
                let short = &pid[..8.min(pid.len())];
                Ok(format!("{}:{}", base, short))
            }
            None => Ok(base),
        }
    }

    // ── Agents ──

    pub fn create_agent_record(
        &self,
        name: &str,
        display_name: &str,
        description: Option<&str>,
        runtime: &str,
        model: &str,
    ) -> Result<String> {
        let conn = self.conn.lock().unwrap();
        let id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO agents (id, name, display_name, description, runtime, model) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, name, display_name, description, runtime, model],
        )?;
        Ok(id)
    }

    /// Remove an agent and its memberships when provisioning fails.
    pub fn delete_agent_record(&self, name: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM channel_members WHERE member_name = ?1",
            params![name],
        )?;
        conn.execute("DELETE FROM agents WHERE name = ?1", params![name])?;
        Ok(())
    }

    pub fn list_agents(&self) -> Result<Vec<Agent>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, display_name, description, runtime, model, status, session_id, created_at FROM agents ORDER BY name",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok(Agent {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    display_name: row.get(2)?,
                    description: row.get(3)?,
                    runtime: row.get(4)?,
                    model: row.get(5)?,
                    status: parse_agent_status(&row.get::<_, String>(6)?),
                    session_id: row.get(7)?,
                    created_at: parse_datetime(&row.get::<_, String>(8)?),
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    pub fn get_agent(&self, name: &str) -> Result<Option<Agent>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, display_name, description, runtime, model, status, session_id, created_at FROM agents WHERE name = ?1",
        )?;
        let mut rows = stmt.query_map(params![name], |row| {
            Ok(Agent {
                id: row.get(0)?,
                name: row.get(1)?,
                display_name: row.get(2)?,
                description: row.get(3)?,
                runtime: row.get(4)?,
                model: row.get(5)?,
                status: parse_agent_status(&row.get::<_, String>(6)?),
                session_id: row.get(7)?,
                created_at: parse_datetime(&row.get::<_, String>(8)?),
            })
        })?;
        Ok(rows.next().transpose()?)
    }

    pub fn update_agent_status(&self, name: &str, status: AgentStatus) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let s = match status {
            AgentStatus::Active => "active",
            AgentStatus::Sleeping => "sleeping",
            AgentStatus::Inactive => "inactive",
        };
        conn.execute(
            "UPDATE agents SET status = ?1 WHERE name = ?2",
            params![s, name],
        )?;
        Ok(())
    }

    pub fn update_agent_session(&self, name: &str, session_id: Option<&str>) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE agents SET session_id = ?1 WHERE name = ?2",
            params![session_id, name],
        )?;
        Ok(())
    }

    // ── Humans ──

    pub fn add_human(&self, name: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("INSERT OR IGNORE INTO humans (name) VALUES (?1)", params![name])?;
        Ok(())
    }

    pub fn list_humans(&self) -> Result<Vec<Human>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT name, created_at FROM humans ORDER BY name")?;
        let rows = stmt
            .query_map([], |row| {
                Ok(Human {
                    name: row.get(0)?,
                    created_at: parse_datetime(&row.get::<_, String>(1)?),
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    // ── Tasks ──

    pub fn create_tasks(
        &self,
        channel_name: &str,
        creator_name: &str,
        titles: &[&str],
    ) -> Result<Vec<TaskInfo>> {
        let conn = self.conn.lock().unwrap();
        let channel = Self::find_channel_by_name_inner(&conn, channel_name)?
            .ok_or_else(|| anyhow!("channel not found: {}", channel_name))?;

        let max_num: i64 = conn.query_row(
            "SELECT COALESCE(MAX(task_number), 0) FROM tasks WHERE channel_id = ?1",
            params![channel.id],
            |row| row.get(0),
        )?;

        let mut result = Vec::new();
        for (i, title) in titles.iter().enumerate() {
            let id = Uuid::new_v4().to_string();
            let task_number = max_num + 1 + i as i64;
            conn.execute(
                "INSERT INTO tasks (id, channel_id, task_number, title, created_by) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![id, channel.id, task_number, title, creator_name],
            )?;
            result.push(TaskInfo {
                task_number,
                title: title.to_string(),
                status: "todo".to_string(),
                claimed_by_name: None,
                created_by_name: Some(creator_name.to_string()),
            });
        }

        Ok(result)
    }

    pub fn list_tasks(
        &self,
        channel_name: &str,
        status_filter: Option<TaskStatus>,
    ) -> Result<Vec<TaskInfo>> {
        let conn = self.conn.lock().unwrap();
        let channel = Self::find_channel_by_name_inner(&conn, channel_name)?
            .ok_or_else(|| anyhow!("channel not found: {}", channel_name))?;

        if let Some(status) = status_filter {
            let mut stmt = conn.prepare(
                "SELECT task_number, title, status, claimed_by, created_by FROM tasks WHERE channel_id = ?1 AND status = ?2 ORDER BY task_number",
            )?;
            let rows: Vec<TaskInfo> = stmt.query_map(params![channel.id, status.as_str()], |row| {
                Ok(TaskInfo {
                    task_number: row.get(0)?,
                    title: row.get(1)?,
                    status: row.get(2)?,
                    claimed_by_name: row.get(3)?,
                    created_by_name: row.get(4)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
            Ok(rows)
        } else {
            let mut stmt = conn.prepare(
                "SELECT task_number, title, status, claimed_by, created_by FROM tasks WHERE channel_id = ?1 ORDER BY task_number",
            )?;
            let rows: Vec<TaskInfo> = stmt.query_map(params![channel.id], |row| {
                Ok(TaskInfo {
                    task_number: row.get(0)?,
                    title: row.get(1)?,
                    status: row.get(2)?,
                    claimed_by_name: row.get(3)?,
                    created_by_name: row.get(4)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
            Ok(rows)
        }
    }

    pub fn claim_tasks(
        &self,
        channel_name: &str,
        claimer_name: &str,
        task_numbers: &[i64],
    ) -> Result<Vec<ClaimResult>> {
        let conn = self.conn.lock().unwrap();
        let channel = Self::find_channel_by_name_inner(&conn, channel_name)?
            .ok_or_else(|| anyhow!("channel not found: {}", channel_name))?;

        let mut results = Vec::new();
        for &tn in task_numbers {
            let task: Option<(String, Option<String>)> = conn
                .query_row(
                    "SELECT status, claimed_by FROM tasks WHERE channel_id = ?1 AND task_number = ?2",
                    params![channel.id, tn],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
                )
                .ok();

            match task {
                Some((status, claimed_by)) => {
                    if status != "todo" || claimed_by.is_some() {
                        results.push(ClaimResult {
                            task_number: tn,
                            success: false,
                            reason: Some("task already claimed or not in todo status".to_string()),
                        });
                    } else {
                        conn.execute(
                            "UPDATE tasks SET claimed_by = ?1, status = 'in_progress', updated_at = datetime('now') WHERE channel_id = ?2 AND task_number = ?3",
                            params![claimer_name, channel.id, tn],
                        )?;
                        results.push(ClaimResult {
                            task_number: tn,
                            success: true,
                            reason: None,
                        });
                    }
                }
                None => {
                    results.push(ClaimResult {
                        task_number: tn,
                        success: false,
                        reason: Some("task not found".to_string()),
                    });
                }
            }
        }

        Ok(results)
    }

    pub fn unclaim_task(
        &self,
        channel_name: &str,
        claimer_name: &str,
        task_number: i64,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let channel = Self::find_channel_by_name_inner(&conn, channel_name)?
            .ok_or_else(|| anyhow!("channel not found: {}", channel_name))?;

        let claimed_by: Option<String> = conn.query_row(
            "SELECT claimed_by FROM tasks WHERE channel_id = ?1 AND task_number = ?2",
            params![channel.id, task_number],
            |row| row.get(0),
        )?;

        if claimed_by.as_deref() != Some(claimer_name) {
            return Err(anyhow!("task not claimed by {}", claimer_name));
        }

        conn.execute(
            "UPDATE tasks SET claimed_by = NULL, status = 'todo', updated_at = datetime('now') WHERE channel_id = ?1 AND task_number = ?2",
            params![channel.id, task_number],
        )?;

        Ok(())
    }

    pub fn update_task_status(
        &self,
        channel_name: &str,
        task_number: i64,
        requester_name: &str,
        new_status: TaskStatus,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let channel = Self::find_channel_by_name_inner(&conn, channel_name)?
            .ok_or_else(|| anyhow!("channel not found: {}", channel_name))?;

        let (current_status_str, claimed_by): (String, Option<String>) = conn.query_row(
            "SELECT status, claimed_by FROM tasks WHERE channel_id = ?1 AND task_number = ?2",
            params![channel.id, task_number],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;

        let current_status = TaskStatus::from_str(&current_status_str)
            .ok_or_else(|| anyhow!("invalid task status: {}", current_status_str))?;

        if claimed_by.as_deref() != Some(requester_name) {
            return Err(anyhow!("task not claimed by {}", requester_name));
        }

        if !current_status.can_transition_to(new_status) {
            return Err(anyhow!(
                "cannot transition from {} to {}",
                current_status.as_str(),
                new_status.as_str()
            ));
        }

        conn.execute(
            "UPDATE tasks SET status = ?1, updated_at = datetime('now') WHERE channel_id = ?2 AND task_number = ?3",
            params![new_status.as_str(), channel.id, task_number],
        )?;

        Ok(())
    }

    // ── Attachments ──

    pub fn store_attachment(
        &self,
        filename: &str,
        mime_type: &str,
        size: i64,
        stored_path: &str,
    ) -> Result<String> {
        let conn = self.conn.lock().unwrap();
        let id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO attachments (id, filename, mime_type, size_bytes, stored_path) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, filename, mime_type, size, stored_path],
        )?;
        Ok(id)
    }

    pub fn get_attachment(&self, id: &str) -> Result<Option<Attachment>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, filename, mime_type, size_bytes, stored_path, uploaded_at FROM attachments WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map(params![id], |row| {
            Ok(Attachment {
                id: row.get(0)?,
                filename: row.get(1)?,
                mime_type: row.get(2)?,
                size_bytes: row.get(3)?,
                stored_path: row.get(4)?,
                uploaded_at: parse_datetime(&row.get::<_, String>(5)?),
            })
        })?;
        Ok(rows.next().transpose()?)
    }

    // ── Sender type lookup ──

    pub fn lookup_sender_type(&self, name: &str) -> Result<Option<SenderType>> {
        let conn = self.conn.lock().unwrap();
        Self::lookup_sender_type_inner(&conn, name)
    }

    fn lookup_sender_type_inner(conn: &Connection, name: &str) -> Result<Option<SenderType>> {
        // Check agents first
        let agent_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM agents WHERE name = ?1",
            params![name],
            |row| row.get(0),
        )?;
        if agent_count > 0 {
            return Ok(Some(SenderType::Agent));
        }
        // Then humans
        let human_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM humans WHERE name = ?1",
            params![name],
            |row| row.get(0),
        )?;
        if human_count > 0 {
            return Ok(Some(SenderType::Human));
        }
        Ok(None)
    }

    // ── Server info ──

    /// Build the sidebar payload for the given human or agent name.
    pub fn get_server_info(&self, for_agent: &str) -> Result<ServerInfo> {
        let conn = self.conn.lock().unwrap();

        // Channels (non-DM only)
        let mut ch_stmt = conn.prepare(
            "SELECT c.name, c.description, EXISTS(SELECT 1 FROM channel_members cm WHERE cm.channel_id = c.id AND cm.member_name = ?1) as joined FROM channels c WHERE c.channel_type = 'channel' ORDER BY c.name",
        )?;
        let channels: Vec<ChannelInfo> = ch_stmt
            .query_map(params![for_agent], |row| {
                Ok(ChannelInfo {
                    name: row.get(0)?,
                    description: row.get(1)?,
                    joined: row.get::<_, i64>(2)? > 0,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        // Agents
        let mut ag_stmt = conn.prepare(
            "SELECT name, display_name, description, runtime, model, status, session_id FROM agents ORDER BY name",
        )?;
        let agents: Vec<AgentInfo> = ag_stmt
            .query_map([], |row| {
                Ok(AgentInfo {
                    name: row.get(0)?,
                    display_name: row.get(1)?,
                    description: row.get(2)?,
                    runtime: row.get(3)?,
                    model: row.get(4)?,
                    status: row.get(5)?,
                    session_id: row.get(6)?,
                    activity: None,
                    activity_detail: None,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        // Humans
        let mut hu_stmt = conn.prepare("SELECT name FROM humans ORDER BY name")?;
        let humans: Vec<HumanInfo> = hu_stmt
            .query_map([], |row| Ok(HumanInfo { name: row.get(0)? }))?
            .filter_map(|r| r.ok())
            .collect();

        Ok(ServerInfo {
            channels,
            agents,
            humans,
        })
    }

    // ── Unread summary ──

    pub fn get_agent_activity(&self, agent_name: &str, limit: i64) -> Result<Vec<ActivityMessage>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT m.id, m.seq, m.content, c.name, m.created_at
             FROM messages m
             JOIN channels c ON c.id = m.channel_id
             WHERE m.sender_name = ?1
             ORDER BY m.created_at DESC
             LIMIT ?2",
        )?;
        let rows = stmt
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

    pub fn get_unread_summary(&self, agent_name: &str) -> Result<HashMap<String, i64>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT c.name, COUNT(m.id)
             FROM channel_members cm
             JOIN channels c ON cm.channel_id = c.id
             JOIN messages m ON m.channel_id = cm.channel_id AND m.seq > cm.last_read_seq AND m.thread_parent_id IS NULL
             WHERE cm.member_name = ?1
             GROUP BY c.name",
        )?;
        let rows = stmt
            .query_map(params![agent_name], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?
            .filter_map(|r| r.ok());

        let mut map = HashMap::new();
        for (name, count) in rows {
            map.insert(name, count);
        }
        Ok(map)
    }
}

// ── Helpers ──

fn parse_datetime(s: &str) -> chrono::DateTime<chrono::Utc> {
    chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
        .map(|dt| dt.and_utc())
        .unwrap_or_else(|_| chrono::Utc::now())
}

fn sender_type_str(st: SenderType) -> &'static str {
    match st {
        SenderType::Human => "human",
        SenderType::Agent => "agent",
    }
}

fn parse_sender_type(s: &str) -> SenderType {
    match s {
        "agent" => SenderType::Agent,
        _ => SenderType::Human,
    }
}

fn parse_agent_status(s: &str) -> AgentStatus {
    match s {
        "active" => AgentStatus::Active,
        "sleeping" => AgentStatus::Sleeping,
        _ => AgentStatus::Inactive,
    }
}
