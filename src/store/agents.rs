use anyhow::Result;
use rusqlite::params;
use uuid::Uuid;

use super::{parse_agent_status, parse_datetime, Store};
use crate::models::*;

/// Shared persisted agent configuration used by store create/update helpers.
pub struct AgentRecordUpsert<'a> {
    pub name: &'a str,
    pub display_name: &'a str,
    pub description: Option<&'a str>,
    pub runtime: &'a str,
    pub model: &'a str,
    pub reasoning_effort: Option<&'a str>,
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

    pub fn list_agents(&self) -> Result<Vec<Agent>> {
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
                    status: parse_agent_status(&row.get::<_, String>(7)?),
                    session_id: row.get(8)?,
                    created_at: parse_datetime(&row.get::<_, String>(9)?),
                })
            })?
            .filter_map(|r| r.ok())
            .map(|mut agent: Agent| {
                agent.env_vars = Self::list_agent_env_vars_inner(&conn, &agent.name).unwrap_or_default();
                agent
            })
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
                status: parse_agent_status(&row.get::<_, String>(7)?),
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

    pub fn list_agent_env_vars(&self, name: &str) -> Result<Vec<AgentEnvVar>> {
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

    pub fn add_human(&self, name: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO humans (name) VALUES (?1)",
            params![name],
        )?;
        Ok(())
    }

    pub fn list_humans(&self) -> Result<Vec<Human>> {
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
}
