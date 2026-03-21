use axum::body::Body;
use axum::http::{Request, StatusCode};
use chorus::server::{build_router, build_router_with_lifecycle, AgentLifecycle};
use chorus::store::Store;
use chorus::models::*;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;

use chorus::activity_log::{self, ActivityLogMap};
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

#[derive(Default)]
struct MockLifecycle {
    started: Mutex<Vec<String>>,
    notified: Mutex<Vec<String>>,
    activity_logs: ActivityLogMap,
}

impl MockLifecycle {
    fn started_names(&self) -> Vec<String> {
        self.started.lock().unwrap().clone()
    }

    fn notified_names(&self) -> Vec<String> {
        self.notified.lock().unwrap().clone()
    }
}

impl AgentLifecycle for MockLifecycle {
    fn start_agent<'a>(
        &'a self,
        agent_name: &'a str,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>> {
        Box::pin(async move {
            self.started.lock().unwrap().push(agent_name.to_string());
            Ok(())
        })
    }

    fn notify_agent<'a>(
        &'a self,
        agent_name: &'a str,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>> {
        Box::pin(async move {
            self.notified.lock().unwrap().push(agent_name.to_string());
            Ok(())
        })
    }

    fn stop_agent<'a>(
        &'a self,
        _agent_name: &'a str,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }

    fn get_activity_log_data(&self, _agent_name: &str, _after_seq: Option<u64>) -> ActivityLogResponse {
        activity_log::get_activity_log(&self.activity_logs, _agent_name, _after_seq)
    }

    fn get_all_agent_activity_states(&self) -> Vec<(String, String, String)> {
        activity_log::all_activity_states(&self.activity_logs)
    }

    fn push_activity_entry(&self, agent_name: &str, entry: ActivityEntry) {
        activity_log::push_activity(&self.activity_logs, agent_name, entry);
    }
}

fn setup_with_lifecycle() -> (Arc<Store>, axum::Router, Arc<MockLifecycle>) {
    let store = Arc::new(Store::open(":memory:").unwrap());
    store.create_channel("general", Some("General"), ChannelType::Channel).unwrap();
    store.add_human("alice").unwrap();
    store.join_channel("general", "alice", SenderType::Human).unwrap();
    store.create_agent_record("bot1", "Bot 1", None, "claude", "sonnet").unwrap();
    store.join_channel("general", "bot1", SenderType::Agent).unwrap();
    let lifecycle = Arc::new(MockLifecycle::default());
    let router = build_router_with_lifecycle(store.clone(), lifecycle.clone());
    (store, router, lifecycle)
}

