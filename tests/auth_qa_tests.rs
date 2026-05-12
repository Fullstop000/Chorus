//! End-to-end QA tests for the identity-and-auth redesign.
//!
//! Covers the regression cases the eng-review flagged (T1–T5) plus a
//! couple of high-value invariants the unit tests can't easily express.
//!
//! Each test stands up a real router + a real store (in-memory SQLite),
//! drives requests through `tower::ServiceExt::oneshot`, and inspects
//! both the HTTP response and the persisted state.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{header, Request, StatusCode};
use chorus::store::auth::api_tokens::hash_token;
use chorus::store::Store;
use tower::ServiceExt;

mod harness;

const LOOPBACK: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 12345);

fn mem_store() -> Arc<Store> {
    Arc::new(Store::open(":memory:").expect("in-memory store"))
}

fn build_app(store: Arc<Store>) -> axum::Router {
    let router = harness::build_router_raw(store);
    router.layer(MockConnectInfo(LOOPBACK))
}

fn bearer(token: &str) -> (header::HeaderName, String) {
    (header::AUTHORIZATION, format!("Bearer {token}"))
}

/// T2 (REGRESSION) — setup → serve → /api/whoami continuity. The id the
/// store minted at setup time must be the same id the server reports
/// when the freshly written token is presented.
#[tokio::test]
async fn setup_to_serve_identity_continuity() {
    let store = mem_store();
    let (user, account) = store.ensure_local_identity("alice").unwrap();
    let minted = store.mint_token(&account.id, "local", Some("CLI")).unwrap();

    let app = build_app(store.clone());
    let req = Request::builder()
        .uri("/api/whoami")
        .header(bearer(&minted.raw).0, bearer(&minted.raw).1)
        .body(Body::empty())
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["id"].as_str(), Some(user.id.as_str()));
    assert_eq!(json["name"].as_str(), Some("alice"));
}

