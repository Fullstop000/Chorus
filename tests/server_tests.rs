use axum::body::Body;
use axum::http::{Request, StatusCode};
use chorus::models::*;
use chorus::server::{build_router, build_router_with_lifecycle, AgentLifecycle};
use chorus::store::Store;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;

use chorus::activity_log::{self, ActivityLogMap};
use tempfile::tempdir;
use tower::ServiceExt;

fn sorted(mut names: Vec<String>) -> Vec<String> {
    names.sort();
    names
}

fn setup() -> (Arc<Store>, axum::Router) {
    let store = Arc::new(Store::open(":memory:").unwrap());
    store
        .create_channel("general", Some("General"), ChannelType::Channel)
        .unwrap();
    store.add_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();
    store
        .create_agent_record("bot1", "Bot 1", None, "claude", "sonnet", &[])
        .unwrap();
    store
        .join_channel("general", "bot1", SenderType::Agent)
        .unwrap();
    let router = build_router(store.clone());
    (store, router)
}

fn setup_with_data_dir() -> (Arc<Store>, axum::Router, tempfile::TempDir) {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("chorus.db");
    let store = Arc::new(Store::open(db_path.to_str().unwrap()).unwrap());
    store
        .create_channel("general", Some("General"), ChannelType::Channel)
        .unwrap();
    store.add_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();
    store
        .create_agent_record("bot1", "Bot 1", None, "claude", "sonnet", &[])
        .unwrap();
    store
        .join_channel("general", "bot1", SenderType::Agent)
        .unwrap();
    let router = build_router(store.clone());
    (store, router, dir)
}

#[derive(Default)]
struct MockLifecycle {
    started: Mutex<Vec<(String, Option<ReceivedMessage>)>>,
    stopped: Mutex<Vec<String>>,
    notified: Mutex<Vec<String>>,
    activity_logs: ActivityLogMap,
}

impl MockLifecycle {
    fn started_names(&self) -> Vec<String> {
        self.started
            .lock()
            .unwrap()
            .iter()
            .map(|(name, _)| name.clone())
            .collect()
    }

    fn notified_names(&self) -> Vec<String> {
        self.notified.lock().unwrap().clone()
    }

    fn started_calls(&self) -> Vec<(String, Option<ReceivedMessage>)> {
        self.started.lock().unwrap().clone()
    }

    fn stopped_names(&self) -> Vec<String> {
        self.stopped.lock().unwrap().clone()
    }
}

