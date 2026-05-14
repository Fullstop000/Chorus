//! Integration tests for the Settings → Devices flow + dev-auth route.
//!
//! Exercises the new endpoints end-to-end against an in-process Axum
//! router:
//!   - `POST /api/devices/mint` returns a script-with-bearer once; second
//!     call returns 410 Gone.
//!   - `GET /api/devices` returns the live bridge_machines rows.
//!   - `DELETE /api/devices/{machine_id}` (Kick) sets `kicked_at`.
//!   - `DELETE /api/devices/{machine_id}?forget=1` deletes the row.
//!   - `POST /api/devices/rotate` revokes the old token + mints a new one.
//!
//! The bridge_machines state-machine is exercised via direct store calls
//! (the WS-level coverage is in `bridge_ws_tests`); these tests focus on
//! the HTTP surface that the UI consumes.

mod harness;

use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use chorus::store::Store;
use harness::{build_router, TEST_AUTH_TOKEN};
use tower::ServiceExt;

async fn json_body(resp: axum::response::Response) -> serde_json::Value {
    let bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).expect("response body must be JSON")
}

fn auth_get(path: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(path)
        .header("Authorization", format!("Bearer {TEST_AUTH_TOKEN}"))
        .body(Body::empty())
        .unwrap()
}

fn auth_post(path: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(path)
        .header("Authorization", format!("Bearer {TEST_AUTH_TOKEN}"))
        .body(Body::empty())
        .unwrap()
}

