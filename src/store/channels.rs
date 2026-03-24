use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::messages::SenderType;
use super::{channel_from_row, parse_sender_type, sender_type_str, Store};

// ── Types owned by this module ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Channel {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub channel_type: ChannelType,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelType {
    Channel,
    Dm,
    /// System-managed channels (e.g. #all, #shared-memory). Surfaced separately
    /// from user-created channels in the UI.
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelMember {
    pub channel_id: String,
    pub member_name: String,
    pub member_type: SenderType,
    pub last_read_seq: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelMemberProfile {
    pub member_name: String,
    pub member_type: SenderType,
    pub display_name: Option<String>,
}

impl Store {
    /// Persist a new channel row. User-visible list queries later filter out
    /// non-channel types and archived entries.
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
            ChannelType::System => "system",
        };
        conn.execute(
            "INSERT INTO channels (id, name, description, channel_type) VALUES (?1, ?2, ?3, ?4)",
            params![id, name, description, ct],
        )?;
        Ok(id)
    }

    pub fn list_channels(&self) -> Result<Vec<Channel>> {
        let conn = self.conn.lock().unwrap();
        // Exclude DM and system channels — only return user-visible channels.
        let mut stmt = conn.prepare(
            "SELECT id, name, description, channel_type, created_at FROM channels WHERE channel_type = 'channel' AND archived = 0 ORDER BY created_at",
        )?;
        let rows = stmt.query_map([], channel_from_row)?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// Channels that newly created agents should join automatically. User-created
    /// channels stay invite-only; only writable built-in system rooms such as
    /// `#all` are auto-joined.
    pub fn list_auto_join_channels(&self) -> Result<Vec<Channel>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, description, channel_type, created_at
             FROM channels
             WHERE archived = 0 AND channel_type = 'system'
             ORDER BY CASE WHEN name = 'all' THEN 0 ELSE 1 END, created_at",
        )?;
        let rows = stmt.query_map([], channel_from_row)?;
        Ok(rows
            .filter_map(|row| row.ok())
            .filter(|channel| !Store::is_system_channel_read_only(&channel.name))
            .collect())
    }

    /// Update a user channel in place so message/task/thread data continues to
    /// point at the same stable channel id.
    pub fn update_channel(
        &self,
        channel_id: &str,
        name: &str,
        description: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let channel = Self::find_channel_by_id_inner(&conn, channel_id)?
            .ok_or_else(|| anyhow!("channel not found: {}", channel_id))?;
        if channel.channel_type != ChannelType::Channel {
            return Err(anyhow!("only user channels can be updated"));
        }

        conn.execute(
            "UPDATE channels SET name = ?1, description = ?2 WHERE id = ?3",
            params![name, description, channel_id],
        )?;
        Ok(())
    }

    /// Archive a user channel without destroying its history so the UI can hide
    /// it from normal navigation while retaining auditability.
    pub fn archive_channel(&self, channel_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let channel = Self::find_channel_by_id_inner(&conn, channel_id)?
            .ok_or_else(|| anyhow!("channel not found: {}", channel_id))?;
        if channel.channel_type != ChannelType::Channel {
            return Err(anyhow!("only user channels can be archived"));
        }

        conn.execute(
            "UPDATE channels SET archived = 1 WHERE id = ?1",
            params![channel_id],
        )?;
        Ok(())
    }

    /// Permanently remove a user channel and its dependent rows. Channel-owned
    /// data does not currently use foreign-key cascades, so cleanup is explicit.
    pub fn delete_channel(&self, channel_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let channel = Self::find_channel_by_id_inner(&conn, channel_id)?
            .ok_or_else(|| anyhow!("channel not found: {}", channel_id))?;
        if channel.channel_type != ChannelType::Channel {
            return Err(anyhow!("only user channels can be deleted"));
        }

        let attachment_rows: Vec<(String, String)> = conn
            .prepare(
                "SELECT DISTINCT a.id, a.stored_path
                 FROM attachments a
                 JOIN message_attachments ma ON ma.attachment_id = a.id
                 JOIN messages m ON m.id = ma.message_id
                 WHERE m.channel_id = ?1",
            )?
            .query_map(params![channel_id], |row| Ok((row.get(0)?, row.get(1)?)))?
            .filter_map(|row| row.ok())
            .collect();

        conn.execute(
            "DELETE FROM message_attachments WHERE message_id IN (SELECT id FROM messages WHERE channel_id = ?1)",
            params![channel_id],
        )?;
        conn.execute(
            "DELETE FROM messages WHERE channel_id = ?1",
            params![channel_id],
        )?;
        conn.execute(
            "DELETE FROM tasks WHERE channel_id = ?1",
            params![channel_id],
        )?;
        conn.execute(
            "DELETE FROM channel_members WHERE channel_id = ?1",
            params![channel_id],
        )?;
        conn.execute("DELETE FROM channels WHERE id = ?1", params![channel_id])?;

        for (attachment_id, stored_path) in attachment_rows {
            let refs_remaining: i64 = conn.query_row(
                "SELECT COUNT(*) FROM message_attachments WHERE attachment_id = ?1",
                params![attachment_id],
                |row| row.get(0),
            )?;
            if refs_remaining == 0 {
                conn.execute(
                    "DELETE FROM attachments WHERE id = ?1",
                    params![attachment_id],
                )?;
                let _ = std::fs::remove_file(stored_path);
            }
        }

        Ok(())
    }

    pub fn find_channel_by_name(&self, name: &str) -> Result<Option<Channel>> {
        let conn = self.conn.lock().unwrap();
        Self::find_channel_by_name_inner(&conn, name)
    }

    pub(crate) fn find_channel_by_name_inner(
        conn: &Connection,
        name: &str,
    ) -> Result<Option<Channel>> {
        let mut stmt = conn.prepare(
            "SELECT id, name, description, channel_type, created_at FROM channels WHERE name = ?1",
        )?;
        let mut rows = stmt.query_map(params![name], channel_from_row)?;
        Ok(rows.next().transpose()?)
    }

    pub fn find_channel_by_id(&self, id: &str) -> Result<Option<Channel>> {
        let conn = self.conn.lock().unwrap();
        Self::find_channel_by_id_inner(&conn, id)
    }

    pub(crate) fn find_channel_by_id_inner(conn: &Connection, id: &str) -> Result<Option<Channel>> {
        let mut stmt = conn.prepare(
            "SELECT id, name, description, channel_type, created_at FROM channels WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map(params![id], channel_from_row)?;
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

    /// Join a channel by stable id so API handlers do not have to resolve the
    /// mutable channel name before mutating membership.
    pub fn join_channel_by_id(
        &self,
        channel_id: &str,
        member_name: &str,
        member_type: SenderType,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let mt = sender_type_str(member_type);
        conn.execute(
            "INSERT OR IGNORE INTO channel_members (channel_id, member_name, member_type, last_read_seq) VALUES (?1, ?2, ?3, 0)",
            params![channel_id, member_name, mt],
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

    /// Load channel members with display metadata for the UI member rail.
    pub fn get_channel_member_profiles(
        &self,
        channel_id: &str,
    ) -> Result<Vec<ChannelMemberProfile>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT cm.member_name,
                    cm.member_type,
                    CASE
                        WHEN cm.member_type = 'agent' THEN a.display_name
                        ELSE NULL
                    END AS display_name
             FROM channel_members cm
             LEFT JOIN agents a ON a.name = cm.member_name
             WHERE cm.channel_id = ?1
             ORDER BY
                CASE cm.member_type
                    WHEN 'human' THEN 0
                    ELSE 1
                END,
                COALESCE(a.display_name, cm.member_name),
                cm.member_name",
        )?;
        let rows = stmt.query_map(params![channel_id], |row| {
            Ok(ChannelMemberProfile {
                member_name: row.get(0)?,
                member_type: parse_sender_type(&row.get::<_, String>(1)?),
                display_name: row.get(2)?,
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
}