impl AgentLifecycle for MockLifecycle {
    fn start_agent<'a>(
        &'a self,
        agent_name: &'a str,
        wake_message: Option<ReceivedMessage>,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>> {
        Box::pin(async move {
            self.started
                .lock()
                .unwrap()
                .push((agent_name.to_string(), wake_message));
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
        agent_name: &'a str,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>> {
        Box::pin(async move {
            self.stopped.lock().unwrap().push(agent_name.to_string());
            Ok(())
        })
    }

    fn get_activity_log_data(
        &self,
        _agent_name: &str,
        _after_seq: Option<u64>,
    ) -> ActivityLogResponse {
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
    store
        .create_channel("general", Some("General"), ChannelType::Channel)
        .unwrap();
    store.add_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();
    store
        .create_agent_record("bot1", "Bot 1", None, "claude", "sonnet", &[])
        .unwrap();
    store
        .join_channel("general", "bot1", SenderType::Agent)
        .unwrap();
    let lifecycle = Arc::new(MockLifecycle::default());
    let router = build_router_with_lifecycle(store.clone(), lifecycle.clone());
    (store, router, lifecycle)
}

#[tokio::test]
async fn test_send_and_receive() {
    let (_store, app) = setup();

    let send_req = serde_json::json!({ "target": "#general", "content": "hello" });
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
        .clone()
        .oneshot(
            Request::builder()
                .uri("/internal/agent/bot1/receive?block=false")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_receive_timeout_is_interpreted_in_milliseconds() {
    let (_store, app) = setup();

    let started = std::time::Instant::now();
    let response = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        app.oneshot(
            Request::builder()
                .uri("/internal/agent/bot1/receive?block=true&timeout=50")
                .body(Body::empty())
                .unwrap(),
        ),
    )
    .await
    .expect("50ms receive timeout should complete quickly")
    .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        started.elapsed() < std::time::Duration::from_millis(500),
        "receive timeout should use millisecond semantics end-to-end"
    );
}

#[tokio::test]
async fn test_send_starts_sleeping_agent_with_wake_message() {
    let (store, app, lifecycle) = setup_with_lifecycle();
    store
        .update_agent_status("bot1", AgentStatus::Sleeping)
        .unwrap();

    let send_req = serde_json::json!({ "target": "#general", "content": "wake up from sleep" });
    let response = app
        .clone()
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

    assert_eq!(response.status(), StatusCode::OK);

    let started = lifecycle.started_calls();
    assert_eq!(started.len(), 1);
    assert_eq!(started[0].0, "bot1");
    let wake_message = started[0]
        .1
        .as_ref()
        .expect("sleeping agent restart should include wake message");
    assert_eq!(wake_message.content, "wake up from sleep");
    assert_eq!(wake_message.sender_name, "alice");
    assert_eq!(wake_message.channel_name, "general");
    assert_eq!(wake_message.channel_type, "channel");
    assert!(lifecycle.notified_names().is_empty());
}

#[tokio::test]
async fn test_send_notifies_active_agent_without_restart() {
    let (store, app, lifecycle) = setup_with_lifecycle();
    store
        .update_agent_status("bot1", AgentStatus::Active)
        .unwrap();

    let send_req = serde_json::json!({ "target": "#general", "content": "stay online" });
    let response = app
        .clone()
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

    assert_eq!(response.status(), StatusCode::OK);
    assert!(lifecycle.started_names().is_empty());
    assert_eq!(lifecycle.notified_names(), vec!["bot1".to_string()]);
}

#[tokio::test]
async fn test_server_info() {
    let (_store, app) = setup();
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/internal/agent/bot1/server")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 1_000_000)
        .await
        .unwrap();
    let info: ServerInfo = serde_json::from_slice(&body).unwrap();
    assert_eq!(info.channels.len(), 1);
    assert_eq!(info.agents.len(), 1);
    assert_eq!(info.humans.len(), 1);
}

#[tokio::test]
async fn test_task_workflow() {
    let (_store, app) = setup();

    let req = serde_json::json!({ "channel": "#general", "tasks": [{"title": "Fix bug"}] });
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/internal/agent/bot1/tasks")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/internal/agent/bot1/tasks?channel=%23general")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let req = serde_json::json!({ "channel": "#general", "task_numbers": [1] });
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/internal/agent/bot1/tasks/claim")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_whoami() {
    let (_store, app) = setup();
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/whoami")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 1_000_000)
        .await
        .unwrap();
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
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/agents")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 1_000_000)
        .await
        .unwrap();
    let val: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(val["name"], "new-bot");

    // Verify agent exists in store
    let agent = store
        .get_agent("new-bot")
        .unwrap()
        .expect("agent should exist");
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
    let body = axum::body::to_bytes(resp.into_body(), 1_000_000)
        .await
        .unwrap();
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
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/agents")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // Empty name should fail
    let bad_req = serde_json::json!({ "name": "  " });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/agents")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&bad_req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_get_and_update_agent_via_api() {
    let (store, app, lifecycle) = setup_with_lifecycle();
    store
        .update_agent_status("bot1", AgentStatus::Active)
        .unwrap();

    let detail_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/agents/bot1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(detail_resp.status(), StatusCode::OK);

    let update_req = serde_json::json!({
        "display_name": "Updated Bot",
        "description": "Updated role",
        "runtime": "codex",
        "model": "gpt-5.4",
        "envVars": [{"key": "DEBUG", "value": "1"}]
    });
    let update_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri("/api/agents/bot1")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&update_req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(update_resp.status(), StatusCode::OK);

    let agent = store.get_agent("bot1").unwrap().unwrap();
    assert_eq!(agent.display_name, "Updated Bot");
    assert_eq!(agent.runtime, "codex");
    assert_eq!(agent.model, "gpt-5.4");
    assert_eq!(agent.env_vars.len(), 1);
    assert_eq!(agent.env_vars[0].key, "DEBUG");
    assert_eq!(lifecycle.stopped_names(), vec!["bot1".to_string()]);
    assert_eq!(lifecycle.started_names(), vec!["bot1".to_string()]);
}

#[tokio::test]
async fn test_restart_agent_reset_session_preserves_workspace() {
    let (store, app, dir) = setup_with_data_dir();
    store
        .update_agent_session("bot1", Some("thread-123"))
        .unwrap();
    let workspace_dir = dir.path().join("agents").join("bot1").join("notes");
    std::fs::create_dir_all(&workspace_dir).unwrap();
    std::fs::write(workspace_dir.join("plan.md"), "hello").unwrap();

    let app = build_router_with_lifecycle(store.clone(), Arc::new(MockLifecycle::default()));
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/agents/bot1/restart")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&serde_json::json!({ "mode": "reset_session" })).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let agent = store.get_agent("bot1").unwrap().unwrap();
    assert_eq!(agent.session_id, None);
    assert!(workspace_dir.join("plan.md").exists());
}

#[tokio::test]
async fn test_delete_agent_marks_history_and_preserves_workspace() {
    let (store, app, dir) = setup_with_data_dir();
    let workspace_dir = dir.path().join("agents").join("bot1").join("notes");
    std::fs::create_dir_all(&workspace_dir).unwrap();
    std::fs::write(workspace_dir.join("plan.md"), "hello").unwrap();
    store
        .send_message("general", None, "bot1", SenderType::Agent, "hello", &[])
        .unwrap();

    let app = build_router_with_lifecycle(store.clone(), Arc::new(MockLifecycle::default()));
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/agents/bot1/delete")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&serde_json::json!({ "mode": "preserve_workspace" })).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(store.get_agent("bot1").unwrap().is_none());
    assert!(workspace_dir.join("plan.md").exists());

    let history_response = app
        .oneshot(
            Request::builder()
                .uri("/internal/agent/alice/history?channel=%23general&limit=10")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(history_response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(history_response.into_body(), 1_000_000)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["messages"][0]["senderDeleted"], true);
}

#[tokio::test]
async fn test_send_starts_inactive_agent_recipients() {
    let (store, app, lifecycle) = setup_with_lifecycle();
    store
        .create_agent_record("bot2", "Bot 2", None, "codex", "gpt-5.4", &[])
        .unwrap();
    store
        .join_channel("general", "bot2", SenderType::Agent)
        .unwrap();

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
    assert_eq!(
        lifecycle.started_names(),
        vec!["bot1".to_string(), "bot2".to_string()]
    );
    assert!(lifecycle.notified_names().is_empty());
}

#[tokio::test]
async fn test_thread_send_only_starts_parent_author_agent() {
    let (store, app, lifecycle) = setup_with_lifecycle();
    store
        .create_agent_record("bot2", "Bot 2", None, "codex", "gpt-5.4", &[])
        .unwrap();
    store
        .join_channel("general", "bot2", SenderType::Agent)
        .unwrap();

    let parent_message_id = store
        .send_message(
            "general",
            None,
            "bot1",
            SenderType::Agent,
            "parent from bot1",
            &[],
        )
        .unwrap();
    let thread_target = format!("#general:{}", &parent_message_id[..8]);

    let send_req = serde_json::json!({
        "target": thread_target,
        "content": "thread reply for parent author only"
    });
    let response = app
        .clone()
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

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        lifecycle.started_names(),
        vec!["bot1".to_string()],
        "thread replies should wake only the parent author when no other agent has joined the thread"
    );
}

#[tokio::test]
async fn test_thread_send_starts_parent_author_and_existing_thread_repliers() {
    let (store, app, lifecycle) = setup_with_lifecycle();
    store
        .create_agent_record("bot2", "Bot 2", None, "codex", "gpt-5.4", &[])
        .unwrap();
    store
        .join_channel("general", "bot2", SenderType::Agent)
        .unwrap();
    store
        .create_agent_record("bot3", "Bot 3", None, "claude", "sonnet", &[])
        .unwrap();
    store
        .join_channel("general", "bot3", SenderType::Agent)
        .unwrap();

    let parent_message_id = store
        .send_message(
            "general",
            None,
            "bot1",
            SenderType::Agent,
            "parent from bot1",
            &[],
        )
        .unwrap();
    store
        .send_message(
            "general",
            Some(&parent_message_id),
            "bot2",
            SenderType::Agent,
            "bot2 already joined the thread",
            &[],
        )
        .unwrap();

    let thread_target = format!("#general:{}", &parent_message_id[..8]);
    let send_req = serde_json::json!({
        "target": thread_target,
        "content": "thread reply for joined participants"
    });
    let response = app
        .clone()
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

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        sorted(lifecycle.started_names()),
        vec!["bot1".to_string(), "bot2".to_string()],
        "thread replies should wake the parent author plus agent participants already present in that thread"
    );
}

#[tokio::test]
async fn test_agent_thread_reply_to_human_parent_does_not_start_unrelated_agents() {
    let (store, app, lifecycle) = setup_with_lifecycle();
    store
        .create_agent_record("bot2", "Bot 2", None, "codex", "gpt-5.4", &[])
        .unwrap();
    store
        .join_channel("general", "bot2", SenderType::Agent)
        .unwrap();
    store
        .update_agent_status("bot1", AgentStatus::Active)
        .unwrap();

    let parent_message_id = store
        .send_message(
            "general",
            None,
            "alice",
            SenderType::Human,
            "human started the thread",
            &[],
        )
        .unwrap();

    let thread_target = format!("#general:{}", &parent_message_id[..8]);
    let send_req = serde_json::json!({
        "target": thread_target,
        "content": "bot1 joins the human thread"
    });
    let response = app
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

    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        lifecycle.started_names().is_empty(),
        "an agent joining a human-owned thread should not wake unrelated agents"
    );
    assert!(
        lifecycle.notified_names().is_empty(),
        "thread replies to a human-owned thread should not notify unrelated active agents"
    );
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
    assert_eq!(
        lifecycle.started_names(),
        vec!["bot1".to_string()],
        "DM to inactive agent must trigger start_agent"
    );
    assert!(lifecycle.notified_names().is_empty());
}

