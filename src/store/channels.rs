use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::messages::SenderType;
use super::Store;

/// Normalize a channel name for storage/display: trim, strip a single leading
/// `#`, trim again, lowercase. Shared between the HTTP handler and CLI so
/// both sides agree on the canonical form.
pub fn normalize_channel_name(raw: &str) -> String {
    raw.trim().trim_start_matches('#').trim().to_lowercase()
}

// ── Types owned by this module ──

/// One row from `channels` (any type: user, DM, system, or team).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Channel {
    /// UUID primary key.
    pub id: String,
    /// Unique slug (no `#`); used in URLs and mentions.
    pub name: String,
    /// Optional human-facing blurb.
    pub description: Option<String>,
    /// Kind of channel (controls permissions and UI grouping).
    pub channel_type: ChannelType,
    /// Row creation time.
    pub created_at: DateTime<Utc>,
}

/// Classifies how the channel behaves in the UI and access control.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelType {
    /// Normal user-created room.
    Channel,
    /// Two-party direct message channel.
    Dm,
    /// System-managed channels (e.g. #all). Surfaced separately
    /// from user-created channels in the UI.
    System,
    /// Channel owned by a team. Managed through team lifecycle, not directly
    /// deletable by the user.
    Team,
}

impl ChannelType {
    /// Stable lowercase tag used in SQLite `channel_type` and in API JSON (`channel_type` field).
    pub const fn as_api_str(self) -> &'static str {
        match self {
            Self::Channel => "channel",
            Self::Dm => "dm",
            Self::System => "system",
            Self::Team => "team",
        }
    }
}

impl Channel {
    /// Parse the standard 5-column `channels` row: id, name, description, channel_type, created_at.
    pub(crate) fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            name: row.get(1)?,
            description: row.get(2)?,
            channel_type: match row.get::<_, String>(3)?.as_str() {
                "team" => ChannelType::Team,
                "dm" => ChannelType::Dm,
                "system" => ChannelType::System,
                _ => ChannelType::Channel,
            },
            created_at: super::parse_datetime(&row.get::<_, String>(4)?),
        })
    }
}

/// Membership row from `channel_members`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelMember {
    /// Foreign key to `channels.id`.
    pub channel_id: String,
    /// Human username or agent name.
    pub member_name: String,
    /// Whether the member is a human or agent.
    pub member_type: SenderType,
    /// Last read message seq for unread calculations.
    pub last_read_seq: i64,
}

/// Member row joined with optional display metadata for API responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelMemberProfile {
    /// Member handle.
    pub member_name: String,
    /// Human vs agent.
    pub member_type: SenderType,
    /// Resolved display name when available (e.g. agent `display_name`).
    pub display_name: Option<String>,
}

/// Search filters shared by raw channel listings and UI-facing channel info
/// listings so handlers can request exactly the channel set they need.
#[derive(Debug, Clone, Default)]
pub struct ChannelListParams<'a> {
    /// When set, callers can derive per-row `joined` from membership.
    pub for_member: Option<&'a str>,
    /// Include rows with `archived = 1`.
    pub include_archived: bool,
    /// Include `dm` type channels.
    pub include_dm: bool,
    /// Include `system` type channels.
    pub include_system: bool,
    /// Include `team` type channels.
    pub include_team: bool,
}

