use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{parse_datetime, Store};

/// Registered human user. `id` is identity; `name` is the display/lookup label.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Human {
    pub id: String,
    pub name: String,
    pub auth_provider: String,
    pub email: Option<String>,
    pub disabled_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

impl Store {
    pub fn create_local_human(&self, name: &str) -> Result<Human> {
        let conn = self.conn.lock().unwrap();
        let id = format!("human_{}", Uuid::new_v4());
        Self::ensure_human_with_id_inner(&conn, &id, name)
    }

    pub fn ensure_human_with_id(&self, id: &str, name: &str) -> Result<Human> {
        let conn = self.conn.lock().unwrap();
        Self::ensure_human_with_id_inner(&conn, id, name)
    }

    pub(crate) fn ensure_human_with_id_inner(
        conn: &Connection,
        id: &str,
        name: &str,
    ) -> Result<Human> {
        conn.execute(
            "INSERT INTO humans (id, name, auth_provider)
             VALUES (?1, ?2, 'local')
             ON CONFLICT(id) DO UPDATE SET name = excluded.name",
            params![id, name],
        )?;
        Self::get_human_by_id_inner(conn, id)?
            .ok_or_else(|| anyhow::anyhow!("human not found after ensure: {id}"))
    }

    /// Compatibility wrapper for transitional call sites. New identity-aware
    /// code should keep the returned `Human` from `create_local_human` or
    /// `ensure_human_with_id` and store its id.
    pub fn create_human(&self, name: &str) -> Result<()> {
        self.create_local_human(name).map(|_| ())
    }

    pub fn get_human_by_id(&self, id: &str) -> Result<Option<Human>> {
        let conn = self.conn.lock().unwrap();
        Self::get_human_by_id_inner(&conn, id)
    }

    pub(crate) fn get_human_by_id_inner(conn: &Connection, id: &str) -> Result<Option<Human>> {
        conn.query_row(
            "SELECT id, name, auth_provider, email, disabled_at, created_at
             FROM humans WHERE id = ?1",
            params![id],
            Self::human_from_row,
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn get_human_by_name(&self, name: &str) -> Result<Option<Human>> {
        let conn = self.conn.lock().unwrap();
        Self::get_human_by_name_inner(&conn, name)
    }

    pub(crate) fn get_human_by_name_inner(conn: &Connection, name: &str) -> Result<Option<Human>> {
        conn.query_row(
            "SELECT id, name, auth_provider, email, disabled_at, created_at
             FROM humans WHERE name = ?1",
            params![name],
            Self::human_from_row,
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn get_humans(&self) -> Result<Vec<Human>> {
        let conn = self.conn.lock().unwrap();
        let rows = conn
            .prepare(
                "SELECT id, name, auth_provider, email, disabled_at, created_at
                 FROM humans ORDER BY name",
            )?
            .query_map([], Self::human_from_row)?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    fn human_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Human> {
        let disabled_at_raw: Option<String> = row.get(4)?;
        Ok(Human {
            id: row.get(0)?,
            name: row.get(1)?,
            auth_provider: row.get(2)?,
            email: row.get(3)?,
            disabled_at: disabled_at_raw.as_deref().map(parse_datetime),
            created_at: parse_datetime(&row.get::<_, String>(5)?),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store() -> Store {
        Store::open(":memory:").expect("in-memory store")
    }

    #[test]
    fn create_human_has_stable_id() {
        let store = test_store();
        store.ensure_human_with_id("human_alice", "alice").unwrap();
        let humans = store.get_humans().unwrap();
        assert_eq!(humans.len(), 1);
        assert_eq!(humans[0].id, "human_alice");
        assert_eq!(humans[0].name, "alice");
        assert_eq!(humans[0].auth_provider, "local");
    }

    #[test]
    fn ensure_human_with_id_updates_label_only() {
        let store = test_store();
        store.ensure_human_with_id("human_carol", "carol").unwrap();
        let updated = store
            .ensure_human_with_id("human_carol", "caroline")
            .unwrap();
        assert_eq!(updated.id, "human_carol");
        assert_eq!(updated.name, "caroline");
    }

    #[test]
    fn get_human_by_name_is_lookup_only() {
        let store = test_store();
        store.ensure_human_with_id("human_dana", "dana").unwrap();
        let human = store.get_human_by_name("dana").unwrap().unwrap();
        assert_eq!(human.id, "human_dana");
    }
}