#[tokio::test]
async fn test_send_and_receive() {
    let (_store, app) = setup();

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
    let (store, app, lifecycle) = setup_with_lifecycle();

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
    assert!(
        store.is_member("general", "new-bot").unwrap(),
        "API-created agents should join existing channels"
    );

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/internal/agent/new-bot/server")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 1_000_000).await.unwrap();
    let info: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(info["channels"][0]["name"], "general");
    assert_eq!(info["channels"][0]["joined"], true);

    let new_bot = info["agents"]
        .as_array()
        .and_then(|agents| {
            agents
                .iter()
                .find(|agent_info| agent_info["name"] == "new-bot")
        })
        .expect("new agent should be present in server info");
    assert_eq!(new_bot["display_name"], "new-bot");
    assert_eq!(new_bot["description"], "A test agent");
    assert_eq!(new_bot["runtime"], "codex");
    assert_eq!(new_bot["model"], "gpt-5.4");
    assert_eq!(new_bot["status"], "inactive");
    assert_eq!(lifecycle.started_names(), vec!["new-bot".to_string()]);

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
async fn test_send_starts_inactive_agent_recipients() {
    let (store, app, lifecycle) = setup_with_lifecycle();
    store
        .create_agent_record("bot2", "Bot 2", None, "codex", "gpt-5.4")
        .unwrap();
    store.join_channel("general", "bot2", SenderType::Agent).unwrap();

    let send_req = serde_json::json!({ "target": "#general", "content": "wake bot2" });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/internal/agent/alice/send")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&send_req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(lifecycle.started_names(), vec!["bot1".to_string(), "bot2".to_string()]);
    assert!(lifecycle.notified_names().is_empty());
}

#[tokio::test]
async fn test_dm_send_starts_inactive_agent() {
    let (_store, app, lifecycle) = setup_with_lifecycle();

    let send_req = serde_json::json!({ "target": "dm:@bot1", "content": "hey bot1 via dm" });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/internal/agent/alice/send")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&send_req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(lifecycle.started_names(), vec!["bot1".to_string()], "DM to inactive agent must trigger start_agent");
    assert!(lifecycle.notified_names().is_empty());
}

#[tokio::test]
async fn test_dm_send_notifies_active_agent() {
    let (store, app, lifecycle) = setup_with_lifecycle();
    store.update_agent_status("bot1", AgentStatus::Active).unwrap();

    let send_req = serde_json::json!({ "target": "dm:@bot1", "content": "hey active bot1 via dm" });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/internal/agent/alice/send")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&send_req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(lifecycle.started_names().is_empty());
    assert_eq!(lifecycle.notified_names(), vec!["bot1".to_string()], "DM to active agent must trigger notify_agent");
}

#[tokio::test]
async fn test_send_notifies_active_agents() {
    let (store, app, lifecycle) = setup_with_lifecycle();
    store.update_agent_status("bot1", AgentStatus::Active).unwrap();

    let send_req = serde_json::json!({ "target": "#general", "content": "ping active bot" });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/internal/agent/alice/send")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&send_req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(lifecycle.started_names().is_empty());
    assert_eq!(lifecycle.notified_names(), vec!["bot1".to_string()]);
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

#[tokio::test]
async fn test_history_accepts_dm_target() {
    let (_store, app) = setup();

    let send_req = serde_json::json!({ "target": "dm:@alice", "content": "hello in dm" });
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/internal/agent/bot1/send")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&send_req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/internal/agent/bot1/history?channel=dm%3A%40alice&limit=10")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = axum::body::to_bytes(resp.into_body(), 1_000_000).await.unwrap();
    let hist: HistoryResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(hist.messages.len(), 1);
    assert_eq!(hist.messages[0].content, "hello in dm");
}

#[tokio::test]
async fn test_activity_log_includes_message_send_and_receive_events() {
    let (store, app, lifecycle) = setup_with_lifecycle();
    store.update_agent_status("bot1", AgentStatus::Active).unwrap();

    store
        .send_message("general", None, "alice", SenderType::Human, "hello bot1", &[])
        .unwrap();

    let recv_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/internal/agent/bot1/receive?block=false")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(recv_resp.status(), StatusCode::OK);

    let send_req = serde_json::json!({ "target": "#general", "content": "reply from bot1" });
    let send_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/internal/agent/bot1/send")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&send_req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(send_resp.status(), StatusCode::OK);

    let activity = lifecycle.get_activity_log_data("bot1", None);
    let kinds: Vec<&str> = activity
        .entries
        .iter()
        .map(|entry| match &entry.entry {
            ActivityEntry::MessageReceived { .. } => "message_received",
            ActivityEntry::MessageSent { .. } => "message_sent",
            ActivityEntry::Status { .. } => "status",
            ActivityEntry::Thinking { .. } => "thinking",
            ActivityEntry::ToolStart { .. } => "tool_start",
            ActivityEntry::Text { .. } => "text",
        })
        .collect();

    assert!(
        kinds.contains(&"message_received"),
        "activity log should surface received messages to the UI"
    );
    assert!(
        kinds.contains(&"message_sent"),
        "activity log should surface sent messages to the UI"
    );
}
