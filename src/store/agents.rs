use anyhow::Result;
use rusqlite::params;
use uuid::Uuid;

use crate::models::*;
use super::{Store, parse_datetime, parse_agent_status};

impl Store {
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

    pub fn delete_agent_record(&self, name: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM channel_members WHERE member_name = ?1", params![name])?;
        conn.execute("DELETE FROM agents WHERE name = ?1", params![name])?;
        Ok(())
    }

    pub fn list_agents(&self) -> Result<Vec<Agent>> {
        let conn = self.conn.lock().unwrap();
        let rows = conn
            .prepare(
                "SELECT id, name, display_name, description, runtime, model, status, session_id, created_at FROM agents ORDER BY name",
            )?
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
        conn.execute("UPDATE agents SET status = ?1 WHERE name = ?2", params![s, name])?;
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
        conn.execute("INSERT OR IGNORE INTO humans (name) VALUES (?1)", params![name])?;
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
