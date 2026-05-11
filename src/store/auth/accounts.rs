use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::store::{parse_datetime, Store};

/// One way a `User` proves who they are.
///
/// `auth_provider` ∈ {`local`, `google`, `github`, ...}. The local-mode
/// invariant: exactly one row per install with `auth_provider='local'` and
/// `email IS NULL`. Enforced by `idx_accounts_local_unique` in schema.sql.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub user_id: String,
    pub auth_provider: String,
    pub email: Option<String>,
    pub disabled_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

impl Store {
    /// Create the singleton local account for a User. Fails if one already
    /// exists (the UNIQUE partial index enforces this).
    pub fn create_local_account(&self, user_id: &str) -> Result<Account> {
        let conn = self.lock_conn();
        Self::create_local_account_inner(&conn, user_id)
    }

    pub(crate) fn create_local_account_inner(conn: &Connection, user_id: &str) -> Result<Account> {
        if Self::get_user_by_id_inner(conn, user_id)?.is_none() {
            return Err(anyhow!("user not found: {user_id}"));
        }
        let id = format!("acc_{}", Uuid::new_v4());
        conn.execute(
            "INSERT INTO accounts (id, user_id, auth_provider, email) VALUES (?1, ?2, 'local', NULL)",
            params![id, user_id],
        )?;
        Self::get_account_by_id_inner(conn, &id)?
            .ok_or_else(|| anyhow!("account not found after insert: {id}"))
    }

    /// Generic account create for non-local providers (cloud).
    pub fn create_account(
        &self,
        user_id: &str,
        auth_provider: &str,
        email: Option<&str>,
    ) -> Result<Account> {
        let conn = self.lock_conn();
        if Self::get_user_by_id_inner(&conn, user_id)?.is_none() {
            return Err(anyhow!("user not found: {user_id}"));
        }
        if auth_provider == "local" {
            return Err(anyhow!(
                "use create_local_account for auth_provider='local'"
            ));
        }
        let id = format!("acc_{}", Uuid::new_v4());
        conn.execute(
            "INSERT INTO accounts (id, user_id, auth_provider, email) VALUES (?1, ?2, ?3, ?4)",
            params![id, user_id, auth_provider, email],
        )?;
        Self::get_account_by_id_inner(&conn, &id)?
            .ok_or_else(|| anyhow!("account not found after insert: {id}"))
    }

    pub fn get_account_by_id(&self, id: &str) -> Result<Option<Account>> {
        let conn = self.lock_conn();
        Self::get_account_by_id_inner(&conn, id)
    }

    pub(crate) fn get_account_by_id_inner(
        conn: &Connection,
        id: &str,
    ) -> Result<Option<Account>> {
        conn.query_row(
            "SELECT id, user_id, auth_provider, email, disabled_at, created_at
             FROM accounts WHERE id = ?1",
            params![id],
            Self::account_from_row,
        )
        .optional()
        .map_err(Into::into)
    }

    /// Returns the singleton local Account (auth_provider='local'), or None
    /// if setup hasn't run yet.
    pub fn get_local_account(&self) -> Result<Option<Account>> {
        let conn = self.lock_conn();
        Self::get_local_account_inner(&conn)
    }

    pub(crate) fn get_local_account_inner(conn: &Connection) -> Result<Option<Account>> {
        conn.query_row(
            "SELECT id, user_id, auth_provider, email, disabled_at, created_at
             FROM accounts WHERE auth_provider = 'local' AND email IS NULL",
            [],
            Self::account_from_row,
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn list_accounts_for_user(&self, user_id: &str) -> Result<Vec<Account>> {
        let conn = self.lock_conn();
        let rows = conn
            .prepare(
                "SELECT id, user_id, auth_provider, email, disabled_at, created_at
                 FROM accounts WHERE user_id = ?1 ORDER BY created_at",
            )?
            .query_map(params![user_id], Self::account_from_row)?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    fn account_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Account> {
        let disabled_at_raw: Option<String> = row.get(4)?;
        Ok(Account {
            id: row.get(0)?,
            user_id: row.get(1)?,
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

    fn store() -> Store {
        Store::open(":memory:").expect("in-memory store")
    }

    #[test]
    fn create_local_account_binds_to_user() {
        let s = store();
        let user = s.create_user("alice").unwrap();
        let acct = s.create_local_account(&user.id).unwrap();
        assert!(acct.id.starts_with("acc_"));
        assert_eq!(acct.user_id, user.id);
        assert_eq!(acct.auth_provider, "local");
        assert!(acct.email.is_none());
    }

    #[test]
    fn create_local_account_rejects_missing_user() {
        let s = store();
        let err = s.create_local_account("usr_missing").unwrap_err();
        assert!(err.to_string().contains("user not found"), "got: {err}");
    }

    #[test]
    fn second_local_account_is_rejected_by_unique_index() {
        let s = store();
        let user = s.create_user("alice").unwrap();
        s.create_local_account(&user.id).unwrap();
        // Even for a different user, only one local account is allowed
        // per install.
        let user2 = s.create_user("bob").unwrap();
        let err = s.create_local_account(&user2.id).unwrap_err();
        let msg = err.to_string().to_lowercase();
        assert!(
            msg.contains("unique") || msg.contains("constraint"),
            "expected unique-constraint error, got: {err}"
        );
    }

    #[test]
    fn get_local_account_returns_the_singleton() {
        let s = store();
        let user = s.create_user("alice").unwrap();
        let created = s.create_local_account(&user.id).unwrap();
        let fetched = s.get_local_account().unwrap().unwrap();
        assert_eq!(fetched.id, created.id);
    }

    #[test]
    fn get_local_account_none_before_setup() {
        let s = store();
        assert!(s.get_local_account().unwrap().is_none());
    }

    #[test]
    fn create_account_rejects_local_provider() {
        let s = store();
        let user = s.create_user("alice").unwrap();
        let err = s
            .create_account(&user.id, "local", Some("a@b.com"))
            .unwrap_err();
        assert!(err.to_string().contains("create_local_account"), "got: {err}");
    }

    #[test]
    fn create_account_with_cloud_provider() {
        let s = store();
        let user = s.create_user("alice").unwrap();
        let acct = s
            .create_account(&user.id, "google", Some("alice@example.com"))
            .unwrap();
        assert_eq!(acct.auth_provider, "google");
        assert_eq!(acct.email.as_deref(), Some("alice@example.com"));
    }

    #[test]
    fn unique_constraint_on_provider_email() {
        let s = store();
        let user = s.create_user("alice").unwrap();
        s.create_account(&user.id, "google", Some("alice@example.com"))
            .unwrap();
        let err = s
            .create_account(&user.id, "google", Some("alice@example.com"))
            .unwrap_err();
        let msg = err.to_string().to_lowercase();
        assert!(
            msg.contains("unique") || msg.contains("constraint"),
            "expected unique-constraint error, got: {err}"
        );
    }

    #[test]
    fn cascading_delete_when_user_removed() {
        let s = store();
        let user = s.create_user("alice").unwrap();
        let acct = s.create_local_account(&user.id).unwrap();
        // Delete the user; account should cascade.
        {
            let conn = s.conn_for_test();
            conn.execute("DELETE FROM users WHERE id = ?1", params![user.id])
                .unwrap();
        }
        assert!(s.get_account_by_id(&acct.id).unwrap().is_none());
    }
}
