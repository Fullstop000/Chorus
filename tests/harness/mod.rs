//! Shared test helpers for integration tests.
#![allow(dead_code)]

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use axum::Router;
use chorus::agent::activity_log::ActivityLogResponse;
use chorus::agent::runtime_status::{SharedRuntimeStatusProvider, SystemRuntimeStatusProvider};
use chorus::agent::AgentLifecycle;
use chorus::server::build_router_with_services;
use chorus::server::event_bus::EventBus;
use chorus::store::messages::ReceivedMessage;
use chorus::store::Store;
use rusqlite::params;

pub struct NoopLifecycle;

impl AgentLifecycle for NoopLifecycle {
    fn start_agent<'a>(
        &'a self,
        _agent_name: &'a str,
        _wake_message: Option<ReceivedMessage>,
        _init_directive: Option<String>,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
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

    fn process_state<'a>(
        &'a self,
        _agent_name: &'a str,
    ) -> Pin<Box<dyn Future<Output = Option<chorus::agent::drivers::ProcessState>> + Send + 'a>>
    {
        Box::pin(async { None })
    }

    fn get_all_agent_activity_states(&self) -> Vec<(String, String, String)> {
        vec![]
    }
}

pub fn build_router(store: Arc<Store>) -> Router {
    build_router_with_lifecycle(store, Arc::new(NoopLifecycle))
}

pub fn build_router_with_lifecycle(
    store: Arc<Store>,
    lifecycle: Arc<dyn AgentLifecycle>,
) -> Router {
    build_router_with_lifecycle_and_dir(store, lifecycle, unique_test_data_dir())
}

pub fn build_router_with_lifecycle_and_dir(
    store: Arc<Store>,
    lifecycle: Arc<dyn AgentLifecycle>,
    data_dir: std::path::PathBuf,
) -> Router {
    let agents_dir = data_dir.join("agents");
    std::fs::create_dir_all(&agents_dir).ok();
    build_router_with_services(
        store,
        Arc::new(EventBus::new()),
        data_dir,
        agents_dir,
        lifecycle,
        Arc::new(SystemRuntimeStatusProvider::new(
            chorus::agent::manager::build_driver_registry(),
        )) as SharedRuntimeStatusProvider,
        vec![],
    )
}

pub fn build_router_with_event_bus(store: Arc<Store>) -> (Router, Arc<EventBus>) {
    build_router_with_event_bus_and_dir(store, unique_test_data_dir())
}

pub fn build_router_with_event_bus_and_dir(
    store: Arc<Store>,
    data_dir: std::path::PathBuf,
) -> (Router, Arc<EventBus>) {
    let agents_dir = data_dir.join("agents");
    std::fs::create_dir_all(&agents_dir).ok();
    let event_bus = Arc::new(EventBus::new());
    let router = build_router_with_services(
        store,
        event_bus.clone(),
        data_dir,
        agents_dir,
        Arc::new(NoopLifecycle),
        Arc::new(SystemRuntimeStatusProvider::new(
            chorus::agent::manager::build_driver_registry(),
        )) as SharedRuntimeStatusProvider,
        vec![],
    );
    (router, event_bus)
}

/// Per-call unique tempdir so parallel cargo tests don't collide on
/// `attachments/`, `agents/`, `teams/` under a shared path. The dir is
/// leaked (no automatic cleanup) because callers don't carry a TempDir
/// guard; `$TMPDIR` is reclaimed by the OS. Tests that need explicit
/// cleanup should use the `_and_dir` variants with their own
/// `tempfile::TempDir`.
pub fn unique_test_data_dir() -> std::path::PathBuf {
    let dir = tempfile::Builder::new()
        .prefix("chorus-test-")
        .tempdir()
        .expect("create test data dir");
    let path = dir.path().to_path_buf();
    // Suppress Drop so the tempdir survives past this function. The OS
    // reclaims `$TMPDIR` entries; tests run for seconds, fixtures don't
    // accumulate meaningfully.
    std::mem::forget(dir);
    path
}

/// Test helper: silently insert a channel membership row without emitting
/// events or creating a system message.
pub fn join_channel_silent(store: &Store, channel_name: &str, member_id: &str, member_type: &str) {
    let conn = store.conn_for_test();
    let channel_id: String = conn
        .query_row(
            "SELECT id FROM channels WHERE name = ?1",
            params![channel_name],
            |r| r.get(0),
        )
        .unwrap();
    conn.execute(
        "INSERT OR IGNORE INTO channel_members (channel_id, member_id, member_type, last_read_seq)
         VALUES (?1, ?2, ?3, 0)",
        params![channel_id, member_id, member_type],
    )
    .unwrap();
}