fn auth_delete(path: &str) -> Request<Body> {
    Request::builder()
        .method("DELETE")
        .uri(path)
        .header("Authorization", format!("Bearer {TEST_AUTH_TOKEN}"))
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn list_devices_returns_empty_when_no_token_minted() {
    let store = Arc::new(Store::open(":memory:").unwrap());
    let router = build_router(store);
    let resp = router.oneshot(auth_get("/api/devices")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body, serde_json::json!({"has_token": false, "devices": []}));
}

#[tokio::test]
async fn mint_device_returns_script_and_locks_out_second_mint() {
    let store = Arc::new(Store::open(":memory:").unwrap());
    let router = build_router(store);

    let resp = router
        .clone()
        .oneshot(auth_post("/api/devices/mint"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    let script = body["script"].as_str().unwrap();
    assert!(script.contains("chrs_bridge_"), "script: {script}");
    assert!(script.contains("exec bridge"));

    // Second call MUST 410 — the raw bearer is unrecoverable from
    // storage, so the only path to a new script is rotate.
    let resp2 = router
        .oneshot(auth_post("/api/devices/mint"))
        .await
        .unwrap();
    assert_eq!(resp2.status(), StatusCode::GONE);
}

#[tokio::test]
async fn rotate_replaces_token_and_returns_fresh_script() {
    let store = Arc::new(Store::open(":memory:").unwrap());
    let router = build_router(store.clone());

    // First mint.
    let first = router
        .clone()
        .oneshot(auth_post("/api/devices/mint"))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let first_body = json_body(first).await;
    let first_script = first_body["script"].as_str().unwrap().to_string();

    // Rotate.
    let rotated = router
        .oneshot(auth_post("/api/devices/rotate"))
        .await
        .unwrap();
    assert_eq!(rotated.status(), StatusCode::OK);
    let rotated_body = json_body(rotated).await;
    let new_script = rotated_body["script"].as_str().unwrap();
    assert_ne!(first_script, new_script, "rotate must yield a fresh script");

    // The old token row should now be revoked, the new one active.
    let active = store
        .find_active_user_bridge_token(&format!("acc_{}", harness::TEST_USER_ID))
        .unwrap()
        .expect("a fresh bridge token should exist after rotate");
    assert!(active.revoked_at.is_none());
    assert_eq!(active.provider, "bridge");
    assert!(active.machine_id.is_none());
}

#[tokio::test]
async fn list_devices_returns_registered_machines() {
    let store = Arc::new(Store::open(":memory:").unwrap());
    let router = build_router(store.clone());

    let mint = router
        .clone()
        .oneshot(auth_post("/api/devices/mint"))
        .await
        .unwrap();
    assert_eq!(mint.status(), StatusCode::OK);

    let token = store
        .find_active_user_bridge_token(&format!("acc_{}", harness::TEST_USER_ID))
        .unwrap()
        .unwrap();

    // Simulate two bridge hellos.
    let _ = store
        .register_bridge_machine_hello(&token.token_hash, "laptop", Some("laptop.local"))
        .unwrap();
    let _ = store
        .register_bridge_machine_hello(&token.token_hash, "homelab", Some("homelab.local"))
        .unwrap();

    let resp = router.oneshot(auth_get("/api/devices")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["has_token"], true);
    let arr = body["devices"].as_array().unwrap();
    assert_eq!(arr.len(), 2);
    let ids: Vec<&str> = arr
        .iter()
        .map(|d| d["machine_id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"laptop"));
    assert!(ids.contains(&"homelab"));
}

#[tokio::test]
async fn kick_device_sets_kicked_at_and_blocks_reconnect_state_machine() {
    let store = Arc::new(Store::open(":memory:").unwrap());
    let router = build_router(store.clone());

    router
        .clone()
        .oneshot(auth_post("/api/devices/mint"))
        .await
        .unwrap();
    let token = store
        .find_active_user_bridge_token(&format!("acc_{}", harness::TEST_USER_ID))
        .unwrap()
        .unwrap();
    let _ = store
        .register_bridge_machine_hello(&token.token_hash, "laptop", None)
        .unwrap();

    let resp = router
        .oneshot(auth_delete("/api/devices/laptop"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // The state machine must now reject a reconnect for the same pair.
    let (_, outcome) = store
        .register_bridge_machine_hello(&token.token_hash, "laptop", None)
        .unwrap();
    assert_eq!(outcome, chorus::store::auth::HelloOutcome::Rejected);
}

#[tokio::test]
async fn forget_device_hard_deletes_row() {
    let store = Arc::new(Store::open(":memory:").unwrap());
    let router = build_router(store.clone());

    router
        .clone()
        .oneshot(auth_post("/api/devices/mint"))
        .await
        .unwrap();
    let token = store
        .find_active_user_bridge_token(&format!("acc_{}", harness::TEST_USER_ID))
        .unwrap()
        .unwrap();
    let _ = store
        .register_bridge_machine_hello(&token.token_hash, "laptop", None)
        .unwrap();
    store
        .kick_bridge_machine(&token.token_hash, "laptop")
        .unwrap();

    let resp = router
        .oneshot(auth_delete("/api/devices/laptop?forget=1"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // After Forget, a reconnect should be a fresh insert (not Rejected).
    let (_, outcome) = store
        .register_bridge_machine_hello(&token.token_hash, "laptop", None)
        .unwrap();
    assert_eq!(outcome, chorus::store::auth::HelloOutcome::Inserted);
}

#[tokio::test]
async fn dev_login_route_is_not_mounted_when_flag_off() {
    // Force-clear in case the test runner inherits the env from a parent.
    std::env::remove_var("CHORUS_DEV_AUTH");
    std::env::remove_var("CHORUS_DEV_AUTH_USERS");
    let store = Arc::new(Store::open(":memory:").unwrap());
    let router = build_router(store);
    let req = Request::builder()
        .method("POST")
        .uri("/api/auth/dev-login")
        .header("Content-Type", "application/json")
        .body(Body::from(r#"{"username":"alice"}"#))
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    // The route is genuinely absent — POST to it lands on the fallback,
    // which only serves GET, so we see 405. The opposite would be
    // `200 OK` (route mounted), which is the misconfiguration we're
    // protecting against. Accept anything *except* 200.
    assert_ne!(
        resp.status(),
        StatusCode::OK,
        "dev-login must NOT be reachable when CHORUS_DEV_AUTH is unset"
    );
}
