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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store() -> Store {
        Store::open(":memory:").expect("in-memory store")
    }

    #[test]
    fn create_human_has_no_display_name() {
        let store = test_store();
        store.create_human("alice").unwrap();
        let humans = store.get_humans().unwrap();
        assert_eq!(humans.len(), 1);
        assert_eq!(humans[0].name, "alice");
        assert!(humans[0].display_name.is_none());
    }

    #[test]
    fn set_display_name() {
        let store = test_store();
        store.create_human("bob").unwrap();
        let updated = store
            .update_human_display_name("bob", Some("Bob Smith"))
            .unwrap();
        assert_eq!(updated.display_name.as_deref(), Some("Bob Smith"));
        let humans = store.get_humans().unwrap();
        assert_eq!(humans[0].display_name.as_deref(), Some("Bob Smith"));
    }

    #[test]
    fn clear_display_name() {
        let store = test_store();
        store.create_human("carol").unwrap();
        store
            .update_human_display_name("carol", Some("Carol"))
            .unwrap();
        let updated = store.update_human_display_name("carol", None).unwrap();
        assert!(updated.display_name.is_none());
    }

    #[test]
    fn update_unknown_human_errors() {
        let store = test_store();
        let result = store.update_human_display_name("nobody", Some("Ghost"));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("human not found"));
    }
}
