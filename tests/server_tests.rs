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
