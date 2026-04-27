use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
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

/// Shared error message for invalid channel names.
pub const INVALID_CHANNEL_NAME_MSG: &str =
    "channel name can only contain lowercase letters, numbers, hyphens, and underscores";

/// Returns true if the channel name contains only allowed characters:
/// lowercase ASCII letters, digits, hyphens, and underscores.
/// Callers should normalize first, then validate.
pub fn is_valid_channel_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

// ── Types owned by this module ──

/// One row from `channels` (any type: user, DM, system, team, or task sub-channel).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Channel {
    /// UUID primary key.
    pub id: String,
    /// Owning workspace id.
    pub workspace_id: String,
    /// Unique slug (no `#`); used in URLs and mentions.
    pub name: String,
    /// Optional human-facing blurb.
    pub description: Option<String>,
    /// Kind of channel (controls permissions and UI grouping).
    pub channel_type: ChannelType,
    /// Row creation time.
    pub created_at: DateTime<Utc>,
    /// Parent channel id, set for `ChannelType::Task` sub-channels pointing at
    /// the channel that owns the task. `None` for all other channel types.
    pub parent_channel_id: Option<String>,
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
    /// Child channel owned by a task. One per task. Hidden from the main channel list.
    Task,
}

impl ChannelType {
    /// Stable lowercase tag used in SQLite `channel_type` and in API JSON (`channel_type` field).
    pub const fn as_api_str(self) -> &'static str {
        match self {
            Self::Channel => "channel",
            Self::Dm => "dm",
            Self::System => "system",
            Self::Team => "team",
            Self::Task => "task",
        }
    }
}

impl Channel {
    /// Parse the standard `channels` row: id, workspace_id, name, description,
    /// channel_type, created_at, parent_channel_id.
    pub(crate) fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            workspace_id: row.get(1)?,
            name: row.get(2)?,
            description: row.get(3)?,
            channel_type: match row.get::<_, String>(4)?.as_str() {
                "team" => ChannelType::Team,
                "dm" => ChannelType::Dm,
                "system" => ChannelType::System,
                "task" => ChannelType::Task,
                _ => ChannelType::Channel,
            },
            created_at: super::parse_datetime(&row.get::<_, String>(5)?),
            parent_channel_id: row.get(6)?,
        })
    }
}

/// Membership row from `channel_members`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelMember {
    /// Foreign key to `channels.id`.
    pub channel_id: String,
    /// Human id or agent id.
    pub member_id: String,
    /// Whether the member is a human or agent.
    pub member_type: SenderType,
    /// Last read message seq for unread calculations.
    pub last_read_seq: i64,
}

