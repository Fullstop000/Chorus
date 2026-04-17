mod harness;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use chorus::agent::activity_log::ActivityLogResponse;
use chorus::agent::drivers::ProbeAuth;
use chorus::agent::runtime_status::{RuntimeStatusInfo, RuntimeStatusProvider};
use chorus::agent::AgentLifecycle;
use chorus::agent::AgentRuntime;
use chorus::server::dto::ChannelInfo;
use chorus::server::dto::ServerInfo;
use chorus::server::{build_router_with_services, AgentDetailResponse, HistoryResponse};
use chorus::store::agents::AgentStatus;
use chorus::store::channels::ChannelType;
use chorus::store::messages::{CreateMessage, ReceivedMessage, SenderType};
use chorus::store::AgentRecordUpsert;
use chorus::store::Store;
use harness::{build_router, build_router_with_lifecycle};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;

use chorus::agent::activity_log::{self, ActivityLogMap};
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
    store.create_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();
    store
        .create_agent_record(&AgentRecordUpsert {
            name: "bot1",
            display_name: "Bot 1",
            description: None,
            system_prompt: None,
            runtime: "claude",
            model: "sonnet",
            reasoning_effort: None,
            env_vars: &[],
        })
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
    store.create_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();
    store
        .create_agent_record(&AgentRecordUpsert {
            name: "bot1",
            display_name: "Bot 1",
            description: None,
            system_prompt: None,
            runtime: "claude",
            model: "sonnet",
            reasoning_effort: None,
            env_vars: &[],
        })
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

struct MockRuntimeStatusProvider {
    statuses: Vec<RuntimeStatusInfo>,
    models_by_runtime: Vec<(String, Vec<String>)>,
}

struct FailStartLifecycle;

#[async_trait::async_trait]
impl RuntimeStatusProvider for MockRuntimeStatusProvider {
    async fn list_statuses(&self) -> anyhow::Result<Vec<RuntimeStatusInfo>> {
        Ok(self.statuses.clone())
    }

    async fn list_models(&self, runtime: AgentRuntime) -> anyhow::Result<Vec<String>> {
        let key = runtime.as_str().to_string();
        Ok(self
            .models_by_runtime
            .iter()
            .find(|(name, _)| name == &key)
            .map(|(_, models)| models.clone())
            .unwrap_or_default())
    }
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
}

impl AgentLifecycle for FailStartLifecycle {
    fn start_agent<'a>(
        &'a self,
        _agent_name: &'a str,
        _wake_message: Option<ReceivedMessage>,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>> {
        Box::pin(async { Err(anyhow::anyhow!("runtime unavailable")) })
    }

    fn notify_agent<'a>(
        &'a self,
        _agent_name: &'a str,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }

    fn stop_agent<'a>(
        &'a self,
        _agent_name: &'a str,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }

    fn get_activity_log_data(
        &self,
        _agent_name: &str,
        _after_seq: Option<u64>,
    ) -> ActivityLogResponse {
        ActivityLogResponse {
            entries: vec![],
            agent_activity: "offline".to_string(),
            agent_detail: String::new(),
        }
    }

    fn get_all_agent_activity_states(&self) -> Vec<(String, String, String)> {
        vec![]
    }
}

fn setup_with_lifecycle() -> (Arc<Store>, axum::Router, Arc<MockLifecycle>) {
    let store = Arc::new(Store::open(":memory:").unwrap());
    store
        .create_channel("general", Some("General"), ChannelType::Channel)
        .unwrap();
    store.create_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();
    store
        .create_agent_record(&AgentRecordUpsert {
            name: "bot1",
            display_name: "Bot 1",
            description: None,
            system_prompt: None,
            runtime: "claude",
            model: "sonnet",
            reasoning_effort: None,
            env_vars: &[],
        })
        .unwrap();
    store
        .join_channel("general", "bot1", SenderType::Agent)
        .unwrap();
    let lifecycle = Arc::new(MockLifecycle::default());
    let router = build_router_with_lifecycle(store.clone(), lifecycle.clone());
    (store, router, lifecycle)
}

fn setup_with_runtime_statuses(
    statuses: Vec<RuntimeStatusInfo>,
    models_by_runtime: Vec<(String, Vec<String>)>,
) -> (Arc<Store>, axum::Router, Arc<MockLifecycle>) {
    let store = Arc::new(Store::open(":memory:").unwrap());
    store
        .create_channel("general", Some("General"), ChannelType::Channel)
        .unwrap();
    store.create_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();
    store
        .create_agent_record(&AgentRecordUpsert {
            name: "bot1",
            display_name: "Bot 1",
            description: None,
            system_prompt: None,
            runtime: "claude",
            model: "sonnet",
            reasoning_effort: None,
            env_vars: &[],
        })
        .unwrap();
    store
        .join_channel("general", "bot1", SenderType::Agent)
        .unwrap();
    let lifecycle = Arc::new(MockLifecycle::default());
    let runtime_status_provider = Arc::new(MockRuntimeStatusProvider {
        statuses,
        models_by_runtime,
    });
    let router = build_router_with_services(
        store.clone(),
        lifecycle.clone(),
        runtime_status_provider,
        Vec::new(),
    );
    (store, router, lifecycle)
}

