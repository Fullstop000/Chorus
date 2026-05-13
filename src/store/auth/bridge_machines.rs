//! Live + historical registry of bridge connections per user-scoped
//! token. One row per `(token_hash, machine_id)` pair.
//!
//! Two state columns make four observable states:
//!
//! | `disconnected_at` | `kicked_at` | Meaning                           |
//! | ----------------- | ----------- | --------------------------------- |
//! | NULL              | NULL        | Live WS connected.                |
//! | SET               | NULL        | Disconnected (network drop, etc.) |
//! | SET               | SET         | Kicked — reconnect rejected.      |
//! | NULL              | SET         | (Invalid; never written.)         |
//!
//! Forget = hard-delete the row; future reconnect re-creates it.

use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::store::{parse_datetime, Store};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BridgeMachine {
    pub token_hash: String,
    pub machine_id: String,
    pub hostname_hint: Option<String>,
    pub first_seen_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub disconnected_at: Option<DateTime<Utc>>,
    pub kicked_at: Option<DateTime<Utc>>,
}

impl BridgeMachine {
    pub fn is_kicked(&self) -> bool {
        self.kicked_at.is_some()
    }

    pub fn is_active(&self) -> bool {
        self.disconnected_at.is_none()
    }
}

/// Outcome of an attempted `bridge.hello` registration for a user-scoped
/// token. Returned by [`Store::register_bridge_machine_hello`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HelloOutcome {
    /// First-ever connection for this `(token, machine_id)`. Row was created.
    Inserted,
    /// Row existed and was offline; reactivated.
    Reconnected,
    /// Row existed and was already live; supersede semantics apply (the
    /// caller's WS layer should close the previous sender). Reflects the
    /// existing `bridge_registry` behavior.
    Superseded,
    /// Row was Kicked. Caller MUST close the new WS with 4004 and not let
    /// it run.
    Rejected,
}

impl Store {
    /// Apply the bridge_machines state machine for an arriving
    /// `bridge.hello` on a user-scoped token. Returns the row + an outcome
    /// describing what happened.
    pub fn register_bridge_machine_hello(
        &self,
        token_hash: &str,
        machine_id: &str,
        hostname_hint: Option<&str>,
    ) -> Result<(BridgeMachine, HelloOutcome)> {
        let conn = self.lock_conn();
        let existing = Self::get_bridge_machine_inner(&conn, token_hash, machine_id)?;
        match existing {
            None => {
                conn.execute(
                    "INSERT INTO bridge_machines (token_hash, machine_id, hostname_hint)
                     VALUES (?1, ?2, ?3)",
                    params![token_hash, machine_id, hostname_hint],
                )?;
                let row = Self::get_bridge_machine_inner(&conn, token_hash, machine_id)?
                    .ok_or_else(|| anyhow::anyhow!("bridge_machine vanished after insert"))?;
                Ok((row, HelloOutcome::Inserted))
            }
            Some(row) if row.is_kicked() => Ok((row, HelloOutcome::Rejected)),
            Some(row) => {
                // Clear `disconnected_at` (the row is live again), bump
                // `last_seen_at`, refresh `hostname_hint`.
                conn.execute(
                    "UPDATE bridge_machines
                       SET disconnected_at = NULL,
                           last_seen_at    = datetime('now'),
                           hostname_hint   = COALESCE(?3, hostname_hint)
                     WHERE token_hash = ?1 AND machine_id = ?2",
                    params![token_hash, machine_id, hostname_hint],
                )?;
                let refreshed = Self::get_bridge_machine_inner(&conn, token_hash, machine_id)?
                    .ok_or_else(|| anyhow::anyhow!("bridge_machine vanished after refresh"))?;
                let outcome = if row.is_active() {
                    HelloOutcome::Superseded
                } else {
                    HelloOutcome::Reconnected
                };
                Ok((refreshed, outcome))
            }
        }
    }