/// Member row joined with optional display metadata for API responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelMemberProfile {
    /// Stable member id.
    pub member_id: String,
    /// Display/lookup label.
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
    /// Restrict results to one workspace.
    pub workspace_id: Option<&'a str>,
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
    /// Include `task` type channels (task sub-channels). Defaults to `false`
    /// so the normal sidebar/channel-list queries keep hiding per-task rooms;
    /// only task-aware views (e.g. the task detail page) opt in.
    pub include_tasks: bool,
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
        if params.include_tasks {
            types.push("task");
        }
        types
    }

    fn get_channels_inner(
        conn: &Connection,
        params: &ChannelListParams<'_>,
    ) -> Result<Vec<Channel>> {
        let mut sql =
            "SELECT id, workspace_id, name, description, channel_type, created_at, parent_channel_id FROM channels".to_string();
        let mut conditions = Vec::new();

        if !params.include_archived {
            conditions.push("archived = 0".to_string());
        }

        let mut values: Vec<String> = Vec::new();
        if let Some(workspace_id) = params.workspace_id {
            conditions.push("workspace_id = ?".to_string());
            values.push(workspace_id.to_string());
        }

        let channel_types = Self::list_channel_type_names(params);
        conditions.push(format!(
            "channel_type IN ({})",
            vec!["?"; channel_types.len()].join(", ")
        ));
        values.extend(channel_types.iter().map(|value| value.to_string()));

        if !conditions.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&conditions.join(" AND "));
        }
        sql.push_str(" ORDER BY created_at");

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(values), Channel::from_row)?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// Persist a new channel row. User-visible list queries later filter out
    /// non-channel types and archived entries. Pass `parent_channel_id` only
    /// for `ChannelType::Task` sub-channels; all other channel types use `None`.
    pub fn create_channel(
        &self,
        name: &str,
        description: Option<&str>,
        channel_type: ChannelType,
        parent_channel_id: Option<&str>,
    ) -> Result<String> {
        let conn = self.conn.lock().unwrap();
        let workspace_id = Self::workspace_id_for_write_inner(&conn)?;
        let id = Uuid::new_v4().to_string();
        let ct = channel_type.as_api_str();
        conn.execute(
            "INSERT INTO channels (id, workspace_id, name, description, channel_type, parent_channel_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, workspace_id, name, description, ct, parent_channel_id],
        )?;
        Ok(id)
    }

    /// Persist a channel row inside an explicit workspace. New workspace-aware
    /// call paths should use this instead of relying on name-only global scope.
    pub fn create_channel_in_workspace(
        &self,
        workspace_id: &str,
        name: &str,
        description: Option<&str>,
        channel_type: ChannelType,
        parent_channel_id: Option<&str>,
    ) -> Result<String> {
        let conn = self.conn.lock().unwrap();
        let id = Uuid::new_v4().to_string();
        let ct = channel_type.as_api_str();
        conn.execute(
            "INSERT INTO channels (id, workspace_id, name, description, channel_type, parent_channel_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, workspace_id, name, description, ct, parent_channel_id],
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

    /// Check whether a member belongs to the given channel.
    ///
    /// # ID namespace invariant
    /// Human IDs use a `human_<uuid>` prefix; agent IDs are bare UUIDs. The
    /// two namespaces are disjoint, so filtering by `member_id` alone uniquely
    /// identifies the actor type and a `member_type` predicate is not required.
    pub fn channel_member_exists(&self, channel_id: &str, member_id: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let exists: i64 = conn.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM channel_members
                WHERE channel_id = ?1 AND member_id = ?2
            )",
            params![channel_id, member_id],
            |row| row.get(0),
        )?;
        Ok(exists != 0)
    }

    /// Channels that newly created agents should join automatically. User-created
    /// channels stay invite-only; only writable built-in system rooms such as
    /// `#all` are auto-joined.
    pub fn get_auto_join_channels(&self) -> Result<Vec<Channel>> {
        self.get_auto_join_channels_for_workspace(None)
    }

    pub fn get_auto_join_channels_for_workspace(
        &self,
        workspace_id: Option<&str>,
    ) -> Result<Vec<Channel>> {
        let conn = self.conn.lock().unwrap();
        let workspace_id = match workspace_id {
            Some(workspace_id) => workspace_id.to_string(),
            None => match Self::workspace_id_for_lookup_inner(&conn)? {
                Some(workspace_id) => workspace_id,
                None => return Ok(Vec::new()),
            },
        };
        let mut sql = "SELECT id, workspace_id, name, description, channel_type, created_at, parent_channel_id
             FROM channels
             WHERE archived = 0 AND channel_type = 'system'"
            .to_string();
        sql.push_str(" AND workspace_id = ?1");
        let values = vec![workspace_id];
        sql.push_str(" ORDER BY CASE WHEN name = 'all' THEN 0 ELSE 1 END, created_at");
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(values), Channel::from_row)?;
        Ok(rows.filter_map(|row| row.ok()).collect())
    }

    pub(crate) fn get_channel_by_workspace_and_name_inner(
        conn: &Connection,
        workspace_id: &str,
        name: &str,
    ) -> Result<Option<Channel>> {
        Ok(conn
            .query_row(
                "SELECT id, workspace_id, name, description, channel_type, created_at, parent_channel_id
                 FROM channels
                 WHERE workspace_id = ?1 AND name = ?2",
                params![workspace_id, name],
                Channel::from_row,
            )
            .optional()?)
    }

    pub fn get_channel_by_workspace_and_name(
        &self,
        workspace_id: &str,
        name: &str,
    ) -> Result<Option<Channel>> {
        let conn = self.conn.lock().unwrap();
        Self::get_channel_by_workspace_and_name_inner(&conn, workspace_id, name)
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

        // Collect any task sub-channels first so the attachment sweep below can
        // include attachments referenced only by sub-channel messages. Without
        // this, attachments uploaded inside a task sub-channel would leak rows
        // and files after the parent channel is deleted.
        let sub_channel_ids: Vec<String> = conn
            .prepare("SELECT id FROM channels WHERE parent_channel_id = ?1")?
            .query_map(params![channel_id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        // Build the `channel_id IN (?1, ?2, ...)` list used by both the
        // attachment sweep and the cascade deletes. Parent first, then subs.
        let doomed_ids: Vec<String> = std::iter::once(channel_id.to_string())
            .chain(sub_channel_ids.iter().cloned())
            .collect();
        let placeholders = (1..=doomed_ids.len())
            .map(|i| format!("?{}", i))
            .collect::<Vec<_>>()
            .join(",");
        let doomed_params = rusqlite::params_from_iter(doomed_ids.iter());

        let attachment_rows: Vec<(String, String)> = conn
            .prepare(&format!(
                "SELECT DISTINCT a.id, a.stored_path
                 FROM attachments a
                 JOIN message_attachments ma ON ma.attachment_id = a.id
                 JOIN messages m ON m.id = ma.message_id
                 WHERE m.channel_id IN ({})",
                placeholders
            ))?
            .query_map(doomed_params, |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;

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

        // Cascade clean-up into every task sub-channel (messages, memberships,
        // inbox state, then the channel row itself).
        for sub_id in &sub_channel_ids {
            conn.execute(
                "DELETE FROM inbox_read_state WHERE conversation_id = ?1",
                params![sub_id],
            )?;
            conn.execute(
                "DELETE FROM message_attachments WHERE message_id IN (SELECT id FROM messages WHERE channel_id = ?1)",
                params![sub_id],
            )?;
            conn.execute(
                "DELETE FROM messages WHERE channel_id = ?1",
                params![sub_id],
            )?;
            conn.execute(
                "DELETE FROM channel_members WHERE channel_id = ?1",
                params![sub_id],
            )?;
            conn.execute("DELETE FROM channels WHERE id = ?1", params![sub_id])?;
        }

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
        let Some(workspace_id) = Self::workspace_id_for_lookup_inner(conn)? else {
            return Ok(None);
        };
        Self::get_channel_by_workspace_and_name_inner(conn, &workspace_id, name)
    }

    pub fn get_channel_by_id(&self, id: &str) -> Result<Option<Channel>> {
        let conn = self.conn.lock().unwrap();
        Self::get_channel_by_id_inner(&conn, id)
    }

    pub(crate) fn get_channel_by_id_inner(conn: &Connection, id: &str) -> Result<Option<Channel>> {
        let mut stmt = conn.prepare(
            "SELECT id, workspace_id, name, description, channel_type, created_at, parent_channel_id FROM channels WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map(params![id], Channel::from_row)?;
        Ok(rows.next().transpose()?)
    }

    pub fn join_channel(
        &self,
        channel_name: &str,
        member_id: &str,
        member_type: SenderType,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let channel = Self::get_channel_by_name_inner(&conn, channel_name)?
            .ok_or_else(|| anyhow!("channel not found: {}", channel_name))?;
        let mt = member_type.as_str();
        let rows = conn.execute(
            "INSERT OR IGNORE INTO channel_members (channel_id, member_id, member_type, last_read_seq) VALUES (?1, ?2, ?3, 0)",
            params![channel.id, member_id, mt],
        )?;
        if rows > 0 {
            let event = super::StreamEvent::member_joined(
                channel.id,
                member_id.to_string(),
                mt.to_string(),
            );
            let _ = self.stream_tx.send(event);
        }
        Ok(())
    }

    /// Join a channel by stable id so API handlers do not have to resolve the
    /// mutable channel name before mutating membership.
    pub fn join_channel_by_id(
        &self,
        channel_id: &str,
        member_id: &str,
        member_type: SenderType,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let mt = member_type.as_str();
        let rows = conn.execute(
            "INSERT OR IGNORE INTO channel_members (channel_id, member_id, member_type, last_read_seq) VALUES (?1, ?2, ?3, 0)",
            params![channel_id, member_id, mt],
        )?;
        if rows > 0 {
            let event = super::StreamEvent::member_joined(
                channel_id.to_string(),
                member_id.to_string(),
                mt.to_string(),
            );
            let _ = self.stream_tx.send(event);
        }
        Ok(())
    }

    pub fn get_channel_members(&self, channel_id: &str) -> Result<Vec<ChannelMember>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT channel_id, member_id, member_type, last_read_seq FROM channel_members WHERE channel_id = ?1",
        )?;
        let rows = stmt.query_map(params![channel_id], |row| {
            Ok(ChannelMember {
                channel_id: row.get(0)?,
                member_id: row.get(1)?,
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
            "SELECT cm.member_id,
                    cm.member_type,
                    COALESCE(h.name, a.name, cm.member_id) AS member_name,
                    CASE
                        WHEN cm.member_type = 'agent' THEN a.display_name
                        WHEN cm.member_type = 'human' THEN h.name
                        ELSE NULL
                    END AS display_name
             FROM channel_members cm
             LEFT JOIN humans h ON cm.member_type = 'human' AND h.id = cm.member_id
             LEFT JOIN agents a ON cm.member_type = 'agent' AND a.id = cm.member_id
             WHERE cm.channel_id = ?1
             ORDER BY
                CASE cm.member_type
                    WHEN 'human' THEN 0
                    ELSE 1
                END,
                COALESCE(a.display_name, h.name, a.name, cm.member_id),
                cm.member_id",
        )?;
        let rows = stmt.query_map(params![channel_id], |row| {
            Ok(ChannelMemberProfile {
                member_id: row.get(0)?,
                member_type: SenderType::from_sender_type_str(&row.get::<_, String>(1)?),
                member_name: row.get(2)?,
                display_name: row.get(3)?,
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// Check whether a member belongs to the given channel (looked up by name).
    ///
    /// # ID namespace invariant
    /// Human IDs use a `human_<uuid>` prefix; agent IDs are bare UUIDs. The
    /// two namespaces are disjoint, so filtering by `member_id` alone uniquely
    /// identifies the actor type and a `member_type` predicate is not required.
    pub fn is_member(&self, channel_name: &str, member_id: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let channel = Self::get_channel_by_name_inner(&conn, channel_name)?;
        match channel {
            None => Ok(false),
            Some(ch) => {
                let count: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM channel_members WHERE channel_id = ?1 AND member_id = ?2",
                    params![ch.id, member_id],
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

    /// Ensure built-in channels exist and upgrade `#general` to the writable
    /// `#all` system channel without changing its stable id.
    pub fn ensure_builtin_channels(&self, default_human_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let workspace_id = Self::workspace_id_for_write_inner(&conn)?;
        let all_id = Self::ensure_all_channel_inner(&conn, &workspace_id)?;

        conn.execute(
            "INSERT OR IGNORE INTO channel_members (channel_id, member_id, member_type, last_read_seq)
             VALUES (?1, ?2, 'human', 0)",
            params![all_id, default_human_id],
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO workspace_members (workspace_id, human_id, role)
             SELECT ?1, id, 'member' FROM humans",
            params![workspace_id],
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO channel_members (channel_id, member_id, member_type, last_read_seq)
             SELECT ?1, human_id, 'human', 0 FROM workspace_members WHERE workspace_id = ?2",
            params![all_id, workspace_id],
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO channel_members (channel_id, member_id, member_type, last_read_seq)
             SELECT ?1, id, 'agent', 0 FROM agents WHERE workspace_id = ?2",
            params![all_id, workspace_id],
        )?;

        Ok(())
    }

    fn ensure_all_channel_inner(conn: &Connection, workspace_id: &str) -> Result<String> {
        if let Some(existing) = Self::get_channel_by_workspace_and_name_inner(
            conn,
            workspace_id,
            Self::DEFAULT_SYSTEM_CHANNEL,
        )? {
            conn.execute(
                "UPDATE channels
                 SET description = ?1, channel_type = 'system', archived = 0
                 WHERE id = ?2",
                params![Self::DEFAULT_SYSTEM_CHANNEL_DESCRIPTION, existing.id],
            )?;
            return Ok(existing.id);
        }

        if let Some(general_channel) =
            Self::get_channel_by_workspace_and_name_inner(conn, workspace_id, "general")?
        {
            conn.execute(
                "UPDATE channels
                 SET name = ?1, description = ?2, channel_type = 'system', archived = 0
                 WHERE id = ?3",
                params![
                    Self::DEFAULT_SYSTEM_CHANNEL,
                    Self::DEFAULT_SYSTEM_CHANNEL_DESCRIPTION,
                    general_channel.id
                ],
            )?;
            tracing::info!(workspace_id, "migrated scoped #general to workspace #all");
            return Ok(general_channel.id);
        }

        let id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO channels (id, workspace_id, name, description, channel_type)
             VALUES (?1, ?2, ?3, ?4, 'system')",
            params![
                id,
                workspace_id,
                Self::DEFAULT_SYSTEM_CHANNEL,
                Self::DEFAULT_SYSTEM_CHANNEL_DESCRIPTION
            ],
        )?;
        tracing::info!(
            channel = Self::DEFAULT_SYSTEM_CHANNEL,
            workspace_id = ?workspace_id,
            "created built-in system channel"
        );
        Ok(id)
    }

    fn ensure_system_channel_inner(conn: &Connection, name: &str, description: &str) -> Result<()> {
        let workspace_id = Self::workspace_id_for_write_inner(conn)?;
        let exists: i64 = conn.query_row(
            "SELECT COUNT(*) FROM channels WHERE workspace_id = ?1 AND name = ?2 AND channel_type = 'system'",
            params![workspace_id, name],
            |row| row.get(0),
        )?;
        if exists == 0 {
            let id = uuid::Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO channels (id, workspace_id, name, description, channel_type) VALUES (?1, ?2, ?3, ?4, 'system')",
                params![id, workspace_id, name, description],
            )?;
            tracing::info!(channel = %name, "created system channel");
        }
        Ok(())
    }
}

#[cfg(test)]
mod channel_name_tests {
    use super::*;

    #[test]
    fn is_valid_channel_name_accepts_letters_digits_hyphens_underscores() {
        assert!(is_valid_channel_name("general"));
        assert!(is_valid_channel_name("eng-team"));
        assert!(is_valid_channel_name("team_42"));
        assert!(is_valid_channel_name("a1-b2_c3"));
    }

    #[test]
    fn is_valid_channel_name_rejects_empty() {
        assert!(!is_valid_channel_name(""));
    }

    #[test]
    fn is_valid_channel_name_rejects_spaces_and_special_chars() {
        assert!(!is_valid_channel_name("space channel"));
        assert!(!is_valid_channel_name("channel/name"));
        assert!(!is_valid_channel_name("channel?"));
        assert!(!is_valid_channel_name("emoji🎉"));
        assert!(!is_valid_channel_name("UPPER"));
        assert!(!is_valid_channel_name("dot.channel"));
    }
}

#[cfg(test)]
mod task_channel_tests {
    use super::*;
    use crate::store::messages::SenderType;
    use crate::store::Store;

    #[test]
    fn channel_type_task_roundtrip() {
        assert_eq!(ChannelType::Task.as_api_str(), "task");
    }

    #[test]
    fn default_channel_list_excludes_task_sub_channels() {
        let store = Store::open(":memory:").unwrap();
        store
            .create_channel("eng", None, ChannelType::Channel, None)
            .unwrap();
        let alice = store.create_local_human("alice").unwrap();
        store
            .create_tasks("eng", &alice.id, SenderType::Human, &["t1"])
            .unwrap();

        let channels = store.get_channels().unwrap();
        assert!(channels.iter().any(|c| c.name == "eng"));
        assert!(
            !channels.iter().any(|c| c.channel_type == ChannelType::Task),
            "task sub-channels must not appear in default list"
        );

        // Opting in via `include_tasks` must surface task sub-channels so the
        // task-detail view can fetch them.
        let with_tasks = store
            .get_channels_by_params(&ChannelListParams {
                include_tasks: true,
                ..ChannelListParams::default()
            })
            .unwrap();
        assert!(
            with_tasks
                .iter()
                .any(|c| c.channel_type == ChannelType::Task),
            "opt-in via include_tasks must include task sub-channels"
        );
    }

    #[test]
    fn delete_channel_cascades_to_task_sub_children() {
        let store = Store::open(":memory:").unwrap();
        let parent_id = store
            .create_channel("eng", None, ChannelType::Channel, None)
            .unwrap();
        let alice = store.create_local_human("alice").unwrap();
        store
            .create_tasks("eng", &alice.id, SenderType::Human, &["t1", "t2"])
            .unwrap();

        // Verify parent + 2 task sub-channels exist before delete.
        {
            let conn = store.conn_for_test();
            let sub_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM channels WHERE parent_channel_id = ?1",
                    params![parent_id],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(sub_count, 2);
        }

        store.delete_channel(&parent_id).unwrap();

        // Parent, sub-channels, and tasks rows must all be gone.
        let conn = store.conn_for_test();
        let parent_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM channels WHERE id = ?1",
                params![parent_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(parent_exists, 0);

        let sub_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM channels WHERE parent_channel_id = ?1",
                params![parent_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            sub_count, 0,
            "task sub-channels must be deleted with parent"
        );

        let task_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM tasks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(task_count, 0, "task rows must be gone too");
    }

    #[test]
    fn delete_channel_cleans_up_sub_channel_attachments() {
        // Regression: before this fix, `delete_channel` only swept attachments
        // referenced by messages on the *parent* channel. Attachments
        // referenced only by sub-channel messages leaked into `attachments`
        // and on disk after delete.
        let store = Store::open(":memory:").unwrap();
        let parent_id = store
            .create_channel("eng", None, ChannelType::Channel, None)
            .unwrap();
        let alice = store.create_local_human("alice").unwrap();
        store
            .create_tasks("eng", &alice.id, SenderType::Human, &["t1"])
            .unwrap();

        let sub_id: String = {
            let conn = store.conn_for_test();
            conn.query_row(
                "SELECT sub_channel_id FROM tasks WHERE task_number = 1",
                [],
                |r| r.get(0),
            )
            .unwrap()
        };

        // Attach a fake attachment to a message in the sub-channel only (no
        // reference from any parent-channel message).
        {
            let conn = store.conn_for_test();
            conn.execute(
                "INSERT INTO attachments (id, filename, mime_type, size_bytes, stored_path) \
                 VALUES ('att-sub', 'note.txt', 'text/plain', 4, '/nonexistent/note.txt')",
                [],
            )
            .unwrap();
            let alice_id: String = conn
                .query_row("SELECT id FROM humans WHERE name = 'alice'", [], |row| {
                    row.get(0)
                })
                .unwrap();
            conn.execute(
                "INSERT INTO messages (id, channel_id, sender_id, sender_type, content, seq) \
                 VALUES ('m-sub', ?1, ?2, 'human', 'here', 1)",
                params![sub_id, alice_id],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO message_attachments (message_id, attachment_id) VALUES ('m-sub', 'att-sub')",
                [],
            )
            .unwrap();
        }

        store.delete_channel(&parent_id).unwrap();

        // Attachment row referenced only by a sub-channel message must be gone.
        let conn = store.conn_for_test();
        let remaining: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM attachments WHERE id = 'att-sub'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            remaining, 0,
            "attachment referenced only by sub-channel message must be swept"
        );
    }
}
