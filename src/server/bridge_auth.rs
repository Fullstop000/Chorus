//! Bridge bearer-token authentication.
//!
//! Token shapes (`(provider, machine_id)`):
//!
//! - `("local",  None)` — CLI bearer. CLI may act as its own user only,
//!   i.e. `/internal/agent/{user_id}/*`.
//! - `("bridge", Some(m))` — Legacy per-machine bridge. Restricted to
//!   agents whose `agents.machine_id = m`.
//! - `("bridge", None)` — User-scoped bridge. Allowed to act on agents
//!   whose `agents.machine_id` corresponds to an active (non-kicked)
//!   `bridge_machines` row for this token, OR to act as the user
//!   itself.
//!
//! Passthrough mode: when no bridge tokens exist at all
//! (`provider='bridge'`), the middleware passes every request through
//! unauthenticated. Once any bridge token exists, all `/internal/agent/*`
//! requests need a valid bearer.

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
    /// Bearer matched a legacy per-machine bridge token. The bridge's
    /// hello and any `/internal/agent/{agent_id}/*` path must reference
    /// an agent whose `agents.machine_id` matches `expected_machine_id`.
    Allowed { expected_machine_id: String },
    /// Bearer matched a CLI token. The CLI may only operate on the user
    /// id the token represents.
    CliAllowed { user_id: String },
    /// Bearer matched a user-scoped bridge token. May act on agents
    /// whose `agents.machine_id` corresponds to an active (non-kicked)
    /// `bridge_machines` row for this `token_hash`, or as the user itself.
    UserBridgeAllowed { user_id: String, token_hash: String },
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
    match (row.provider.as_str(), row.machine_id.as_deref()) {
        ("bridge", Some(m)) if !m.trim().is_empty() => AuthOutcome::Allowed {
            expected_machine_id: m.to_string(),
        },
        ("bridge", _) => {
            // User-scoped bridge token: machine_id comes from each
            // `bridge.hello` (for WS) or is derived from the agent row's
            // `machine_id` cross-checked against `bridge_machines` (for
            // HTTPS /internal/* calls).
            match store.get_account_by_id(&row.account_id) {
                Ok(Some(acct)) => AuthOutcome::UserBridgeAllowed {
                    user_id: acct.user_id,
                    token_hash,
                },
                _ => AuthOutcome::Rejected,
            }
        }
        ("local", _) => {
            // CLI token. Resolve its user_id so the middleware can
            // enforce "this token can only act as its own user."
            match store.get_account_by_id(&row.account_id) {
                Ok(Some(acct)) => AuthOutcome::CliAllowed {
                    user_id: acct.user_id,
                },
                _ => AuthOutcome::Rejected,
            }
        }
        _ => AuthOutcome::Rejected,
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
        AuthOutcome::UserBridgeAllowed {
            user_id,
            token_hash,
        } => {
            // User-scoped bridge token. Allowed paths:
            //   1. /internal/agent/{user_id}/* — acting as the user itself.
            //   2. /internal/agent/{agent_id}/* — where the agent's
            //      machine_id has a non-kicked bridge_machines row tied
            //      to this token.
            let path = req.uri().path().to_string();
            let actor = match agent_id_from_internal_path(&path) {
                Some(a) => a.to_string(),
                None => {
                    warn!(
                        path = %path,
                        "bridge_auth: rejecting /internal request — no actor scope in path"
                    );
                    return (StatusCode::FORBIDDEN, "no actor scope in path").into_response();
                }
            };
            if actor == user_id {
                return next.run(req).await;
            }
            // Look up the agent row to find its machine_id, then
            // cross-check with bridge_machines.
            let agent_machine_id = match state.store.get_agent_by_id(&actor, false) {
                Ok(Some(agent)) => agent.machine_id,
                Ok(None) => {
                    warn!(
                        path = %path,
                        token_user_id = %user_id,
                        "bridge_auth: rejecting /internal request — agent_id not in store"
                    );
                    return (StatusCode::FORBIDDEN, "agent not found").into_response();
                }
                Err(err) => {
                    warn!(
                        path = %path,
                        error = %err,
                        "bridge_auth: rejecting /internal request — agent store error"
                    );
                    return (StatusCode::INTERNAL_SERVER_ERROR, "store error").into_response();
                }
            };
            match state
                .store
                .get_bridge_machine(&token_hash, &agent_machine_id)
            {
                Ok(Some(m)) if !m.is_kicked() => next.run(req).await,
                Ok(_) => {
                    warn!(
                        path = %path,
                        token_user_id = %user_id,
                        agent_machine_id = %agent_machine_id,
                        "bridge_auth: rejecting /internal request — agent's machine not registered (or kicked) for this user-scoped token"
                    );
                    (
                        StatusCode::FORBIDDEN,
                        "user-scoped bridge token not authorized for this agent's machine",
                    )
                        .into_response()
                }
                Err(err) => {
                    warn!(
                        path = %path,
                        error = %err,
                        "bridge_auth: rejecting /internal request — bridge_machines lookup failed"
                    );
                    (StatusCode::INTERNAL_SERVER_ERROR, "store error").into_response()
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
