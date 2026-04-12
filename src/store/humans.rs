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
