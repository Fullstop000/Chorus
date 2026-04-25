use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{parse_datetime, Store};

// ── Types owned by this module ──

/// Full agent row as loaded from `agents` (+ env vars from child table).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    /// UUID primary key.
    pub id: String,
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
    /// Full env var list to replace existing rows.
    pub env_vars: &'a [AgentEnvVar],
}

impl Store {
    pub fn create_agent_record(&self, record: &AgentRecordUpsert<'_>) -> Result<String> {
        let conn = self.conn.lock().unwrap();
        Self::create_agent_record_inner(&conn, None, record)
    }

    pub fn create_agent_record_in_workspace(
        &self,
        workspace_id: &str,
        record: &AgentRecordUpsert<'_>,
    ) -> Result<String> {
        let conn = self.conn.lock().unwrap();
        Self::create_agent_record_inner(&conn, Some(workspace_id), record)
    }

    fn create_agent_record_inner(
        conn: &rusqlite::Connection,
        workspace_id: Option<&str>,
        record: &AgentRecordUpsert<'_>,
    ) -> Result<String> {
        let id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO agents (id, workspace_id, name, display_name, description, system_prompt, runtime, model, reasoning_effort) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![id, workspace_id, record.name, record.display_name, record.description, record.system_prompt, record.runtime, record.model, record.reasoning_effort],
        )?;
        Self::replace_agent_env_vars_inner(conn, record.name, record.env_vars)?;
        let all_channel = match workspace_id {
            Some(workspace_id) => conn
                .query_row(
                    "SELECT id, name, description, channel_type, created_at, parent_channel_id
                     FROM channels
                     WHERE workspace_id = ?1 AND name = ?2",
                    params![workspace_id, Self::DEFAULT_SYSTEM_CHANNEL],
                    super::channels::Channel::from_row,
                )
                .ok(),
            None => Self::get_channel_by_name_inner(conn, Self::DEFAULT_SYSTEM_CHANNEL)?,
        };
        if let Some(all_channel) = all_channel {
            conn.execute(
                "INSERT OR IGNORE INTO channel_members (channel_id, member_name, member_type, last_read_seq)
                 VALUES (?1, ?2, 'agent', 0)",
                params![all_channel.id, record.name],
            )?;
        }
        Ok(id)
    }

    pub fn delete_agent_record(&self, name: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM channel_members WHERE member_name = ?1",
            params![name],
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
        let (sql, params) = match workspace_id {
            Some(workspace_id) => (
                "SELECT id, name, display_name, description, system_prompt, runtime, model, reasoning_effort, created_at FROM agents WHERE workspace_id = ?1 ORDER BY name",
                vec![workspace_id.to_string()],
            ),
            None => (
                "SELECT id, name, display_name, description, system_prompt, runtime, model, reasoning_effort, created_at FROM agents WHERE workspace_id IS NULL ORDER BY name",
                Vec::new(),
            ),
        };
        let rows = conn
            .prepare(sql)?
            .query_map(rusqlite::params_from_iter(params), Self::agent_from_row)?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    pub fn get_agent(&self, name: &str) -> Result<Option<Agent>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, display_name, description, system_prompt, runtime, model, reasoning_effort, created_at FROM agents WHERE name = ?1",
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
            "SELECT id, name, display_name, description, system_prompt, runtime, model, reasoning_effort, created_at FROM agents WHERE id = ?1",
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
            "UPDATE agents SET display_name = ?1, description = ?2, system_prompt = ?3, runtime = ?4, model = ?5, reasoning_effort = ?6 WHERE name = ?7",
            params![
                record.display_name,
                record.description,
                record.system_prompt,
                record.runtime,
                record.model,
                record.reasoning_effort,
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
        let created_at = row.get::<_, String>(8)?;
        Ok(Agent {
            id: row.get(0)?,
            name: row.get(1)?,
            display_name: row.get(2)?,
            description: row.get(3)?,
            system_prompt: row.get(4)?,
            runtime: row.get(5)?,
            model: row.get(6)?,
            reasoning_effort: row.get(7)?,
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
        let mut stmt =
            conn.prepare("SELECT DISTINCT channel_id FROM channel_members WHERE member_name = ?1")?;
        let ids = stmt
            .query_map(rusqlite::params![agent_name], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect();
        Ok(ids)
    }
}