#[tokio::test]
async fn test_dm_send_notifies_active_agent() {
    let (store, app, lifecycle) = setup_with_lifecycle();
    store
        .update_agent_status("bot1", AgentStatus::Active)
        .unwrap();

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
    assert_eq!(
        lifecycle.notified_names(),
        vec!["bot1".to_string()],
        "DM to active agent must trigger notify_agent"
    );
}

#[tokio::test]
async fn test_send_notifies_active_agents() {
    let (store, app, lifecycle) = setup_with_lifecycle();
    store
        .update_agent_status("bot1", AgentStatus::Active)
        .unwrap();

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
    store
        .send_message("general", None, "alice", SenderType::Human, "msg 1", &[])
        .unwrap();
    store
        .send_message("general", None, "alice", SenderType::Human, "msg 2", &[])
        .unwrap();

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/internal/agent/bot1/history?channel=%23general&limit=10")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 1_000_000)
        .await
        .unwrap();
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

    let body = axum::body::to_bytes(resp.into_body(), 1_000_000)
        .await
        .unwrap();
    let hist: HistoryResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(hist.messages.len(), 1);
    assert_eq!(hist.messages[0].content, "hello in dm");
}

#[tokio::test]
async fn test_activity_log_includes_message_send_and_receive_events() {
    let (store, app, lifecycle) = setup_with_lifecycle();
    store
        .update_agent_status("bot1", AgentStatus::Active)
        .unwrap();

    store
        .send_message(
            "general",
            None,
            "alice",
            SenderType::Human,
            "hello bot1",
            &[],
        )
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

#[tokio::test]
async fn test_upload_uses_configured_data_dir() {
    let (store, app, dir) = setup_with_data_dir();
    let boundary = "chorus-boundary";
    let body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"qa.txt\"\r\nContent-Type: text/plain\r\n\r\nhello upload\r\n--{boundary}--\r\n"
    );

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/internal/agent/alice/upload")
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = axum::body::to_bytes(resp.into_body(), 1_000_000)
        .await
        .unwrap();
    let val: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let attachment_id = val["id"].as_str().expect("attachment id");
    let attachment = store
        .get_attachment(attachment_id)
        .unwrap()
        .expect("attachment record");

    assert!(
        attachment
            .stored_path
            .starts_with(dir.path().join("attachments").to_string_lossy().as_ref()),
        "attachment should be stored under the configured data dir"
    );
}

