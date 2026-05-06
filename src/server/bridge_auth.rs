//! Bridge bearer-token authentication.
//!
//! Phase 3 slice 4: minimum auth shape that closes the two real gaps
//! before this protocol is safe past loopback —
//!
//! 1. **Bearer token required on WS upgrade.** Anyone reaching
//!    `/api/bridge/ws` with no `Authorization` header gets `401`.
//! 2. **`machine_id` is pinned to a token.** The platform looks up the
//!    token to find its bound `machine_id`; the bridge can no longer
//!    claim an arbitrary `machine_id` in `bridge.hello`. If the hello
//!    payload's `machine_id` doesn't match the token's binding, the
//!    connection is closed (Open Decision D-A from the design doc).
//!
//! Tokens are configured via the `CHORUS_BRIDGE_TOKENS` env var:
//!
//! ```text
//! CHORUS_BRIDGE_TOKENS="dev-token-1:machine-alpha,dev-token-2:machine-beta"
//! ```
//!
//! Comma-separates entries; each is `token:machine_id`. Whitespace is
//! trimmed. Empty values, malformed pairs, and duplicate tokens are
//! logged as warnings and ignored — startup never fails for a bad
//! token-list, the operator just sees an empty (= disabled) auth state.
//!
//! When the parsed map is empty (env unset or unparseable),
//! authentication is **disabled** entirely — the WS endpoint accepts
//! any client and trusts the `bridge.hello.machine_id` it sees. This
//! matches the loopback default and keeps existing tests working
//! without env-var fiddling.
//!
//! What this slice deliberately does NOT do:
//!   - Token rotation/revocation. Tokens are static for the process
//!     lifetime; restart `chorus serve` to roll them.
//!   - Anomaly detection (IP flapping, rate-limiting).
//!   - Auth on `/internal/agent/*`. Those handlers are still
//!     loopback-only in Phase 2; Phase 3-remote needs a separate auth
//!     story for them (likely the same tokens, but the call sites need
//!     to be updated to send the header — out of scope here).

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use tracing::warn;

use super::handlers::AppState;

/// Outcome of validating an incoming `Authorization` header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthOutcome {
    /// Auth is disabled (empty token map). The bridge may declare any
    /// `machine_id` in its hello frame.
    Disabled,
    /// Auth is enabled and the header matched a known token. The
    /// bridge's `bridge.hello.machine_id` MUST equal `expected_machine_id`.
    Allowed { expected_machine_id: String },
    /// Auth is enabled and the request must be rejected (missing or
    /// malformed `Authorization` header, or unknown token).
    Rejected,
}

/// Token map for the bridge endpoint. Cheap to clone; held inside an
/// `Arc` on `AppState`.
#[derive(Debug, Default)]
pub struct BridgeAuth {
    /// `token → machine_id`. Empty when auth is disabled.
    tokens: HashMap<String, String>,
}

impl BridgeAuth {
    pub fn empty() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Construct directly from a `(token, machine_id)` iterator. Useful
    /// for tests that want to inject specific tokens without touching
    /// the process environment.
    pub fn from_pairs<I, S1, S2>(pairs: I) -> Arc<Self>
    where
        I: IntoIterator<Item = (S1, S2)>,
        S1: Into<String>,
        S2: Into<String>,
    {
        let tokens: HashMap<String, String> = pairs
            .into_iter()
            .map(|(t, m)| (t.into(), m.into()))
            .collect();
        Arc::new(Self { tokens })
    }

    /// Read `CHORUS_BRIDGE_TOKENS` from the process environment. Always
    /// returns `Some(Arc<Self>)` — the inner map may be empty if the
    /// variable is unset or unparseable, which means auth is disabled.
    pub fn from_env() -> Arc<Self> {
        let raw = match std::env::var("CHORUS_BRIDGE_TOKENS") {
            Ok(v) if !v.trim().is_empty() => v,
            _ => return Self::empty(),
        };
        let mut tokens: HashMap<String, String> = HashMap::new();
        for entry in raw.split(',') {
            let entry = entry.trim();
            if entry.is_empty() {
                continue;
            }
            let Some((token, machine)) = entry.split_once(':') else {
                warn!(
                    entry = %entry,
                    "bridge_auth: ignoring malformed CHORUS_BRIDGE_TOKENS entry (expected token:machine_id)"
                );
                continue;
            };
            let token = token.trim();
            let machine = machine.trim();
            if token.is_empty() || machine.is_empty() {
                warn!(
                    entry = %entry,
                    "bridge_auth: ignoring CHORUS_BRIDGE_TOKENS entry with empty token or machine_id"
                );
                continue;
            }
            if tokens
                .insert(token.to_string(), machine.to_string())
                .is_some()
            {
                warn!(
                    token_prefix = %&token[..token.len().min(6)],
                    "bridge_auth: duplicate token in CHORUS_BRIDGE_TOKENS — last wins"
                );
            }
        }
        Arc::new(Self { tokens })
    }

