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
        Arc::new(SystemRuntimeStatusProvider) as SharedRuntimeStatusProvider,
        vec![],
    )
}
