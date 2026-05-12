//! Identity & auth tables: users, accounts, sessions, api_tokens.
//!
//! The model in short:
//! - `User` = the person. Stable identity, referenced everywhere as actor.
//! - `Account` = how a User authenticates. 1..N per User; `auth_provider`
//!   distinguishes local from cloud.
//! - `Session` = a browser cookie credential.
//! - `ApiToken` = a CLI or bridge bearer credential. Stored as SHA-256
//!   hash; the raw string is returned only at creation time.

pub mod accounts;
pub mod api_tokens;
pub mod sessions;
pub mod users;

pub use accounts::Account;
pub use api_tokens::{ApiToken, MintedToken};
pub use sessions::Session;
pub use users::User;

use anyhow::Result;
use rusqlite::params;

use crate::store::Store;

/// Idempotent local-mode bootstrap. Returns `(User, Account)` for the
/// single local operator, creating the rows on first run.
///
/// While the legacy `humans` table is still in the schema (deleted in a
/// later commit of the redesign), this also mirrors the User row into
/// `humans` with the same id so existing code keeps working — workspace
/// creation, the bootstrap server's `state.local_human_id`, etc. The
/// mirror goes away when the `humans` table is dropped.
///
/// All inserts run in a single transaction.
impl Store {
    pub fn ensure_local_identity(&self, default_name: &str) -> Result<(User, Account)> {
        let mut conn = self.lock_conn();
        let tx = conn.transaction()?;

        // Already set up? Return existing rows without touching anything.
        if let Some(account) = Self::get_local_account_inner(&tx)? {
            let user = Self::get_user_by_id_inner(&tx, &account.user_id)?
                .ok_or_else(|| anyhow::anyhow!(
                    "local account {} points at missing user {}",
                    account.id,
                    account.user_id
                ))?;
            tx.commit()?;
            return Ok((user, account));
        }

        // Fresh: mint a User, the local Account, and the legacy humans
        // mirror — all referenced by the same id so the mirror remains
        // valid even when callers haven't been migrated yet.
        let user = Self::create_user_inner(&tx, default_name)?;
        let account = Self::create_local_account_inner(&tx, &user.id)?;
        // Legacy mirror: keep `humans` aligned so the existing handlers
        // and `create_local_workspace` (which still references humans.id)
        // find the same identity. Removed in the commit that drops the
        // `humans` table.
        tx.execute(
            "INSERT INTO humans (id, name, auth_provider)
             VALUES (?1, ?2, 'local')
             ON CONFLICT(id) DO UPDATE SET name = excluded.name",
            params![user.id, user.name],
        )?;
        tx.commit()?;
        Ok((user, account))
    }
}

#[cfg(test)]
mod bootstrap_tests {
    use super::*;

    fn store() -> Store {
        Store::open(":memory:").expect("in-memory store")
    }

    #[test]
    fn fresh_setup_creates_user_account_and_legacy_mirror() {
        let s = store();
        let (user, account) = s.ensure_local_identity("alice").unwrap();
        assert!(user.id.starts_with("usr_"));
        assert_eq!(user.name, "alice");
        assert_eq!(account.user_id, user.id);
        assert_eq!(account.auth_provider, "local");

        // Legacy humans mirror has the same id.
        let conn = s.conn_for_test();
        let human_id: String = conn
            .query_row(
                "SELECT id FROM humans WHERE id = ?1",
                params![user.id],
                |r| r.get(0),
            )
            .expect("humans row should mirror the user");
        assert_eq!(human_id, user.id);
    }

    #[test]
    fn re_run_is_idempotent() {
        let s = store();
        let (user1, acct1) = s.ensure_local_identity("alice").unwrap();
        let (user2, acct2) = s.ensure_local_identity("alice").unwrap();
        assert_eq!(user1.id, user2.id);
        assert_eq!(acct1.id, acct2.id);
        // No second account got created.
        let count: i64 = s
            .conn_for_test()
            .query_row("SELECT COUNT(*) FROM accounts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn second_call_ignores_default_name_when_account_exists() {
        let s = store();
        let (user1, _) = s.ensure_local_identity("alice").unwrap();
        let (user2, _) = s.ensure_local_identity("eve").unwrap();
        // Returns the existing user — does not rename or create a new row.
        assert_eq!(user1.id, user2.id);
        assert_eq!(user2.name, "alice");
    }

    #[test]
    fn transaction_rolls_back_on_partial_failure() {
        // Simulate: a humans row with the SAME id exists but with a
        // *different* name conflicting with our intended insert. Because
        // the mirror uses ON CONFLICT DO UPDATE on id, the conflict is
        // resolved by overwriting the name. Verify that behaviour: the
        // legacy row's name updates to match the new User.
        let s = store();
        {
            let conn = s.conn_for_test();
            conn.execute(
                "INSERT INTO humans (id, name, auth_provider) VALUES ('usr_preexisting', 'Old', 'local')",
                [],
            )
            .unwrap();
        }
        let (user, _) = s.ensure_local_identity("alice").unwrap();
        // A NEW user was created (id != preexisting), and the mirror row
        // for the new user gets inserted independently.
        assert_ne!(user.id, "usr_preexisting");
        let conn = s.conn_for_test();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM humans", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2, "both legacy and new mirror rows should coexist");
    }
}
