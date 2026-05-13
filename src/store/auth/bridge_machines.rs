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

        // Cross-user takeover guard. Under dev-auth multi-user, two users
        // could pick the same hostname for their machines. Without this
        // check, user B's bridge claiming `machine_id="laptop"` would
        // get cross-access to user A's agents (which were tagged with
        // the same string). Reject if any ACTIVE bridge_machines row
        // exists for this machine_id under a DIFFERENT account.
        let conflict: Option<String> = conn
            .query_row(
                "SELECT bm.token_hash
                   FROM bridge_machines bm
                   JOIN api_tokens at_in ON at_in.token_hash = ?1
                   JOIN api_tokens at_ex ON at_ex.token_hash = bm.token_hash
                  WHERE bm.machine_id = ?2
                    AND bm.disconnected_at IS NULL
                    AND at_ex.account_id != at_in.account_id
                  LIMIT 1",
                params![token_hash, machine_id],
                |r| r.get::<_, String>(0),
            )
            .optional()?;
        if let Some(other_token) = conflict {
            // Synthesize a Rejected row so the caller's close-code path
            // fires. `kicked_at` doubles as the marker for "do not
            // accept" semantics without persisting anything new.
            let now = Utc::now();
            return Ok((
                BridgeMachine {
                    token_hash: other_token,
                    machine_id: machine_id.to_string(),
                    hostname_hint: hostname_hint.map(|s| s.to_string()),
                    first_seen_at: now,
                    last_seen_at: now,
                    disconnected_at: Some(now),
                    kicked_at: Some(now),
                },
                HelloOutcome::Rejected,
            ));
        }

        // Snapshot the pre-existing row (if any) so we can return the
        // right outcome AND short-circuit on Kick before mutating
        // anything.
        let prior = Self::get_bridge_machine_inner(&conn, token_hash, machine_id)?;
        if let Some(ref row) = prior {
            if row.is_kicked() {
                return Ok((row.clone(), HelloOutcome::Rejected));
            }
        }

        // Race-safe upsert. Two simultaneous hellos for the same
        // (token, machine_id) used to crash on the UNIQUE PK
        // constraint; now the second one updates the row instead.
        conn.execute(
            "INSERT INTO bridge_machines (token_hash, machine_id, hostname_hint)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(token_hash, machine_id) DO UPDATE
               SET disconnected_at = NULL,
                   last_seen_at    = datetime('now'),
                   hostname_hint   = COALESCE(excluded.hostname_hint, hostname_hint)",
            params![token_hash, machine_id, hostname_hint],
        )?;
        let row = Self::get_bridge_machine_inner(&conn, token_hash, machine_id)?
            .ok_or_else(|| anyhow::anyhow!("bridge_machine vanished after upsert"))?;
        let outcome = match prior {
            None => HelloOutcome::Inserted,
            Some(p) if p.is_active() => HelloOutcome::Superseded,
            Some(_) => HelloOutcome::Reconnected,
        };
        Ok((row, outcome))
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
    fn cross_user_machine_id_collision_is_rejected() {
        // Two users on one install pick the same hostname for their
        // bridges. User A onboards "laptop"; user B's bridge hello with
        // "laptop" must be REJECTED, not given cross-access to user A's
        // agents. See PR #159 Gemini review finding #1.
        let s = Store::open(":memory:").unwrap();
        let u_a = s.create_user("alice").unwrap();
        let acct_a = s.create_local_account(&u_a.id).unwrap();
        let token_a = s.mint_user_bridge_token(&acct_a.id, None).unwrap();
        let u_b = s.create_user("bob").unwrap();
        let acct_b = s
            .create_account(&u_b.id, "dev", Some("bob@dev.local"))
            .unwrap();
        let token_b = s.mint_user_bridge_token(&acct_b.id, None).unwrap();

        // Alice connects "laptop".
        let (_, outcome_a) = s
            .register_bridge_machine_hello(&token_a.row.token_hash, "laptop", None)
            .unwrap();
        assert_eq!(outcome_a, HelloOutcome::Inserted);

        // Bob attempts to claim the same machine_id.
        let (_, outcome_b) = s
            .register_bridge_machine_hello(&token_b.row.token_hash, "laptop", None)
            .unwrap();
        assert_eq!(
            outcome_b,
            HelloOutcome::Rejected,
            "Bob's hello with Alice's machine_id must be rejected"
        );
    }

    #[test]
    fn concurrent_hellos_for_same_pair_do_not_violate_unique_constraint() {
        // Race: two simultaneous bridge.hello calls for (token, machine).
        // Both observe no prior row; the upsert handles the second one
        // as a clean update instead of a SQLite UNIQUE constraint crash.
        let (s, _, token_hash) = store_with_user_bridge_token();
        let (_, o1) = s
            .register_bridge_machine_hello(&token_hash, "laptop", None)
            .unwrap();
        let (_, o2) = s
            .register_bridge_machine_hello(&token_hash, "laptop", None)
            .unwrap();
        assert_eq!(o1, HelloOutcome::Inserted);
        assert_eq!(o2, HelloOutcome::Superseded);
        // Idempotent third call.
        let (row, _) = s
            .register_bridge_machine_hello(&token_hash, "laptop", None)
            .unwrap();
        assert!(row.is_active());
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