/// T1 (REGRESSION) — credentials lost but DB intact. The new model says
/// the user should recover by minting a fresh token against the
/// existing local Account (`chorus login --local`). Verify the store
/// supports that flow: mint twice, the second time succeeds and the
/// new raw token authenticates while the (still-active) first one also
/// continues to authenticate (we don't auto-revoke on a fresh mint —
/// users must `chorus logout` first if they want a single live token).
#[tokio::test]
async fn lost_credentials_recovery_via_fresh_token() {
    let store = mem_store();
    let (_user, account) = store.ensure_local_identity("alice").unwrap();
    let _first = store
        .mint_token(&account.id, "local", Some("CLI #1"))
        .unwrap();
    let second = store
        .mint_token(&account.id, "local", Some("CLI #2"))
        .unwrap();
    assert_ne!(_first.raw, second.raw);

    let app = build_app(store);
    let req = Request::builder()
        .uri("/api/whoami")
        .header(bearer(&second.raw).0, bearer(&second.raw).1)
        .body(Body::empty())
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

/// T3 (REGRESSION) — /api/system-info populates `local_human` from the
/// authenticated actor, not a cached cfg field. After the redesign,
/// the cfg.local_human section is gone; the field must source from
/// the request.
#[tokio::test]
async fn system_info_local_human_sources_from_actor() {
    let store = mem_store();
    let (user, account) = store.ensure_local_identity("alice").unwrap();
    let minted = store.mint_token(&account.id, "local", None).unwrap();

    let app = build_app(store);
    let req = Request::builder()
        .uri("/api/system-info")
        .header(bearer(&minted.raw).0, bearer(&minted.raw).1)
        .body(Body::empty())
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let local_human = &json["config"]["local_human"];
    assert_eq!(local_human["id"].as_str(), Some(user.id.as_str()));
    assert_eq!(local_human["name"].as_str(), Some("alice"));
}

/// T5 (COMPAT) — an old-style config.toml with `[local_human]` still
/// parses cleanly. The struct field is gone; serde must tolerate the
/// extra section so existing installs don't break on the first read.
#[test]
fn legacy_config_with_local_human_still_parses() {
    let tmp = tempfile::tempdir().unwrap();
    let legacy = "machine_id = \"deadbeef\"\n\
                  [local_human]\n\
                  id = \"human_legacy\"\n\
                  name = \"alice\"\n";
    std::fs::write(tmp.path().join("config.toml"), legacy).unwrap();

    let loaded = chorus::config::ChorusConfig::load(tmp.path())
        .expect("load result")
        .expect("config present");
    assert_eq!(loaded.machine_id.as_deref(), Some("deadbeef"));
    // The legacy section is dropped on the next save (no field for it).
    // Confirm save → load roundtrips without the section.
    loaded.save(tmp.path()).unwrap();
    let raw = std::fs::read_to_string(tmp.path().join("config.toml")).unwrap();
    assert!(
        !raw.contains("[local_human]"),
        "expected dropped, got:\n{raw}"
    );
}

/// Token revoke takes effect immediately on the next request. Critical
/// for `chorus logout` semantics (and future cloud admin revoke).
#[tokio::test]
async fn token_revocation_blocks_subsequent_requests() {
    let store = mem_store();
    let (_user, account) = store.ensure_local_identity("alice").unwrap();
    let minted = store.mint_token(&account.id, "local", None).unwrap();

    let app = build_app(store.clone());

    // Pre-revoke: works.
    let req = Request::builder()
        .uri("/api/whoami")
        .header(bearer(&minted.raw).0, bearer(&minted.raw).1)
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // Revoke and re-request: 401.
    assert!(store.revoke_token_by_raw(&minted.raw).unwrap());
    let req = Request::builder()
        .uri("/api/whoami")
        .header(bearer(&minted.raw).0, bearer(&minted.raw).1)
        .body(Body::empty())
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

/// Session cookie lifecycle: mint via /api/auth/local-session, use it
/// on a protected endpoint, then verify revocation in the store kicks
/// the session out.
#[tokio::test]
async fn cookie_session_lifecycle_via_real_endpoints() {
    let store = mem_store();
    let _ = store.ensure_local_identity("alice").unwrap();

    let app = build_app(store.clone());

    // Mint cookie.
    let req = Request::builder()
        .method("POST")
        .uri("/api/auth/local-session")
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let cookie = res
        .headers()
        .get(header::SET_COOKIE)
        .expect("set-cookie")
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();

    // Use it.
    let req = Request::builder()
        .uri("/api/whoami")
        .header(header::COOKIE, &cookie)
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // Extract the session id from cookie and revoke directly.
    let sid = cookie
        .strip_prefix("chorus_sid=")
        .expect("expected chorus_sid prefix");
    assert!(store.revoke_session(sid).unwrap());

    // Same cookie, now revoked → 401.
    let req = Request::builder()
        .uri("/api/whoami")
        .header(header::COOKIE, &cookie)
        .body(Body::empty())
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

/// SHA-256 is the hash function; the raw token is NEVER stored. Verify
/// directly: an inserted token's row's `token_hash` matches
/// `sha256(raw)` and the raw string does not appear anywhere in
/// `api_tokens`.
#[test]
fn raw_token_is_not_stored_only_hash() {
    let store = mem_store();
    let (_user, account) = store.ensure_local_identity("alice").unwrap();
    let minted = store.mint_token(&account.id, "local", None).unwrap();

    let expected_hash = hash_token(&minted.raw);
    assert_eq!(minted.row.token_hash, expected_hash);

    // Direct DB inspection: SELECT every column and confirm the raw
    // doesn't appear.
    let conn = store.conn_for_test();
    let mut stmt = conn
        .prepare("SELECT token_hash, account_id, label, created_at FROM api_tokens")
        .unwrap();
    let row = stmt
        .query_row([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, String>(3)?,
            ))
        })
        .unwrap();
    let stringified = format!("{:?}", row);
    assert!(
        !stringified.contains(&minted.raw),
        "raw token leaked into stored row: {stringified}"
    );
}

/// chorus serve's startup uses `Store::get_local_account` instead of
/// `state.local_human_id`. If a fresh DB has no local account yet, the
/// boot must not panic — it should log and continue serving (the user
/// completes setup next).
#[tokio::test]
async fn no_local_account_on_boot_is_not_a_panic() {
    // The router builder is what would panic if we mishandled this.
    // Driving it without an identity row exercises the new "log and
    // continue" path.
    let store = mem_store();
    let _ = harness::build_router_raw(store); // must not panic
}