fn setup_with_lifecycle_and_data_dir() -> (
    Arc<Store>,
    axum::Router,
    Arc<MockLifecycle>,
    tempfile::TempDir,
) {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("chorus.db");
    let store = Arc::new(Store::open(db_path.to_str().unwrap()).unwrap());
    store
        .create_channel("general", Some("General"), ChannelType::Channel)
        .unwrap();
    store.create_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();
    store
        .create_agent_record(&AgentRecordUpsert {
            name: "bot1",
            display_name: "Bot 1",
            description: None,
            system_prompt: None,
            runtime: "claude",
            model: "sonnet",
            reasoning_effort: None,
            env_vars: &[],
        })
        .unwrap();
    store
        .join_channel("general", "bot1", SenderType::Agent)
        .unwrap();
    let lifecycle = Arc::new(MockLifecycle::default());
    let router = build_router_with_lifecycle(store.clone(), lifecycle.clone());
    (store, router, lifecycle, dir)
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
async fn test_history_includes_last_read_seq() {
    let (_store, app) = setup();

    let send_req = serde_json::json!({ "target": "#general", "content": "hello" });
    let send_resp = app
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
    assert_eq!(send_resp.status(), StatusCode::OK);

    let history_resp = app
        .oneshot(
            Request::builder()
                .uri("/internal/agent/alice/history?channel=%23general&limit=10")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(history_resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(history_resp.into_body(), 1_000_000)
        .await
        .unwrap();
    let history: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(history["last_read_seq"], 1);
    assert!(!history["messages"].as_array().unwrap().is_empty());
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
    let (store, app) = setup();
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
    let bot1 = store.get_agent("bot1").unwrap().unwrap();
    assert_eq!(info.channels.len(), 1);
    assert_eq!(info.agents.len(), 1);
    assert_eq!(info.agents[0].id, bot1.id);
    assert_eq!(info.humans.len(), 1);
}

#[tokio::test]
async fn test_list_agents_via_public_api_includes_ids() {
    let (store, app) = setup();
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/agents")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = axum::body::to_bytes(resp.into_body(), 1_000_000)
        .await
        .unwrap();
    let agents: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let bot1 = store.get_agent("bot1").unwrap().unwrap();
    assert_eq!(agents.as_array().unwrap().len(), 1);
    assert_eq!(agents[0]["id"], bot1.id);
}

#[tokio::test]
async fn test_ui_server_info_is_shell_only() {
    let (_store, app) = setup();
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/server-info")
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
    assert!(info.get("system_channels").is_some());
    assert!(info.get("humans").is_some());
    assert!(info.get("channels").is_none());
    assert!(info.get("agents").is_none());
}

#[tokio::test]
async fn test_shared_memory_endpoints_are_not_registered() {
    let (_store, app) = setup();

    let remember = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/internal/agent/bot1/remember")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&serde_json::json!({
                        "key": "obsolete",
                        "value": "obsolete",
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(matches!(
        remember.status(),
        StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED
    ));

    let recall = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/internal/agent/bot1/recall?query=obsolete")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let recall_content_type = recall
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    assert!(
        recall.status() != StatusCode::OK || !recall_content_type.contains("application/json"),
        "recall endpoint should no longer return an API JSON payload"
    );
}

#[tokio::test]
async fn test_list_humans_via_public_api() {
    let (_store, app) = setup();
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/humans")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 1_000_000)
        .await
        .unwrap();
    let humans: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(humans.as_array().unwrap().len(), 1);
    assert_eq!(humans[0]["name"], "alice");
}

#[tokio::test]
async fn test_public_inbox_matches_current_human() {
    let (store, app) = setup();
    let viewer = whoami::username();
    if viewer != "alice" {
        store.create_human(&viewer).unwrap();
        store
            .join_channel("general", &viewer, SenderType::Human)
            .unwrap();
    }
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/inbox")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 1_000_000)
        .await
        .unwrap();
    let inbox: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(inbox["conversations"].is_array());
}

#[tokio::test]
async fn test_public_conversation_inbox_notification_matches_human_viewer() {
    let (store, app) = setup();
    let viewer = whoami::username();
    if viewer != "alice" {
        store.create_human(&viewer).unwrap();
        store
            .join_channel("general", &viewer, SenderType::Human)
            .unwrap();
    }
    let channel_id = store
        .get_channel_by_name("general")
        .unwrap()
        .expect("general channel should exist")
        .id;
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/conversations/{channel_id}/inbox-notification"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 1_000_000)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(v["conversation"].is_object());
    assert_eq!(
        v["conversation"]["conversationId"].as_str().unwrap(),
        channel_id
    );
    assert!(v["conversation"]["unreadCount"].is_number());
    assert!(v["conversation"]["latestSeq"].is_number());
}

#[tokio::test]
async fn test_public_conversation_messages_route_uses_conversation_id() {
    let (store, app) = setup();
    let viewer = whoami::username();
    if viewer != "alice" {
        store.create_human(&viewer).unwrap();
        store
            .join_channel("general", &viewer, SenderType::Human)
            .unwrap();
    }
    let channel_id = store
        .get_channel_by_name("general")
        .unwrap()
        .expect("general channel should exist")
        .id;
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/conversations/{channel_id}/messages?limit=10"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 1_000_000)
        .await
        .unwrap();
    let history: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(history["messages"].is_array());
    assert_eq!(history["last_read_seq"], 0);
}

#[tokio::test]
async fn test_public_conversation_tasks_route_uses_conversation_id() {
    let (store, app) = setup();
    store
        .create_tasks("general", "alice", &["task from public route"])
        .unwrap();
    let channel_id = store
        .get_channel_by_name("general")
        .unwrap()
        .expect("general channel should exist")
        .id;
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/conversations/{channel_id}/tasks?status=all"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 1_000_000)
        .await
        .unwrap();
    let tasks: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(tasks["tasks"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn test_public_dm_route_returns_or_creates_dm_for_current_human() {
    let (store, app) = setup();
    let viewer = whoami::username();
    if viewer != "alice" {
        store.create_human(&viewer).unwrap();
    }

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/dms/bot1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 1_000_000)
        .await
        .unwrap();
    let dm: ChannelInfo = serde_json::from_slice(&body).unwrap();
    assert_eq!(dm.channel_type.as_deref(), Some("dm"));
    assert!(dm.joined);

    let list_resp = app
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/channels?member={viewer}&include_dm=true&include_system=true"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(list_resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(list_resp.into_body(), 1_000_000)
        .await
        .unwrap();
    let channels: Vec<ChannelInfo> = serde_json::from_slice(&body).unwrap();
    assert!(channels.iter().any(|channel| channel.id == dm.id));
}

#[tokio::test]
async fn test_list_channels_honors_search_params() {
    let (store, app) = setup();
    store.ensure_builtin_channels("alice").unwrap();
    store
        .create_channel("engineering", Some("Engineering"), ChannelType::Channel)
        .unwrap();
    store
        .create_channel("eng-team", Some("Engineering"), ChannelType::Team)
        .unwrap();
    store
        .create_channel("dm-bot1", None, ChannelType::Dm)
        .unwrap();

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/channels?include_system=true&include_dm=true")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 1_000_000)
        .await
        .unwrap();
    let channels: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let channels = channels.as_array().expect("channel list array");
    assert!(channels.iter().any(|entry| entry["name"] == "engineering"));
    assert!(channels.iter().any(|entry| entry["name"] == "eng-team"));
    assert!(channels.iter().any(|entry| entry["name"] == "all"));
    assert!(channels.iter().any(|entry| entry["name"] == "dm-bot1"));
}

#[tokio::test]
async fn test_update_channel_via_api_normalizes_and_preserves_identity() {
    let (store, app) = setup();
    let channel_id = store.get_channel_by_name("general").unwrap().unwrap().id;

    let req = serde_json::json!({
        "name": "#Engineering",
        "description": "Platform work"
    });
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/channels/{channel_id}"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let renamed = store.get_channel_by_name("engineering").unwrap().unwrap();
    assert_eq!(renamed.id, channel_id);
    assert_eq!(renamed.description.as_deref(), Some("Platform work"));
    assert!(store.get_channel_by_name("general").unwrap().is_none());
}

#[tokio::test]
async fn test_update_channel_via_api_rejects_duplicate_name() {
    let (store, app) = setup();
    store
        .create_channel("random", Some("Random"), ChannelType::Channel)
        .unwrap();
    let channel_id = store.get_channel_by_name("general").unwrap().unwrap().id;

    let req = serde_json::json!({
        "name": "#RANDOM",
        "description": "Duplicate"
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/channels/{channel_id}"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let (store, app) = setup();

    let req = serde_json::json!({
        "name": "#Engineering",
        "description": "Platform work"
    });
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/channels")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let channel = store.get_channel_by_name("engineering").unwrap().unwrap();
    let members = store.get_channel_members(&channel.id).unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].member_name, whoami::username());
    assert_eq!(members[0].member_type, SenderType::Human);

    let members_resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/channels/{}/members", channel.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(members_resp.status(), StatusCode::OK);
    let members_body = axum::body::to_bytes(members_resp.into_body(), 1_000_000)
        .await
        .unwrap();
    let members_json: serde_json::Value = serde_json::from_slice(&members_body).unwrap();
    assert_eq!(members_json["memberCount"], 1);
}

#[tokio::test]
async fn test_channel_members_api_lists_members_and_supports_invite() {
    let (store, app) = setup();
    store.create_human("zoe").unwrap();
    store
        .create_agent_record(&AgentRecordUpsert {
            name: "bot2",
            display_name: "Bot 2",
            description: None,
            system_prompt: None,
            runtime: "codex",
            model: "gpt-5.4",
            reasoning_effort: None,
            env_vars: &[],
        })
        .unwrap();
    let channel_id = store.get_channel_by_name("general").unwrap().unwrap().id;

    let initial = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/channels/{channel_id}/members"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(initial.status(), StatusCode::OK);
    let initial_body = axum::body::to_bytes(initial.into_body(), 1_000_000)
        .await
        .unwrap();
    let initial_json: serde_json::Value = serde_json::from_slice(&initial_body).unwrap();
    assert_eq!(initial_json["memberCount"], 2);

    let invite_human = serde_json::json!({ "memberName": "zoe" });
    let invite_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/channels/{channel_id}/members"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&invite_human).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(invite_resp.status(), StatusCode::OK);

    let invite_agent = serde_json::json!({ "memberName": "bot2" });
    let invite_agent_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/channels/{channel_id}/members"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&invite_agent).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(invite_agent_resp.status(), StatusCode::OK);

    let listed = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/channels/{channel_id}/members"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    let body = axum::body::to_bytes(listed.into_body(), 1_000_000)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["memberCount"], 4);

    let members = json["members"].as_array().unwrap();
    assert!(members.iter().any(|member| member["memberName"] == "alice"));
    assert!(members.iter().any(|member| member["memberName"] == "bot1"));
    assert!(members.iter().any(|member| member["memberName"] == "zoe"));
    assert!(members.iter().any(|member| member["memberName"] == "bot2"));
}

#[tokio::test]
async fn test_channel_members_api_rejects_unknown_member() {
    let (store, app) = setup();
    let channel_id = store.get_channel_by_name("general").unwrap().unwrap().id;
    let req = serde_json::json!({ "memberName": "missing-user" });

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/channels/{channel_id}/members"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_all_channel_member_count_matches_agents_plus_humans() {
    let (store, app) = setup();
    store.ensure_builtin_channels("alice").unwrap();
    store.create_human("zoe").unwrap();
    store
        .create_agent_record(&AgentRecordUpsert {
            name: "bot2",
            display_name: "Bot 2",
            description: None,
            system_prompt: None,
            runtime: "codex",
            model: "gpt-5.4",
            reasoning_effort: None,
            env_vars: &[],
        })
        .unwrap();

    let all = store.get_channel_by_name("all").unwrap().unwrap();
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/channels/{}/members", all.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 1_000_000)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    let expected_member_count =
        store.get_agents().unwrap().len() + store.get_humans().unwrap().len();
    assert_eq!(
        json["memberCount"].as_u64().unwrap(),
        expected_member_count as u64
    );
}

#[tokio::test]
async fn test_history_rejects_non_member_agent() {
    let (store, app) = setup();
    store
        .create_agent_record(&AgentRecordUpsert {
            name: "bot2",
            display_name: "Bot 2",
            description: None,
            system_prompt: None,
            runtime: "codex",
            model: "gpt-5.4",
            reasoning_effort: None,
            env_vars: &[],
        })
        .unwrap();
    store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: None,
            sender_name: "alice",
            sender_type: SenderType::Human,
            content: "secret channel update",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/internal/agent/bot2/history?channel=%23general&limit=20")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let body = axum::body::to_bytes(resp.into_body(), 1_000_000)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        json["error"].as_str(),
        Some("you are not a member of channel #general")
    );
}

#[tokio::test]
async fn test_archive_channel_via_api_hides_it_from_server_info() {
    let (store, app) = setup();
    let channel_id = store.get_channel_by_name("general").unwrap().unwrap().id;

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/channels/{channel_id}/archive"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(store.get_channel_by_id(&channel_id).unwrap().is_some());

    let channels_resp = app
        .oneshot(
            Request::builder()
                .uri("/api/channels")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(channels_resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(channels_resp.into_body(), 1_000_000)
        .await
        .unwrap();
    let channels: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(channels.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_delete_channel_via_api_removes_channel_owned_data() {
    let (store, app) = setup();
    let channel_id = store.get_channel_by_name("general").unwrap().unwrap().id;
    store.create_tasks("general", "bot1", &["Fix bug"]).unwrap();
    store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: None,
            sender_name: "alice",
            sender_type: SenderType::Human,
            content: "hello",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/channels/{channel_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(store.get_channel_by_id(&channel_id).unwrap().is_none());

    let channels_resp = app
        .oneshot(
            Request::builder()
                .uri("/api/channels")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(channels_resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(channels_resp.into_body(), 1_000_000)
        .await
        .unwrap();
    let channels: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(channels.as_array().unwrap().is_empty());
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
async fn test_list_runtime_statuses() {
    let (_store, app, _lifecycle) = setup_with_runtime_statuses(
        vec![
            RuntimeStatusInfo {
                runtime: "claude".to_string(),
                auth: ProbeAuth::Authed,
            },
            RuntimeStatusInfo {
                runtime: "codex".to_string(),
                auth: ProbeAuth::Unauthed,
            },
            RuntimeStatusInfo {
                runtime: "kimi".to_string(),
                auth: ProbeAuth::NotInstalled,
            },
        ],
        vec![],
    );

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/runtimes")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = axum::body::to_bytes(resp.into_body(), 1_000_000)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let runtimes = payload
        .as_array()
        .expect("runtimes payload should be an array");
    assert_eq!(runtimes.len(), 3);
    assert_eq!(runtimes[0]["runtime"], "claude");
    assert_eq!(runtimes[0]["auth"], "authed");
    assert_eq!(runtimes[1]["runtime"], "codex");
    assert_eq!(runtimes[1]["auth"], "unauthed");
    assert_eq!(runtimes[2]["runtime"], "kimi");
    assert_eq!(runtimes[2]["auth"], "not_installed");
}

#[tokio::test]
async fn test_list_runtime_models() {
    let (_store, app, _lifecycle) = setup_with_runtime_statuses(
        vec![],
        vec![
            (
                "codex".to_string(),
                vec!["gpt-5.4".to_string(), "gpt-5.4-mini".to_string()],
            ),
            ("opencode".to_string(), vec!["openai/gpt-5.4".to_string()]),
        ],
    );

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/runtimes/opencode/models")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = axum::body::to_bytes(resp.into_body(), 1_000_000)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload, serde_json::json!(["openai/gpt-5.4"]));
}

#[tokio::test]
async fn test_create_agent_via_api_keeps_inactive_record_when_start_fails() {
    let store = Arc::new(Store::open(":memory:").unwrap());
    store
        .create_channel("general", Some("General"), ChannelType::Channel)
        .unwrap();
    store.create_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();
    store.ensure_builtin_channels("alice").unwrap();
    let app = build_router_with_lifecycle(store.clone(), Arc::new(FailStartLifecycle));

    let req = serde_json::json!({
        "name": "stuck-bot",
        "runtime": "claude",
        "model": "sonnet"
    });
    let resp = app
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

    let payload = body_json(resp).await;
    let name = payload["name"].as_str().unwrap().to_string();
    assert!(
        name.starts_with("stuck-bot-"),
        "expected suffixed slug, got `{name}`"
    );
    let agent = store
        .get_agent(&name)
        .unwrap()
        .expect("agent should remain in the store");
    assert_eq!(agent.status, AgentStatus::Inactive);
    assert!(store.is_member("all", &name).unwrap());
}

#[tokio::test]
async fn test_create_kimi_agent_via_api() {
    let (store, app, lifecycle) = setup_with_lifecycle();
    store.ensure_builtin_channels("alice").unwrap();

    let req = serde_json::json!({
        "name": "kimi-bot",
        "description": "A Kimi test agent",
        "runtime": "kimi",
        "model": "kimi-code/kimi-for-coding"
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

    let payload: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), 1_000_000)
            .await
            .unwrap(),
    )
    .unwrap();

    let name = payload["name"].as_str().unwrap().to_string();
    assert!(
        name.starts_with("kimi-bot-"),
        "expected suffixed slug, got `{name}`"
    );
    let agent = store
        .get_agent(&name)
        .unwrap()
        .expect("agent should exist");
    assert_eq!(payload["id"], agent.id);
    assert_eq!(payload["status"], "active");
    assert_eq!(agent.runtime, "kimi");
    assert_eq!(agent.model, "kimi-code/kimi-for-coding");
    assert_eq!(agent.reasoning_effort, None);
    assert_eq!(lifecycle.started_names(), vec![name]);
}

#[tokio::test]
async fn test_get_and_update_agent_via_api() {
    let (store, app, lifecycle) = setup_with_lifecycle();
    store
        .update_agent_status("bot1", AgentStatus::Active)
        .unwrap();
    let bot1 = store.get_agent("bot1").unwrap().unwrap();

    let detail_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/agents/{}", bot1.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(detail_resp.status(), StatusCode::OK);
    let detail_body = axum::body::to_bytes(detail_resp.into_body(), 1_000_000)
        .await
        .unwrap();
    let detail: AgentDetailResponse = serde_json::from_slice(&detail_body).unwrap();
    let bot1 = store.get_agent("bot1").unwrap().unwrap();
    assert_eq!(detail.agent.id, bot1.id);
    assert_eq!(detail.agent.reasoning_effort, None);

    let update_req = serde_json::json!({
        "display_name": "Updated Bot",
        "description": "Updated role",
        "runtime": "codex",
        "model": "gpt-5.4",
        "reasoningEffort": "low",
        "envVars": [{"key": "DEBUG", "value": "1"}]
    });
    let update_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/agents/{}", bot1.id))
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
    assert_eq!(agent.reasoning_effort.as_deref(), Some("low"));
    assert_eq!(agent.env_vars.len(), 1);
    assert_eq!(agent.env_vars[0].key, "DEBUG");
    assert_eq!(lifecycle.stopped_names(), vec!["bot1".to_string()]);
    assert_eq!(lifecycle.started_names(), vec!["bot1".to_string()]);
}

#[tokio::test]
async fn test_update_agent_to_kimi_clears_reasoning_effort() {
    let (store, app, lifecycle) = setup_with_lifecycle();
    store
        .update_agent_status("bot1", AgentStatus::Active)
        .unwrap();
    store
        .update_agent_record(&AgentRecordUpsert {
            name: "bot1",
            display_name: "Bot 1",
            description: Some("Replies in Chorus"),
            system_prompt: None,
            runtime: "codex",
            model: "gpt-5.4-mini",
            reasoning_effort: Some("high"),
            env_vars: &[],
        })
        .unwrap();
    let bot1 = store.get_agent("bot1").unwrap().unwrap();

    let update_req = serde_json::json!({
        "display_name": "Kimi Bot",
        "description": "Updated role",
        "runtime": "kimi",
        "model": "kimi-code/kimi-for-coding",
        "reasoningEffort": "high",
        "envVars": [{"key": "DEBUG", "value": "1"}]
    });
    let update_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/agents/{}", bot1.id))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&update_req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(update_resp.status(), StatusCode::OK);

    let agent = store.get_agent("bot1").unwrap().unwrap();
    assert_eq!(agent.display_name, "Kimi Bot");
    assert_eq!(agent.runtime, "kimi");
    assert_eq!(agent.model, "kimi-code/kimi-for-coding");
    assert_eq!(agent.reasoning_effort, None);
    assert_eq!(agent.env_vars.len(), 1);
    assert_eq!(agent.env_vars[0].key, "DEBUG");
    assert_eq!(lifecycle.stopped_names(), vec!["bot1".to_string()]);
    assert_eq!(lifecycle.started_names(), vec!["bot1".to_string()]);
}

#[tokio::test]
async fn test_restart_agent_reset_session_preserves_workspace() {
    let (store, _app, dir) = setup_with_data_dir();
    store
        .update_agent_session("bot1", Some("thread-123"))
        .unwrap();
    let bot1 = store.get_agent("bot1").unwrap().unwrap();
    let workspace_dir = dir.path().join("agents").join("bot1").join("notes");
    std::fs::create_dir_all(&workspace_dir).unwrap();
    std::fs::write(workspace_dir.join("plan.md"), "hello").unwrap();

    let app = build_router_with_lifecycle(store.clone(), Arc::new(MockLifecycle::default()));
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/agents/{}/restart", bot1.id))
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
    let (store, _app, dir) = setup_with_data_dir();
    let bot1 = store.get_agent("bot1").unwrap().unwrap();
    let workspace_dir = dir.path().join("agents").join("bot1").join("notes");
    std::fs::create_dir_all(&workspace_dir).unwrap();
    std::fs::write(workspace_dir.join("plan.md"), "hello").unwrap();
    store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: None,
            sender_name: "bot1",
            sender_type: SenderType::Agent,
            content: "hello",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();

    let app = build_router_with_lifecycle(store.clone(), Arc::new(MockLifecycle::default()));
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/agents/{}/delete", bot1.id))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&serde_json::json!({ "mode": "preserve_workspace" }))
                        .unwrap(),
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
        .create_agent_record(&AgentRecordUpsert {
            name: "bot2",
            display_name: "Bot 2",
            description: None,
            system_prompt: None,
            runtime: "codex",
            model: "gpt-5.4",
            reasoning_effort: None,
            env_vars: &[],
        })
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
async fn test_send_persists_message_even_if_agent_delivery_fails() {
    let store = Arc::new(Store::open(":memory:").unwrap());
    store
        .create_channel("general", Some("General"), ChannelType::Channel)
        .unwrap();
    store.create_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();
    store
        .create_agent_record(&AgentRecordUpsert {
            name: "bot1",
            display_name: "Bot 1",
            description: None,
            system_prompt: None,
            runtime: "claude",
            model: "sonnet",
            reasoning_effort: None,
            env_vars: &[],
        })
        .unwrap();
    store
        .join_channel("general", "bot1", SenderType::Agent)
        .unwrap();
    let app = build_router_with_lifecycle(store.clone(), Arc::new(FailStartLifecycle));

    let send_req =
        serde_json::json!({ "target": "#general", "content": "persist despite delivery failure" });
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

    let history = store
        .get_history_snapshot("general", "alice", None, 10, None, None)
        .unwrap();
    assert!(history
        .messages
        .iter()
        .any(|message| message.content == "persist despite delivery failure"));
}

#[tokio::test]
async fn test_thread_send_only_starts_parent_author_agent() {
    let (store, app, lifecycle) = setup_with_lifecycle();
    store
        .create_agent_record(&AgentRecordUpsert {
            name: "bot2",
            display_name: "Bot 2",
            description: None,
            system_prompt: None,
            runtime: "codex",
            model: "gpt-5.4",
            reasoning_effort: None,
            env_vars: &[],
        })
        .unwrap();
    store
        .join_channel("general", "bot2", SenderType::Agent)
        .unwrap();

    let parent_message_id = store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: None,
            sender_name: "bot1",
            sender_type: SenderType::Agent,
            content: "parent from bot1",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
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
        .create_agent_record(&AgentRecordUpsert {
            name: "bot2",
            display_name: "Bot 2",
            description: None,
            system_prompt: None,
            runtime: "codex",
            model: "gpt-5.4",
            reasoning_effort: None,
            env_vars: &[],
        })
        .unwrap();
    store
        .join_channel("general", "bot2", SenderType::Agent)
        .unwrap();
    store
        .create_agent_record(&AgentRecordUpsert {
            name: "bot3",
            display_name: "Bot 3",
            description: None,
            system_prompt: None,
            runtime: "claude",
            model: "sonnet",
            reasoning_effort: None,
            env_vars: &[],
        })
        .unwrap();
    store
        .join_channel("general", "bot3", SenderType::Agent)
        .unwrap();

    let parent_message_id = store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: None,
            sender_name: "bot1",
            sender_type: SenderType::Agent,
            content: "parent from bot1",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();
    store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: Some(&parent_message_id),
            sender_name: "bot2",
            sender_type: SenderType::Agent,
            content: "bot2 already joined the thread",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
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
        .create_agent_record(&AgentRecordUpsert {
            name: "bot2",
            display_name: "Bot 2",
            description: None,
            system_prompt: None,
            runtime: "codex",
            model: "gpt-5.4",
            reasoning_effort: None,
            env_vars: &[],
        })
        .unwrap();
    store
        .join_channel("general", "bot2", SenderType::Agent)
        .unwrap();
    store
        .update_agent_status("bot1", AgentStatus::Active)
        .unwrap();

    let parent_message_id = store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: None,
            sender_name: "alice",
            sender_type: SenderType::Human,
            content: "human started the thread",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
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
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: None,
            sender_name: "alice",
            sender_type: SenderType::Human,
            content: "msg 1",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
        .unwrap();
    store
        .create_message(CreateMessage {
            channel_name: "general",
            thread_parent_id: None,
            sender_name: "alice",
            sender_type: SenderType::Human,
            content: "msg 2",
            attachment_ids: &[],
            suppress_event: false,
            run_id: None,
        })
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
    let (store, app, dir) = setup_with_data_dir();
    let bot1 = store.get_agent("bot1").unwrap().unwrap();
    let workspace_dir = dir.path().join("agents").join("bot1").join("notes");
    std::fs::create_dir_all(&workspace_dir).unwrap();
    std::fs::write(workspace_dir.join("plan.md"), "# test\n").unwrap();

    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/agents/{}/workspace", bot1.id))
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
    let (store, app, dir) = setup_with_data_dir();
    let bot1 = store.get_agent("bot1").unwrap().unwrap();
    let workspace_dir = dir.path().join("agents").join("bot1").join("notes");
    std::fs::create_dir_all(&workspace_dir).unwrap();
    std::fs::write(workspace_dir.join("plan.md"), "# plan\nship it\n").unwrap();

    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/agents/{}/workspace/file?path=notes%2Fplan.md",
                    bot1.id
                ))
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

#[tokio::test]
async fn test_create_team_endpoint() {
    let (store, app, lifecycle, dir) = setup_with_lifecycle_and_data_dir();
    let bot1 = store.get_agent("bot1").unwrap().unwrap();

    let body = serde_json::json!({
        "name": "eng-team",
        "display_name": "Engineering Team",
        "collaboration_model": "leader_operators",
        "leader_agent_name": "bot1",
        "members": [{
            "member_name": "bot1",
            "member_type": "agent",
            "member_id": bot1.id,
            "role": "operator"
        }]
    });

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/teams")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();

    let ch = store.get_channel_by_name("eng-team").unwrap().unwrap();
    assert_eq!(ch.channel_type, ChannelType::Team);
    assert_eq!(payload["team"]["channel_id"], ch.id);

    let team = store.get_team("eng-team").unwrap().unwrap();
    let members = store.get_team_members(&team.id).unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].member_name, "bot1");
    assert_eq!(members[0].role, "operator");

    assert_eq!(lifecycle.stopped_names(), vec!["bot1".to_string()]);
    assert_eq!(lifecycle.started_names(), vec!["bot1".to_string()]);

    let teams_root = dir.path().join("teams").join("eng-team");
    assert!(teams_root.join("TEAM.md").exists());
    assert!(teams_root.join("members").join("bot1").exists());

    let role_md = dir
        .path()
        .join("agents")
        .join("bot1")
        .join("teams")
        .join("eng-team")
        .join("ROLE.md");
    assert!(role_md.exists());
}

#[tokio::test]
async fn test_list_and_update_team_endpoints() {
    let (store, app, lifecycle) = setup_with_lifecycle();
    store
        .create_agent_record(&AgentRecordUpsert {
            name: "bot2",
            display_name: "Bot 2",
            description: None,
            system_prompt: None,
            runtime: "codex",
            model: "gpt-5.4-mini",
            reasoning_effort: None,
            env_vars: &[],
        })
        .unwrap();
    let team_id = store
        .create_team(
            "eng-team",
            "Engineering Team",
            "leader_operators",
            Some("bot1"),
        )
        .unwrap();

    let list_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/teams")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(list_resp.status(), StatusCode::OK);
    let teams: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(list_resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(teams.as_array().unwrap().len(), 1);
    let listed_team_id = teams[0]["id"]
        .as_str()
        .expect("team list payload should expose public id")
        .to_string();
    assert_eq!(listed_team_id, team_id);
    assert_eq!(teams[0]["name"], "eng-team");
    assert_eq!(teams[0]["display_name"], "Engineering Team");

    let get_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/teams/{listed_team_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(get_resp.status(), StatusCode::OK);
    let team_payload: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(get_resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(team_payload["team"]["id"], team_id);
    assert_eq!(team_payload["team"]["name"], "eng-team");
    assert_eq!(team_payload["team"]["display_name"], "Engineering Team");
    assert_eq!(
        team_payload["team"]["collaboration_model"],
        "leader_operators"
    );
    assert_eq!(team_payload["team"]["leader_agent_name"], "bot1");
    assert!(team_payload["members"].as_array().unwrap().is_empty());

    let patch_body = serde_json::json!({
        "display_name": "Applied Science"
    });
    let patch_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/teams/{team_id}"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&patch_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(patch_resp.status(), StatusCode::OK);

    let updated = store.get_team_by_id(&team_id).unwrap().unwrap();
    assert_eq!(updated.display_name, "Applied Science");

    store
        .create_team_member(&team_id, "bot1", "agent", "bot1", "leader")
        .unwrap();
    store
        .create_team_member(&team_id, "bot2", "agent", "bot2", "operator")
        .unwrap();

    let leader_patch_body = serde_json::json!({
        "display_name": "Applied Science Platform"
    });
    let leader_patch_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/teams/{team_id}"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&leader_patch_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(leader_patch_resp.status(), StatusCode::OK);

    let updated_members = store.get_team_members(&team_id).unwrap();
    let updated_team = store.get_team_by_id(&team_id).unwrap().unwrap();
    let bot1_member = updated_members
        .iter()
        .find(|member| member.member_name == "bot1")
        .unwrap();
    let bot2_member = updated_members
        .iter()
        .find(|member| member.member_name == "bot2")
        .unwrap();
    assert_eq!(updated_team.display_name, "Applied Science Platform");
    assert_eq!(bot1_member.role, "leader");
    assert_eq!(bot2_member.role, "operator");
    assert_eq!(lifecycle.started_names().len(), 2);
    assert_eq!(lifecycle.stopped_names().len(), 2);
}

#[tokio::test]
async fn test_list_channels_includes_team_without_human_membership() {
    let (store, app) = setup();
    let team_id = store
        .create_team("qa-eng", "QA Engineering", "leader_operators", Some("bot1"))
        .unwrap();
    store
        .create_channel("qa-eng", None, ChannelType::Team)
        .unwrap();
    store
        .create_team_member(&team_id, "bot1", "agent", "bot1", "leader")
        .unwrap();
    store
        .join_channel("qa-eng", "bot1", SenderType::Agent)
        .unwrap();

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/channels?member=alice")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let channels: Vec<ChannelInfo> = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();

    let team = channels
        .iter()
        .find(|channel| channel.name == "qa-eng")
        .expect("team channel should be listed even without human membership");
    assert_eq!(team.channel_type.as_deref(), Some("team"));
    assert!(!team.joined);
}

#[tokio::test]
async fn test_add_remove_and_delete_team_endpoints() {
    let (store, app, lifecycle, dir) = setup_with_lifecycle_and_data_dir();
    let bot1 = store.get_agent("bot1").unwrap().unwrap();

    let team_id = store
        .create_team("eng-team", "Engineering Team", "leader_operators", None)
        .unwrap();
    store
        .create_channel("eng-team", None, ChannelType::Team)
        .unwrap();

    let add_body = serde_json::json!({
        "member_name": "bot1",
        "member_type": "agent",
        "member_id": bot1.id,
        "role": "operator"
    });
    let add_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/teams/{team_id}/members"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&add_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(add_resp.status(), StatusCode::OK);
    assert_eq!(
        store.get_teams_by_agent_name("bot1").unwrap()[0].team_name,
        "eng-team"
    );
    assert!(dir
        .path()
        .join("agents")
        .join("bot1")
        .join("teams")
        .join("eng-team")
        .join("ROLE.md")
        .exists());

    let remove_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/teams/{team_id}/members/bot1"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(remove_resp.status(), StatusCode::OK);
    assert!(store.get_teams_by_agent_name("bot1").unwrap().is_empty());
    assert_eq!(
        store
            .get_last_read_seq("eng-team", "bot1")
            .unwrap_err()
            .to_string(),
        "Query returned no rows"
    );

    let delete_resp = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/teams/{team_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(delete_resp.status(), StatusCode::OK);
    assert!(store.get_team_by_id(&team_id).unwrap().is_none());
    let listed = store.get_channels().unwrap();
    assert!(listed.iter().all(|channel| channel.name != "eng-team"));
    assert!(!dir.path().join("teams").join("eng-team").exists());
    assert_eq!(lifecycle.started_names().len(), 2);
    assert_eq!(lifecycle.stopped_names().len(), 2);
}

#[tokio::test]
async fn test_at_mention_forwards_to_team_channel() {
    let (store, app, lifecycle) = setup_with_lifecycle();
    store
        .create_agent_record(&AgentRecordUpsert {
            name: "bot2",
            display_name: "Bot 2",
            description: None,
            system_prompt: None,
            runtime: "codex",
            model: "gpt-5.4-mini",
            reasoning_effort: None,
            env_vars: &[],
        })
        .unwrap();
    store
        .update_agent_status("bot1", AgentStatus::Active)
        .unwrap();
    store
        .update_agent_status("bot2", AgentStatus::Active)
        .unwrap();
    let team_id = store
        .create_team("eng-team", "Engineering", "leader_operators", Some("bot1"))
        .unwrap();
    store
        .create_channel("eng-team", None, ChannelType::Team)
        .unwrap();
    let bot1 = store.get_agent("bot1").unwrap().unwrap();
    let bot2 = store.get_agent("bot2").unwrap().unwrap();
    store
        .create_team_member(&team_id, "bot1", "agent", &bot1.id, "leader")
        .unwrap();
    store
        .create_team_member(&team_id, "bot2", "agent", &bot2.id, "operator")
        .unwrap();
    store
        .join_channel("eng-team", "bot1", SenderType::Agent)
        .unwrap();
    store
        .join_channel("eng-team", "bot2", SenderType::Agent)
        .unwrap();

    let send_req =
        serde_json::json!({ "target": "#general", "content": "hey @eng-team build something" });
    let response = app
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

    let forwarded = store
        .get_messages_for_agent("bot1", false)
        .unwrap()
        .into_iter()
        .find(|msg| msg.channel_name == "eng-team")
        .expect("forwarded team message");
    assert_eq!(forwarded.content, "hey @eng-team build something");
    let provenance = forwarded.forwarded_from.expect("forwarded metadata");
    assert_eq!(provenance.channel_name, "general");
    assert_eq!(provenance.sender_name, "alice");

    let notified = lifecycle.notified_names();
    assert_eq!(
        notified
            .iter()
            .filter(|name| name.as_str() == "bot1")
            .count(),
        2
    );
    assert_eq!(
        notified
            .iter()
            .filter(|name| name.as_str() == "bot2")
            .count(),
        1
    );
}

// ── Template API tests ──

#[tokio::test]
async fn test_get_templates_returns_grouped_categories() {
    use chorus::agent::templates::AgentTemplate;

    let templates = vec![
        AgentTemplate {
            id: "engineering/backend-architect".to_string(),
            name: "Backend Architect".to_string(),
            emoji: Some("🏗️".to_string()),
            color: Some("blue".to_string()),
            vibe: Some("Builds systems".to_string()),
            description: Some("Designs scalable systems".to_string()),
            category: "engineering".to_string(),
            suggested_runtime: "claude".to_string(),
            prompt_body: "You are a backend architect.".to_string(),
        },
        AgentTemplate {
            id: "product/nudge-engine".to_string(),
            name: "Nudge Engine".to_string(),
            emoji: Some("🧠".to_string()),
            color: None,
            vibe: None,
            description: None,
            category: "product".to_string(),
            suggested_runtime: "claude".to_string(),
            prompt_body: "You are a nudge engine.".to_string(),
        },
    ];

    let dir = tempdir().unwrap();
    let db_path = dir.path().join("chorus.db");
    let store = Arc::new(Store::open(db_path.to_str().unwrap()).unwrap());
    let lifecycle = Arc::new(MockLifecycle::default());
    let runtime_status_provider = Arc::new(MockRuntimeStatusProvider {
        statuses: vec![],
        models_by_runtime: vec![],
    });
    let router = build_router_with_services(store, lifecycle, runtime_status_provider, templates);

    let response = router
        .oneshot(
            Request::builder()
                .uri("/api/templates")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap(),
    )
    .unwrap();

    let categories = body["categories"].as_array().unwrap();
    assert_eq!(categories.len(), 2);
    assert_eq!(categories[0]["name"], "engineering");
    assert_eq!(categories[1]["name"], "product");
    assert_eq!(
        categories[0]["templates"][0]["prompt_body"],
        "You are a backend architect."
    );
}

#[tokio::test]
async fn test_get_templates_returns_empty_when_no_templates() {
    let (_, router) = setup();
    let response = router
        .oneshot(
            Request::builder()
                .uri("/api/templates")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(body["categories"].as_array().unwrap().len(), 0);
}

// ── AppErrorCode HTTP round-trip tests ────────────────────────────────────────

async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), 1_000_000)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn test_create_agent_appends_random_suffix() {
    let (store, app, _lifecycle) = setup_with_lifecycle();
    store.ensure_builtin_channels("alice").unwrap();

    let req = serde_json::json!({ "name": "bot1", "runtime": "claude", "model": "sonnet" });
    let resp = app
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
    let body = body_json(resp).await;
    let name = body["name"].as_str().expect("name is a string");
    // Assert the shape: `bot1-<4 lowercase hex chars>`. The suffix is
    // always present even without a name collision.
    let prefix = "bot1-";
    assert!(
        name.starts_with(prefix),
        "name `{name}` should start with `{prefix}`"
    );
    let suffix = &name[prefix.len()..];
    assert_eq!(suffix.len(), 4, "suffix `{suffix}` should be 4 chars");
    assert!(
        suffix
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "suffix `{suffix}` should be lowercase hex"
    );
}

#[tokio::test]
async fn test_create_agent_derives_slug_from_display_name() {
    let (store, app, _lifecycle) = setup_with_lifecycle();
    store.ensure_builtin_channels("alice").unwrap();

    // Send no explicit name. Server must slugify the display name.
    let req = serde_json::json!({
        "display_name": "Code Reviewer!!!",
        "runtime": "claude",
        "model": "sonnet"
    });
    let resp = app
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
    let body = body_json(resp).await;
    let name = body["name"].as_str().expect("name is a string");
    // Expect `code-reviewer-<4 hex>`: lowercased, non-alnum collapsed
    // to a single dash, trailing `!!!` trimmed, random hash appended.
    assert!(
        name.starts_with("code-reviewer-"),
        "name `{name}` should start with `code-reviewer-`"
    );
}

#[tokio::test]
async fn test_create_agent_falls_back_when_display_name_has_no_ascii() {
    let (store, app, _lifecycle) = setup_with_lifecycle();
    store.ensure_builtin_channels("alice").unwrap();

    // Pure non-ASCII display name: server can't slugify it, must fall
    // back to the `agent-<hex4>` shape rather than 400ing.
    let req = serde_json::json!({
        "display_name": "名字",
        "runtime": "claude",
        "model": "sonnet"
    });
    let resp = app
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
    let body = body_json(resp).await;
    let name = body["name"].as_str().expect("name is a string");
    assert!(
        name.starts_with("agent-"),
        "name `{name}` should fall back to `agent-` prefix"
    );
}

#[tokio::test]
async fn test_duplicate_channel_name_returns_channel_name_taken() {
    let (_store, app) = setup();

    let req = serde_json::json!({ "name": "general", "description": "" });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/channels")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body = body_json(resp).await;
    assert_eq!(body["code"], "CHANNEL_NAME_TAKEN");
}

#[tokio::test]
async fn test_duplicate_team_name_returns_team_name_taken() {
    let (store, app, _lifecycle) = setup_with_lifecycle();
    store
        .create_team("eng-team", "Engineering", "leader_operators", None)
        .unwrap();

    let req = serde_json::json!({
        "name": "eng-team",
        "display_name": "Engineering Again",
        "members": []
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/teams")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body = body_json(resp).await;
    assert_eq!(body["code"], "TEAM_NAME_TAKEN");
}

#[tokio::test]
async fn test_patch_system_channel_returns_operation_unsupported() {
    let (store, app) = setup();
    let system_channel_id = store
        .create_channel("all", None, ChannelType::System)
        .unwrap();

    let req = serde_json::json!({ "name": "all", "description": "everyone" });
    let resp = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/channels/{system_channel_id}"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = body_json(resp).await;
    assert_eq!(body["code"], "CHANNEL_OPERATION_UNSUPPORTED");
}

#[tokio::test]
async fn test_non_member_history_returns_message_not_a_member() {
    let (store, app, _lifecycle) = setup_with_lifecycle();
    // bot2 exists but is NOT a member of #general
    store
        .create_agent_record(&AgentRecordUpsert {
            name: "bot2",
            display_name: "Bot 2",
            description: None,
            system_prompt: None,
            runtime: "claude",
            model: "sonnet",
            reasoning_effort: None,
            env_vars: &[],
        })
        .unwrap();

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/internal/agent/bot2/history?channel=%23general&limit=10")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let body = body_json(resp).await;
    assert_eq!(body["code"], "MESSAGE_NOT_A_MEMBER");
}

#[tokio::test]
async fn test_restart_agent_start_fails_returns_agent_restart_failed() {
    let store = Arc::new(Store::open(":memory:").unwrap());
    store
        .create_channel("general", Some("General"), ChannelType::Channel)
        .unwrap();
    store.create_human("alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();
    store
        .create_agent_record(&AgentRecordUpsert {
            name: "bot1",
            display_name: "Bot 1",
            description: None,
            system_prompt: None,
            runtime: "claude",
            model: "sonnet",
            reasoning_effort: None,
            env_vars: &[],
        })
        .unwrap();
    let req = serde_json::json!({ "mode": "restart" });
    let bot1 = store.get_agent("bot1").unwrap().unwrap();
    let app = build_router_with_lifecycle(store, Arc::new(FailStartLifecycle));
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/agents/{}/restart", bot1.id))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = body_json(resp).await;
    assert_eq!(body["code"], "AGENT_RESTART_FAILED");
}
