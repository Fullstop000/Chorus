//! Bridge bearer-token authentication.
//!
//! Three guarantees needed before the bridge protocol is safe past loopback:
//!
//! 1. **Bearer token required on WS upgrade.** Anyone reaching
//!    `/api/bridge/ws` with no `Authorization` header gets `401`.
//! 2. **`machine_id` is pinned to a token.** The token row in
//!    `api_tokens` carries the `machine_id` it's bound to; the bridge
//!    can't claim an arbitrary id in `bridge.hello`. If the hello
//!    payload's `machine_id` doesn't match, the connection is closed.
//! 3. **`/internal/agent/<id>/*` is scoped to the token's `machine_id`.**
//!    A valid token alone is not enough — the agent named in the URL
//!    must be owned by the same `machine_id` the token is bound to.
//!    Prevents one bridge's token from acting on another bridge's agents.
//!
//! Tokens live in `api_tokens` (with `machine_id` set; CLI tokens have
//! `machine_id IS NULL` and are rejected here). Setup mints the initial
//! local bridge token via `chorus::cli::login::mint_local_bridge_credentials`.
//! Future tokens come from a `chorus tokens mint --bridge --machine-id`
//! admin command (not yet implemented).
//!
//! Passthrough mode: when the `api_tokens` table contains **zero** rows
//! with a non-null `machine_id`, the middleware passes every request
//! through unauthenticated. This keeps test harnesses and pre-setup
//! installs working without forcing every consumer to mint a bridge
//! token. Once any bridge token exists in the DB the middleware switches
//! to "all bridge requests require a valid token."

use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use tracing::warn;

use super::handlers::AppState;
use crate::store::auth::api_tokens::hash_token;
use crate::store::Store;

/// Outcome of validating an incoming `Authorization` header against the
/// `api_tokens` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthOutcome {
    /// No bridge tokens exist in the DB yet. Passes through.
    Disabled,
    /// Bearer matched a bridge token; the bridge's hello (and any
    /// `/internal/agent/{agent_id}/*` path) must reference an agent
    /// whose `agents.machine_id` matches.
    Allowed { expected_machine_id: String },
    /// Bearer matched a CLI token. The CLI may only operate on the user
    /// id the token represents — i.e. `/internal/agent/{actor_id}/*`
    /// where `actor_id == users.id`. Agents are off-limits to CLI
    /// tokens; those need bridges.
    CliAllowed { user_id: String },
    /// Missing/malformed header, unknown token, or revoked.
    Rejected,
}

/// Look up the bearer token in `api_tokens` and decide.
///
/// `headers`: incoming request headers.
/// `store`: the canonical token table.
pub fn check(store: &Store, headers: &HeaderMap) -> AuthOutcome {
    let any_bridge_token = match store.has_any_bridge_token() {
        Ok(b) => b,
        Err(err) => {
            warn!(err = %err, "bridge_auth: store lookup failed; failing closed");
            return AuthOutcome::Rejected;
        }
    };
    if !any_bridge_token {
        return AuthOutcome::Disabled;
    }
    let Some(value) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    else {
        return AuthOutcome::Rejected;
    };
    let (scheme, rest) = match value.trim().split_once(char::is_whitespace) {
        Some(pair) => pair,
        None => return AuthOutcome::Rejected,
    };
    if !scheme.eq_ignore_ascii_case("Bearer") {
        return AuthOutcome::Rejected;
    }
    let raw = rest.trim();
    if raw.is_empty() {
        return AuthOutcome::Rejected;
    }
    let token_hash = hash_token(raw);
    let row = match store.get_token_by_hash(&token_hash) {
        Ok(Some(r)) => r,
        Ok(None) => return AuthOutcome::Rejected,
        Err(err) => {
            warn!(err = %err, "bridge_auth: token lookup failed");
            return AuthOutcome::Rejected;
        }
    };
    if row.revoked_at.is_some() {
        return AuthOutcome::Rejected;
    }
    match row.machine_id {
        Some(m) if !m.trim().is_empty() => AuthOutcome::Allowed {
            expected_machine_id: m,
        },
        _ => {
            // CLI token. Resolve its user_id so the middleware can
            // enforce "this token can only act as its own user."
            match store.get_account_by_id(&row.account_id) {
                Ok(Some(acct)) => AuthOutcome::CliAllowed {
                    user_id: acct.user_id,
                },
                _ => AuthOutcome::Rejected,
            }
        }
    }
}

