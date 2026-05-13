//! Bearer-token credentials for CLI and bridge clients.
//!
//! Raw token format (D2=A): `chrs_<provider>_<base64url(32 random bytes)>`.
//! Storage is the SHA-256 of the raw bytes — the raw is never persisted,
//! only handed back to the caller from `mint_token`. On every request,
//! the middleware hashes the incoming header and looks up the row.

use anyhow::Result;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{DateTime, Utc};
use rand::RngCore;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::store::{parse_datetime, Store};

/// Row in `api_tokens`. Never carries the raw token.
///
/// Three valid `(provider, machine_id)` shapes:
///   `("local",  None)`   — CLI bearer; acts as its `account_id`'s User.
///   `("bridge", Some(_))` — Legacy per-machine bridge token; restricted
///                           to agents whose `agents.machine_id` matches.
///   `("bridge", None)`   — User-scoped bridge token (one per user,
///                           shared across that user's machines). Each
///                           connection's machine_id comes from
///                           `bridge.hello`; see `bridge_machines`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiToken {
    pub token_hash: String,
    pub account_id: String,
    pub provider: String,
    pub machine_id: Option<String>,
    pub label: Option<String>,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

/// Returned from `mint_token`. The raw is shown to the caller once; after
/// this the only way to revoke it is by its hash (which is the PK).
#[derive(Debug, Clone)]
pub struct MintedToken {
    pub raw: String,
    pub row: ApiToken,
}

const RAW_LEN_BYTES: usize = 32;

/// Hash the raw token string for storage and lookup. SHA-256, hex-encoded.
pub fn hash_token(raw: &str) -> String {
    let mut h = Sha256::new();
    h.update(raw.as_bytes());
    format!("{:x}", h.finalize())
}

/// Build a fresh raw token with the project prefix. `provider` is folded
/// into the prefix so a leaked token's provenance is obvious in shell
/// history / logs without revealing the secret part.
pub fn generate_raw_token(provider: &str) -> String {
    let mut bytes = [0u8; RAW_LEN_BYTES];
    rand::rng().fill_bytes(&mut bytes);
    let encoded = URL_SAFE_NO_PAD.encode(bytes);
    format!("chrs_{provider}_{encoded}")
}

impl Store {
    /// Mint a CLI token bound to an Account (`machine_id = NULL`).
    /// Returns the raw token (caller must persist it — we don't keep it)
    /// plus the persisted row.
    pub fn mint_token(
        &self,
        account_id: &str,
        provider: &str,
        label: Option<&str>,
    ) -> Result<MintedToken> {
        let conn = self.lock_conn();
        Self::mint_token_inner(&conn, account_id, provider, None, label)
    }

    /// Mint a legacy bridge token bound to an Account *and* a `machine_id`.
    /// The auth layer enforces that the token only acts on agents whose
    /// `agents.machine_id` matches. Kept for pre-PRD installs and the
    /// `chorus setup --yes` local-install path.
    pub fn mint_bridge_token(
        &self,
        account_id: &str,
        machine_id: &str,
        label: Option<&str>,
    ) -> Result<MintedToken> {
        let conn = self.lock_conn();
        Self::mint_token_inner(&conn, account_id, "bridge", Some(machine_id), label)
    }

    /// Mint a user-scoped bridge token (`provider='bridge', machine_id=NULL`).
    /// One per user; the machine_id of each live connection comes from
    /// `bridge.hello` and is tracked in `bridge_machines`.
    pub fn mint_user_bridge_token(
        &self,
        account_id: &str,
        label: Option<&str>,
    ) -> Result<MintedToken> {
        let conn = self.lock_conn();
        Self::mint_token_inner(&conn, account_id, "bridge", None, label)
    }

    pub(crate) fn mint_token_inner(
        conn: &Connection,
        account_id: &str,
        provider: &str,
        machine_id: Option<&str>,
        label: Option<&str>,
    ) -> Result<MintedToken> {
        if Self::get_account_by_id_inner(conn, account_id)?.is_none() {
            return Err(anyhow::anyhow!("account not found: {account_id}"));
        }
        let raw = generate_raw_token(provider);
        let token_hash = hash_token(&raw);
        conn.execute(
            "INSERT INTO api_tokens (token_hash, account_id, provider, machine_id, label)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![token_hash, account_id, provider, machine_id, label],
        )?;
        let row = Self::get_token_by_hash_inner(conn, &token_hash)?
            .ok_or_else(|| anyhow::anyhow!("token not found after insert"))?;
        Ok(MintedToken { raw, row })
    }