    /// Mark a machine as no-longer-connected (clean WS drop / TCP timeout).
    /// Idempotent. Does NOT set `kicked_at` — kick is an explicit operator
    /// action via [`Self::kick_bridge_machine`].
    pub fn mark_bridge_machine_disconnected(
        &self,
        token_hash: &str,
        machine_id: &str,
    ) -> Result<()> {
        let conn = self.lock_conn();
        conn.execute(
            "UPDATE bridge_machines
               SET disconnected_at = COALESCE(disconnected_at, datetime('now')),
                   last_seen_at    = datetime('now')
             WHERE token_hash = ?1 AND machine_id = ?2",
            params![token_hash, machine_id],
        )?;
        Ok(())
    }

    /// Operator clicked Kick. Sets both `disconnected_at` and `kicked_at`.
    /// The caller is also responsible for closing the live WS (if any)
    /// with close code 4004. Returns true iff a row was updated.
    pub fn kick_bridge_machine(&self, token_hash: &str, machine_id: &str) -> Result<bool> {
        let conn = self.lock_conn();
        let n = conn.execute(
            "UPDATE bridge_machines
               SET disconnected_at = COALESCE(disconnected_at, datetime('now')),
                   kicked_at       = COALESCE(kicked_at, datetime('now'))
             WHERE token_hash = ?1 AND machine_id = ?2",
            params![token_hash, machine_id],
        )?;
        Ok(n > 0)
    }

    /// Operator clicked Forget. Hard-deletes the row; a future reconnect
    /// will recreate it via `register_bridge_machine_hello`.
    pub fn forget_bridge_machine(&self, token_hash: &str, machine_id: &str) -> Result<bool> {
        let conn = self.lock_conn();
        let n = conn.execute(
            "DELETE FROM bridge_machines WHERE token_hash = ?1 AND machine_id = ?2",
            params![token_hash, machine_id],
        )?;
        Ok(n > 0)
    }