impl Store {
    fn list_channel_type_names(params: &ChannelListParams<'_>) -> Vec<&'static str> {
        let mut types = vec!["channel"];
        if params.include_team {
            types.push("team");
        }
        if params.include_dm {
            types.push("dm");
        }
        if params.include_system {
            types.push("system");
        }
        types
    }

    fn get_channels_inner(
        conn: &Connection,
        params: &ChannelListParams<'_>,
    ) -> Result<Vec<Channel>> {
        let mut sql =
            "SELECT id, name, description, channel_type, created_at FROM channels".to_string();
        let mut conditions = Vec::new();

        if !params.include_archived {
            conditions.push("archived = 0".to_string());
        }

        let channel_types = Self::list_channel_type_names(params);
        conditions.push(format!(
            "channel_type IN ({})",
            vec!["?"; channel_types.len()].join(", ")
        ));

        if !conditions.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&conditions.join(" AND "));
        }
        sql.push_str(" ORDER BY created_at");

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(
            rusqlite::params_from_iter(channel_types.iter().copied()),
            Channel::from_row,
        )?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

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
        let ct = channel_type.as_api_str();
        conn.execute(
            "INSERT INTO channels (id, name, description, channel_type) VALUES (?1, ?2, ?3, ?4)",
            params![id, name, description, ct],
        )?;
        Ok(id)
    }

    pub fn get_channels(&self) -> Result<Vec<Channel>> {
        let conn = self.conn.lock().unwrap();
        Self::get_channels_inner(
            &conn,
            &ChannelListParams {
                include_team: true,
                ..ChannelListParams::default()
            },
        )
    }

    /// Return channel rows matching the filter list (archived, DM, system, team).
    pub fn get_channels_by_params(&self, params: &ChannelListParams<'_>) -> Result<Vec<Channel>> {
        let conn = self.conn.lock().unwrap();
        Self::get_channels_inner(&conn, params)
    }

    pub fn channel_member_exists(&self, channel_id: &str, member_name: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let exists: i64 = conn.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM channel_members
                WHERE channel_id = ?1 AND member_name = ?2
            )",
            params![channel_id, member_name],
            |row| row.get(0),
        )?;
        Ok(exists != 0)
    }

    /// Channels that newly created agents should join automatically. User-created
    /// channels stay invite-only; only writable built-in system rooms such as
    /// `#all` are auto-joined.
    pub fn get_auto_join_channels(&self) -> Result<Vec<Channel>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, description, channel_type, created_at
             FROM channels
             WHERE archived = 0 AND channel_type = 'system'
             ORDER BY CASE WHEN name = 'all' THEN 0 ELSE 1 END, created_at",
        )?;
        let rows = stmt.query_map([], Channel::from_row)?;
        Ok(rows.filter_map(|row| row.ok()).collect())
    }

    /// Update a user channel in place so message/task data continues to
    /// point at the same stable channel id.
    pub fn update_channel(
        &self,
        channel_id: &str,
        name: &str,
        description: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let channel = Self::get_channel_by_id_inner(&conn, channel_id)?
            .ok_or_else(|| anyhow!("channel not found: {}", channel_id))?;
        if !matches!(
            channel.channel_type,
            ChannelType::Channel | ChannelType::Team
        ) {
            return Err(anyhow!("only user and team channels can be updated"));
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
        let channel = Self::get_channel_by_id_inner(&conn, channel_id)?
            .ok_or_else(|| anyhow!("channel not found: {}", channel_id))?;
        if !matches!(
            channel.channel_type,
            ChannelType::Channel | ChannelType::Team
        ) {
            return Err(anyhow!("only user and team channels can be archived"));
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
        let channel = Self::get_channel_by_id_inner(&conn, channel_id)?
            .ok_or_else(|| anyhow!("channel not found: {}", channel_id))?;
        if channel.channel_type == ChannelType::Team {
            return Err(anyhow!(
                "team channels cannot be deleted directly; delete the team instead"
            ));
        }
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
            "DELETE FROM inbox_read_state WHERE conversation_id = ?1",
            params![channel_id],
        )?;
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

    pub fn get_channel_by_name(&self, name: &str) -> Result<Option<Channel>> {
        let conn = self.conn.lock().unwrap();
        Self::get_channel_by_name_inner(&conn, name)
    }

    pub(crate) fn get_channel_by_name_inner(
        conn: &Connection,
        name: &str,
    ) -> Result<Option<Channel>> {
        let mut stmt = conn.prepare(
            "SELECT id, name, description, channel_type, created_at FROM channels WHERE name = ?1",
        )?;
        let mut rows = stmt.query_map(params![name], Channel::from_row)?;
        Ok(rows.next().transpose()?)
    }

    pub fn get_channel_by_id(&self, id: &str) -> Result<Option<Channel>> {
        let conn = self.conn.lock().unwrap();
        Self::get_channel_by_id_inner(&conn, id)
    }

    pub(crate) fn get_channel_by_id_inner(conn: &Connection, id: &str) -> Result<Option<Channel>> {
        let mut stmt = conn.prepare(
            "SELECT id, name, description, channel_type, created_at FROM channels WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map(params![id], Channel::from_row)?;
        Ok(rows.next().transpose()?)
    }

    pub fn join_channel(
        &self,
        channel_name: &str,
        member_name: &str,
        member_type: SenderType,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let channel = Self::get_channel_by_name_inner(&conn, channel_name)?
            .ok_or_else(|| anyhow!("channel not found: {}", channel_name))?;
        let mt = member_type.as_str();
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
        let mt = member_type.as_str();
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
                member_type: SenderType::from_sender_type_str(&row.get::<_, String>(2)?),
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
                member_type: SenderType::from_sender_type_str(&row.get::<_, String>(1)?),
                display_name: row.get(2)?,
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn is_member(&self, channel_name: &str, member_name: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let channel = Self::get_channel_by_name_inner(&conn, channel_name)?;
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

    /// Ensure a system channel with the given name exists. Idempotent — safe to call on every startup.
    pub fn ensure_system_channel(&self, name: &str, description: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        Self::ensure_system_channel_inner(&conn, name, description)?;
        Ok(())
    }

    /// Ensure built-in channels exist and upgrade legacy `#general` installs to
    /// the new writable `#all` system channel without changing its stable id.
    pub fn ensure_builtin_channels(&self, default_human: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let all_id = if let Some(existing) =
            Self::get_channel_by_name_inner(&conn, Self::DEFAULT_SYSTEM_CHANNEL)?
        {
            conn.execute(
                "UPDATE channels
                 SET description = ?1, channel_type = 'system', archived = 0
                 WHERE id = ?2",
                params![Self::DEFAULT_SYSTEM_CHANNEL_DESCRIPTION, existing.id],
            )?;
            existing.id
        } else if let Some(legacy) = Self::get_channel_by_name_inner(&conn, "general")? {
            conn.execute(
                "UPDATE channels
                 SET name = ?1, description = ?2, channel_type = 'system', archived = 0
                 WHERE id = ?3",
                params![
                    Self::DEFAULT_SYSTEM_CHANNEL,
                    Self::DEFAULT_SYSTEM_CHANNEL_DESCRIPTION,
                    legacy.id
                ],
            )?;
            tracing::info!("migrated built-in channel #general to #all");
            legacy.id
        } else {
            let id = uuid::Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO channels (id, name, description, channel_type)
                 VALUES (?1, ?2, ?3, 'system')",
                params![
                    id,
                    Self::DEFAULT_SYSTEM_CHANNEL,
                    Self::DEFAULT_SYSTEM_CHANNEL_DESCRIPTION
                ],
            )?;
            tracing::info!(
                channel = Self::DEFAULT_SYSTEM_CHANNEL,
                "created built-in system channel"
            );
            id
        };

        conn.execute(
            "INSERT OR IGNORE INTO channel_members (channel_id, member_name, member_type, last_read_seq)
             VALUES (?1, ?2, 'human', 0)",
            params![all_id, default_human],
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO channel_members (channel_id, member_name, member_type, last_read_seq)
             SELECT ?1, name, 'human', 0 FROM humans",
            params![all_id],
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO channel_members (channel_id, member_name, member_type, last_read_seq)
             SELECT ?1, name, 'agent', 0 FROM agents",
            params![all_id],
        )?;

        Ok(())
    }

    fn ensure_system_channel_inner(conn: &Connection, name: &str, description: &str) -> Result<()> {
        let exists: i64 = conn.query_row(
            "SELECT COUNT(*) FROM channels WHERE name = ?1 AND channel_type = 'system'",
            params![name],
            |row| row.get(0),
        )?;
        if exists == 0 {
            let id = uuid::Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO channels (id, name, description, channel_type) VALUES (?1, ?2, ?3, 'system')",
                params![id, name, description],
            )?;
            tracing::info!(channel = %name, "created system channel");
        }
        Ok(())
    }
}