    /// Look up an active token by the raw value the caller sent. Returns
    /// `Ok(None)` if the token is unknown or revoked. Bumps
    /// `last_used_at` at most once per minute per token so middleware
    /// doesn't acquire a write lock on every authenticated request.
    pub fn touch_active_token(&self, raw: &str) -> Result<Option<ApiToken>> {
        let conn = self.lock_conn();
        let token_hash = hash_token(raw);
        let row = match Self::get_token_by_hash_inner(&conn, &token_hash)? {
            Some(r) => r,
            None => return Ok(None),
        };
        if row.revoked_at.is_some() {
            return Ok(None);
        }
        let now = Utc::now();
        let stale = row
            .last_used_at
            .map(|t| (now - t).num_seconds() >= 60)
            .unwrap_or(true);
        if stale {
            conn.execute(
                "UPDATE api_tokens SET last_used_at = datetime('now') WHERE token_hash = ?1",
                params![token_hash],
            )?;
        }
        Ok(Some(row))
    }

    /// Revoke a token by its raw value (for `chorus logout`). Returns true
    /// iff the row existed and was not already revoked.
    pub fn revoke_token_by_raw(&self, raw: &str) -> Result<bool> {
        let conn = self.lock_conn();
        let token_hash = hash_token(raw);
        let affected = conn.execute(
            "UPDATE api_tokens SET revoked_at = datetime('now')
             WHERE token_hash = ?1 AND revoked_at IS NULL",
            params![token_hash],
        )?;
        Ok(affected > 0)
    }

    /// Revoke a token by its hash (the device-rotate path can't recover
    /// the raw — only the hash row is reachable). Idempotent.
    pub fn revoke_token_by_hash(&self, token_hash: &str) -> Result<bool> {
        let conn = self.lock_conn();
        let affected = conn.execute(
            "UPDATE api_tokens SET revoked_at = datetime('now')
             WHERE token_hash = ?1 AND revoked_at IS NULL",
            params![token_hash],
        )?;
        Ok(affected > 0)
    }

    /// Public lookup by SHA-256 hash. Returns the row without any
    /// validity checks (caller decides what's acceptable).
    pub fn get_token_by_hash(&self, token_hash: &str) -> Result<Option<ApiToken>> {
        let conn = self.lock_conn();
        Self::get_token_by_hash_inner(&conn, token_hash)
    }

