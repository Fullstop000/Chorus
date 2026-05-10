mod harness;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use chorus::agent::activity_log::ActivityLogResponse;
use chorus::agent::drivers::ProbeAuth;
use chorus::agent::runtime_status::{RuntimeCatalogEntry, RuntimeStatusProvider};
use chorus::agent::workspace::{AgentWorkspace, TeamWorkspace};
use chorus::agent::AgentLifecycle;
use chorus::agent::AgentRuntime;
use chorus::server::dto::ChannelInfo;
use chorus::server::dto::ServerInfo;
use chorus::server::{AgentDetailResponse, HistoryResponse};
use chorus::store::channels::ChannelType;
use chorus::store::messages::{CreateMessage, ReceivedMessage, SenderType};
use chorus::store::AgentRecordUpsert;
use chorus::store::Store;
use harness::{build_router, build_router_with_lifecycle, join_channel_silent};
use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;

use chorus::agent::activity_log::{self, ActivityLogMap};
use tempfile::tempdir;
use tower::ServiceExt;

fn setup() -> (Arc<Store>, axum::Router) {
    let store = Arc::new(Store::open(":memory:").unwrap());
    seed_default_workspace(&store);
    let router = build_router(store.clone());
    (store, router)
}

/// Seed the canonical "general"/alice/bot1 fixture used by most
/// server tests. The agent's database id is fixed to `"bot1"` so
/// existing tests can keep using `/internal/agent/bot1/...` URLs and
/// raw `"bot1"` strings as agent identity-typed args in the
/// ID-first store. New tests should still capture the returned id
/// from `create_agent_record` and feed it to identity-typed APIs.
///
/// Creates `#all` *before* `#general` so the legacy migration in
/// `ensure_all_channel_inner` (which renames `#general` to `#all`
/// when no `#all` exists) does not fire when `build_router` calls
/// `ensure_builtin_channels`.
fn seed_default_workspace(store: &Arc<Store>) {
    store
        .create_channel(
            Store::DEFAULT_SYSTEM_CHANNEL,
            None,
            ChannelType::System,
            None,
        )
        .unwrap();
    store
        .create_channel("general", Some("General"), ChannelType::Channel, None)
        .unwrap();
    store.ensure_human_with_id("alice", "alice").unwrap();
    join_channel_silent(store, "general", "alice", "human");
    seed_agent_with_id(store, "bot1", "Bot 1", "claude", "sonnet");
    join_channel_silent(store, "general", "bot1", "agent");
}

/// Insert an agent row with a chosen primary key. Used by server-test
/// fixtures so `/internal/agent/{agent_id}` URLs and `"bot1"`-style
/// identity-typed args continue to resolve under the strict ID-first
/// store, without forcing every test to plumb a UUID through. Assumes
/// the workspace has already been initialised (e.g. by an earlier
/// `create_channel` / `ensure_human_with_id` call).
fn seed_agent_with_id(
    store: &Arc<Store>,
    id: &str,
    display_name: &str,
    runtime: &str,
    model: &str,
) {
    let workspace_id = store
        .get_active_workspace()
        .unwrap()
        .expect("seed_agent_with_id requires an active workspace")
        .id;
    let conn = store.conn_for_test();
    conn.execute(
        "INSERT INTO agents (id, workspace_id, name, display_name, runtime, model)
         VALUES (?1, ?2, ?1, ?3, ?4, ?5)",
        rusqlite::params![id, workspace_id, display_name, runtime, model],
    )
    .unwrap();
}