/// Axum middleware that protects `/internal/agent/*` (and any other
/// route it's applied to). When passthrough is active (no bridge tokens
/// in DB), the request flows through unchanged. Otherwise a valid
/// `Authorization: Bearer <token>` is required, the token must be a
/// bridge token (non-null `machine_id`), and the `agent_id` in the URL
/// must be owned by that machine.
pub async fn require_bridge_auth(
    State(state): State<AppState>,
    headers: HeaderMap,
    req: Request,
    next: Next,
) -> Response {
    match check(state.store.as_ref(), &headers) {
        AuthOutcome::Disabled => next.run(req).await,
        AuthOutcome::Allowed {
            expected_machine_id,
        } => {
            let path = req.uri().path().to_string();
            match agent_id_from_internal_path(&path) {
                Some(agent_id) => {
                    let owner = match state.store.get_agent_by_id(agent_id, false) {
                        Ok(Some(agent)) => Some(agent.machine_id),
                        Ok(None) => {
                            // No agent row with this id. If the id is the
                            // CLI user's user_id this would be a CLI-acting-
                            // as-self path (CliAllowed handles that branch);
                            // for bridge tokens, an unknown agent is forbidden.
                            None
                        }
                        Err(err) => {
                            warn!(
                                path = %path,
                                error = %err,
                                "bridge_auth: rejecting /internal request — store lookup failed"
                            );
                            return (StatusCode::INTERNAL_SERVER_ERROR, "store error")
                                .into_response();
                        }
                    };
                    match owner {
                        Some(owner) if owner == *expected_machine_id => next.run(req).await,
                        Some(owner) => {
                            warn!(
                                path = %path,
                                token_machine_id = %expected_machine_id,
                                agent_owner = %owner,
                                "bridge_auth: rejecting /internal request — token's machine_id does not own this agent"
                            );
                            (StatusCode::FORBIDDEN, "token not authorized for this agent")
                                .into_response()
                        }
                        None => {
                            warn!(
                                path = %path,
                                token_machine_id = %expected_machine_id,
                                "bridge_auth: rejecting /internal request — agent_id not in store"
                            );
                            (StatusCode::FORBIDDEN, "agent not found").into_response()
                        }
                    }
                }
                None => {
                    warn!(
                        path = %path,
                        "bridge_auth: rejecting /internal request — no agent_id in path"
                    );
                    (StatusCode::FORBIDDEN, "no agent scope in path").into_response()
                }
            }
        }
        AuthOutcome::CliAllowed { user_id } => {
            // CLI token: may only act under its own user_id. The classic
            // case is `chorus send` posting to /internal/agent/{me.id}/send
            // where `me.id` is the human's user id, not an agent id.
            let path = req.uri().path().to_string();
            match agent_id_from_internal_path(&path) {
                Some(actor_id) if actor_id == user_id => next.run(req).await,
                Some(actor_id) => {
                    warn!(
                        path = %path,
                        token_user_id = %user_id,
                        actor_id = %actor_id,
                        "bridge_auth: rejecting /internal request — CLI token can only act as its own user"
                    );
                    (
                        StatusCode::FORBIDDEN,
                        "CLI token not authorized for this actor",
                    )
                        .into_response()
                }
                None => {
                    warn!(
                        path = %path,
                        "bridge_auth: rejecting /internal request — no actor scope in path"
                    );
                    (StatusCode::FORBIDDEN, "no actor scope in path").into_response()
                }
            }
        }
        AuthOutcome::Rejected => {
            warn!(
                path = %req.uri().path(),
                "bridge_auth: rejecting /internal request — invalid or missing bearer token"
            );
            (StatusCode::UNAUTHORIZED, "missing or invalid bearer token").into_response()
        }
    }
}