    /// Cheap "has any bridge token ever been minted on this install?"
    /// probe. Used by `bridge_auth` to flip between passthrough mode
    /// (no tokens ever) and enforcing mode (tokens exist — possibly all
    /// revoked, which is a deliberate lock-down rather than an invite
    /// for unauthenticated access).
    pub fn has_any_bridge_token(&self) -> Result<bool> {
        let conn = self.lock_conn();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM api_tokens WHERE provider = 'bridge'",
            [],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    pub fn list_tokens_for_account(&self, account_id: &str) -> Result<Vec<ApiToken>> {
        let conn = self.lock_conn();
        let rows = conn
            .prepare(
                "SELECT token_hash, account_id, provider, machine_id, label, created_at, last_used_at, revoked_at
                 FROM api_tokens WHERE account_id = ?1 ORDER BY created_at",
            )?
            .query_map(params![account_id], Self::token_from_row)?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    /// Find the active user-scoped bridge token for an account, if any.
    /// Used by the device-onboarding mint route: returns `None` for "OK,
    /// mint a fresh one"; `Some(_)` for "already minted, force Rotate."
    pub fn find_active_user_bridge_token(
        &self,
        account_id: &str,
    ) -> Result<Option<ApiToken>> {
        let conn = self.lock_conn();
        conn.query_row(
            "SELECT token_hash, account_id, provider, machine_id, label, created_at, last_used_at, revoked_at
             FROM api_tokens
             WHERE account_id = ?1
               AND provider = 'bridge'
               AND machine_id IS NULL
               AND revoked_at IS NULL
             ORDER BY created_at DESC
             LIMIT 1",
            params![account_id],
            Self::token_from_row,
        )
        .optional()
        .map_err(Into::into)
    }

    pub(crate) fn get_token_by_hash_inner(
        conn: &Connection,
        token_hash: &str,
    ) -> Result<Option<ApiToken>> {
        conn.query_row(
            "SELECT token_hash, account_id, provider, machine_id, label, created_at, last_used_at, revoked_at
             FROM api_tokens WHERE token_hash = ?1",
            params![token_hash],
            Self::token_from_row,
        )
        .optional()
        .map_err(Into::into)
    }

    fn token_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ApiToken> {
        let last_used_raw: Option<String> = row.get(6)?;
        let revoked_raw: Option<String> = row.get(7)?;
        Ok(ApiToken {
            token_hash: row.get(0)?,
            account_id: row.get(1)?,
            provider: row.get(2)?,
            machine_id: row.get(3)?,
            label: row.get(4)?,
            created_at: parse_datetime(&row.get::<_, String>(5)?),
            last_used_at: last_used_raw.as_deref().map(parse_datetime),
            revoked_at: revoked_raw.as_deref().map(parse_datetime),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::auth::Account;

    fn store() -> Store {
        Store::open(":memory:").expect("in-memory store")
    }

    fn make_account(s: &Store) -> Account {
        let user = s.create_user("alice").unwrap();
        s.create_local_account(&user.id).unwrap()
    }

    #[test]
    fn generate_raw_token_has_expected_shape() {
        let t = generate_raw_token("local");
        assert!(t.starts_with("chrs_local_"));
        // 32 bytes base64url-no-pad → 43 chars; total >= 11+43.
        assert!(t.len() >= 54, "token too short: {t}");
    }

    #[test]
    fn hash_token_is_deterministic_and_sha256_shape() {
        let h1 = hash_token("chrs_local_abc");
        let h2 = hash_token("chrs_local_abc");
        assert_eq!(h1, h2);
        // SHA-256 hex = 64 chars.
        assert_eq!(h1.len(), 64);
        assert!(h1.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn hash_token_differs_per_input() {
        assert_ne!(hash_token("a"), hash_token("b"));
    }

    #[test]
    fn mint_token_returns_raw_and_stores_hash() {
        let s = store();
        let acct = make_account(&s);
        let minted = s.mint_token(&acct.id, "local", Some("CLI")).unwrap();
        assert!(minted.raw.starts_with("chrs_local_"));
        assert_eq!(minted.row.token_hash, hash_token(&minted.raw));
        assert_eq!(minted.row.account_id, acct.id);
        assert_eq!(minted.row.label.as_deref(), Some("CLI"));
        // Raw never persisted: a direct SELECT must not find the raw string.
        let conn = s.conn_for_test();
        let raw_in_db: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM api_tokens WHERE token_hash = ?1",
                params![minted.raw],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(raw_in_db, 0, "raw token string must not be stored");
    }

    #[test]
    fn mint_token_rejects_missing_account() {
        let s = store();
        let err = s.mint_token("acc_missing", "local", None).unwrap_err();
        assert!(err.to_string().contains("account not found"), "got: {err}");
    }

    #[test]
    fn touch_active_token_validates_and_writes_last_used_lazily() {
        // First touch on a never-used token records a write (last_used_at
        // was None, so the debounce considers it stale).
        let s = store();
        let acct = make_account(&s);
        let minted = s.mint_token(&acct.id, "local", Some("CLI")).unwrap();
        assert!(minted.row.last_used_at.is_none());

        // First call writes; the returned row predates the write (we
        // return the row we already fetched rather than round-tripping).
        let _ = s.touch_active_token(&minted.raw).unwrap().unwrap();

        // Read back: last_used_at is now set in the DB.
        let row = s
            .list_tokens_for_account(&acct.id)
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        assert!(
            row.last_used_at.is_some(),
            "first touch should populate last_used_at"
        );
    }

    #[test]
    fn touch_active_token_debounces_within_60s() {
        // Two rapid touches in succession: only the first writes; the
        // second sees the field set and skips the write.
        let s = store();
        let acct = make_account(&s);
        let minted = s.mint_token(&acct.id, "local", None).unwrap();

        let _ = s.touch_active_token(&minted.raw).unwrap();
        let first_seen = s
            .list_tokens_for_account(&acct.id)
            .unwrap()
            .into_iter()
            .next()
            .unwrap()
            .last_used_at;

        // Second touch — should not advance the timestamp because <60s
        // elapsed.
        let _ = s.touch_active_token(&minted.raw).unwrap();
        let second_seen = s
            .list_tokens_for_account(&acct.id)
            .unwrap()
            .into_iter()
            .next()
            .unwrap()
            .last_used_at;

        assert_eq!(
            first_seen, second_seen,
            "second touch within 60s should not have rewritten last_used_at"
        );
    }

    #[test]
    fn touch_active_token_returns_none_for_unknown_raw() {
        let s = store();
        let _acct = make_account(&s);
        let r = s.touch_active_token("chrs_local_does_not_exist").unwrap();
        assert!(r.is_none());
    }

    #[test]
    fn touch_active_token_returns_none_for_revoked() {
        let s = store();
        let acct = make_account(&s);
        let minted = s.mint_token(&acct.id, "local", None).unwrap();
        assert!(s.revoke_token_by_raw(&minted.raw).unwrap());
        assert!(s.touch_active_token(&minted.raw).unwrap().is_none());
    }

    #[test]
    fn revoke_is_idempotent() {
        let s = store();
        let acct = make_account(&s);
        let minted = s.mint_token(&acct.id, "local", None).unwrap();
        assert!(s.revoke_token_by_raw(&minted.raw).unwrap());
        assert!(!s.revoke_token_by_raw(&minted.raw).unwrap());
    }

    #[test]
    fn revoke_unknown_returns_false() {
        let s = store();
        assert!(!s.revoke_token_by_raw("chrs_local_nope").unwrap());
    }

    #[test]
    fn list_tokens_for_account_returns_all() {
        let s = store();
        let acct = make_account(&s);
        let _m1 = s.mint_token(&acct.id, "local", Some("CLI")).unwrap();
        let _m2 = s.mint_token(&acct.id, "local", Some("Bridge")).unwrap();
        let tokens = s.list_tokens_for_account(&acct.id).unwrap();
        assert_eq!(tokens.len(), 2);
    }

    #[test]
    fn cli_token_has_no_machine_id() {
        let s = store();
        let acct = make_account(&s);
        let minted = s.mint_token(&acct.id, "local", Some("CLI")).unwrap();
        assert!(minted.row.machine_id.is_none());
    }

    #[test]
    fn bridge_token_is_bound_to_machine_id() {
        let s = store();
        let acct = make_account(&s);
        let minted = s
            .mint_bridge_token(&acct.id, "machine-abc", Some("Local bridge"))
            .unwrap();
        assert_eq!(minted.row.machine_id.as_deref(), Some("machine-abc"));
        // Round-trip the hash → row lookup to confirm machine_id persists.
        let row = s.touch_active_token(&minted.raw).unwrap().unwrap();
        assert_eq!(row.machine_id.as_deref(), Some("machine-abc"));
    }

    #[test]
    fn bridge_and_cli_tokens_coexist_on_one_account() {
        let s = store();
        let acct = make_account(&s);
        let cli = s.mint_token(&acct.id, "local", Some("CLI")).unwrap();
        let bridge = s
            .mint_bridge_token(&acct.id, "machine-xyz", Some("Local bridge"))
            .unwrap();
        assert_ne!(cli.raw, bridge.raw);
        let tokens = s.list_tokens_for_account(&acct.id).unwrap();
        assert_eq!(tokens.len(), 2);
        let machine_ids: Vec<_> = tokens.iter().map(|t| t.machine_id.clone()).collect();
        assert!(machine_ids.contains(&None));
        assert!(machine_ids.contains(&Some("machine-xyz".to_string())));
    }
}
