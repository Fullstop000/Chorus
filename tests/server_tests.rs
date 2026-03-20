use axum::body::Body;
use axum::http::{Request, StatusCode};
use chorus::server::build_router;
use chorus::store::Store;
use chorus::models::*;
use std::sync::Arc;
use tower::ServiceExt;

fn setup() -> (Arc<Store>, axum::Router) {
    let store = Arc::new(Store::open(":memory:").unwrap());
    store.create_channel("general", Some("General"), ChannelType::Channel).unwrap();
    store.add_human("alice").unwrap();
    store.join_channel("general", "alice", SenderType::Human).unwrap();
    store.create_agent_record("bot1", "Bot 1", None, "claude", "sonnet").unwrap();
    store.join_channel("general", "bot1", SenderType::Agent).unwrap();
    let router = build_router(store.clone());
    (store, router)
}

#[tokio::test]
async fn test_send_and_receive() {
    let (store, app) = setup();

    let send_req = serde_json::json!({ "target": "#general", "content": "hello" });
    let resp = app.clone().oneshot(
        Request::builder()
            .method("POST")
            .uri("/internal/agent/bot1/send")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&send_req).unwrap()))
            .unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app.clone().oneshot(
        Request::builder()
            .uri("/internal/agent/bot1/receive?block=false")
            .body(Body::empty())
            .unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_server_info() {
    let (_store, app) = setup();
    let resp = app.oneshot(
        Request::builder()
            .uri("/internal/agent/bot1/server")
            .body(Body::empty())
            .unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 1_000_000).await.unwrap();
    let info: ServerInfo = serde_json::from_slice(&body).unwrap();
    assert_eq!(info.channels.len(), 1);
    assert_eq!(info.agents.len(), 1);
    assert_eq!(info.humans.len(), 1);
}

#[tokio::test]
async fn test_task_workflow() {
    let (_store, app) = setup();

    let req = serde_json::json!({ "channel": "#general", "tasks": [{"title": "Fix bug"}] });
    let resp = app.clone().oneshot(
        Request::builder()
            .method("POST")
            .uri("/internal/agent/bot1/tasks")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&req).unwrap()))
            .unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app.clone().oneshot(
        Request::builder()
            .uri("/internal/agent/bot1/tasks?channel=%23general")
            .body(Body::empty())
            .unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let req = serde_json::json!({ "channel": "#general", "task_numbers": [1] });
    let resp = app.clone().oneshot(
        Request::builder()
            .method("POST")
            .uri("/internal/agent/bot1/tasks/claim")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&req).unwrap()))
            .unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_whoami() {
    let (_store, app) = setup();
    let resp = app.oneshot(
        Request::builder()
            .uri("/api/whoami")
            .body(Body::empty())
            .unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 1_000_000).await.unwrap();
    let val: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(val["username"].as_str().is_some(), "username field missing");
}

#[tokio::test]
async fn test_create_agent_via_api() {
    let (store, app) = setup();

    // Create a new agent via POST /api/agents
    let req = serde_json::json!({
        "name": "new-bot",
        "description": "A test agent",
        "runtime": "codex",
        "model": "gpt-5.4"
    });
    let resp = app.clone().oneshot(
        Request::builder()
            .method("POST")
            .uri("/api/agents")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&req).unwrap()))
            .unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 1_000_000).await.unwrap();
    let val: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(val["name"], "new-bot");

    // Verify agent exists in store
    let agent = store.get_agent("new-bot").unwrap().expect("agent should exist");
    assert_eq!(agent.runtime, "codex");
    assert_eq!(agent.model, "gpt-5.4");
    assert_eq!(agent.description, Some("A test agent".to_string()));

    // Duplicate name should fail
    let resp = app.clone().oneshot(
        Request::builder()
            .method("POST")
            .uri("/api/agents")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&req).unwrap()))
            .unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // Empty name should fail
    let bad_req = serde_json::json!({ "name": "  " });
    let resp = app.oneshot(
        Request::builder()
            .method("POST")
            .uri("/api/agents")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&bad_req).unwrap()))
            .unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_history() {
    let (store, app) = setup();
    store.send_message("general", None, "alice", SenderType::Human, "msg 1", &[]).unwrap();
    store.send_message("general", None, "alice", SenderType::Human, "msg 2", &[]).unwrap();

    let resp = app.oneshot(
        Request::builder()
            .uri("/internal/agent/bot1/history?channel=%23general&limit=10")
            .body(Body::empty())
            .unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 1_000_000).await.unwrap();
    let hist: HistoryResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(hist.messages.len(), 2);
}