#[tokio::test]
async fn test_workspace_lists_files_from_configured_data_dir() {
    let (_store, app, dir) = setup_with_data_dir();
    let workspace_dir = dir.path().join("agents").join("bot1").join("notes");
    std::fs::create_dir_all(&workspace_dir).unwrap();
    std::fs::write(workspace_dir.join("plan.md"), "# test\n").unwrap();

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/agents/bot1/workspace")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = axum::body::to_bytes(resp.into_body(), 1_000_000)
        .await
        .unwrap();
    let val: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        val["path"].as_str(),
        Some(
            dir.path()
                .join("agents")
                .join("bot1")
                .to_string_lossy()
                .as_ref()
        )
    );
    let files = val["files"].as_array().expect("files array");
    assert!(files.iter().any(|entry| entry == "notes/"));
    assert!(files.iter().any(|entry| entry == "notes/plan.md"));
}

#[tokio::test]
async fn test_workspace_file_returns_content_from_configured_data_dir() {
    let (_store, app, dir) = setup_with_data_dir();
    let workspace_dir = dir.path().join("agents").join("bot1").join("notes");
    std::fs::create_dir_all(&workspace_dir).unwrap();
    std::fs::write(workspace_dir.join("plan.md"), "# plan\nship it\n").unwrap();

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/agents/bot1/workspace/file?path=notes%2Fplan.md")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = axum::body::to_bytes(resp.into_body(), 1_000_000)
        .await
        .unwrap();
    let val: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(val["path"].as_str(), Some("notes/plan.md"));
    assert_eq!(val["content"].as_str(), Some("# plan\nship it\n"));
    assert_eq!(val["truncated"].as_bool(), Some(false));
    assert_eq!(val["sizeBytes"].as_u64(), Some(15));
    assert!(val["modifiedMs"].as_u64().is_some());
}