    /// All machines registered against a specific user-scoped token. Used
    /// by `GET /api/devices` and during Rotate-sweep.
    pub fn list_bridge_machines_for_token(&self, token_hash: &str) -> Result<Vec<BridgeMachine>> {
        let conn = self.lock_conn();
        let rows = conn
            .prepare(
                "SELECT token_hash, machine_id, hostname_hint, first_seen_at, last_seen_at,
                        disconnected_at, kicked_at
                   FROM bridge_machines
                  WHERE token_hash = ?1
               ORDER BY first_seen_at",
            )?
            .query_map(params![token_hash], Self::bridge_machine_from_row)?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    pub fn get_bridge_machine(
        &self,
        token_hash: &str,
        machine_id: &str,
    ) -> Result<Option<BridgeMachine>> {
        let conn = self.lock_conn();
        Self::get_bridge_machine_inner(&conn, token_hash, machine_id)
    }

    fn get_bridge_machine_inner(
        conn: &rusqlite::Connection,
        token_hash: &str,
        machine_id: &str,
    ) -> Result<Option<BridgeMachine>> {
        conn.query_row(
            "SELECT token_hash, machine_id, hostname_hint, first_seen_at, last_seen_at,
                    disconnected_at, kicked_at
               FROM bridge_machines
              WHERE token_hash = ?1 AND machine_id = ?2",
            params![token_hash, machine_id],
            Self::bridge_machine_from_row,
        )
        .optional()
        .map_err(Into::into)
    }

    fn bridge_machine_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<BridgeMachine> {
        let disc_raw: Option<String> = row.get(5)?;
        let kick_raw: Option<String> = row.get(6)?;
        Ok(BridgeMachine {
            token_hash: row.get(0)?,
            machine_id: row.get(1)?,
            hostname_hint: row.get(2)?,
            first_seen_at: parse_datetime(&row.get::<_, String>(3)?),
            last_seen_at: parse_datetime(&row.get::<_, String>(4)?),
            disconnected_at: disc_raw.as_deref().map(parse_datetime),
            kicked_at: kick_raw.as_deref().map(parse_datetime),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::auth::Account;

    fn store_with_user_bridge_token() -> (Store, Account, String) {
        let s = Store::open(":memory:").unwrap();
        let user = s.create_user("alice").unwrap();
        let acct = s.create_local_account(&user.id).unwrap();
        let minted = s.mint_user_bridge_token(&acct.id, Some("dev")).unwrap();
        (s, acct, minted.row.token_hash)
    }

    #[test]
    fn first_hello_inserts() {
        let (s, _, token_hash) = store_with_user_bridge_token();
        let (row, outcome) = s
            .register_bridge_machine_hello(&token_hash, "laptop", Some("laptop.local"))
            .unwrap();
        assert_eq!(outcome, HelloOutcome::Inserted);
        assert_eq!(row.machine_id, "laptop");
        assert_eq!(row.hostname_hint.as_deref(), Some("laptop.local"));
        assert!(row.is_active());
        assert!(!row.is_kicked());
    }

    #[test]
    fn second_hello_on_live_row_is_superseded() {
        let (s, _, token_hash) = store_with_user_bridge_token();
        s.register_bridge_machine_hello(&token_hash, "laptop", None)
            .unwrap();
        let (_, outcome) = s
            .register_bridge_machine_hello(&token_hash, "laptop", None)
            .unwrap();
        assert_eq!(outcome, HelloOutcome::Superseded);
    }

    #[test]
    fn reconnect_after_clean_drop_is_reconnected_not_superseded() {
        let (s, _, token_hash) = store_with_user_bridge_token();
        s.register_bridge_machine_hello(&token_hash, "laptop", None)
            .unwrap();
        s.mark_bridge_machine_disconnected(&token_hash, "laptop")
            .unwrap();
        let (row, outcome) = s
            .register_bridge_machine_hello(&token_hash, "laptop", None)
            .unwrap();
        assert_eq!(outcome, HelloOutcome::Reconnected);
        assert!(row.is_active());
    }

    #[test]
    fn kicked_row_rejects_reconnect() {
        let (s, _, token_hash) = store_with_user_bridge_token();
        s.register_bridge_machine_hello(&token_hash, "laptop", None)
            .unwrap();
        assert!(s.kick_bridge_machine(&token_hash, "laptop").unwrap());
        let (row, outcome) = s
            .register_bridge_machine_hello(&token_hash, "laptop", None)
            .unwrap();
        assert_eq!(outcome, HelloOutcome::Rejected);
        assert!(row.is_kicked());
    }

    #[test]
    fn forget_then_reconnect_yields_fresh_inserted_row() {
        let (s, _, token_hash) = store_with_user_bridge_token();
        s.register_bridge_machine_hello(&token_hash, "laptop", None)
            .unwrap();
        s.kick_bridge_machine(&token_hash, "laptop").unwrap();
        assert!(s.forget_bridge_machine(&token_hash, "laptop").unwrap());

        let (row, outcome) = s
            .register_bridge_machine_hello(&token_hash, "laptop", None)
            .unwrap();
        assert_eq!(outcome, HelloOutcome::Inserted);
        assert!(!row.is_kicked());
    }

    #[test]
    fn list_returns_all_machines_for_token() {
        let (s, _, token_hash) = store_with_user_bridge_token();
        s.register_bridge_machine_hello(&token_hash, "laptop", None)
            .unwrap();
        s.register_bridge_machine_hello(&token_hash, "homelab", None)
            .unwrap();
        let machines = s.list_bridge_machines_for_token(&token_hash).unwrap();
        assert_eq!(machines.len(), 2);
        let ids: Vec<&str> = machines.iter().map(|m| m.machine_id.as_str()).collect();
        assert!(ids.contains(&"laptop"));
        assert!(ids.contains(&"homelab"));
    }

    #[test]
    fn rows_cascade_when_parent_token_is_deleted() {
        let (s, _, token_hash) = store_with_user_bridge_token();
        s.register_bridge_machine_hello(&token_hash, "laptop", None)
            .unwrap();
        // Hard-delete the parent (we don't do this in product code — Rotate
        // sets revoked_at — but verify ON DELETE CASCADE for safety nets).
        s.conn_for_test()
            .execute(
                "DELETE FROM api_tokens WHERE token_hash = ?1",
                params![token_hash],
            )
            .unwrap();
        assert!(s
            .list_bridge_machines_for_token(&token_hash)
            .unwrap()
            .is_empty());
    }
}
