use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::params;
use serde::{Deserialize, Serialize};

use super::{parse_datetime, Store};

/// Registered human user (can post and own channels).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Human {
    /// Username (typically OS login) used as sender id.
    pub name: String,
    /// Optional user-chosen display name.
    pub display_name: Option<String>,
    /// When the human row was inserted.
    pub created_at: DateTime<Utc>,
}

impl Store {
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
            .prepare("SELECT name, display_name, created_at FROM humans ORDER BY name")?
            .query_map([], |row| {
                Ok(Human {
                    name: row.get(0)?,
                    display_name: row.get(1)?,
                    created_at: parse_datetime(&row.get::<_, String>(2)?),
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    /// Update the display name for a human user. Pass `None` to clear.
    pub fn update_human_display_name(
        &self,
        name: &str,
        display_name: Option<&str>,
    ) -> Result<Human> {
        let conn = self.conn.lock().unwrap();
        let updated = conn.execute(
            "UPDATE humans SET display_name = ?2 WHERE name = ?1",
            params![name, display_name],
        )?;
        if updated == 0 {
            anyhow::bail!("human not found: {name}");
        }
        let human = conn.query_row(
            "SELECT name, display_name, created_at FROM humans WHERE name = ?1",
            params![name],
            |row| {
                Ok(Human {
                    name: row.get(0)?,
                    display_name: row.get(1)?,
                    created_at: parse_datetime(&row.get::<_, String>(2)?),
                })
            },
        )?;
        Ok(human)
    }
}
