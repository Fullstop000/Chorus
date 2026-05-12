//! Integration tests for the `POST /api/auth/local-session` endpoint.
//! Drives the real router so the loopback gate + cookie-set behaviour is
//! exercised end-to-end.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Request, StatusCode};
use chorus::store::Store;
use tower::ServiceExt;

mod harness;

const LOOPBACK: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 12345);
const REMOTE: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7)), 12345);

fn mem_store() -> Arc<Store> {
    let s = Store::open(":memory:").expect("in-memory store");
    Arc::new(s)
}

fn build_app_with_peer(store: Arc<Store>, peer: SocketAddr) -> axum::Router {
    // Use the raw router (no auto-bootstrapped identity, no header
    // injection) so the test fully controls what the server's DB and
    // request look like.
    let router = harness::build_router_raw(store);
    router.layer(MockConnectInfo(peer))
}

fn post_local_session() -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/api/auth/local-session")
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn loopback_with_local_account_mints_session_and_sets_cookie() {
    let store = mem_store();
    let user = store.create_user("alice").unwrap();
    let _acct = store.create_local_account(&user.id).unwrap();

    let app = build_app_with_peer(store.clone(), LOOPBACK);
    let req = post_local_session();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let cookie = res
        .headers()
        .get(axum::http::header::SET_COOKIE)
        .expect("expected Set-Cookie")
        .to_str()
        .unwrap();
    assert!(cookie.starts_with("chorus_sid=ses_"), "got: {cookie}");
    assert!(cookie.contains("HttpOnly"));
    assert!(cookie.contains("SameSite=Strict"));
    assert!(cookie.contains("Path=/"));

    let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["user"]["id"].as_str().unwrap(), user.id);
    assert_eq!(json["user"]["name"].as_str(), Some("alice"));
}

#[tokio::test]
async fn remote_peer_gets_404() {
    let store = mem_store();
    let user = store.create_user("alice").unwrap();
    let _acct = store.create_local_account(&user.id).unwrap();

    let app = build_app_with_peer(store, REMOTE);
    let req = post_local_session();
    let res = app.oneshot(req).await.unwrap();
    // 404 — endpoint pretends not to exist for non-loopback callers, so
    // remote scanners can't even see it's there.
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn loopback_with_no_local_account_returns_conflict() {
    let store = mem_store();
    // Note: deliberately no create_local_account call.

    let app = build_app_with_peer(store, LOOPBACK);
    let req = post_local_session();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::CONFLICT);

    let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let msg = std::str::from_utf8(&body).unwrap();
    assert!(msg.contains("chorus setup"), "expected hint, got: {msg}");
}

#[tokio::test]
async fn loopback_with_non_local_origin_is_rejected() {
    // Defense-in-depth: if a reverse proxy on loopback forwards a remote
    // request, the TCP peer looks loopback even though the browser is
    // not. Send a non-local Origin header to simulate that scenario;
    // the server must refuse to mint a session.
    let store = mem_store();
    let user = store.create_user("alice").unwrap();
    let _acct = store.create_local_account(&user.id).unwrap();

    let app = build_app_with_peer(store, LOOPBACK);
    let req = Request::builder()
        .method("POST")
        .uri("/api/auth/local-session")
        .header("Origin", "https://evil.example")
        .body(Body::empty())
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn loopback_with_local_origin_is_allowed() {
    // The legitimate browser path: Origin is the loopback dev server.
    let store = mem_store();
    let user = store.create_user("alice").unwrap();
    let _acct = store.create_local_account(&user.id).unwrap();

    let app = build_app_with_peer(store, LOOPBACK);
    let req = Request::builder()
        .method("POST")
        .uri("/api/auth/local-session")
        .header("Origin", "http://127.0.0.1:3001")
        .body(Body::empty())
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn loopback_with_disabled_account_returns_forbidden() {
    let store = mem_store();
    let user = store.create_user("alice").unwrap();
    let acct = store.create_local_account(&user.id).unwrap();
    // Disable the account.
    {
        let conn = store.conn_for_test();
        conn.execute(
            "UPDATE accounts SET disabled_at = datetime('now') WHERE id = ?1",
            rusqlite::params![acct.id],
        )
        .unwrap();
    }

    let app = build_app_with_peer(store, LOOPBACK);
    let req = post_local_session();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}
