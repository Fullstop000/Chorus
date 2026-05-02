use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use rusqlite::types::Type;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::io;
use uuid::Uuid;

use super::{parse_datetime, Store};
use crate::store::stream::StreamEvent;
use crate::utils::slug::slugify_base;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceMode {
    LocalOnly,
    Cloud,
}

impl WorkspaceMode {
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::LocalOnly => "local_only",
            Self::Cloud => "cloud",
        }
    }

    fn from_db_str(value: &str) -> rusqlite::Result<Self> {
        match value {
            "local_only" => Ok(Self::LocalOnly),
            "cloud" => Ok(Self::Cloud),
            other => Err(rusqlite::Error::FromSqlConversionFailure(
                3,
                Type::Text,
                Box::new(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("unknown workspace mode: {other}"),
                )),
            )),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub mode: WorkspaceMode,
    pub created_by_human_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct WorkspaceCounts {
    pub channel_count: i64,
    pub agent_count: i64,
    pub human_count: i64,
}

impl Store {
    pub fn create_local_workspace(&self, name: &str, owner_human_id: &str) -> Result<(Workspace, StreamEvent)> {
        self.create_local_workspace_inner(name, owner_human_id, true)
    }

    pub fn create_local_workspace_without_activation(
        &self,
        name: &str,
        owner_human_id: &str,
    ) -> Result<(Workspace, StreamEvent)> {
        self.create_local_workspace_inner(name, owner_human_id, false)
    }

    fn create_local_workspace_inner(
        &self,
        name: &str,
        owner_human_id: &str,
        activate: bool,
    ) -> Result<(Workspace, StreamEvent)> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let id = Uuid::new_v4().to_string();
        let slug = Self::unique_workspace_slug_inner(&tx, name)?;
        if Self::get_human_by_id_inner(&tx, owner_human_id)?.is_none() {
            return Err(anyhow!("human not found: {owner_human_id}"));
        }
        tx.execute(
            "INSERT INTO workspaces (id, name, slug, mode, created_by_human_id)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                id,
                name,
                slug,
                WorkspaceMode::LocalOnly.as_db_str(),
                owner_human_id
            ],
        )?;
        tx.execute(
            "INSERT INTO workspace_members (workspace_id, human_id, role)
             VALUES (?1, ?2, 'owner')",
            params![id, owner_human_id],
        )?;
        let all_channel_id = Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO channels (id, workspace_id, name, description, channel_type)
             VALUES (?1, ?2, ?3, ?4, 'system')",
            params![
                all_channel_id,
                id,
                Store::DEFAULT_SYSTEM_CHANNEL,
                Store::DEFAULT_SYSTEM_CHANNEL_DESCRIPTION
            ],
        )?;
        tx.execute(
            "INSERT INTO channel_members (channel_id, member_id, member_type, last_read_seq)
             VALUES (?1, ?2, 'human', 0)",
            params![all_channel_id, owner_human_id],
        )?;
        if activate {
            tx.execute(
                "INSERT INTO local_workspace_state (key, workspace_id)
                 VALUES ('active_workspace_id', ?1)
                 ON CONFLICT(key) DO UPDATE SET workspace_id = excluded.workspace_id",
                params![id],
            )?;
        }

        let workspace = Self::get_workspace_by_id_inner(&tx, &id)?
            .ok_or_else(|| anyhow!("workspace not found after insert: {id}"))?;
        tx.commit()?;
        let event = super::StreamEvent::member_joined(
            all_channel_id,
            owner_human_id.to_string(),
            "human".to_string(),
        );
        Ok((workspace, event))
    }

    pub fn get_active_workspace(&self) -> Result<Option<Workspace>> {
        let conn = self.conn.lock().unwrap();
        let workspace_id = Self::active_workspace_id_inner(&conn)?;
        match workspace_id {
            Some(id) => Self::get_workspace_by_id_inner(&conn, &id),
            None => Ok(None),
        }
    }

    pub(crate) fn active_workspace_id_inner(conn: &rusqlite::Connection) -> Result<Option<String>> {
        conn.query_row(
            "SELECT workspace_id FROM local_workspace_state WHERE key = 'active_workspace_id'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(Into::into)
    }

    pub(crate) fn workspace_id_for_lookup_inner(
        conn: &rusqlite::Connection,
    ) -> Result<Option<String>> {
        if let Some(workspace_id) = Self::active_workspace_id_inner(conn)? {
            return Ok(Some(workspace_id));
        }
        conn.query_row(
            "SELECT id FROM workspaces ORDER BY created_at, name LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(Into::into)
    }

    pub(crate) fn workspace_id_for_write_inner(conn: &rusqlite::Connection) -> Result<String> {
        if let Some(workspace_id) = Self::active_workspace_id_inner(conn)? {
            return Ok(workspace_id);
        }

        let existing_count: i64 =
            conn.query_row("SELECT COUNT(*) FROM workspaces", [], |row| row.get(0))?;
        if existing_count > 0 {
            return Err(anyhow!(
                "no active workspace; run `chorus setup` or `chorus workspace switch <name>`"
            ));
        }

        let id = Uuid::new_v4().to_string();
        let slug = Self::unique_workspace_slug_inner(conn, "Chorus Local")?;

        conn.execute(
            "INSERT INTO workspaces (id, name, slug, mode, created_by_human_id)
             VALUES (?1, 'Chorus Local', ?2, ?3, NULL)",
            params![id, slug, WorkspaceMode::LocalOnly.as_db_str()],
        )?;
        conn.execute(
            "INSERT INTO local_workspace_state (key, workspace_id)
             VALUES ('active_workspace_id', ?1)",
            params![id],
        )?;
        Ok(id)
    }

    pub fn set_active_workspace(&self, workspace_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        if Self::get_workspace_by_id_inner(&conn, workspace_id)?.is_none() {
            return Err(anyhow!("workspace not found: {workspace_id}"));
        }
        conn.execute(
            "INSERT INTO local_workspace_state (key, workspace_id)
             VALUES ('active_workspace_id', ?1)
             ON CONFLICT(key) DO UPDATE SET workspace_id = excluded.workspace_id",
            params![workspace_id],
        )?;
        Ok(())
    }

    pub fn get_workspace_by_selector(&self, selector: &str) -> Result<Option<Workspace>> {
        let selector = selector.trim();
        if selector.is_empty() {
            return Err(anyhow!("workspace selector cannot be empty"));
        }

        let conn = self.conn.lock().unwrap();
        if let Some(workspace) = conn
            .query_row(
                "SELECT id, name, slug, mode, created_by_human_id, created_at
                 FROM workspaces
                 WHERE id = ?1 OR slug = ?1",
                params![selector],
                Self::workspace_from_row,
            )
            .optional()?
        {
            return Ok(Some(workspace));
        }

        let rows = conn
            .prepare(
                "SELECT id, name, slug, mode, created_by_human_id, created_at
                 FROM workspaces
                 WHERE name = ?1
                 ORDER BY created_at, slug",
            )?
            .query_map(params![selector], Self::workspace_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        match rows.len() {
            0 => Ok(None),
            1 => Ok(rows.into_iter().next()),
            _ => Err(anyhow!(
                "workspace selector is ambiguous: {selector}; use the workspace slug"
            )),
        }
    }

    pub fn rename_workspace(&self, workspace_id: &str, new_name: &str) -> Result<Workspace> {
        if slugify_base(new_name).is_none() {
            return Err(anyhow!("workspace name has no slug"));
        }
        let conn = self.conn.lock().unwrap();
        let updated = conn.execute(
            "UPDATE workspaces SET name = ?1 WHERE id = ?2",
            params![new_name, workspace_id],
        )?;
        if updated == 0 {
            return Err(anyhow!("workspace not found: {workspace_id}"));
        }
        Self::get_workspace_by_id_inner(&conn, workspace_id)?
            .ok_or_else(|| anyhow!("workspace not found after rename: {workspace_id}"))
    }

    pub fn list_workspaces_for_human(&self, human_id: &str) -> Result<Vec<Workspace>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT w.id, w.name, w.slug, w.mode, w.created_by_human_id, w.created_at
             FROM workspaces w
             JOIN workspace_members wm ON wm.workspace_id = w.id
             WHERE wm.human_id = ?1
             ORDER BY w.created_at, w.name",
        )?;
        let rows = stmt.query_map(params![human_id], Self::workspace_from_row)?;
        let workspaces: rusqlite::Result<Vec<_>> = rows.collect();
        Ok(workspaces?)
    }

    pub fn list_workspaces(&self) -> Result<Vec<Workspace>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, slug, mode, created_by_human_id, created_at
             FROM workspaces
             ORDER BY created_at, name",
        )?;
        let rows = stmt.query_map([], Self::workspace_from_row)?;
        let workspaces: rusqlite::Result<Vec<_>> = rows.collect();
        Ok(workspaces?)
    }

    pub fn count_workspace_resources(&self, workspace_id: &str) -> Result<WorkspaceCounts> {
        let conn = self.conn.lock().unwrap();
        let channel_count = conn.query_row(
            "SELECT COUNT(*) FROM channels WHERE workspace_id = ?1",
            params![workspace_id],
            |row| row.get(0),
        )?;
        let agent_count = conn.query_row(
            "SELECT COUNT(*) FROM agents WHERE workspace_id = ?1",
            params![workspace_id],
            |row| row.get(0),
        )?;
        let human_count = conn.query_row(
            "SELECT COUNT(*) FROM workspace_members WHERE workspace_id = ?1",
            params![workspace_id],
            |row| row.get(0),
        )?;
        Ok(WorkspaceCounts {
            channel_count,
            agent_count,
            human_count,
        })
    }

    pub fn delete_workspace(&self, workspace_id: &str) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let exists = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM workspaces WHERE id = ?1)",
                params![workspace_id],
                |row| row.get::<_, i64>(0),
            )
            .map(|count| count != 0)?;
        if !exists {
            return Err(anyhow!("workspace not found: {workspace_id}"));
        }

        let attachment_rows: Vec<(String, String)> = tx
            .prepare(
                "WITH RECURSIVE doomed_channels(id) AS (
                    SELECT id FROM channels WHERE workspace_id = ?1
                    UNION
                    SELECT c.id FROM channels c
                    JOIN doomed_channels d ON c.parent_channel_id = d.id
                 )
                 SELECT DISTINCT a.id, a.stored_path
                 FROM attachments a
                 JOIN message_attachments ma ON ma.attachment_id = a.id
                 JOIN messages m ON m.id = ma.message_id
                 JOIN doomed_channels d ON d.id = m.channel_id",
            )?
            .query_map(params![workspace_id], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        tx.execute(
            "WITH RECURSIVE doomed_channels(id) AS (
                SELECT id FROM channels WHERE workspace_id = ?1
                UNION
                SELECT c.id FROM channels c
                JOIN doomed_channels d ON c.parent_channel_id = d.id
             )
             DELETE FROM trace_events
             WHERE run_id IN (
                SELECT DISTINCT m.run_id
                FROM messages m
                JOIN doomed_channels d ON d.id = m.channel_id
                WHERE m.run_id IS NOT NULL
             )",
            params![workspace_id],
        )?;
        tx.execute(
            "WITH RECURSIVE doomed_channels(id) AS (
                SELECT id FROM channels WHERE workspace_id = ?1
                UNION
                SELECT c.id FROM channels c
                JOIN doomed_channels d ON c.parent_channel_id = d.id
             )
             DELETE FROM inbox_read_state
             WHERE conversation_id IN (SELECT id FROM doomed_channels)",
            params![workspace_id],
        )?;
        tx.execute(
            "WITH RECURSIVE doomed_channels(id) AS (
                SELECT id FROM channels WHERE workspace_id = ?1
                UNION
                SELECT c.id FROM channels c
                JOIN doomed_channels d ON c.parent_channel_id = d.id
             )
             DELETE FROM message_attachments
             WHERE message_id IN (
                SELECT m.id
                FROM messages m
                JOIN doomed_channels d ON d.id = m.channel_id
             )",
            params![workspace_id],
        )?;
        tx.execute(
            "WITH RECURSIVE doomed_channels(id) AS (
                SELECT id FROM channels WHERE workspace_id = ?1
                UNION
                SELECT c.id FROM channels c
                JOIN doomed_channels d ON c.parent_channel_id = d.id
             )
             DELETE FROM messages
             WHERE channel_id IN (SELECT id FROM doomed_channels)",
            params![workspace_id],
        )?;
        tx.execute(
            "WITH RECURSIVE doomed_channels(id) AS (
                SELECT id FROM channels WHERE workspace_id = ?1
                UNION
                SELECT c.id FROM channels c
                JOIN doomed_channels d ON c.parent_channel_id = d.id
             )
             DELETE FROM tasks
             WHERE channel_id IN (SELECT id FROM doomed_channels)
                OR sub_channel_id IN (SELECT id FROM doomed_channels)",
            params![workspace_id],
        )?;
        tx.execute(
            "WITH RECURSIVE doomed_channels(id) AS (
                SELECT id FROM channels WHERE workspace_id = ?1
                UNION
                SELECT c.id FROM channels c
                JOIN doomed_channels d ON c.parent_channel_id = d.id
             )
             DELETE FROM channel_members
             WHERE channel_id IN (SELECT id FROM doomed_channels)",
            params![workspace_id],
        )?;

        tx.execute(
            "DELETE FROM agent_sessions
             WHERE agent_id IN (SELECT id FROM agents WHERE workspace_id = ?1)",
            params![workspace_id],
        )?;
        tx.execute(
            "DELETE FROM agent_env_vars
             WHERE agent_name IN (SELECT name FROM agents WHERE workspace_id = ?1)",
            params![workspace_id],
        )?;
        tx.execute(
            "DELETE FROM team_members
             WHERE team_id IN (SELECT id FROM teams WHERE workspace_id = ?1)",
            params![workspace_id],
        )?;
        tx.execute(
            "DELETE FROM agents WHERE workspace_id = ?1",
            params![workspace_id],
        )?;
        tx.execute(
            "DELETE FROM teams WHERE workspace_id = ?1",
            params![workspace_id],
        )?;
        tx.execute(
            "WITH RECURSIVE doomed_channels(id) AS (
                SELECT id FROM channels WHERE workspace_id = ?1
                UNION
                SELECT c.id FROM channels c
                JOIN doomed_channels d ON c.parent_channel_id = d.id
             )
             DELETE FROM channels
             WHERE id IN (SELECT id FROM doomed_channels)",
            params![workspace_id],
        )?;

        let mut files_to_remove = Vec::new();
        for (attachment_id, stored_path) in attachment_rows {
            let refs_remaining: i64 = tx.query_row(
                "SELECT COUNT(*) FROM message_attachments WHERE attachment_id = ?1",
                params![attachment_id],
                |row| row.get(0),
            )?;
            if refs_remaining == 0 {
                tx.execute(
                    "DELETE FROM attachments WHERE id = ?1",
                    params![attachment_id],
                )?;
                files_to_remove.push(stored_path);
            }
        }

        tx.execute(
            "DELETE FROM local_workspace_state WHERE workspace_id = ?1",
            params![workspace_id],
        )?;
        tx.execute(
            "DELETE FROM workspace_members WHERE workspace_id = ?1",
            params![workspace_id],
        )?;
        tx.execute(
            "DELETE FROM workspaces WHERE id = ?1",
            params![workspace_id],
        )?;
        tx.commit()?;

        for path in files_to_remove {
            let _ = std::fs::remove_file(path);
        }
        Ok(())
    }

    fn get_workspace_by_id_inner(
        conn: &rusqlite::Connection,
        id: &str,
    ) -> Result<Option<Workspace>> {
        conn.query_row(
            "SELECT id, name, slug, mode, created_by_human_id, created_at
             FROM workspaces
             WHERE id = ?1",
            params![id],
            Self::workspace_from_row,
        )
        .optional()
        .map_err(Into::into)
    }

    fn workspace_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Workspace> {
        let mode: String = row.get(3)?;
        let created_at: String = row.get(5)?;
        Ok(Workspace {
            id: row.get(0)?,
            name: row.get(1)?,
            slug: row.get(2)?,
            mode: WorkspaceMode::from_db_str(&mode)?,
            created_by_human_id: row.get(4)?,
            created_at: parse_datetime(&created_at),
        })
    }

    fn unique_workspace_slug_inner(conn: &rusqlite::Connection, name: &str) -> Result<String> {
        let base = slugify_base(name).ok_or_else(|| anyhow!("workspace name has no slug"))?;
        for suffix in 0.. {
            let candidate = if suffix == 0 {
                base.clone()
            } else {
                format!("{base}-{suffix}")
            };
            let exists: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM workspaces WHERE slug = ?1)",
                    params![candidate],
                    |row| row.get::<_, i64>(0),
                )
                .map(|count| count != 0)?;
            if !exists {
                return Ok(candidate);
            }
        }
        unreachable!("unbounded suffix search should return")
    }
}
