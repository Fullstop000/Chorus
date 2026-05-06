use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{parse_datetime, Store};

// ── Types owned by this module ──

/// Full agent row as loaded from `agents` (+ env vars from child table).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    /// UUID primary key.
    pub id: String,
    /// Owning workspace id.
    pub workspace_id: String,
    /// Unique handle used in channels and APIs.
    pub name: String,
    /// Human-readable title in the UI.
    pub display_name: String,
    /// Optional longer description.
    pub description: Option<String>,
    /// Full system prompt for the LLM (rich template prompts go here).
    pub system_prompt: Option<String>,
    /// Which subprocess driver to spawn (`claude`, `codex`, `kimi`, …).
    pub runtime: String,
    /// Model identifier for the driver.
    pub model: String,
    /// Optional Codex reasoning effort override.
    pub reasoning_effort: Option<String>,
    /// Bridge ownership: which `machine_id` should run this agent.
    /// `None` means "any bridge may run it" (or platform-local).
    pub machine_id: Option<String>,
    /// Injected environment variables (ordered by `position`).
    pub env_vars: Vec<AgentEnvVar>,
    /// Row creation time.
    pub created_at: DateTime<Utc>,
}

/// One key/value pair stored for an agent, with stable ordering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentEnvVar {
    /// Variable name (non-empty).
    pub key: String,
    /// Variable value.
    pub value: String,
    /// Sort order when listing / injecting into the process env.
    pub position: i64,
}

/// Shared persisted agent configuration used by store create/update helpers.
pub struct AgentRecordUpsert<'a> {
    /// Agent handle (primary key for updates).
    pub name: &'a str,
    /// Display name column.
    pub display_name: &'a str,
    /// Optional description column.
    pub description: Option<&'a str>,
    /// Optional system prompt for the LLM.
    pub system_prompt: Option<&'a str>,
    /// Driver column.
    pub runtime: &'a str,
    /// Model column.
    pub model: &'a str,
    /// Optional reasoning effort (Codex).
    pub reasoning_effort: Option<&'a str>,
    /// Bridge ownership: which `machine_id` should run this agent.
    /// `None` = any bridge or platform-local.
    pub machine_id: Option<&'a str>,
    /// Full env var list to replace existing rows.
    pub env_vars: &'a [AgentEnvVar],
}

impl Store {
    pub fn create_agent_record(&self, record: &AgentRecordUpsert<'_>) -> Result<String> {
        let (id, _events) = self.create_agent_record_with_events(record)?;
        Ok(id)
    }

    pub fn create_agent_record_with_events(
        &self,
        record: &AgentRecordUpsert<'_>,
    ) -> Result<(String, Vec<crate::store::stream::StreamEvent>)> {
        let (workspace_id, id) = {
            let conn = self.conn.lock().unwrap();
            let workspace_id = Self::workspace_id_for_write_inner(&conn)?;
            let id = Self::create_agent_record_inner(&conn, &workspace_id, record)?;
            (workspace_id, id)
        };
        let mut events = Vec::new();
        if let Ok(Some(all_channel)) =
            self.get_channel_by_workspace_and_name(&workspace_id, Self::DEFAULT_SYSTEM_CHANNEL)
        {
            if let Ok((_, evs)) =
                self.join_channel_by_id(&all_channel.id, &id, super::SenderType::Agent)
            {
                events.extend(evs);
            }
        }
        Ok((id, events))
    }

    pub fn create_agent_record_in_workspace(
        &self,
        workspace_id: &str,
        record: &AgentRecordUpsert<'_>,
    ) -> Result<String> {
        let (id, _events) =
            self.create_agent_record_in_workspace_with_events(workspace_id, record)?;
        Ok(id)
    }

    pub fn create_agent_record_in_workspace_with_events(
        &self,
        workspace_id: &str,
        record: &AgentRecordUpsert<'_>,
    ) -> Result<(String, Vec<crate::store::stream::StreamEvent>)> {
        let id = {
            let conn = self.conn.lock().unwrap();
            Self::create_agent_record_inner(&conn, workspace_id, record)?
        };
        let mut events = Vec::new();
        if let Ok(Some(all_channel)) =
            self.get_channel_by_workspace_and_name(workspace_id, Self::DEFAULT_SYSTEM_CHANNEL)
        {
            if let Ok((_, evs)) =
                self.join_channel_by_id(&all_channel.id, &id, super::SenderType::Agent)
            {
                events.extend(evs);
            }
        }
        Ok((id, events))
    }

