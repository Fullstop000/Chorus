use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use rusqlite::types::Type;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::io;
use uuid::Uuid;

use super::{parse_datetime, Store};
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
    pub created_by_human: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl Store {
    pub fn create_local_workspace(&self, name: &str, owner_human: &str) -> Result<Workspace> {
        let conn = self.conn.lock().unwrap();
        let id = Uuid::new_v4().to_string();
        let slug = slugify_base(name).ok_or_else(|| anyhow!("workspace name has no slug"))?;

        conn.execute(
            "INSERT OR IGNORE INTO humans (name, display_name) VALUES (?1, ?1)",
            params![owner_human],
        )?;
        conn.execute(
            "INSERT INTO workspaces (id, name, slug, mode, created_by_human)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                id,
                name,
                slug,
                WorkspaceMode::LocalOnly.as_db_str(),
                owner_human
            ],
        )?;
        conn.execute(
            "INSERT INTO workspace_members (workspace_id, human_name, role)
             VALUES (?1, ?2, 'owner')",
            params![id, owner_human],
        )?;
        conn.execute(
            "INSERT INTO local_workspace_state (key, workspace_id)
             VALUES ('active_workspace_id', ?1)
             ON CONFLICT(key) DO UPDATE SET workspace_id = excluded.workspace_id",
            params![id],
        )?;

        Self::get_workspace_by_id_inner(&conn, &id)?
            .ok_or_else(|| anyhow!("workspace not found after insert: {id}"))
    }

    pub fn get_active_workspace(&self) -> Result<Option<Workspace>> {
        let conn = self.conn.lock().unwrap();
        let workspace_id: Option<String> = conn
            .query_row(
                "SELECT workspace_id FROM local_workspace_state WHERE key = 'active_workspace_id'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        match workspace_id {
            Some(id) => Self::get_workspace_by_id_inner(&conn, &id),
            None => Ok(None),
        }
    }

    pub fn list_workspaces_for_human(&self, human_name: &str) -> Result<Vec<Workspace>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT w.id, w.name, w.slug, w.mode, w.created_by_human, w.created_at
             FROM workspaces w
             JOIN workspace_members wm ON wm.workspace_id = w.id
             WHERE wm.human_name = ?1
             ORDER BY w.created_at, w.name",
        )?;
        let rows = stmt.query_map(params![human_name], Self::workspace_from_row)?;
        let workspaces: rusqlite::Result<Vec<_>> = rows.collect();
        Ok(workspaces?)
    }

    fn get_workspace_by_id_inner(
        conn: &rusqlite::Connection,
        id: &str,
    ) -> Result<Option<Workspace>> {
        conn.query_row(
            "SELECT id, name, slug, mode, created_by_human, created_at
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
            created_by_human: row.get(4)?,
            created_at: parse_datetime(&created_at),
        })
    }
}
