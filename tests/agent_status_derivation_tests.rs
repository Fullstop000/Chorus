// Integration tests for Task 2.3: AgentInfo.status derivation in API responses.
//
// These tests verify that the /api/agents and /api/agents/{id} endpoints return
// the new four-value Status (working / ready / asleep / failed) rather than the
// old persisted AgentStatus strings.

mod harness;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use chorus::agent::AgentLifecycle;
use chorus::store::channels::ChannelType;
use chorus::store::messages::{ReceivedMessage, SenderType};
use chorus::store::AgentRecordUpsert;
use chorus::store::Store;
use harness::build_router_with_lifecycle;
use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

use chorus::agent::activity_log::{self, ActivityLogMap, ActivityLogResponse};

// ── MockLifecycle (mirrored from server_tests.rs) ────────────────────────────

#[derive(Default)]
struct MockLifecycle {
    running: Mutex<HashSet<String>>,
    activity_logs: ActivityLogMap,
}

impl AgentLifecycle for MockLifecycle {
    fn start_agent<'a>(
        &'a self,
        agent_name: &'a str,
        _wake_message: Option<ReceivedMessage>,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>> {
        Box::pin(async move {
            self.running.lock().unwrap().insert(agent_name.to_string());
            Ok(())
        })
    }

    fn notify_agent<'a>(
        &'a self,
        _agent_name: &'a str,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }

    fn stop_agent<'a>(
        &'a self,
        agent_name: &'a str,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>> {
        Box::pin(async move {
            self.running.lock().unwrap().remove(agent_name);
            Ok(())
        })
    }

    fn process_state<'a>(
        &'a self,
        agent_name: &'a str,
    ) -> Pin<Box<dyn Future<Output = Option<chorus::agent::drivers::ProcessState>> + Send + 'a>>
    {
        let is_running = self.running.lock().unwrap().contains(agent_name);
        Box::pin(async move {
            if is_running {
                Some(chorus::agent::drivers::ProcessState::Active {
                    session_id: "test-session".into(),
                })
            } else {
                None
            }
        })
    }

    fn get_activity_log_data(
        &self,
        agent_name: &str,
        after_seq: Option<u64>,
    ) -> ActivityLogResponse {
        activity_log::get_activity_log(&self.activity_logs, agent_name, after_seq)
    }

    fn get_all_agent_activity_states(&self) -> Vec<(String, String, String)> {
        activity_log::all_activity_states(&self.activity_logs)
    }
}

// ── Setup helpers ─────────────────────────────────────────────────────────────

fn setup_with_lifecycle() -> (Arc<Store>, axum::Router, Arc<MockLifecycle>) {
    let store = Arc::new(Store::open(":memory:").unwrap());
    store
        .create_channel("general", Some("General"), ChannelType::Channel, None)
        .unwrap();
    store.ensure_human_with_id("alice", "alice").unwrap();
    store
        .join_channel("general", "alice", SenderType::Human)
        .unwrap();
    let lifecycle = Arc::new(MockLifecycle::default());
    let router = build_router_with_lifecycle(store.clone(), lifecycle.clone());
    (store, router, lifecycle)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// A freshly-created agent has no live process (MockLifecycle.running is empty
/// for it when created via store directly, without start_agent). The API must
/// return `status: "asleep"`.
#[tokio::test]
async fn list_agents_returns_asleep_for_unmanaged_agent() {
    let (store, app, _lifecycle) = setup_with_lifecycle();

    // Insert agent directly via store — does NOT call start_agent, so the
    // running set stays empty for this agent.
    store
        .create_agent_record(&AgentRecordUpsert {
            name: "bot-asleep",
            display_name: "Asleep Bot",
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
    let agents: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
    let bot = agents
        .iter()
        .find(|a| a["name"] == "bot-asleep")
        .expect("bot-asleep should be in the agent list");
    assert_eq!(
        bot["status"], "asleep",
        "unmanaged agent must report `asleep`, got `{}`",
        bot["status"]
    );
}

/// An agent that has an active process (inserted into the running set) returns
/// `status: "ready"` because MockLifecycle.process_state returns Active.
#[tokio::test]
async fn list_agents_returns_ready_for_running_agent() {
    let (store, app, lifecycle) = setup_with_lifecycle();

    store
        .create_agent_record(&AgentRecordUpsert {
            name: "bot-running",
            display_name: "Running Bot",
            description: None,
            system_prompt: None,
            runtime: "claude",
            model: "sonnet",
            reasoning_effort: None,
            env_vars: &[],
        })
        .unwrap();

    // Manually insert into the running set (simulates start_agent having been
    // called, and the process reaching the Active/idle state).
    lifecycle
        .running
        .lock()
        .unwrap()
        .insert("bot-running".to_string());

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
    let agents: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
    let bot = agents
        .iter()
        .find(|a| a["name"] == "bot-running")
        .expect("bot-running should be in the agent list");
    assert_eq!(
        bot["status"], "ready",
        "running agent (Active ProcessState) must report `ready`, got `{}`",
        bot["status"]
    );
}

/// The /api/agents/{id} detail endpoint must also surface the derived status.
#[tokio::test]
async fn get_agent_returns_asleep_for_unmanaged_agent() {
    let (store, app, _lifecycle) = setup_with_lifecycle();

    store
        .create_agent_record(&AgentRecordUpsert {
            name: "bot-detail-asleep",
            display_name: "Detail Asleep Bot",
            description: None,
            system_prompt: None,
            runtime: "claude",
            model: "sonnet",
            reasoning_effort: None,
            env_vars: &[],
        })
        .unwrap();

    let agent = store
        .get_agent("bot-detail-asleep")
        .unwrap()
        .expect("agent must exist");

    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/agents/{}", agent.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = axum::body::to_bytes(resp.into_body(), 1_000_000)
        .await
        .unwrap();
    let detail: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        detail["agent"]["status"], "asleep",
        "unmanaged agent detail must report `asleep`, got `{}`",
        detail["agent"]["status"]
    );
}