    /// Whether any tokens are configured. `false` means auth is disabled.
    pub fn is_enabled(&self) -> bool {
        !self.tokens.is_empty()
    }

    /// Look up a token's bound `machine_id` directly. Returns `None`
    /// when auth is disabled or the token is unknown.
    #[cfg(test)]
    pub fn machine_id_for_token(&self, token: &str) -> Option<&str> {
        self.tokens.get(token).map(|s| s.as_str())
    }

    /// Inspect the request's `Authorization` header against the
    /// configured token map.
    pub fn check(&self, headers: &HeaderMap) -> AuthOutcome {
        if !self.is_enabled() {
            return AuthOutcome::Disabled;
        }
        let Some(value) = headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
        else {
            return AuthOutcome::Rejected;
        };
        // Standard "Bearer <token>" parse. Tolerate extra whitespace.
        let token = match value.trim().strip_prefix("Bearer ") {
            Some(rest) => rest.trim(),
            None => return AuthOutcome::Rejected,
        };
        if token.is_empty() {
            return AuthOutcome::Rejected;
        }
        match self.tokens.get(token) {
            Some(machine_id) => AuthOutcome::Allowed {
                expected_machine_id: machine_id.clone(),
            },
            None => AuthOutcome::Rejected,
        }
    }
}

/// Axum middleware that protects `/internal/agent/*` (and any other
/// route it's applied to) the same way as `handle_bridge_ws`: when
/// tokens are configured, require a valid `Authorization: Bearer <t>`
/// header and reject with 401 otherwise. When tokens are unset (the
/// loopback default and existing test harness mode), the request
/// passes through unchanged.
pub async fn require_bridge_auth(
    State(state): State<AppState>,
    headers: HeaderMap,
    req: Request,
    next: Next,
) -> Response {
    match state.bridge_auth.check(&headers) {
        AuthOutcome::Disabled | AuthOutcome::Allowed { .. } => next.run(req).await,
        AuthOutcome::Rejected => {
            warn!(
                path = %req.uri().path(),
                "bridge_auth: rejecting /internal request — invalid or missing bearer token"
            );
            (StatusCode::UNAUTHORIZED, "missing or invalid bearer token").into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers_with_auth(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_str(value).unwrap(),
        );
        h
    }

    #[test]
    fn disabled_when_empty() {
        let auth = BridgeAuth::empty();
        assert!(!auth.is_enabled());
        assert_eq!(auth.check(&HeaderMap::new()), AuthOutcome::Disabled);
        assert_eq!(
            auth.check(&headers_with_auth("Bearer anything")),
            AuthOutcome::Disabled
        );
    }

    #[test]
    fn allowed_with_valid_bearer() {
        let auth = BridgeAuth::from_pairs([("tok-1", "machine-alpha")]);
        let h = headers_with_auth("Bearer tok-1");
        assert_eq!(
            auth.check(&h),
            AuthOutcome::Allowed {
                expected_machine_id: "machine-alpha".to_string()
            }
        );
    }

    #[test]
    fn rejected_when_header_missing() {
        let auth = BridgeAuth::from_pairs([("tok-1", "m")]);
        assert_eq!(auth.check(&HeaderMap::new()), AuthOutcome::Rejected);
    }

    #[test]
    fn rejected_when_token_unknown() {
        let auth = BridgeAuth::from_pairs([("tok-1", "m")]);
        let h = headers_with_auth("Bearer not-the-token");
        assert_eq!(auth.check(&h), AuthOutcome::Rejected);
    }

    #[test]
    fn rejected_when_scheme_not_bearer() {
        let auth = BridgeAuth::from_pairs([("tok-1", "m")]);
        let h = headers_with_auth("Basic tok-1");
        assert_eq!(auth.check(&h), AuthOutcome::Rejected);
    }

    #[test]
    fn rejected_when_bearer_value_empty() {
        let auth = BridgeAuth::from_pairs([("tok-1", "m")]);
        let h = headers_with_auth("Bearer ");
        assert_eq!(auth.check(&h), AuthOutcome::Rejected);
    }

    // `CHORUS_BRIDGE_TOKENS` is process-wide. Serialize on `bridge_env` so this
    // test doesn't race a future parallel test that sets the same var.
    #[test]
    #[serial_test::serial(bridge_env)]
    fn from_env_handles_empty_var() {
        // SAFETY: env mutation is serialized by the `#[serial(bridge_env)]`
        // attribute above; no other test in this group runs concurrently.
        unsafe {
            std::env::remove_var("CHORUS_BRIDGE_TOKENS");
        }
        let auth = BridgeAuth::from_env();
        assert!(!auth.is_enabled());
    }
}