#[tokio::test]
async fn test_send_can_skip_agent_delivery() {
    let (_store, app, lifecycle) = setup_with_lifecycle();

    let send_req = serde_json::json!({
        "target": "#general",
        "content": "create one task only",
        "suppressAgentDelivery": true
    });
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
    assert!(lifecycle.notified_names().is_empty());
}

// ──────────────────────────────────────────────────────────────────────────────
// Knowledge store tests
// ──────────────────────────────────────────────────────────────────────────────

/// Helper: build a store with #general and #shared-memory already created.
fn setup_knowledge() -> (Arc<Store>, axum::Router) {
    let store = Arc::new(Store::open(":memory:").unwrap());
    store
        .create_channel("general", Some("General"), ChannelType::Channel)
        .unwrap();
    store.add_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();
    store
        .create_agent_record("bot1", "Bot 1", None, "claude", "sonnet", &[])
        .unwrap();
    store
        .join_channel("general", "bot1", SenderType::Agent)
        .unwrap();
    // Ensure system channel exists (mirrors main.rs startup).
    store
        .ensure_system_channel("shared-memory", "Agent group memory")
        .unwrap();
    let router = build_router(store.clone());
    (store, router)
}

// 1. remember happy path: knowledge entry is stored and breadcrumb appears in #shared-memory
#[tokio::test]
async fn knowledge_remember_happy_path() {
    let (store, app) = setup_knowledge();

    let body = serde_json::json!({
        "key": "rate-limiting approach",
        "value": "token bucket is best for this codebase",
        "tags": ["research", "task-42"]
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/internal/agent/bot1/remember")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let data: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let id = data["id"].as_str().expect("id should be present");
    assert!(!id.is_empty());

    // Knowledge entry must be retrievable via recall.
    let entries = store.recall(Some("token bucket"), None).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].key, "rate-limiting approach");
    assert_eq!(entries[0].author_agent_id, "bot1");

    // Breadcrumb message must appear in #shared-memory channel.
    let (msgs, _) = store
        .get_history("shared-memory", None, 10, None, None)
        .unwrap();
    assert_eq!(msgs.len(), 1);
    assert!(msgs[0].content.contains("rate-limiting approach"));
}

// 2. remember when #shared-memory is missing: best-effort — knowledge is stored, no panic
#[tokio::test]
async fn knowledge_remember_channel_missing() {
    // Build store WITHOUT #shared-memory to test graceful degradation.
    let store = Arc::new(Store::open(":memory:").unwrap());
    store
        .create_channel("general", Some("General"), ChannelType::Channel)
        .unwrap();
    store.add_human("alice").unwrap();
    store
        .create_agent_record("bot1", "Bot 1", None, "claude", "sonnet", &[])
        .unwrap();
    let router = build_router(store.clone());

    let body = serde_json::json!({
        "key": "no-channel test",
        "value": "should store even without shared-memory",
        "tags": []
    });
    let resp = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/internal/agent/bot1/remember")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    // Knowledge write must succeed even if channel post fails.
    assert_eq!(resp.status(), StatusCode::OK);
    let entries = store.recall(Some("no-channel"), None).unwrap();
    assert_eq!(entries.len(), 1);
}

