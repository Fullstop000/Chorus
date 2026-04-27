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
use chorus::store::messages::ReceivedMessage;
use chorus::store::Store;
use rusqlite::params;

pub struct NoopLifecycle;

impl AgentLifecycle for NoopLifecycle {
    fn start_agent<'a>(
        &'a self,
        _agent_name: &'a str,
        _wake_message: Option<ReceivedMessage>,
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
    build_router_with_services(
        store,
        lifecycle,
        Arc::new(SystemRuntimeStatusProvider::new(
            chorus::agent::manager::build_driver_registry(),
        )) as SharedRuntimeStatusProvider,
        vec![],
    )
}

/// Test helper: silently insert a channel membership row without emitting
/// events or creating a system message.
pub fn join_channel_silent(
    store: &Store,
    channel_name: &str,
    member_id: &str,
    member_type: &str,
) {
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