/// Extract the `<agent_id>` segment from an `/internal/agent/<id>/...`
/// path (the middleware is wired to that route shape). Returns `None`
/// if the path doesn't match. The returned reference borrows from the
/// input.
fn agent_id_from_internal_path(path: &str) -> Option<&str> {
    let after = path.split_once("/agent/")?.1;
    let id = after.split('/').next()?;
    if id.is_empty() {
        None
    } else {
        Some(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn mk_headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                HeaderValue::from_str(v).unwrap(),
            );
        }
        h
    }

    fn mk_store_with_bridge_token() -> (Store, String, String) {
        let s = Store::open(":memory:").unwrap();
        let user = s.create_user("alice").unwrap();
        let acct = s.create_local_account(&user.id).unwrap();
        let machine_id = "machine-x".to_string();
        let minted = s
            .mint_bridge_token(&acct.id, &machine_id, Some("test"))
            .unwrap();
        (s, minted.raw, machine_id)
    }

    #[test]
    fn passthrough_when_no_bridge_tokens_in_db() {
        let s = Store::open(":memory:").unwrap();
        // No bridge tokens minted.
        let outcome = check(&s, &HeaderMap::new());
        assert_eq!(outcome, AuthOutcome::Disabled);
    }

    #[test]
    fn rejects_missing_auth_when_bridge_tokens_exist() {
        let (s, _raw, _m) = mk_store_with_bridge_token();
        assert_eq!(check(&s, &HeaderMap::new()), AuthOutcome::Rejected);
    }

    #[test]
    fn rejects_unknown_token() {
        let (s, _raw, _m) = mk_store_with_bridge_token();
        let h = mk_headers(&[("Authorization", "Bearer chrs_bridge_unknown")]);
        assert_eq!(check(&s, &h), AuthOutcome::Rejected);
    }

    #[test]
    fn allows_valid_bridge_token_and_returns_machine_id() {
        let (s, raw, machine_id) = mk_store_with_bridge_token();
        let h = mk_headers(&[("Authorization", &format!("Bearer {raw}"))]);
        match check(&s, &h) {
            AuthOutcome::Allowed {
                expected_machine_id,
            } => assert_eq!(expected_machine_id, machine_id),
            other => panic!("expected Allowed, got {other:?}"),
        }
    }

    #[test]
    fn cli_token_resolves_to_cli_allowed_with_user_id() {
        // CLI tokens (machine_id = NULL) are NOT outright rejected at
        // /internal/* — they're returned as `CliAllowed { user_id }`
        // so the middleware can let `chorus send` post as the user
        // itself while still gating bridges by machine_id.
        let s = Store::open(":memory:").unwrap();
        let user = s.create_user("alice").unwrap();
        let acct = s.create_local_account(&user.id).unwrap();
        // Mint a bridge token so passthrough doesn't kick in.
        let _ = s
            .mint_bridge_token(&acct.id, "machine-x", Some("bridge"))
            .unwrap();
        let cli = s.mint_token(&acct.id, "local", Some("CLI")).unwrap();
        let h = mk_headers(&[("Authorization", &format!("Bearer {}", cli.raw))]);
        match check(&s, &h) {
            AuthOutcome::CliAllowed { user_id: resolved } => assert_eq!(resolved, user.id),
            other => panic!("expected CliAllowed, got {other:?}"),
        }
    }

    #[test]
    fn rejects_revoked_bridge_token() {
        let (s, raw, _) = mk_store_with_bridge_token();
        assert!(s.revoke_token_by_raw(&raw).unwrap());
        let h = mk_headers(&[("Authorization", &format!("Bearer {raw}"))]);
        assert_eq!(check(&s, &h), AuthOutcome::Rejected);
    }

    #[test]
    fn case_insensitive_bearer_scheme() {
        let (s, raw, machine_id) = mk_store_with_bridge_token();
        let h = mk_headers(&[("Authorization", &format!("bearer {raw}"))]);
        match check(&s, &h) {
            AuthOutcome::Allowed {
                expected_machine_id,
            } => assert_eq!(expected_machine_id, machine_id),
            other => panic!("expected Allowed, got {other:?}"),
        }
    }

    #[test]
    fn agent_id_extraction() {
        assert_eq!(
            agent_id_from_internal_path("/internal/agent/alice/send"),
            Some("alice")
        );
        assert_eq!(
            agent_id_from_internal_path("/internal/agent/alice"),
            Some("alice")
        );
        assert!(agent_id_from_internal_path("/internal/foo/bar").is_none());
        assert!(agent_id_from_internal_path("/internal/agent/").is_none());
    }
}
