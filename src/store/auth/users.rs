use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::store::{parse_datetime, Store};

/// A person. The identity layer.
///
/// Every actor reference (`messages.sender_id` for sender_type='user',
/// `tasks.created_by_id` for type='user', `workspaces.created_by_user_id`,
/// etc.) points at a row in this table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
}

impl Store {
    /// Create a new User with a freshly minted `usr_<uuid>` id.
    pub fn create_user(&self, name: &str) -> Result<User> {
        let conn = self.lock_conn();
        Self::create_user_inner(&conn, name)
    }

    pub(crate) fn create_user_inner(conn: &Connection, name: &str) -> Result<User> {
        let id = format!("usr_{}", Uuid::new_v4());
        conn.execute(
            "INSERT INTO users (id, name) VALUES (?1, ?2)",
            params![id, name],
        )?;
        Self::get_user_by_id_inner(conn, &id)?
            .ok_or_else(|| anyhow::anyhow!("user not found after insert: {id}"))
    }

    pub fn get_user_by_id(&self, id: &str) -> Result<Option<User>> {
        let conn = self.lock_conn();
        Self::get_user_by_id_inner(&conn, id)
    }

    pub(crate) fn get_user_by_id_inner(conn: &Connection, id: &str) -> Result<Option<User>> {
        conn.query_row(
            "SELECT id, name, created_at FROM users WHERE id = ?1",
            params![id],
            Self::user_from_row,
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn list_users(&self) -> Result<Vec<User>> {
        let conn = self.lock_conn();
        let rows = conn
            .prepare("SELECT id, name, created_at FROM users ORDER BY name")?
            .query_map([], Self::user_from_row)?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    /// Update a User's display name. Returns the updated row, or `Ok(None)`
    /// if no row matched the id.
    pub fn update_user_name(&self, id: &str, name: &str) -> Result<Option<User>> {
        let conn = self.lock_conn();
        let affected = conn.execute(
            "UPDATE users SET name = ?2 WHERE id = ?1",
            params![id, name],
        )?;
        if affected == 0 {
            return Ok(None);
        }
        Self::get_user_by_id_inner(&conn, id)
    }

    fn user_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<User> {
        Ok(User {
            id: row.get(0)?,
            name: row.get(1)?,
            created_at: parse_datetime(&row.get::<_, String>(2)?),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        Store::open(":memory:").expect("in-memory store")
    }

    #[test]
    fn create_user_assigns_prefixed_id() {
        let s = store();
        let user = s.create_user("alice").unwrap();
        assert!(user.id.starts_with("usr_"), "expected usr_ prefix, got {}", user.id);
        assert_eq!(user.name, "alice");
    }

    #[test]
    fn get_user_by_id_returns_existing_row() {
        let s = store();
        let created = s.create_user("alice").unwrap();
        let fetched = s.get_user_by_id(&created.id).unwrap().unwrap();
        assert_eq!(fetched.id, created.id);
        assert_eq!(fetched.name, "alice");
    }

    #[test]
    fn get_user_by_id_returns_none_for_missing() {
        let s = store();
        assert!(s.get_user_by_id("usr_does_not_exist").unwrap().is_none());
    }

    #[test]
    fn list_users_orders_by_name() {
        let s = store();
        s.create_user("charlie").unwrap();
        s.create_user("alice").unwrap();
        s.create_user("bob").unwrap();
        let names: Vec<_> = s.list_users().unwrap().into_iter().map(|u| u.name).collect();
        assert_eq!(names, vec!["alice", "bob", "charlie"]);
    }

    #[test]
    fn update_user_name_persists() {
        let s = store();
        let user = s.create_user("alice").unwrap();
        let updated = s.update_user_name(&user.id, "alicia").unwrap().unwrap();
        assert_eq!(updated.name, "alicia");
        let fetched = s.get_user_by_id(&user.id).unwrap().unwrap();
        assert_eq!(fetched.name, "alicia");
    }

    #[test]
    fn update_user_name_returns_none_when_missing() {
        let s = store();
        let result = s.update_user_name("usr_missing", "x").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn names_are_not_unique() {
        // Cloud-era collaborators may share display names. The `users` table
        // does NOT enforce UNIQUE(name); uniqueness moves to (auth_provider,
        // email) on `accounts`.
        let s = store();
        let alice1 = s.create_user("alice").unwrap();
        let alice2 = s.create_user("alice").unwrap();
        assert_ne!(alice1.id, alice2.id);
    }
}