    fn create_agent_record_inner(
        conn: &rusqlite::Connection,
        workspace_id: &str,
        record: &AgentRecordUpsert<'_>,
    ) -> Result<String> {
        Self::create_agent_record_inner_with_id(
            conn,
            workspace_id,
            &Uuid::new_v4().to_string(),
            record,
        )
    }

    /// Insert an agent row with a caller-supplied id. Used by the bridge
    /// client so the local row's id matches the platform's `agent_id`,
    /// removing the need for a separate name↔platform_id translation cache.
    /// Production agent creation goes through [`create_agent_record_inner`]
    /// which mints a fresh UUID.
    fn create_agent_record_inner_with_id(
        conn: &rusqlite::Connection,
        workspace_id: &str,
        id: &str,
        record: &AgentRecordUpsert<'_>,
    ) -> Result<String> {
        conn.execute(
            "INSERT INTO agents (id, workspace_id, name, display_name, description, system_prompt, runtime, model, reasoning_effort, machine_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![id, workspace_id, record.name, record.display_name, record.description, record.system_prompt, record.runtime, record.model, record.reasoning_effort, record.machine_id],
        )?;
        Self::replace_agent_env_vars_inner(conn, record.name, record.env_vars)?;
        Ok(id.to_string())
    }

    /// Bridge entry point: insert (or replace) an agent row using the
    /// platform-supplied id. If a row with the same name already exists
    /// but a different id, it is deleted first — the bridge's local DB
    /// is a cache of the platform's view, so reusing the platform id keeps
    /// `agents(id)` semantically equal across processes.
    pub fn create_agent_record_with_id(
        &self,
        id: &str,
        record: &AgentRecordUpsert<'_>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let workspace_id = Self::workspace_id_for_write_inner(&conn)?;
        let existing_id: Option<String> = conn
            .query_row(
                "SELECT id FROM agents WHERE name = ?1",
                params![record.name],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(existing) = existing_id {
            if existing == id {
                // Same id, no-op insert — caller should be using update_agent_record.
                return Ok(());
            }
            // Stale local row from a previous reconcile. Drop it.
            conn.execute("DELETE FROM agents WHERE id = ?1", params![existing])?;
        }
        Self::create_agent_record_inner_with_id(&conn, &workspace_id, id, record)?;
        Ok(())
    }

    pub fn delete_agent_record(&self, name: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let agent_id: Option<String> = conn
            .query_row(
                "SELECT id FROM agents WHERE name = ?1",
                params![name],
                |row| row.get(0),
            )
            .optional()?;
        let Some(agent_id) = agent_id else {
            return Ok(());
        };
        conn.execute(
            "DELETE FROM channel_members WHERE member_id = ?1 AND member_type = 'agent'",
            params![agent_id],
        )?;
        conn.execute("DELETE FROM agents WHERE name = ?1", params![name])?;
        Ok(())
    }

    pub fn get_agents(&self) -> Result<Vec<Agent>> {
        self.get_agents_inner(None)
    }

    pub fn get_agents_in_workspace(&self, workspace_id: &str) -> Result<Vec<Agent>> {
        self.get_agents_inner(Some(workspace_id))
    }

    pub fn get_agents_for_workspace(&self, workspace_id: Option<&str>) -> Result<Vec<Agent>> {
        self.get_agents_inner(workspace_id)
    }

    fn get_agents_inner(&self, workspace_id: Option<&str>) -> Result<Vec<Agent>> {
        let conn = self.conn.lock().unwrap();
        let workspace_id = match workspace_id {
            Some(workspace_id) => workspace_id.to_string(),
            None => match Self::workspace_id_for_lookup_inner(&conn)? {
                Some(workspace_id) => workspace_id,
                None => return Ok(Vec::new()),
            },
        };
        let sql = "SELECT id, workspace_id, name, display_name, description, system_prompt, runtime, model, reasoning_effort, machine_id, created_at
                   FROM agents WHERE workspace_id = ?1 ORDER BY name";
        let rows = conn
            .prepare(sql)?
            .query_map(params![workspace_id], Self::agent_from_row)?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    pub fn get_agent(&self, name: &str) -> Result<Option<Agent>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, workspace_id, name, display_name, description, system_prompt, runtime, model, reasoning_effort, machine_id, created_at FROM agents WHERE name = ?1",
        )?;
        let mut rows = stmt.query_map(params![name], Self::agent_from_row)?;
        let mut agent = rows.next().transpose()?;
        if let Some(ref mut agent) = agent {
            Self::hydrate_agent_env_vars_inner(&conn, agent)?;
        }
        Ok(agent)
    }

