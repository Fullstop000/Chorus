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
    /// Which subprocess driver to spawn (`claude`, `codex`, `kimi`, …).
    pub runtime: String,
    /// Model identifier for the driver.
    pub model: String,
    /// Optional Codex reasoning effort override.
    pub reasoning_effort: Option<String>,
    /// Injected environment variables (ordered by `position`).
    pub env_vars: Vec<AgentEnvVar>,
    /// Process / bridge lifecycle state.
    pub status: AgentStatus,
    /// Current bridge session id when connected.
    pub session_id: Option<String>,
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

/// Persisted agent process state (independent of in-memory activity strings).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    /// Bridge connected and agent runnable.
    Active,
    /// Connected but blocked in receive / idle wait.
    Sleeping,
    /// No running bridge process.
    Inactive,
}

impl AgentStatus {
    /// Value stored in `agents.status` and returned in API JSON.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Sleeping => "sleeping",
            Self::Inactive => "inactive",
        }
    }

    /// Parse DB / API status string; unknown values map to [`Inactive`].
    pub fn from_status_str(s: &str) -> Self {
        match s {
            "active" => Self::Active,
            "sleeping" => Self::Sleeping,
            _ => Self::Inactive,
        }
    }
}

/// Supported local agent runtimes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRuntime {
    Claude,
    Codex,
    Kimi,
    Opencode,
}

impl AgentRuntime {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Kimi => "kimi",
            Self::Opencode => "opencode",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            "kimi" => Some(Self::Kimi),
            "opencode" => Some(Self::Opencode),
            _ => None,
        }
    }

    pub const fn binary_name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Kimi => "kimi",
            Self::Opencode => "opencode",
        }
    }

    pub const fn acp_adaptor_binary(self) -> &'static str {
        match self {
            Self::Claude => "claude-agent-acp",
            Self::Codex => "codex-acp",
            // Kimi and OpenCode have native ACP support via subcommands,
            // so we check for the main binary itself.
            Self::Kimi => "kimi",
            Self::Opencode => "opencode",
        }
    }
}

/// Registered human user (can post and own channels).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Human {
    /// Username (typically OS login) used as sender id.
    pub name: String,
    /// When the human row was inserted.
    pub created_at: DateTime<Utc>,
}

/// Shared persisted agent configuration used by store create/update helpers.
pub struct AgentRecordUpsert<'a> {
    /// Agent handle (primary key for updates).
    pub name: &'a str,
    /// Display name column.
    pub display_name: &'a str,
    /// Optional description column.
    pub description: Option<&'a str>,
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
    pub fn create_agent_record(
        &self,
        name: &str,
        display_name: &str,
        description: Option<&str>,
        runtime: &str,
        model: &str,
        env_vars: &[AgentEnvVar],
    ) -> Result<String> {
        self.create_agent_record_with_reasoning(&AgentRecordUpsert {
            name,
            display_name,
            description,
            runtime,
            model,
            reasoning_effort: None,
            env_vars,
        })
    }

    pub fn create_agent_record_with_reasoning(
        &self,
        record: &AgentRecordUpsert<'_>,
    ) -> Result<String> {
        let conn = self.conn.lock().unwrap();
        let id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO agents (id, name, display_name, description, runtime, model, reasoning_effort) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                id,
                record.name,
                record.display_name,
                record.description,
                record.runtime,
                record.model,
                record.reasoning_effort
            ],
        )?;
        Self::replace_agent_env_vars_inner(&conn, record.name, record.env_vars)?;
        if let Some(all_channel) =
            Self::get_channel_by_name_inner(&conn, Self::DEFAULT_SYSTEM_CHANNEL)?
        {
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
        let conn = self.conn.lock().unwrap();
        let rows = conn
            .prepare(
                "SELECT id, name, display_name, description, runtime, model, reasoning_effort, status, session_id, created_at FROM agents ORDER BY name",
            )?
            .query_map([], |row| {
                Ok(Agent {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    display_name: row.get(2)?,
                    description: row.get(3)?,
                    runtime: row.get(4)?,
                    model: row.get(5)?,
                    reasoning_effort: row.get(6)?,
                    env_vars: Vec::new(),
                    status: AgentStatus::from_status_str(&row.get::<_, String>(7)?),
                    session_id: row.get(8)?,
                    created_at: parse_datetime(&row.get::<_, String>(9)?),
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    pub fn get_agent(&self, name: &str) -> Result<Option<Agent>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, display_name, description, runtime, model, reasoning_effort, status, session_id, created_at FROM agents WHERE name = ?1",
        )?;
        let mut rows = stmt.query_map(params![name], |row| {
            Ok(Agent {
                id: row.get(0)?,
                name: row.get(1)?,
                display_name: row.get(2)?,
                description: row.get(3)?,
                runtime: row.get(4)?,
                model: row.get(5)?,
                reasoning_effort: row.get(6)?,
                env_vars: Vec::new(),
                status: AgentStatus::from_status_str(&row.get::<_, String>(7)?),
                session_id: row.get(8)?,
                created_at: parse_datetime(&row.get::<_, String>(9)?),
            })
        })?;
        let mut agent = rows.next().transpose()?;
        if let Some(ref mut agent) = agent {
            agent.env_vars = Self::list_agent_env_vars_inner(&conn, &agent.name)?;
        }
        Ok(agent)
    }

    pub fn update_agent_record(
        &self,
        name: &str,
        display_name: &str,
        description: Option<&str>,
        runtime: &str,
        model: &str,
        env_vars: &[AgentEnvVar],
    ) -> Result<()> {
        self.update_agent_record_with_reasoning(&AgentRecordUpsert {
            name,
            display_name,
            description,
            runtime,
            model,
            reasoning_effort: None,
            env_vars,
        })
    }

    pub fn update_agent_record_with_reasoning(&self, record: &AgentRecordUpsert<'_>) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE agents SET display_name = ?1, description = ?2, runtime = ?3, model = ?4, reasoning_effort = ?5 WHERE name = ?6",
            params![
                record.display_name,
                record.description,
                record.runtime,
                record.model,
                record.reasoning_effort,
                record.name
            ],
        )?;
        Self::replace_agent_env_vars_inner(&conn, record.name, record.env_vars)?;
        Ok(())
    }

    pub fn update_agent_status(&self, name: &str, status: AgentStatus) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE agents SET status = ?1 WHERE name = ?2",
            params![status.as_str(), name],
        )?;
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

    pub fn update_agent_session(&self, name: &str, session_id: Option<&str>) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE agents SET session_id = ?1 WHERE name = ?2",
            params![session_id, name],
        )?;
        Ok(())
    }

    pub fn create_human(&self, name: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO humans (name) VALUES (?1)",
            params![name],
        )?;
        if let Some(all_channel) =
            Self::get_channel_by_name_inner(&conn, Self::DEFAULT_SYSTEM_CHANNEL)?
        {
            conn.execute(
                "INSERT OR IGNORE INTO channel_members (channel_id, member_name, member_type, last_read_seq)
                 VALUES (?1, ?2, 'human', 0)",
                params![all_channel.id, name],
            )?;
        }
        Ok(())
    }

    pub fn get_humans(&self) -> Result<Vec<Human>> {
        let conn = self.conn.lock().unwrap();
        let rows = conn
            .prepare("SELECT name, created_at FROM humans ORDER BY name")?
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
}
