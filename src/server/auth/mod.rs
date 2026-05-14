//! Server-side auth middleware.
//!
//! Resolves the per-request authenticated principal (`Actor`) from one of
//! two credentials:
//! - a `Cookie: chorus_sid=<id>` (browser session)
//! - an `Authorization: Bearer chrs_<...>` (CLI / bridge token)
//!
//! On success injects `Actor` into `req.extensions()` for handlers to read.
//!
//! Permissive vs strict modes:
//! - `require_auth` — strict; rejects with 401 if no valid credential.
//! - `permissive_auth` — injects Actor when present, falls through
//!   otherwise. Used while migrating handlers off `state.local_human_id`
//!   in subsequent commits. Removed once every handler reads from the
//!   request extension.

pub mod dev_login;
pub mod local_session;
pub use dev_login::handle_dev_login;
pub use local_session::handle_local_session;

use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use tracing::warn;

use crate::server::handlers::AppState;
use crate::store::{Account, ApiToken, Session, Store, User};

/// Cookie name carrying the session id. HttpOnly + SameSite=Strict.
pub const SESSION_COOKIE_NAME: &str = "chorus_sid";

/// Per-request authenticated principal.
///
/// `user_id` is the stable User identity (used as `messages.sender_id`,
/// `tasks.created_by_id`, etc.). `account_id` records which Account
/// (credential) the request authenticated through.
#[derive(Debug, Clone)]
pub struct Actor {
    pub user_id: String,
    pub user_name: String,
    pub account_id: String,
    pub auth: AuthKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthKind {
    Session,
    ApiToken,
}

/// Try to resolve an Actor from either credential.
///
/// Returns `None` only when NO credential is present. If a credential
/// IS present but invalid (stale cookie, revoked token, …), returns
/// `None` AND does NOT silently fall through to the other type — the
/// caller should treat that as a hard 401. No silent fallbacks.
///
/// Precedence when both credentials are present: cookie wins (standard
/// browser fallback semantics; an explicit cookie generally indicates
/// an active session UI session, while a leftover Authorization header
/// in a browser context is unusual).
pub fn resolve_actor(store: &Store, headers: &HeaderMap) -> Option<Actor> {
    let cookie = extract_session_cookie(headers);
    let bearer = extract_bearer_token(headers);
    match (cookie, bearer) {
        (Some(sid), _) => match resolve_session(store, &sid) {
            Some(actor) => Some(actor),
            None => {
                warn!(
                    cookie = %redact(&sid),
                    "auth: session cookie did not resolve; rejecting without trying bearer"
                );
                None
            }
        },
        (None, Some(raw)) => match resolve_token(store, &raw) {
            Some(actor) => Some(actor),
            None => {
                warn!(
                    token = %redact(&raw),
                    "auth: bearer token did not resolve to an active token"
                );
                None
            }
        },
        (None, None) => None,
    }
}

/// Strict middleware: rejects with 401 if no Actor resolves.
pub async fn require_auth(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut req: Request<Body>,
    next: Next,
) -> Response {
    match resolve_actor(state.store.as_ref(), &headers) {
        Some(actor) => {
            req.extensions_mut().insert(actor);
            next.run(req).await
        }
        None => unauthorized(),
    }
}

/// Permissive middleware: injects Actor when present, passes through
/// otherwise. Used during the handler migration so the layer can ship
/// before every call site is converted. Replaced by `require_auth`
/// once the sweep lands.
pub async fn permissive_auth(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut req: Request<Body>,
    next: Next,
) -> Response {
    if let Some(actor) = resolve_actor(state.store.as_ref(), &headers) {
        req.extensions_mut().insert(actor);
    }
    next.run(req).await
}

fn unauthorized() -> Response {
    let mut r = (StatusCode::UNAUTHORIZED, "authentication required").into_response();
    r.headers_mut().insert(
        axum::http::header::WWW_AUTHENTICATE,
        axum::http::HeaderValue::from_static(r#"Bearer realm="chorus", Cookie realm="chorus""#),
    );
    r
}

fn extract_bearer_token(headers: &HeaderMap) -> Option<String> {
    // RFC 7235: auth scheme is case-insensitive. We split on the first
    // whitespace and compare the scheme literal that way.
    let raw = headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .trim();
    let (scheme, rest) = raw.split_once(char::is_whitespace)?;
    if !scheme.eq_ignore_ascii_case("Bearer") {
        return None;
    }
    let token = rest.trim();
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

fn extract_session_cookie(headers: &HeaderMap) -> Option<String> {
    // HTTP/2 allows the client to split the cookie set across multiple
    // Cookie headers. Iterate over all of them so the second-or-later
    // header isn't silently dropped.
    let prefix = format!("{SESSION_COOKIE_NAME}=");
    for hv in headers.get_all(axum::http::header::COOKIE) {
        let Ok(raw) = hv.to_str() else { continue };
        for piece in raw.split(';') {
            let piece = piece.trim();
            if let Some(val) = piece.strip_prefix(prefix.as_str()) {
                let val = val.trim();
                if !val.is_empty() {
                    return Some(val.to_string());
                }
            }
        }
    }
    None
}

fn resolve_session(store: &Store, sid: &str) -> Option<Actor> {
    let session: Session = store.touch_active_session(sid).ok().flatten()?;
    let account: Account = store
        .get_account_by_id(&session.account_id)
        .ok()
        .flatten()?;
    if account.disabled_at.is_some() {
        return None;
    }
    let user: User = store.get_user_by_id(&account.user_id).ok().flatten()?;
    Some(Actor {
        user_id: user.id,
        user_name: user.name,
        account_id: account.id,
        auth: AuthKind::Session,
    })
}

fn resolve_token(store: &Store, raw: &str) -> Option<Actor> {
    let token: ApiToken = store.touch_active_token(raw).ok().flatten()?;
    let account: Account = store.get_account_by_id(&token.account_id).ok().flatten()?;
    if account.disabled_at.is_some() {
        return None;
    }
    let user: User = store.get_user_by_id(&account.user_id).ok().flatten()?;
    Some(Actor {
        user_id: user.id,
        user_name: user.name,
        account_id: account.id,
        auth: AuthKind::ApiToken,
    })
}

/// Show only the first 8 chars of a credential in logs. Never log the
/// full value — a leaked log line should not be enough to replay.
fn redact(s: &str) -> String {
    let n = s.chars().count().min(8);
    let head: String = s.chars().take(n).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn make_headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (name, val) in pairs {
            h.insert(
                axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_str(val).unwrap(),
            );
        }
        h
    }

    fn store_with_local_account() -> (Store, Account) {
        let s = Store::open(":memory:").expect("in-memory store");
        let user = s.create_user("alice").unwrap();
        let acct = s.create_local_account(&user.id).unwrap();
        (s, acct)
    }

    #[test]
    fn extract_bearer_handles_lowercase_and_whitespace() {
        let h = make_headers(&[("Authorization", "  Bearer  chrs_x  ")]);
        assert_eq!(extract_bearer_token(&h).as_deref(), Some("chrs_x"));
        let h = make_headers(&[("Authorization", "bearer chrs_y")]);
        assert_eq!(extract_bearer_token(&h).as_deref(), Some("chrs_y"));
    }

    #[test]
    fn extract_bearer_rejects_empty_and_non_bearer() {
        let h = make_headers(&[("Authorization", "Bearer ")]);
        assert!(extract_bearer_token(&h).is_none());
        let h = make_headers(&[("Authorization", "Basic dXNlcjpwYXNz")]);
        assert!(extract_bearer_token(&h).is_none());
        let h = make_headers(&[]);
        assert!(extract_bearer_token(&h).is_none());
    }

    #[test]
    fn extract_session_cookie_finds_named_cookie() {
        let h = make_headers(&[("Cookie", "other=1; chorus_sid=ses_abc; theme=dark")]);
        assert_eq!(extract_session_cookie(&h).as_deref(), Some("ses_abc"));
    }

    #[test]
    fn extract_session_cookie_missing_returns_none() {
        let h = make_headers(&[("Cookie", "other=1; theme=dark")]);
        assert!(extract_session_cookie(&h).is_none());
        let h = make_headers(&[]);
        assert!(extract_session_cookie(&h).is_none());
    }

    #[test]
    fn resolve_actor_via_token() {
        let (store, acct) = store_with_local_account();
        let minted = store.mint_token(&acct.id, "local", Some("CLI")).unwrap();
        let h = make_headers(&[("Authorization", &format!("Bearer {}", minted.raw))]);
        let actor = resolve_actor(&store, &h).expect("should resolve");
        assert_eq!(actor.account_id, acct.id);
        assert_eq!(actor.auth, AuthKind::ApiToken);
        assert_eq!(actor.user_name, "alice");
    }

    #[test]
    fn resolve_actor_via_session_cookie() {
        let (store, acct) = store_with_local_account();
        let session = store.create_session(&acct.id, None).unwrap();
        let h = make_headers(&[("Cookie", &format!("chorus_sid={}", session.id))]);
        let actor = resolve_actor(&store, &h).expect("should resolve");
        assert_eq!(actor.account_id, acct.id);
        assert_eq!(actor.auth, AuthKind::Session);
    }

    #[test]
    fn cookie_wins_when_both_present() {
        let (store, acct) = store_with_local_account();
        let session = store.create_session(&acct.id, None).unwrap();
        let minted = store.mint_token(&acct.id, "local", None).unwrap();
        let h = make_headers(&[
            ("Cookie", &format!("chorus_sid={}", session.id)),
            ("Authorization", &format!("Bearer {}", minted.raw)),
        ]);
        let actor = resolve_actor(&store, &h).expect("should resolve");
        assert_eq!(actor.auth, AuthKind::Session);
    }

    #[test]
    fn invalid_cookie_does_not_fall_through_to_bearer() {
        // If the client sends BOTH a stale/revoked cookie AND a valid
        // bearer token, the auth layer must NOT silently authenticate
        // them via the bearer — a present-but-invalid credential is a
        // hard failure, not a fallback signal.
        let (store, acct) = store_with_local_account();
        let session = store.create_session(&acct.id, None).unwrap();
        assert!(store.revoke_session(&session.id).unwrap());
        let minted = store.mint_token(&acct.id, "local", None).unwrap();
        let h = make_headers(&[
            ("Cookie", &format!("chorus_sid={}", session.id)),
            ("Authorization", &format!("Bearer {}", minted.raw)),
        ]);
        assert!(resolve_actor(&store, &h).is_none());
    }

    #[test]
    fn bearer_scheme_is_case_insensitive() {
        let (store, acct) = store_with_local_account();
        let minted = store.mint_token(&acct.id, "local", None).unwrap();
        for variant in ["Bearer", "bearer", "BEARER", "BeArEr"] {
            let h = make_headers(&[("Authorization", &format!("{variant} {}", minted.raw))]);
            assert!(
                resolve_actor(&store, &h).is_some(),
                "scheme `{variant}` should resolve"
            );
        }
    }

    #[test]
    fn cookie_extraction_handles_multiple_cookie_headers() {
        let (store, acct) = store_with_local_account();
        let session = store.create_session(&acct.id, None).unwrap();
        // HTTP/2 split: two Cookie headers, the target one is in the
        // second header. The naive single-header parser would miss it.
        let mut h = HeaderMap::new();
        h.append(
            axum::http::header::COOKIE,
            HeaderValue::from_static("theme=dark; other=1"),
        );
        h.append(
            axum::http::header::COOKIE,
            HeaderValue::from_str(&format!("chorus_sid={}", session.id)).unwrap(),
        );
        let actor = resolve_actor(&store, &h).expect("should resolve");
        assert_eq!(actor.account_id, acct.id);
    }

    #[test]
    fn revoked_token_returns_none() {
        let (store, acct) = store_with_local_account();
        let minted = store.mint_token(&acct.id, "local", None).unwrap();
        assert!(store.revoke_token_by_raw(&minted.raw).unwrap());
        let h = make_headers(&[("Authorization", &format!("Bearer {}", minted.raw))]);
        assert!(resolve_actor(&store, &h).is_none());
    }

    #[test]
    fn revoked_session_returns_none() {
        let (store, acct) = store_with_local_account();
        let session = store.create_session(&acct.id, None).unwrap();
        assert!(store.revoke_session(&session.id).unwrap());
        let h = make_headers(&[("Cookie", &format!("chorus_sid={}", session.id))]);
        assert!(resolve_actor(&store, &h).is_none());
    }

    #[test]
    fn disabled_account_returns_none() {
        let (store, acct) = store_with_local_account();
        let minted = store.mint_token(&acct.id, "local", None).unwrap();
        // Manually disable the account.
        {
            let conn = store.conn_for_test();
            conn.execute(
                "UPDATE accounts SET disabled_at = datetime('now') WHERE id = ?1",
                rusqlite::params![acct.id],
            )
            .unwrap();
        }
        let h = make_headers(&[("Authorization", &format!("Bearer {}", minted.raw))]);
        assert!(resolve_actor(&store, &h).is_none());
    }

    #[test]
    fn no_credentials_returns_none() {
        let store = Store::open(":memory:").unwrap();
        assert!(resolve_actor(&store, &HeaderMap::new()).is_none());
    }

    #[test]
    fn redact_truncates() {
        let r = redact("chrs_local_supersecretvalue");
        assert!(r.starts_with("chrs_loc") || r.starts_with("chrs_"));
        assert!(r.ends_with('…'));
        // The full secret never appears.
        assert!(!r.contains("supersecretvalue"));
    }
}