    pub fn get_agent_by_id(&self, id: &str, hydrate_env: bool) -> Result<Option<Agent>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, workspace_id, name, display_name, description, system_prompt, runtime, model, reasoning_effort, machine_id, created_at FROM agents WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map(params![id], Self::agent_from_row)?;
        let mut agent = rows.next().transpose()?;
        if hydrate_env {
            if let Some(ref mut agent) = agent {
                Self::hydrate_agent_env_vars_inner(&conn, agent)?;
            }
        }
        Ok(agent)
    }

    pub fn update_agent_record(&self, record: &AgentRecordUpsert<'_>) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE agents SET display_name = ?1, description = ?2, system_prompt = ?3, runtime = ?4, model = ?5, reasoning_effort = ?6, machine_id = ?7 WHERE name = ?8",
            params![
                record.display_name,
                record.description,
                record.system_prompt,
                record.runtime,
                record.model,
                record.reasoning_effort,
                record.machine_id,
                record.name
            ],
        )?;
        Self::replace_agent_env_vars_inner(&conn, record.name, record.env_vars)?;
        Ok(())
    }

    pub fn get_agent_env_vars(&self, name: &str) -> Result<Vec<AgentEnvVar>> {
        let conn = self.conn.lock().unwrap();
        Self::list_agent_env_vars_inner(&conn, name)
    }

    fn list_agent_env_vars_inner(
        conn: &rusqlite::Connection,
        name: &str,
    ) -> Result<Vec<AgentEnvVar>> {
        let rows = conn
            .prepare(
                "SELECT key, value, position FROM agent_env_vars WHERE agent_name = ?1 ORDER BY position ASC, key ASC",
            )?
            .query_map(params![name], |row| {
                Ok(AgentEnvVar {
                    key: row.get(0)?,
                    value: row.get(1)?,
                    position: row.get(2)?,
                })
        })?
            .filter_map(|row| row.ok())
            .collect();
        Ok(rows)
    }

    fn hydrate_agent_env_vars_inner(conn: &rusqlite::Connection, agent: &mut Agent) -> Result<()> {
        agent.env_vars = Self::list_agent_env_vars_inner(conn, &agent.name)?;
        Ok(())
    }

    fn agent_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Agent> {
        let created_at = row.get::<_, String>(10)?;
        Ok(Agent {
            id: row.get(0)?,
            workspace_id: row.get(1)?,
            name: row.get(2)?,
            display_name: row.get(3)?,
            description: row.get(4)?,
            system_prompt: row.get(5)?,
            runtime: row.get(6)?,
            model: row.get(7)?,
            reasoning_effort: row.get(8)?,
            machine_id: row.get(9)?,
            env_vars: Vec::new(),
            created_at: parse_datetime(&created_at),
        })
    }

    fn replace_agent_env_vars_inner(
        conn: &rusqlite::Connection,
        name: &str,
        env_vars: &[AgentEnvVar],
    ) -> Result<()> {
        conn.execute(
            "DELETE FROM agent_env_vars WHERE agent_name = ?1",
            params![name],
        )?;
        for env_var in env_vars {
            conn.execute(
                "INSERT INTO agent_env_vars (agent_name, key, value, position) VALUES (?1, ?2, ?3, ?4)",
                params![name, env_var.key, env_var.value, env_var.position],
            )?;
        }
        Ok(())
    }

    /// Get all channel IDs where an agent is a member (includes DM channels).
    pub fn agent_channel_ids(&self, agent_name: &str) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let agent_id: Option<String> = conn
            .query_row(
                "SELECT id FROM agents WHERE name = ?1",
                params![agent_name],
                |row| row.get(0),
            )
            .optional()?;
        let Some(agent_id) = agent_id else {
            return Ok(Vec::new());
        };
        let mut stmt =
            conn.prepare("SELECT DISTINCT channel_id FROM channel_members WHERE member_id = ?1")?;
        let ids = stmt
            .query_map(rusqlite::params![agent_id], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect();
        Ok(ids)
    }
}
