//! Browser session cookies. Distinct from agent sessions (which live in
//! `src/store/sessions.rs`). The cookie value is the row id directly —
//! it's an opaque random string the server hands out at login.

use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::store::{parse_datetime, Store};

/// One browser session, looked up by cookie.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub account_id: String,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

impl Store {
    /// Mint a session. `expires_at` = None → no expiry (D1=A; local mode).
    pub fn create_session(
        &self,
        account_id: &str,
        expires_at: Option<DateTime<Utc>>,
    ) -> Result<Session> {
        let conn = self.lock_conn();
        Self::create_session_inner(&conn, account_id, expires_at)
    }

    pub(crate) fn create_session_inner(
        conn: &Connection,
        account_id: &str,
        expires_at: Option<DateTime<Utc>>,
    ) -> Result<Session> {
        if Self::get_account_by_id_inner(conn, account_id)?.is_none() {
            return Err(anyhow::anyhow!("account not found: {account_id}"));
        }
        let id = format!("ses_{}", Uuid::new_v4());
        let expires_at_str = expires_at.map(|t| t.to_rfc3339());
        conn.execute(
            "INSERT INTO sessions (id, account_id, expires_at) VALUES (?1, ?2, ?3)",
            params![id, account_id, expires_at_str],
        )?;
        Self::get_session_by_id_inner(conn, &id)?
            .ok_or_else(|| anyhow::anyhow!("session not found after insert: {id}"))
    }

    /// Look up a session by cookie value. Returns `Ok(None)` if the session
    /// is missing, revoked, or expired. Bumps `last_seen_at` no more often
    /// than once per minute per session to avoid a SQLite write on every
    /// authenticated request (which would serialize all reads behind a
    /// write lock under load).
    pub fn touch_active_session(&self, id: &str) -> Result<Option<Session>> {
        let conn = self.lock_conn();
        let session = match Self::get_session_by_id_inner(&conn, id)? {
            Some(s) => s,
            None => return Ok(None),
        };
        if session.revoked_at.is_some() {
            return Ok(None);
        }
        if let Some(exp) = session.expires_at {
            if exp <= Utc::now() {
                return Ok(None);
            }
        }
        // Debounce: only write last_seen_at if the stored value is stale.
        let now = Utc::now();
        if (now - session.last_seen_at).num_seconds() >= 60 {
            conn.execute(
                "UPDATE sessions SET last_seen_at = datetime('now') WHERE id = ?1",
                params![id],
            )?;
        }
        Ok(Some(session))
    }

    pub fn revoke_session(&self, id: &str) -> Result<bool> {
        let conn = self.lock_conn();
        let affected = conn.execute(
            "UPDATE sessions SET revoked_at = datetime('now')
             WHERE id = ?1 AND revoked_at IS NULL",
            params![id],
        )?;
        Ok(affected > 0)
    }

    pub(crate) fn get_session_by_id_inner(conn: &Connection, id: &str) -> Result<Option<Session>> {
        conn.query_row(
            "SELECT id, account_id, created_at, last_seen_at, expires_at, revoked_at
             FROM sessions WHERE id = ?1",
            params![id],
            Self::session_from_row,
        )
        .optional()
        .map_err(Into::into)
    }

    fn session_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Session> {
        let expires_raw: Option<String> = row.get(4)?;
        let revoked_raw: Option<String> = row.get(5)?;
        Ok(Session {
            id: row.get(0)?,
            account_id: row.get(1)?,
            created_at: parse_datetime(&row.get::<_, String>(2)?),
            last_seen_at: parse_datetime(&row.get::<_, String>(3)?),
            expires_at: expires_raw.as_deref().map(parse_datetime),
            revoked_at: revoked_raw.as_deref().map(parse_datetime),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn store() -> Store {
        Store::open(":memory:").expect("in-memory store")
    }

    fn make_local_account(s: &Store) -> Account {
        let user = s.create_user("alice").unwrap();
        s.create_local_account(&user.id).unwrap()
    }

    use super::super::Account;

    #[test]
    fn create_session_assigns_prefixed_id() {
        let s = store();
        let acct = make_local_account(&s);
        let session = s.create_session(&acct.id, None).unwrap();
        assert!(session.id.starts_with("ses_"));
        assert_eq!(session.account_id, acct.id);
        assert!(session.expires_at.is_none());
        assert!(session.revoked_at.is_none());
    }

    #[test]
    fn create_session_rejects_missing_account() {
        let s = store();
        let err = s.create_session("acc_missing", None).unwrap_err();
        assert!(err.to_string().contains("account not found"), "got: {err}");
    }

    #[test]
    fn touch_active_session_returns_active() {
        let s = store();
        let acct = make_local_account(&s);
        let session = s.create_session(&acct.id, None).unwrap();
        let touched = s.touch_active_session(&session.id).unwrap().unwrap();
        assert_eq!(touched.id, session.id);
    }

    #[test]
    fn touch_active_session_returns_none_when_missing() {
        let s = store();
        assert!(s.touch_active_session("ses_missing").unwrap().is_none());
    }

    #[test]
    fn touch_active_session_returns_none_when_revoked() {
        let s = store();
        let acct = make_local_account(&s);
        let session = s.create_session(&acct.id, None).unwrap();
        assert!(s.revoke_session(&session.id).unwrap());
        assert!(s.touch_active_session(&session.id).unwrap().is_none());
    }

    #[test]
    fn touch_active_session_returns_none_when_expired() {
        let s = store();
        let acct = make_local_account(&s);
        let past = Utc::now() - Duration::seconds(60);
        let session = s.create_session(&acct.id, Some(past)).unwrap();
        assert!(s.touch_active_session(&session.id).unwrap().is_none());
    }

    #[test]
    fn revoke_session_is_idempotent() {
        let s = store();
        let acct = make_local_account(&s);
        let session = s.create_session(&acct.id, None).unwrap();
        assert!(s.revoke_session(&session.id).unwrap());
        // Second revoke returns false (already revoked).
        assert!(!s.revoke_session(&session.id).unwrap());
    }

    #[test]
    fn revoke_unknown_session_returns_false() {
        let s = store();
        assert!(!s.revoke_session("ses_missing").unwrap());
    }
}
