use axum::body::Body;
use axum::http::{Request, StatusCode};
use chorus::agent::activity_log::{ActivityEntry, ActivityLogResponse};
use chorus::agent::runtime_status::{RuntimeAuthStatus, RuntimeStatus, RuntimeStatusProvider};
use chorus::agent::AgentLifecycle;
use chorus::server::dto::ChannelInfo;
use chorus::server::dto::ServerInfo;
use chorus::server::transport::realtime::event_to_json_value;
use chorus::server::{
    build_router, build_router_with_lifecycle, build_router_with_services, AgentDetailResponse,
    HistoryResponse,
};
use chorus::store::agents::AgentStatus;
use chorus::store::channels::ChannelType;
use chorus::store::messages::{ReceivedMessage, SenderType};
use chorus::store::AgentRecordUpsert;
use chorus::store::Store;
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

struct MockRuntimeStatusProvider {
    statuses: Vec<RuntimeStatus>,
}

impl RuntimeStatusProvider for MockRuntimeStatusProvider {
    fn list_statuses(&self) -> anyhow::Result<Vec<RuntimeStatus>> {
        Ok(self.statuses.clone())
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

fn setup_with_runtime_statuses(
    statuses: Vec<RuntimeStatus>,
) -> (Arc<Store>, axum::Router, Arc<MockLifecycle>) {
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
    let runtime_status_provider = Arc::new(MockRuntimeStatusProvider { statuses });
    let router =
        build_router_with_services(store.clone(), lifecycle.clone(), runtime_status_provider);
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
async fn test_history_includes_latest_event_id_cursor() {
    let (store, app) = setup();

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
    let channel_id = store.find_channel_by_name("general").unwrap().unwrap().id;
    assert_eq!(history["latestEventId"], 1);
    assert_eq!(history["streamId"], format!("conversation:{channel_id}"));
    assert_eq!(history["streamPos"], 1);
}

#[tokio::test]
async fn test_realtime_event_serializes_as_notification_state() {
    let (store, _app) = setup();

    store
        .send_message("general", None, "alice", SenderType::Human, "hello", &[])
        .unwrap();

    let event = store
        .list_events(None, 10)
        .unwrap()
        .into_iter()
        .find(|event| event.event_type == "message.created")
        .expect("message.created event");
    let frame = event_to_json_value(&store, &event);

    assert_eq!(frame["eventType"], "conversation.state");
    assert_eq!(frame["scopeKind"], "channel");
    assert_eq!(frame["payload"]["latestSeq"], 1);
    assert_eq!(frame["payload"]["lastReadSeq"], 1);
    assert_eq!(frame["payload"]["unreadCount"], 0);
    assert!(frame["payload"].get("content").is_none());
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
    let channel_id = store.find_channel_by_name("general").unwrap().unwrap().id;

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

    let renamed = store.find_channel_by_name("engineering").unwrap().unwrap();
    assert_eq!(renamed.id, channel_id);
    assert_eq!(renamed.description.as_deref(), Some("Platform work"));
    assert!(store.find_channel_by_name("general").unwrap().is_none());
}

#[tokio::test]
async fn test_update_channel_via_api_rejects_duplicate_name() {
    let (store, app) = setup();
    store
        .create_channel("random", Some("Random"), ChannelType::Channel)
        .unwrap();
    let channel_id = store.find_channel_by_name("general").unwrap().unwrap().id;

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
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_create_channel_via_api_only_adds_current_human_member() {
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

    let channel = store.find_channel_by_name("engineering").unwrap().unwrap();
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
    store.add_human("zoe").unwrap();
    store
        .create_agent_record("bot2", "Bot 2", None, "codex", "gpt-5.4", &[])
        .unwrap();
    let channel_id = store.find_channel_by_name("general").unwrap().unwrap().id;

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
    let channel_id = store.find_channel_by_name("general").unwrap().unwrap().id;
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
    store.add_human("zoe").unwrap();
    store
        .create_agent_record("bot2", "Bot 2", None, "codex", "gpt-5.4", &[])
        .unwrap();

    let all = store.find_channel_by_name("all").unwrap().unwrap();
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
        store.list_agents().unwrap().len() + store.list_humans().unwrap().len();
    assert_eq!(
        json["memberCount"].as_u64().unwrap(),
        expected_member_count as u64
    );
}

#[tokio::test]
async fn test_history_rejects_non_member_agent() {
    let (store, app) = setup();
    store
        .create_agent_record("bot2", "Bot 2", None, "codex", "gpt-5.4", &[])
        .unwrap();
    store
        .send_message(
            "general",
            None,
            "alice",
            SenderType::Human,
            "secret channel update",
            &[],
        )
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
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
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
    let channel_id = store.find_channel_by_name("general").unwrap().unwrap().id;

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
    assert!(store.find_channel_by_id(&channel_id).unwrap().is_some());

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
    let channel_id = store.find_channel_by_name("general").unwrap().unwrap().id;
    store.create_tasks("general", "bot1", &["Fix bug"]).unwrap();
    store
        .send_message("general", None, "alice", SenderType::Human, "hello", &[])
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
    assert!(store.find_channel_by_id(&channel_id).unwrap().is_none());

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
    let (_store, app, _lifecycle) = setup_with_runtime_statuses(vec![
        RuntimeStatus {
            runtime: "claude".to_string(),
            installed: true,
            auth_status: Some(RuntimeAuthStatus::Authed),
        },
        RuntimeStatus {
            runtime: "codex".to_string(),
            installed: true,
            auth_status: Some(RuntimeAuthStatus::Unauthed),
        },
        RuntimeStatus {
            runtime: "kimi".to_string(),
            installed: false,
            auth_status: None,
        },
    ]);

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
    assert_eq!(runtimes[0]["installed"], true);
    assert_eq!(runtimes[0]["authStatus"], "authed");
    assert_eq!(runtimes[1]["runtime"], "codex");
    assert_eq!(runtimes[1]["authStatus"], "unauthed");
    assert_eq!(runtimes[2]["runtime"], "kimi");
    assert_eq!(runtimes[2]["installed"], false);
    assert!(runtimes[2].get("authStatus").is_none());
}

#[tokio::test]
async fn test_create_agent_via_api() {
    let (store, app, lifecycle) = setup_with_lifecycle();
    store.ensure_builtin_channels("alice").unwrap();

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
        store.is_member("all", "new-bot").unwrap(),
        "API-created agents should join the built-in default room"
    );
    assert!(
        !store.is_member("general", "new-bot").unwrap(),
        "API-created agents should not auto-join user-created channels"
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
    let system_channels = info["system_channels"]
        .as_array()
        .expect("system_channels should be present");
    let all = system_channels
        .iter()
        .find(|channel| channel["name"] == "all")
        .expect("#all should be exposed as a system channel");
    assert_eq!(all["joined"], true);
    assert_eq!(all["read_only"], false);

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

    let agents_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/agents")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(agents_resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(agents_resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let agents: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let listed_new_bot = agents
        .as_array()
        .and_then(|entries| entries.iter().find(|entry| entry["name"] == "new-bot"))
        .expect("new agent should be listed by /api/agents");
    assert_eq!(listed_new_bot["runtime"], "codex");
    assert_eq!(listed_new_bot["model"], "gpt-5.4");

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

    let agent = store
        .get_agent("kimi-bot")
        .unwrap()
        .expect("agent should exist");
    assert_eq!(agent.runtime, "kimi");
    assert_eq!(agent.model, "kimi-code/kimi-for-coding");
    assert_eq!(agent.reasoning_effort, None);
    assert_eq!(lifecycle.started_names(), vec!["kimi-bot".to_string()]);
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
    let detail_body = axum::body::to_bytes(detail_resp.into_body(), 1_000_000)
        .await
        .unwrap();
    let detail: AgentDetailResponse = serde_json::from_slice(&detail_body).unwrap();
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
        .update_agent_record_with_reasoning(&AgentRecordUpsert {
            name: "bot1",
            display_name: "Bot 1",
            description: Some("Replies in Chorus"),
            runtime: "codex",
            model: "gpt-5.4-mini",
            reasoning_effort: Some("high"),
            env_vars: &[],
        })
        .unwrap();

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
                .uri("/api/agents/bot1")
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
    let (store, _app, dir) = setup_with_data_dir();
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
            ActivityEntry::RawOutput { .. } => "raw_output",
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

    let ch = store.find_channel_by_name("eng-team").unwrap().unwrap();
    assert_eq!(ch.channel_type, ChannelType::Team);
    assert_eq!(payload["team"]["channel_id"], ch.id);

    let team = store.get_team("eng-team").unwrap().unwrap();
    let members = store.get_team_members(&team.id).unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].member_name, "bot1");
    assert_eq!(members[0].role, "leader");

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
        .create_agent_record("bot2", "Bot 2", None, "codex", "gpt-5.4-mini", &[])
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
    assert_eq!(teams[0]["name"], "eng-team");

    let patch_body = serde_json::json!({
        "display_name": "Applied Science",
        "collaboration_model": "swarm",
        "leader_agent_name": null
    });
    let patch_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri("/api/teams/eng-team")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&patch_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(patch_resp.status(), StatusCode::OK);

    let updated = store.get_team_by_id(&team_id).unwrap().unwrap();
    assert_eq!(updated.display_name, "Applied Science");
    assert_eq!(updated.collaboration_model, "swarm");
    assert_eq!(updated.leader_agent_name, None);

    store
        .add_team_member(&team_id, "bot1", "agent", "bot1", "leader")
        .unwrap();
    store
        .add_team_member(&team_id, "bot2", "agent", "bot2", "operator")
        .unwrap();

    let leader_patch_body = serde_json::json!({
        "collaboration_model": "leader_operators",
        "leader_agent_name": "bot2"
    });
    let leader_patch_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri("/api/teams/eng-team")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&leader_patch_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(leader_patch_resp.status(), StatusCode::OK);

    let updated_members = store.get_team_members(&team_id).unwrap();
    let bot1_member = updated_members
        .iter()
        .find(|member| member.member_name == "bot1")
        .unwrap();
    let bot2_member = updated_members
        .iter()
        .find(|member| member.member_name == "bot2")
        .unwrap();
    assert_eq!(bot1_member.role, "operator");
    assert_eq!(bot2_member.role, "leader");
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
        .add_team_member(&team_id, "bot1", "agent", "bot1", "leader")
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
                .uri("/api/teams/eng-team/members")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&add_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(add_resp.status(), StatusCode::OK);
    assert_eq!(
        store.list_teams_for_agent("bot1").unwrap()[0].team_name,
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
                .uri("/api/teams/eng-team/members/bot1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(remove_resp.status(), StatusCode::OK);
    assert!(store.list_teams_for_agent("bot1").unwrap().is_empty());
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
                .uri("/api/teams/eng-team")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(delete_resp.status(), StatusCode::OK);
    assert!(store.get_team_by_id(&team_id).unwrap().is_none());
    let listed = store.list_channels().unwrap();
    assert!(listed.iter().all(|channel| channel.name != "eng-team"));
    assert!(!dir.path().join("teams").join("eng-team").exists());
    assert_eq!(lifecycle.started_names().len(), 2);
    assert_eq!(lifecycle.stopped_names().len(), 2);
}

#[tokio::test]
async fn test_at_mention_forwards_to_team_channel() {
    let (store, app, lifecycle) = setup_with_lifecycle();
    store
        .create_agent_record("bot2", "Bot 2", None, "codex", "gpt-5.4-mini", &[])
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
        .add_team_member(&team_id, "bot1", "agent", &bot1.id, "leader")
        .unwrap();
    store
        .add_team_member(&team_id, "bot2", "agent", &bot2.id, "operator")
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

    let events = store.list_events(None, 20).unwrap();
    let delegation_event = events
        .iter()
        .find(|event| event.event_type == "team.delegation_requested")
        .expect("team delegation event");
    assert_eq!(delegation_event.stream_id, format!("team:{team_id}"));
    assert_eq!(delegation_event.stream_kind, "team");
    assert_eq!(delegation_event.scope_kind, "team");
    assert_eq!(delegation_event.scope_id, format!("team:{team_id}"));
    assert_eq!(
        delegation_event.payload["sourceChannelName"].as_str(),
        Some("general")
    );

    let forwarded_message_event = events
        .iter()
        .find(|event| {
            event.event_type == "message.created"
                && event.channel_name.as_deref() == Some("eng-team")
        })
        .expect("forwarded message event");
    assert_eq!(forwarded_message_event.stream_kind, "conversation");
}

#[tokio::test]
async fn test_swarm_ready_signals_emit_consensus_system_message() {
    let (store, app, _lifecycle) = setup_with_lifecycle();
    store
        .create_agent_record("bot2", "Bot 2", None, "codex", "gpt-5.4-mini", &[])
        .unwrap();
    store
        .join_channel("general", "bot2", SenderType::Agent)
        .unwrap();
    let team_id = store
        .create_team("eng-team", "Engineering", "swarm", None)
        .unwrap();
    store
        .create_channel("eng-team", None, ChannelType::Team)
        .unwrap();
    let bot1 = store.get_agent("bot1").unwrap().unwrap();
    let bot2 = store.get_agent("bot2").unwrap().unwrap();
    store
        .add_team_member(&team_id, "bot1", "agent", &bot1.id, "builder")
        .unwrap();
    store
        .add_team_member(&team_id, "bot2", "agent", &bot2.id, "reviewer")
        .unwrap();
    store
        .join_channel("eng-team", "bot1", SenderType::Agent)
        .unwrap();
    store
        .join_channel("eng-team", "bot2", SenderType::Agent)
        .unwrap();

    let trigger_req = serde_json::json!({
        "target": "#general",
        "content": "team please handle @eng-team"
    });
    let trigger_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/internal/agent/alice/send")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&trigger_req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(trigger_resp.status(), StatusCode::OK);

    let ready_req = |agent: &str| {
        Request::builder()
            .method("POST")
            .uri(format!("/internal/agent/{agent}/send"))
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&serde_json::json!({
                    "target": "#eng-team",
                    "content": "READY: begin execution"
                }))
                .unwrap(),
            ))
            .unwrap()
    };

    let bot1_resp = app.clone().oneshot(ready_req("bot1")).await.unwrap();
    assert_eq!(bot1_resp.status(), StatusCode::OK);
    let bot2_resp = app.oneshot(ready_req("bot2")).await.unwrap();
    assert_eq!(bot2_resp.status(), StatusCode::OK);

    let (history, _) = store.get_history("eng-team", None, 20, None, None).unwrap();
    assert!(
        history
            .iter()
            .any(|msg| msg.content.contains("All members ready")),
        "consensus system message should be posted"
    );

    let events = store.list_events(None, 40).unwrap();
    let coordination_events: Vec<_> = events
        .iter()
        .filter(|event| event.stream_id == format!("team:{team_id}"))
        .map(|event| event.event_type.as_str())
        .collect();
    assert!(coordination_events.contains(&"team.delegation_requested"));
    assert!(coordination_events.contains(&"team.deliberation_requested"));
    assert!(coordination_events.contains(&"team.quorum_snapshot"));
    assert!(coordination_events.contains(&"team.quorum_signaled"));
    assert!(coordination_events.contains(&"team.quorum_reached"));
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