// 3. recall FTS5 match: store a fact and find it by keyword
#[tokio::test]
async fn knowledge_recall_fts_match() {
    let (store, app) = setup_knowledge();

    let body = serde_json::json!({
        "key": "auth flow",
        "value": "uses JWT tokens with 1h expiry",
        "tags": ["auth"]
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/internal/agent/bot1/remember")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let entries = store.recall(Some("JWT"), None).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].key, "auth flow");
}

// 4. recall tag filter: only entries with matching tag are returned
#[tokio::test]
async fn knowledge_recall_tag_filter() {
    let (store, _app) = setup_knowledge();

    store
        .remember("finding A", "value A", "research task-1", "bot1", None)
        .unwrap();
    store
        .remember("finding B", "value B", "design task-2", "bot1", None)
        .unwrap();

    let by_research = store.recall(None, Some("research")).unwrap();
    assert_eq!(by_research.len(), 1);
    assert_eq!(by_research[0].key, "finding A");

    let by_task2 = store.recall(None, Some("task-2")).unwrap();
    assert_eq!(by_task2.len(), 1);
    assert_eq!(by_task2[0].key, "finding B");
}

// 5. recall empty result: non-matching query returns empty list, not an error
#[tokio::test]
async fn knowledge_recall_empty_result() {
    let (store, _app) = setup_knowledge();

    let entries = store.recall(Some("nonexistent-term-xyz"), None).unwrap();
    assert!(entries.is_empty());
}

// 6. ChannelType::System round-trips through the DB
#[tokio::test]
async fn channel_type_system_parse() {
    let (store, _app) = setup_knowledge();

    // #shared-memory was created in setup_knowledge as a system channel.
    let ch = store
        .find_channel_by_name("shared-memory")
        .unwrap()
        .unwrap();
    assert_eq!(ch.channel_type, ChannelType::System);

    // A regular channel should not be System.
    let gen = store.find_channel_by_name("general").unwrap().unwrap();
    assert_eq!(gen.channel_type, ChannelType::Channel);
}

// 7. list_channels excludes system channels
#[tokio::test]
async fn list_channels_excludes_system() {
    let (store, _app) = setup_knowledge();

    let channels = store.list_channels().unwrap();
    let names: Vec<&str> = channels.iter().map(|c| c.name.as_str()).collect();
    assert!(names.contains(&"general"), "general must be listed");
    assert!(
        !names.contains(&"shared-memory"),
        "shared-memory must not appear in list_channels"
    );
}

// 8. send_message to system channel is rejected
#[tokio::test]
async fn send_message_to_system_channel_rejected() {
    let (store, app) = setup_knowledge();

    // Join bot1 to #shared-memory so channel resolution succeeds (guard fires before membership check).
    store
        .join_channel("shared-memory", "bot1", SenderType::Agent)
        .unwrap();

    let body = serde_json::json!({
        "target": "#shared-memory",
        "content": "direct post attempt"
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/internal/agent/bot1/send")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let data: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(data["error"]
        .as_str()
        .unwrap_or("")
        .contains("mcp_chat_remember"));
}

// 9. ensure_system_channel is idempotent — calling twice creates only one channel
#[tokio::test]
async fn shared_memory_auto_creation_idempotent() {
    let store = Arc::new(Store::open(":memory:").unwrap());
    // Call twice — must not panic or duplicate the row (UNIQUE constraint + explicit check).
    store
        .ensure_system_channel("shared-memory", "Group memory")
        .unwrap();
    store
        .ensure_system_channel("shared-memory", "Group memory")
        .unwrap();

    // Verify it exists and is of the correct type.
    let ch = store
        .find_channel_by_name("shared-memory")
        .unwrap()
        .expect("channel should exist after ensure");
    assert_eq!(ch.channel_type, ChannelType::System);

    // Verify list_channels (which excludes system) still lists nothing for this fresh store.
    let listed = store.list_channels().unwrap();
    assert!(
        listed.iter().all(|c| c.name != "shared-memory"),
        "shared-memory must not appear in list_channels"
    );
}

// 10. tags are stored as FTS5 tokens — partial tag name does not match
#[tokio::test]
async fn knowledge_tags_fts_token_boundary() {
    let (store, _app) = setup_knowledge();

    store
        .remember("boundary test", "some value", "task-42", "bot1", None)
        .unwrap();

    // Exact tag match must work.
    let exact = store.recall(None, Some("task-42")).unwrap();
    assert_eq!(exact.len(), 1);

    // A different tag that is NOT a prefix/substring in the tags string must not match.
    let no_match = store.recall(None, Some("task-4")).unwrap();
    assert!(
        no_match.is_empty(),
        "partial tag 'task-4' should not match 'task-42'"
    );
}