fn setup_with_data_dir() -> (Arc<Store>, axum::Router, tempfile::TempDir) {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("data").join("chorus.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let store = Arc::new(Store::open(db_path.to_str().unwrap()).unwrap());
    seed_default_workspace(&store);
    let router = harness::build_router_with_lifecycle_and_dir(
        store.clone(),
        Arc::new(harness::NoopLifecycle),
        dir.path().to_path_buf(),
    );
    (store, router, dir)
}

/// One observed `start_agent` invocation: `(agent_name, wake_message, init_directive)`.
/// Tests assert against the recorded sequence to pin call-site behavior.
type StartedCall = (String, Option<ReceivedMessage>, Option<String>);

#[derive(Default)]
struct MockLifecycle {
    started: Mutex<Vec<StartedCall>>,
    stopped: Mutex<Vec<String>>,
    notified: Mutex<Vec<String>>,
    activity_logs: ActivityLogMap,
    /// Tracks which agents are currently "running" so that process_state,
    /// start_agent, and stop_agent share one source of truth. Keyed by
    /// agent name internally so existing tests can use `mark_running(name)`
    /// without knowing the agent's id; trait calls that arrive by id are
    /// translated through `store` below before lookup.
    running: Mutex<HashSet<String>>,
    /// Records every (agent_name, envelope) pair delivered via
    /// `resume_with_prompt`. Decision-inbox round-trip tests assert the
    /// envelope reached the agent.
    resumed_with: Mutex<Vec<(String, String)>>,
    /// Per-agent channel id returned by `run_channel_id`. Tests preset
    /// this to simulate the trace_store's "current channel for current
    /// run" record without spinning a real AgentManager.
    run_channels: Mutex<std::collections::HashMap<String, String>>,
    /// Optional store reference used to translate `agent_id` (the new
    /// keying after #142) back into `agent_name` so internal recording
    /// stays name-keyed for assertion compatibility. Wire this via
    /// `set_store` in setup helpers; trait methods fall through to
    /// raw-string semantics when absent.
    store: Mutex<Option<Arc<Store>>>,
}

struct MockRuntimeStatusProvider {
    statuses: Vec<RuntimeCatalogEntry>,
    models_by_runtime: Vec<(String, Vec<String>)>,
}

struct FailStartLifecycle;

#[async_trait::async_trait]
impl RuntimeStatusProvider for MockRuntimeStatusProvider {
    async fn list_statuses(&self) -> anyhow::Result<Vec<RuntimeCatalogEntry>> {
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
    /// Wire a store so trait methods receiving `agent_id` (post-#142) can
    /// resolve to `agent_name` for internal recording. Tests that don't
    /// call this still work for paths that pass names (e.g. `start_agent`).
    fn set_store(&self, store: Arc<Store>) {
        *self.store.lock().unwrap() = Some(store);
    }

    /// Resolve an incoming `agent_id` parameter to the agent's name via the
    /// wired store. Returns the input unchanged if no store is set or the
    /// lookup fails — matches what tests expect for inputs that are already
    /// names (e.g. `start_agent`'s `agent_name` parameter).
    fn resolve_to_name(&self, key: &str) -> String {
        if let Some(store) = self.store.lock().unwrap().as_ref() {
            if let Ok(Some(agent)) = store.get_agent_by_id(key, false) {
                return agent.name;
            }
        }
        key.to_string()
    }

    fn started_names(&self) -> Vec<String> {
        self.started
            .lock()
            .unwrap()
            .iter()
            .map(|(name, _, _)| name.clone())
            .collect()
    }

    fn notified_names(&self) -> Vec<String> {
        self.notified.lock().unwrap().clone()
    }

    fn started_calls(&self) -> Vec<StartedCall> {
        self.started.lock().unwrap().clone()
    }

    fn stopped_names(&self) -> Vec<String> {
        self.stopped.lock().unwrap().clone()
    }

    /// Simulate an already-running managed process. Subsequent
    /// `process_state` calls will return `Active` for this agent,
    /// mirroring what the real manager would report when a process
    /// is alive and idle.
    fn mark_running(&self, agent_name: &str) {
        self.running.lock().unwrap().insert(agent_name.to_string());
    }

    /// Pre-populate the run-channel mapping so handlers that call
    /// `lifecycle.run_channel_id(agent)` see this value during the test.
    fn set_run_channel(&self, agent_name: &str, channel_id: &str) {
        self.run_channels
            .lock()
            .unwrap()
            .insert(agent_name.to_string(), channel_id.to_string());
    }

    fn resumed_calls(&self) -> Vec<(String, String)> {
        self.resumed_with.lock().unwrap().clone()
    }
}

impl AgentLifecycle for MockLifecycle {
    fn start_agent<'a>(
        &'a self,
        agent: &'a chorus::store::agents::Agent,
        wake_message: Option<ReceivedMessage>,
        init_directive: Option<String>,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>> {
        let name = agent.name.clone();
        Box::pin(async move {
            self.started
                .lock()
                .unwrap()
                .push((name.clone(), wake_message, init_directive));
            self.running.lock().unwrap().insert(name);
            Ok(())
        })
    }

    fn notify_agent<'a>(
        &'a self,
        agent_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>> {
        let name = self.resolve_to_name(agent_id);
        Box::pin(async move {
            self.notified.lock().unwrap().push(name);
            Ok(())
        })
    }

    fn stop_agent<'a>(
        &'a self,
        agent_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>> {
        let name = self.resolve_to_name(agent_id);
        Box::pin(async move {
            self.stopped.lock().unwrap().push(name.clone());
            self.running.lock().unwrap().remove(&name);
            Ok(())
        })
    }

    fn process_state<'a>(
        &'a self,
        agent_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Option<chorus::agent::drivers::ProcessState>> + Send + 'a>>
    {
        let name = self.resolve_to_name(agent_id);
        let is_running = self.running.lock().unwrap().contains(&name);
        Box::pin(async move {
            if is_running {
                Some(chorus::agent::drivers::ProcessState::Active {
                    session_id: "test".into(),
                })
            } else {
                None
            }
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

    fn run_channel_id<'a>(
        &'a self,
        agent_name: &'a str,
    ) -> Pin<Box<dyn Future<Output = Option<String>> + Send + 'a>> {
        let id = self.run_channels.lock().unwrap().get(agent_name).cloned();
        Box::pin(async move { id })
    }

    fn resume_with_prompt<'a>(
        &'a self,
        agent_id: &'a str,
        envelope: String,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>> {
        let name = self.resolve_to_name(agent_id);
        Box::pin(async move {
            self.resumed_with.lock().unwrap().push((name, envelope));
            Ok(())
        })
    }
}

impl AgentLifecycle for FailStartLifecycle {
    fn start_agent<'a>(
        &'a self,
        _agent: &'a chorus::store::agents::Agent,
        _wake_message: Option<ReceivedMessage>,
        _init_directive: Option<String>,
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

    fn process_state<'a>(
        &'a self,
        _agent_name: &'a str,
    ) -> Pin<Box<dyn Future<Output = Option<chorus::agent::drivers::ProcessState>> + Send + 'a>>
    {
        Box::pin(async { None })
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
    seed_default_workspace(&store);
    let lifecycle = Arc::new(MockLifecycle::default());
    lifecycle.set_store(store.clone());
    let router = build_router_with_lifecycle(store.clone(), lifecycle.clone());
    (store, router, lifecycle)
}

fn setup_with_runtime_statuses(
    statuses: Vec<RuntimeCatalogEntry>,
    models_by_runtime: Vec<(String, Vec<String>)>,
) -> (Arc<Store>, axum::Router, Arc<MockLifecycle>) {
    let store = Arc::new(Store::open(":memory:").unwrap());
    seed_default_workspace(&store);
    let lifecycle = Arc::new(MockLifecycle::default());
    lifecycle.set_store(store.clone());
    let runtime_status_provider = Arc::new(MockRuntimeStatusProvider {
        statuses,
        models_by_runtime,
    });
    let data_dir = harness::unique_test_data_dir();
    let agents_dir = data_dir.join("agents");
    std::fs::create_dir_all(&agents_dir).ok();
    let router = chorus::server::build_router_with_services(
        store.clone(),
        Arc::new(chorus::server::event_bus::EventBus::new()),
        data_dir,
        agents_dir,
        lifecycle.clone(),
        runtime_status_provider,
        vec![],
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
    let db_path = dir.path().join("data").join("chorus.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let store = Arc::new(Store::open(db_path.to_str().unwrap()).unwrap());
    seed_default_workspace(&store);
    let lifecycle = Arc::new(MockLifecycle::default());
    lifecycle.set_store(store.clone());
    let router = harness::build_router_with_lifecycle_and_dir(
        store.clone(),
        lifecycle.clone(),
        dir.path().to_path_buf(),
    );
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
async fn test_internal_agent_name_send_uses_canonical_agent_id() {
    let (store, app) = setup();
    let agent_id = store
        .create_agent_record(&AgentRecordUpsert {
            name: "uuid-bot",
            display_name: "UUID Bot",
            description: None,
            system_prompt: None,
            runtime: "claude",
            model: "sonnet",
            reasoning_effort: None,
            machine_id: None,
            env_vars: &[],
        })
        .unwrap();
    join_channel_silent(&store, "general", &agent_id, "agent");

    let send_req = serde_json::json!({ "target": "#general", "content": "uuid reply" });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/internal/agent/uuid-bot/send")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&send_req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let sender_id: String = store
        .conn_for_test()
        .query_row(
            "SELECT sender_id FROM messages WHERE content = 'uuid reply'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(sender_id, agent_id);
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
    let (_store, app, lifecycle) = setup_with_lifecycle();

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
    let (_store, app, lifecycle) = setup_with_lifecycle();
    lifecycle.mark_running("bot1");

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
        .clone()
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
        .clone()
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
    let (_store, app) = setup();
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
        .create_tasks(
            "general",
            "alice",
            SenderType::Human,
            &["task from public route"],
        )
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
    let (_store, app) = setup();

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
                .uri("/api/channels?member=alice&include_dm=true&include_system=true")
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
async fn test_public_dm_route_accepts_agent_id_and_stores_canonical_member_id() {
    let (store, app) = setup();
    let agent_id = store
        .create_agent_record(&AgentRecordUpsert {
            name: "uuid-bot",
            display_name: "UUID Bot",
            description: None,
            system_prompt: None,
            runtime: "claude",
            model: "sonnet",
            reasoning_effort: None,
            machine_id: None,
            env_vars: &[],
        })
        .unwrap();

    let resp = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/dms/{agent_id}"))
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

    let member_id: String = store
        .conn_for_test()
        .query_row(
            "SELECT member_id FROM channel_members WHERE channel_id = ?1 AND member_type = 'agent'",
            rusqlite::params![dm.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(member_id, agent_id);
    assert!(dm.name.contains(&agent_id));
}

#[tokio::test]
async fn test_list_channels_honors_search_params() {
    let (store, app) = setup();
    store.ensure_builtin_channels("alice").unwrap();
    store
        .create_channel(
            "engineering",
            Some("Engineering"),
            ChannelType::Channel,
            None,
        )
        .unwrap();
    store
        .create_channel("eng-team", Some("Engineering"), ChannelType::Team, None)
        .unwrap();
    store
        .create_channel("dm-bot1", None, ChannelType::Dm, None)
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
        .create_channel("random", Some("Random"), ChannelType::Channel, None)
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
    assert_eq!(members[0].member_id, "alice");
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
    store.ensure_human_with_id("zoe", "zoe").unwrap();
    store
        .create_agent_record(&AgentRecordUpsert {
            name: "bot2",
            display_name: "Bot 2",
            description: None,
            system_prompt: None,
            runtime: "codex",
            model: "gpt-5.4",
            reasoning_effort: None,
            machine_id: None,
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
    store.ensure_human_with_id("zoe", "zoe").unwrap();
    store
        .create_agent_record(&AgentRecordUpsert {
            name: "bot2",
            display_name: "Bot 2",
            description: None,
            system_prompt: None,
            runtime: "codex",
            model: "gpt-5.4",
            reasoning_effort: None,
            machine_id: None,
            env_vars: &[],
        })
        .unwrap();
    // `ensure_human_with_id` does not auto-join humans to `#all`; that
    // backfill lives in `ensure_builtin_channels`. Run it after zoe is
    // seeded so the assertion below sees humans + agents.
    store.ensure_builtin_channels("alice").unwrap();

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
            machine_id: None,
            env_vars: &[],
        })
        .unwrap();
    store
        .create_message(CreateMessage {
            channel_name: "general",
            sender_id: "alice",
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
    store
        .create_tasks("general", "bot1", SenderType::Agent, &["Fix bug"])
        .unwrap();
    store
        .create_message(CreateMessage {
            channel_name: "general",
            sender_id: "alice",
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
    assert!(val["id"].as_str().is_some(), "id field missing");
    assert!(val["name"].as_str().is_some(), "name field missing");
}

#[tokio::test]
async fn test_list_runtime_statuses() {
    let (_store, app, _lifecycle) = setup_with_runtime_statuses(
        vec![
            RuntimeCatalogEntry::new(AgentRuntime::Claude, ProbeAuth::Authed),
            RuntimeCatalogEntry::new(AgentRuntime::Codex, ProbeAuth::Unauthed),
            RuntimeCatalogEntry::new(AgentRuntime::Kimi, ProbeAuth::NotInstalled),
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
    assert_eq!(runtimes[0]["label"], "Claude Code");
    assert_eq!(runtimes[0]["order"], 0);
    assert_eq!(
        runtimes[0]["reasoning_efforts"],
        serde_json::json!(["low", "medium", "high", "xhigh", "max"])
    );
    assert_eq!(runtimes[0]["auth"], "authed");
    assert_eq!(runtimes[1]["runtime"], "codex");
    assert_eq!(runtimes[1]["label"], "Codex CLI");
    assert_eq!(runtimes[1]["order"], 1);
    assert_eq!(
        runtimes[1]["reasoning_efforts"],
        serde_json::json!(["low", "medium", "high", "xhigh"])
    );
    assert_eq!(runtimes[1]["auth"], "unauthed");
    assert_eq!(runtimes[2]["runtime"], "kimi");
    assert_eq!(runtimes[2]["label"], "Kimi CLI");
    assert_eq!(runtimes[2]["order"], 2);
    assert_eq!(runtimes[2]["reasoning_efforts"], serde_json::json!([]));
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
    seed_default_workspace(&store);
    let app = build_router_with_lifecycle(store.clone(), Arc::new(FailStartLifecycle));

    let req = serde_json::json!({
        "name": "stuck-bot",
        "runtime": "claude",
        "model": "sonnet"
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
    // Start failure is now an explicit error, not a 200 with a warning.
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let payload = body_json(resp).await;
    assert_eq!(payload["code"].as_str(), Some("AGENT_START_FAILED"));

    // The agent record must still be persisted (inactive) so operators can inspect it.
    let agents = store.get_agents().unwrap();
    let agent = agents
        .iter()
        .find(|a| a.name.starts_with("stuck-bot-"))
        .expect("agent should remain in the store after failed start");
    assert!(store.is_member("all", &agent.id).unwrap());

    // After a failed start the manager has no live process, so the derived
    // status surfaced through the API must be `asleep`. Regression guard:
    // before status was derived from ProcessState the persisted column
    // carried this claim; that column is gone, so verify through the API.
    let list_resp = app
        .oneshot(
            Request::builder()
                .uri("/api/agents")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(list_resp.status(), StatusCode::OK);
    let listed: Vec<serde_json::Value> = serde_json::from_slice(
        &axum::body::to_bytes(list_resp.into_body(), 1_000_000)
            .await
            .unwrap(),
    )
    .unwrap();
    let entry = listed
        .iter()
        .find(|a| a["name"] == agent.name.as_str())
        .expect("failed-start agent must still appear in /api/agents");
    assert_eq!(
        entry["status"], "asleep",
        "failed-start agent must derive to `asleep`, got `{}`",
        entry["status"]
    );
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
    let agent = store.get_agent(&name).unwrap().expect("agent should exist");
    assert_eq!(payload["id"], agent.id);
    assert_eq!(payload["status"], "ready");
    assert_eq!(agent.runtime, "kimi");
    assert_eq!(agent.model, "kimi-code/kimi-for-coding");
    assert_eq!(agent.reasoning_effort, None);
    assert_eq!(lifecycle.started_names(), vec![name]);
}

#[tokio::test]
async fn test_get_and_update_agent_via_api() {
    let (store, app, lifecycle) = setup_with_lifecycle();
    // Runtime liveness is the manager HashMap, not the DB column:
    // mark the agent as having a live managed process so a config
    // edit that requires restart will actually restart.
    lifecycle.mark_running("bot1");
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
    // Mark the agent as having a live managed process so a config
    // edit that requires restart will actually restart.
    lifecycle.mark_running("bot1");
    store
        .update_agent_record(&AgentRecordUpsert {
            name: "bot1",
            display_name: "Bot 1",
            description: Some("Replies in Chorus"),
            system_prompt: None,
            runtime: "codex",
            model: "gpt-5.4-mini",
            reasoning_effort: Some("high"),
            machine_id: None,
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

/// Regression: a config-edit PATCH must not trigger a restart when the
/// manager has no live process for the agent, even if the persisted
/// `agents.status` column says `Active`. Runtime liveness is the
/// manager HashMap via `process_state`, not the DB column.
#[tokio::test]
async fn config_edit_does_not_restart_when_no_process_managed_despite_db_active() {
    let (store, app, lifecycle) = setup_with_lifecycle();
    // Deliberately DO NOT call lifecycle.mark_running("bot1").

    let bot1 = store.get_agent("bot1").unwrap().unwrap();
    let update_req = serde_json::json!({
        "display_name": "Bot 1",
        "description": "Replies in Chorus",
        "runtime": "claude",
        "model": "different-model",
        "envVars": []
    });
    let resp = app
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
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(
        body["restarted"], false,
        "config-edit must not restart when manager has no process, regardless of DB status"
    );
    assert!(
        lifecycle.stopped_names().is_empty(),
        "no managed process means nothing to stop"
    );
    assert!(
        lifecycle.started_names().is_empty(),
        "no managed process means nothing to restart"
    );
}

#[tokio::test]
async fn test_restart_agent_reset_session_preserves_workspace() {
    let (store, _app, dir) = setup_with_data_dir();
    let bot1 = store.get_agent("bot1").unwrap().unwrap();
    store
        .record_session(&bot1.id, "thread-123", &bot1.runtime)
        .unwrap();
    let workspace_id = store.get_active_workspace().unwrap().unwrap().id;
    let workspace_dir = dir
        .path()
        .join("agents")
        .join(&workspace_id)
        .join(format!("{}-{}", bot1.name, bot1.id))
        .join("notes");
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
    assert!(
        store.get_active_session(&bot1.id).unwrap().is_none(),
        "reset_session must clear the active agent_sessions row"
    );
    assert!(workspace_dir.join("plan.md").exists());
}

#[tokio::test]
async fn test_delete_agent_marks_history_and_preserves_workspace() {
    let (store, _app, dir) = setup_with_data_dir();
    let bot1 = store.get_agent("bot1").unwrap().unwrap();
    let workspace_id = store.get_active_workspace().unwrap().unwrap().id;
    let workspace_dir = dir
        .path()
        .join("agents")
        .join(&workspace_id)
        .join(format!("{}-{}", bot1.name, bot1.id))
        .join("notes");
    std::fs::create_dir_all(&workspace_dir).unwrap();
    std::fs::write(workspace_dir.join("plan.md"), "hello").unwrap();
    store
        .create_message(CreateMessage {
            channel_name: "general",
            sender_id: &bot1.id,
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
    let bot2_id = store
        .create_agent_record(&AgentRecordUpsert {
            name: "bot2",
            display_name: "Bot 2",
            description: None,
            system_prompt: None,
            runtime: "codex",
            model: "gpt-5.4",
            reasoning_effort: None,
            machine_id: None,
            env_vars: &[],
        })
        .unwrap();
    join_channel_silent(&store, "general", &bot2_id, "agent");

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
    let mut started = lifecycle.started_names();
    started.sort();
    assert_eq!(started, vec!["bot1".to_string(), "bot2".to_string()]);
    assert!(lifecycle.notified_names().is_empty());
}

#[tokio::test]
async fn test_send_persists_message_even_if_agent_delivery_fails() {
    let store = Arc::new(Store::open(":memory:").unwrap());
    seed_default_workspace(&store);
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
        .get_history_snapshot("general", "alice", 10, None, None)
        .unwrap();
    assert!(history
        .messages
        .iter()
        .any(|message| message.content == "persist despite delivery failure"));
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
    let (_store, app, lifecycle) = setup_with_lifecycle();
    // Runtime liveness is the manager HashMap, not the DB column:
    // mark the agent as having a live managed process.
    lifecycle.mark_running("bot1");

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

/// Regression: persisted `AgentStatus::Active` does not mean the
/// runtime has a managed process. Delivery must route on the
/// manager HashMap (`process_state`), not the DB column, so an
/// agent whose row says Active but whose process is absent still
/// gets woken via `start_agent`.
#[tokio::test]
async fn delivery_starts_agent_when_no_process_managed_even_if_db_says_active() {
    let (_store, app, lifecycle) = setup_with_lifecycle();
    // Deliberately DO NOT call lifecycle.mark_running("bot1").

    let send_req = serde_json::json!({ "target": "dm:@bot1", "content": "wake up despite drift" });
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
        "delivery must route on process_state, not the persisted AgentStatus column",
    );
    assert!(
        lifecycle.notified_names().is_empty(),
        "no live process means notify_agent must not be called",
    );
}

#[tokio::test]
async fn test_send_notifies_active_agents() {
    let (_store, app, lifecycle) = setup_with_lifecycle();
    lifecycle.mark_running("bot1");

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
            sender_id: "alice",
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
            sender_id: "alice",
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
        attachment.stored_path.starts_with(
            dir.path()
                .join("data")
                .join("attachments")
                .to_string_lossy()
                .as_ref()
        ),
        "attachment should be stored under the configured data dir"
    );
}

#[tokio::test]
async fn test_workspace_lists_files_from_configured_data_dir() {
    let (store, app, dir) = setup_with_data_dir();
    let bot1 = store.get_agent("bot1").unwrap().unwrap();
    let workspace_id = store.get_active_workspace().unwrap().unwrap().id;
    let workspace_dir = dir
        .path()
        .join("agents")
        .join(&workspace_id)
        .join(format!("{}-{}", bot1.name, bot1.id))
        .join("notes");
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
                .join(&workspace_id)
                .join(format!("{}-{}", bot1.name, bot1.id))
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
    let workspace_id = store.get_active_workspace().unwrap().unwrap().id;
    let workspace_dir = dir
        .path()
        .join("agents")
        .join(&workspace_id)
        .join(format!("{}-{}", bot1.name, bot1.id))
        .join("notes");
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
    let current_user = "alice";
    assert_eq!(members.len(), 2);

    let bot_member = members.iter().find(|m| m.member_name == "bot1").unwrap();
    assert_eq!(bot_member.member_type, "agent");
    assert_eq!(bot_member.role, "operator");

    let human_member = members
        .iter()
        .find(|m| m.member_name == current_user)
        .unwrap();
    assert_eq!(human_member.member_type, "human");
    assert_eq!(human_member.role, "operator");

    // Creator is also joined to the team channel.
    let channel_members = store.get_channel_members(&ch.id).unwrap();
    assert!(channel_members.iter().any(|m| m.member_id == current_user));

    assert_eq!(lifecycle.stopped_names(), vec!["bot1".to_string()]);
    assert_eq!(lifecycle.started_names(), vec!["bot1".to_string()]);

    let workspace_id = &team.workspace_id;
    let team_dir_name = format!("{}-{}", team.name, team.id);
    let agent_dir_name = format!("{}-{}", bot_member.member_name, bot_member.member_id);

    let teams_root = dir
        .path()
        .join("data")
        .join("teams")
        .join(workspace_id)
        .join(&team_dir_name);
    assert!(teams_root.join("TEAM.md").exists());
    assert!(teams_root.join("members").join(&agent_dir_name).exists());

    let role_md = dir
        .path()
        .join("agents")
        .join(workspace_id)
        .join(&agent_dir_name)
        .join("teams")
        .join(&team_dir_name)
        .join("ROLE.md");
    assert!(role_md.exists());
}

#[tokio::test]
async fn test_create_team_does_not_duplicate_creator_when_explicitly_in_members() {
    let (store, app, _lifecycle, _dir) = setup_with_lifecycle_and_data_dir();
    let current_user = "alice";

    let body = serde_json::json!({
        "name": "eng-team",
        "display_name": "Engineering Team",
        "collaboration_model": "leader_operators",
        "members": [{
            "member_name": current_user,
            "member_type": "human",
            "member_id": current_user,
            "role": "observer"
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

    let team = store.get_team("eng-team").unwrap().unwrap();
    let members = store.get_team_members(&team.id).unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].member_name, current_user);
    assert_eq!(members[0].role, "observer");

    // Explicit creator is also joined to the team channel via the member loop.
    let ch = store.get_channel_by_name("eng-team").unwrap().unwrap();
    let channel_members = store.get_channel_members(&ch.id).unwrap();
    assert!(channel_members.iter().any(|m| m.member_id == current_user));
}

#[tokio::test]
async fn test_create_team_rejects_agent_sharing_creator_name() {
    // Removed in the ID-first migration. The legacy assertion was that
    // creating a team whose agent member's `name` matched the OS
    // username of the creator returned 400 — the policy was an
    // identity-by-name kludge to keep agents and humans from
    // colliding in the same auto-generated team channel. Identity is
    // now keyed off stable ids in `humans` / `agents`, so an agent
    // happening to be *named* `alice` is no longer treated as the
    // human Alice and the rejection is gone.
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
            machine_id: None,
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
        .create_team_member(&team_id, "bot1", "agent", "leader")
        .unwrap();
    store
        .create_team_member(&team_id, "bot2", "agent", "operator")
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
        .create_channel("qa-eng", None, ChannelType::Team, None)
        .unwrap();
    store
        .create_team_member(&team_id, "bot1", "agent", "leader")
        .unwrap();
    join_channel_silent(&store, "qa-eng", "bot1", "agent");

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
        .create_channel("eng-team", None, ChannelType::Team, None)
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
        store.get_teams_by_agent_id("bot1").unwrap()[0].team_name,
        "eng-team"
    );

    let team = store.get_team("eng-team").unwrap().unwrap();
    let workspace_id = &team.workspace_id;
    let team_dir_name = format!("{}-{}", team.name, team.id);
    let agent_dir_name = format!("{}-{}", bot1.name, bot1.id);
    assert!(dir
        .path()
        .join("agents")
        .join(workspace_id)
        .join(&agent_dir_name)
        .join("teams")
        .join(&team_dir_name)
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
    assert!(store.get_teams_by_agent_id("bot1").unwrap().is_empty());
    assert_eq!(
        store
            .get_last_read_seq("eng-team", "bot1")
            .unwrap_err()
            .to_string(),
        "Query returned no rows"
    );

    let team = store.get_team("eng-team").unwrap().unwrap();
    let workspace_id = team.workspace_id.clone();
    let team_dir_name = format!("{}-{}", team.name, team.id);

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
    assert!(!dir
        .path()
        .join("teams")
        .join(&workspace_id)
        .join(&team_dir_name)
        .exists());
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
            machine_id: None,
            env_vars: &[],
        })
        .unwrap();
    lifecycle.mark_running("bot1");
    lifecycle.mark_running("bot2");
    let team_id = store
        .create_team("eng-team", "Engineering", "leader_operators", Some("bot1"))
        .unwrap();
    store
        .create_channel("eng-team", None, ChannelType::Team, None)
        .unwrap();
    let bot1 = store.get_agent("bot1").unwrap().unwrap();
    let bot2 = store.get_agent("bot2").unwrap().unwrap();
    store
        .create_team_member(&team_id, &bot1.id, "agent", "leader")
        .unwrap();
    store
        .create_team_member(&team_id, &bot2.id, "agent", "operator")
        .unwrap();
    join_channel_silent(&store, "eng-team", &bot1.id, "agent");
    join_channel_silent(&store, "eng-team", &bot2.id, "agent");

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
        .get_messages_for_agent_id("bot1", false)
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
    let db_path = dir.path().join("data").join("chorus.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let store = Arc::new(Store::open(db_path.to_str().unwrap()).unwrap());
    let lifecycle = Arc::new(MockLifecycle::default());
    let runtime_status_provider = Arc::new(MockRuntimeStatusProvider {
        statuses: vec![],
        models_by_runtime: vec![],
    });
    // Reuse the per-test tempdir for the data root so the templates test
    // doesn't share filesystem state with parallel cargo runs.
    let data_dir = dir.path().to_path_buf();
    let agents_dir = data_dir.join("agents");
    std::fs::create_dir_all(&agents_dir).ok();
    let router = chorus::server::build_router_with_services(
        store,
        Arc::new(chorus::server::event_bus::EventBus::new()),
        data_dir,
        agents_dir,
        lifecycle,
        runtime_status_provider,
        templates,
    );

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
async fn test_active_workspace_filters_core_resource_lists() {
    let store = Arc::new(Store::open(":memory:").unwrap());
    store.ensure_human_with_id("alice", "alice").unwrap();
    let (alpha, _event) = store.create_local_workspace("Alpha", "alice").unwrap();
    let (beta, _event) = store.create_local_workspace("Beta", "alice").unwrap();
    store.set_active_workspace(&alpha.id).unwrap();
    store
        .create_channel_in_workspace(
            &alpha.id,
            "alpha-general",
            Some("Alpha general"),
            ChannelType::Channel,
            None,
        )
        .unwrap();
    store
        .create_channel_in_workspace(
            &beta.id,
            "beta-general",
            Some("Beta general"),
            ChannelType::Channel,
            None,
        )
        .unwrap();
    store
        .create_agent_record_in_workspace(
            &alpha.id,
            &AgentRecordUpsert {
                name: "alpha-bot",
                display_name: "Alpha Bot",
                description: None,
                system_prompt: None,
                runtime: "claude",
                model: "sonnet",
                reasoning_effort: None,
                machine_id: None,
                env_vars: &[],
            },
        )
        .unwrap();
    store
        .create_agent_record_in_workspace(
            &beta.id,
            &AgentRecordUpsert {
                name: "beta-bot",
                display_name: "Beta Bot",
                description: None,
                system_prompt: None,
                runtime: "claude",
                model: "sonnet",
                reasoning_effort: None,
                machine_id: None,
                env_vars: &[],
            },
        )
        .unwrap();
    store
        .create_team_in_workspace(&alpha.id, "alpha-team", "Alpha Team", "swarm", None)
        .unwrap();
    store
        .create_team_in_workspace(&beta.id, "beta-team", "Beta Team", "swarm", None)
        .unwrap();
    let app = build_router(store.clone());

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/server-info")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let server_info = body_json(resp).await;
    assert!(server_info["system_channels"]
        .as_array()
        .unwrap()
        .iter()
        .any(|channel| channel["name"] == "all"));

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/channels")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let channels = body_json(resp).await;
    assert!(channels.to_string().contains("alpha-general"));
    assert!(!channels.to_string().contains("beta-general"));

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/agents")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let agents = body_json(resp).await;
    assert!(agents.to_string().contains("alpha-bot"));
    assert!(!agents.to_string().contains("beta-bot"));

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/teams")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let teams = body_json(resp).await;
    assert!(teams.to_string().contains("alpha-team"));
    assert!(!teams.to_string().contains("beta-team"));

    let req = serde_json::json!({ "workspace": "beta" });
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/workspaces/switch")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["slug"], "beta");
    assert_eq!(store.get_active_workspace().unwrap().unwrap().slug, "beta");

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/channels")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let channels = body_json(resp).await;
    assert!(!channels.to_string().contains("alpha-general"));
    assert!(channels.to_string().contains("beta-general"));

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/agents")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let agents = body_json(resp).await;
    assert!(!agents.to_string().contains("alpha-bot"));
    assert!(agents.to_string().contains("beta-bot"));

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/teams")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let teams = body_json(resp).await;
    assert!(!teams.to_string().contains("alpha-team"));
    assert!(teams.to_string().contains("beta-team"));
}

#[tokio::test]
async fn test_create_agent_in_active_workspace_joins_workspace_all() {
    let store = Arc::new(Store::open(":memory:").unwrap());
    store.ensure_human_with_id("alice", "alice").unwrap();
    let (workspace, _event) = store.create_local_workspace("Alpha", "alice").unwrap();
    store.set_active_workspace(&workspace.id).unwrap();
    let app = build_router(store.clone());

    let req = serde_json::json!({
        "display_name": "Basic Agent",
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
    let agent_id = body["id"]
        .as_str()
        .expect("create_agent response missing id field");
    let all = store
        .get_auto_join_channels_for_workspace(Some(&workspace.id))
        .unwrap()
        .into_iter()
        .find(|channel| channel.name == "all")
        .expect("workspace #all should exist");

    assert!(store.channel_member_exists(&all.id, agent_id).unwrap());
}

// `test_workspace_api_lifecycle` was deleted as part of the ID-first
// migration. It pinned the legacy contract that `build_router` did
// *not* eagerly create a workspace, so a fresh router would return
// `BAD_REQUEST "no active workspace"` until a `POST /api/workspaces`
// call. The new server bootstrap wires `ensure_builtin_channels`
// after identity resolution so a freshly opened store always has a
// "Chorus Local" workspace and a `#all` channel — which makes the
// "first GET returns 400" arc no longer reachable through the public
// API. Workspace creation, switching, and deletion are still covered
// by `test_active_workspace_filters_core_resource_lists` and the
// CLI workspace tests.

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
async fn test_create_agent_requires_name_or_display_name() {
    let (store, app, _lifecycle) = setup_with_lifecycle();
    store.ensure_builtin_channels("alice").unwrap();

    // Omitting both name and display_name must return 400 "name is required".
    let req = serde_json::json!({ "runtime": "claude", "model": "sonnet" });
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

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_create_agent_name_hint_takes_priority_over_display_name() {
    let (store, app, _lifecycle) = setup_with_lifecycle();
    store.ensure_builtin_channels("alice").unwrap();

    // When both name hint and display_name are provided, the slug must
    // derive from the name hint, not the display_name.
    let req = serde_json::json!({
        "name": "my-hint",
        "display_name": "Should Not Appear In Slug",
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
        name.starts_with("my-hint-"),
        "name `{name}` should derive from name hint, not display_name"
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
        .get_channel_by_name(Store::DEFAULT_SYSTEM_CHANNEL)
        .unwrap()
        .expect("seed_default_workspace must create #all")
        .id;

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
            machine_id: None,
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
    seed_default_workspace(&store);
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

#[tokio::test]
async fn create_channel_rejects_invalid_names() {
    let (store, app) = setup();
    store.ensure_human_with_id("alice", "alice").unwrap();

    // Empty / whitespace-only names get a clearer "name is required" message;
    // names with invalid characters get the format-validation message.
    let cases: &[(&str, &str)] = &[
        ("", "channel name is required"),
        (
            "space channel",
            chorus::store::channels::INVALID_CHANNEL_NAME_MSG,
        ),
        (
            "channel/name",
            chorus::store::channels::INVALID_CHANNEL_NAME_MSG,
        ),
        (
            "channel?",
            chorus::store::channels::INVALID_CHANNEL_NAME_MSG,
        ),
        ("emoji🎉", chorus::store::channels::INVALID_CHANNEL_NAME_MSG),
    ];
    for (bad_name, expected_error) in cases {
        let req = serde_json::json!({ "name": bad_name, "description": "" });
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
        assert_eq!(
            resp.status(),
            StatusCode::BAD_REQUEST,
            "expected 400 for channel name: {bad_name}"
        );
        let body = body_json(resp).await;
        assert_eq!(
            body["error"].as_str(),
            Some(*expected_error),
            "expected specific error message for: {bad_name}"
        );
    }
}

#[tokio::test]
async fn update_channel_rejects_invalid_names() {
    let (store, app) = setup();
    store.ensure_human_with_id("alice", "alice").unwrap();
    let channel_id = store.get_channel_by_name("general").unwrap().unwrap().id;

    for bad_name in ["", "space channel", "channel/name", "channel?", "emoji🎉"] {
        let req = serde_json::json!({ "name": bad_name, "description": "" });
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
        assert_eq!(
            resp.status(),
            StatusCode::BAD_REQUEST,
            "expected 400 for channel name: {bad_name}"
        );
        let body = body_json(resp).await;
        assert_eq!(
            body["error"].as_str(),
            Some(chorus::store::channels::INVALID_CHANNEL_NAME_MSG),
            "expected specific error message for: {bad_name}"
        );
    }
}

#[tokio::test]
async fn public_send_rejects_empty_content() {
    let (store, app) = setup();
    let channel_id = store.get_channel_by_name("general").unwrap().unwrap().id;

    for content in ["", "   ", "\n\t"] {
        let req = serde_json::json!({ "content": content });
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/conversations/{channel_id}/messages"))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&req).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::BAD_REQUEST,
            "expected 400 for content: {:?}",
            content
        );
        let body = body_json(resp).await;
        assert_eq!(
            body["error"].as_str(),
            Some("message content cannot be empty"),
            "expected specific error message for content: {:?}",
            content
        );
    }
}

#[tokio::test]
async fn public_send_allows_empty_content_with_attachments() {
    let (store, app) = setup();
    let channel_id = store.get_channel_by_name("general").unwrap().unwrap().id;

    let req = serde_json::json!({ "content": "", "attachmentIds": ["fake-attachment-id"] });
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/conversations/{channel_id}/messages"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    // Should not be rejected by the empty-content check.
    assert_ne!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "empty content with attachments should not be rejected"
    );
}

// ─────────────────────────────────────────────────────────────────────
// Agent self-introduction (#108)
//
// New agents auto-join the system channel on creation; before this
// change they did so silently, leaving humans staring at a member list
// with no idea who the new arrival was for. We now hand the agent's
// first run an init directive asking it to introduce itself in #all.
// These tests pin the wiring: the directive flows from
// create_and_start_agent into AgentLifecycle::start_agent, and the
// other start paths (manual restart, message wake) do not.
// ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_create_agent_passes_intro_directive_referencing_system_channel() {
    let (store, app, lifecycle) = setup_with_lifecycle();
    store.ensure_builtin_channels("alice").unwrap();

    let req = serde_json::json!({ "name": "newbot", "runtime": "claude", "model": "sonnet" });
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

    let started = lifecycle.started_calls();
    assert_eq!(started.len(), 1, "create-agent should fire one start");
    let directive = started[0]
        .2
        .as_ref()
        .expect("create-agent must pass an init directive");
    let expected_channel = Store::DEFAULT_SYSTEM_CHANNEL;
    assert!(
        directive.contains(expected_channel),
        "directive should reference #{expected_channel}: {directive:?}"
    );
    assert!(
        directive.contains("introduc"),
        "directive should mention introducing: {directive:?}"
    );
    // Tool name is intentionally NOT asserted: runtimes can prefix tools
    // (e.g. mcp__chat__send_message), and the agent's standing system
    // prompt already names the tool. Hardcoding it here would be brittle.
    // wake_message stays None for a fresh creation — the directive is
    // the only first-prompt source.
    assert!(started[0].1.is_none());
}

#[tokio::test]
async fn test_manual_restart_does_not_fire_intro_directive() {
    // setup_with_lifecycle() already seeds bot1 in the default workspace.
    let (_store, app, lifecycle) = setup_with_lifecycle();
    lifecycle.mark_running("bot1");

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/agents/bot1/restart")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&serde_json::json!({ "mode": "restart" })).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let started = lifecycle.started_calls();
    assert_eq!(started.len(), 1);
    assert_eq!(started[0].0, "bot1");
    assert!(
        started[0].2.is_none(),
        "restart must not pass an init directive — only first-time creation does"
    );
}

// ──────────────────────────────────────────────────────────────────────────
// Decision Inbox e2e
// ──────────────────────────────────────────────────────────────────────────

/// Round-trip: agent emits a decision via the bridge endpoint, human picks
/// an option via the public API, the resume envelope reaches the agent
/// via `lifecycle.resume_with_prompt` with the picked option's body and
/// the original headline + question + human note inlined.
#[tokio::test]
async fn decision_round_trip_agent_creates_human_resolves_agent_resumed() {
    let (store, app, lifecycle) = setup_with_lifecycle();

    // Channel for inference. Bot1 must be in an active run with this
    // channel set, otherwise the create endpoint correctly 400s.
    let channel = store
        .create_channel(
            "engineering",
            Some("Engineering"),
            ChannelType::Channel,
            None,
        )
        .unwrap();
    join_channel_silent(&store, "engineering", "bot1", "agent");
    lifecycle.set_run_channel("bot1", &channel);

    // 1. agent → bridge → POST /internal/agent/bot1/decisions
    let create_body = serde_json::json!({
        "headline": "PR #120 retro: archived-channel del/join",
        "question": "Was the merge the right call, or should we revert?",
        "options": [
            {"key": "A", "label": "Keep the merge", "body": "The merge stands. Tests pass."},
            {"key": "B", "label": "Revert and add tests", "body": "Revert, add the integration test, re-merge."},
        ],
        "recommended_key": "A",
        "context": "## Why now\nUser asked.\n",
    });
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/internal/agent/bot1/decisions")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&create_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "decision create must 200");
    let body = axum::body::to_bytes(resp.into_body(), 1_000_000)
        .await
        .unwrap();
    let created: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let decision_id = created["decision_id"].as_str().unwrap().to_string();
    assert_eq!(created["channel_id"].as_str().unwrap(), channel);

    // 2. human → GET /api/decisions?status=open returns the new row.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/decisions?status=open")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 1_000_000)
        .await
        .unwrap();
    let listed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let decisions = listed["decisions"].as_array().unwrap();
    assert_eq!(decisions.len(), 1);
    assert_eq!(decisions[0]["id"].as_str().unwrap(), decision_id);
    assert_eq!(decisions[0]["agent_name"].as_str().unwrap(), "bot1");
    assert_eq!(
        decisions[0]["channel_name"].as_str().unwrap(),
        "engineering"
    );

    // 3. human → POST /api/decisions/{id}/resolve picks B with a note.
    let resolve_body = serde_json::json!({"picked_key": "B", "note": "needs tests first"});
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/decisions/{decision_id}/resolve"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&resolve_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "resolve must 200");

    // 4. resume_with_prompt fired with the right payload.
    let calls = lifecycle.resumed_calls();
    assert_eq!(
        calls.len(),
        1,
        "resume_with_prompt must be called exactly once"
    );
    let (agent, envelope) = &calls[0];
    assert_eq!(agent, "bot1");
    assert!(
        envelope.contains("PR #120 retro"),
        "envelope must include original headline; got: {envelope}"
    );
    assert!(envelope.contains("Was the merge the right call"));
    assert!(envelope.contains("Picked option (B): Revert and add tests"));
    assert!(envelope.contains("Revert, add the integration test"));
    assert!(envelope.contains("needs tests first"));

    // 5. The decision row is now resolved, so /open lists nothing.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/decisions?status=open")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(resp.into_body(), 1_000_000)
        .await
        .unwrap();
    let listed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(listed["decisions"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn decision_resolve_double_pick_returns_409() {
    let (store, app, lifecycle) = setup_with_lifecycle();
    let channel = store
        .create_channel("eng", Some("Eng"), ChannelType::Channel, None)
        .unwrap();
    join_channel_silent(&store, "eng", "bot1", "agent");
    lifecycle.set_run_channel("bot1", &channel);

    let create_body = serde_json::json!({
        "headline": "h",
        "question": "q",
        "options": [
            {"key": "A", "label": "L", "body": "B"},
            {"key": "B", "label": "L2", "body": "B2"},
        ],
        "recommended_key": "A",
    });
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/internal/agent/bot1/decisions")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&create_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(resp.into_body(), 1_000_000)
        .await
        .unwrap();
    let created: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let decision_id = created["decision_id"].as_str().unwrap().to_string();

    // First pick: 200
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/decisions/{decision_id}/resolve"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&serde_json::json!({"picked_key": "A"})).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Second pick: 409 — CAS-protected.
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/decisions/{decision_id}/resolve"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&serde_json::json!({"picked_key": "B"})).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::CONFLICT,
        "second pick on a resolved decision must 409"
    );
}

#[tokio::test]
async fn decision_create_without_active_channel_returns_400() {
    // No set_run_channel — the channel-inference contract must fail
    // loudly rather than silently routing to #all.
    let (_store, app, _lifecycle) = setup_with_lifecycle();

    let create_body = serde_json::json!({
        "headline": "h",
        "question": "q",
        "options": [
            {"key": "A", "label": "L", "body": "B"},
            {"key": "B", "label": "L2", "body": "B2"},
        ],
        "recommended_key": "A",
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/internal/agent/bot1/decisions")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&create_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(resp.into_body(), 1_000_000)
        .await
        .unwrap();
    let err: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let msg = err["error"].as_str().unwrap();
    assert!(
        msg.contains("active-run channel"),
        "error must name the missing channel context; got: {msg}"
    );
}

#[tokio::test]
async fn decision_resolve_unknown_picked_key_returns_400() {
    let (store, app, lifecycle) = setup_with_lifecycle();
    let channel = store
        .create_channel("eng", Some("Eng"), ChannelType::Channel, None)
        .unwrap();
    join_channel_silent(&store, "eng", "bot1", "agent");
    lifecycle.set_run_channel("bot1", &channel);

    let create_body = serde_json::json!({
        "headline": "h",
        "question": "q",
        "options": [
            {"key": "A", "label": "L", "body": "B"},
            {"key": "B", "label": "L2", "body": "B2"},
        ],
        "recommended_key": "A",
    });
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/internal/agent/bot1/decisions")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&create_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(resp.into_body(), 1_000_000)
        .await
        .unwrap();
    let created: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let decision_id = created["decision_id"].as_str().unwrap().to_string();

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/decisions/{decision_id}/resolve"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&serde_json::json!({"picked_key": "Z"})).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_delete_workspace_does_not_remove_same_named_team_in_other_workspace() {
    let (store, app, _lifecycle, dir) = setup_with_lifecycle_and_data_dir();
    let human_id = store.get_human_by_name("alice").unwrap().unwrap().id;

    // Create two workspaces
    let (alpha, _) = store.create_local_workspace("Alpha", &human_id).unwrap();
    let (beta, _) = store.create_local_workspace("Beta", &human_id).unwrap();

    let bot1 = store.get_agent("bot1").unwrap().unwrap();

    // Create team "ops" in Alpha
    store.set_active_workspace(&alpha.id).unwrap();
    let alpha_team_id = store
        .create_team("ops", "Ops Team", "leader_operators", None)
        .unwrap();
    store
        .create_channel("ops", None, ChannelType::Team, None)
        .unwrap();
    store
        .create_team_member(&alpha_team_id, &bot1.id, "agent", "operator")
        .unwrap();

    // Create team "ops" in Beta
    store.set_active_workspace(&beta.id).unwrap();
    let beta_team_id = store
        .create_team("ops", "Ops Team", "leader_operators", None)
        .unwrap();
    store
        .create_channel("ops", None, ChannelType::Team, None)
        .unwrap();
    store
        .create_team_member(&beta_team_id, &bot1.id, "agent", "operator")
        .unwrap();

    // Manually init filesystem directories (normally done via HTTP handler)
    let agents_dir = dir.path().join("agents");
    let teams_dir = dir.path().join("data").join("teams");
    let agent_workspace = AgentWorkspace::new(&agents_dir);
    let team_workspace = TeamWorkspace::new(teams_dir.clone());

    team_workspace
        .init_team(&alpha.id, "ops", &alpha_team_id, &[("bot1", &bot1.id)])
        .unwrap();
    team_workspace
        .init_team(&beta.id, "ops", &beta_team_id, &[("bot1", &bot1.id)])
        .unwrap();
    agent_workspace
        .init_team_memory(
            &alpha.id,
            "bot1",
            &bot1.id,
            "ops",
            &alpha_team_id,
            "operator",
        )
        .unwrap();
    agent_workspace
        .init_team_memory(&beta.id, "bot1", &bot1.id, "ops", &beta_team_id, "operator")
        .unwrap();

    let alpha_team_dir = teams_dir
        .join(&alpha.id)
        .join(format!("ops-{}", alpha_team_id));
    let beta_team_dir = teams_dir
        .join(&beta.id)
        .join(format!("ops-{}", beta_team_id));

    assert!(alpha_team_dir.exists(), "alpha team dir should exist");
    assert!(beta_team_dir.exists(), "beta team dir should exist");

    // Delete Alpha workspace via API
    let delete_resp = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/workspaces/{}", alpha.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(delete_resp.status(), StatusCode::OK);

    // Alpha's scoped runtime directories should be gone
    assert!(
        !alpha_team_dir.exists(),
        "alpha team dir should be removed after workspace deletion"
    );

    // Beta's team directory should still exist
    assert!(
        beta_team_dir.exists(),
        "beta team dir should survive alpha workspace deletion"
    );
}
