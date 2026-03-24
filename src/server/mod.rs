mod handlers;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::{collections::HashSet, sync::Mutex};

use axum::routing::{get, patch, post};
use axum::Router;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};

use crate::models::*;
use crate::store::Store;

pub use handlers::AppState;

/// Runtime lifecycle operations the HTTP server can trigger for agents.
pub trait AgentLifecycle: Send + Sync {
    fn start_agent<'a>(
        &'a self,
        agent_name: &'a str,
        wake_message: Option<ReceivedMessage>,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>>;

    fn notify_agent<'a>(
        &'a self,
        agent_name: &'a str,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>>;

    fn stop_agent<'a>(
        &'a self,
        agent_name: &'a str,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>>;

    fn get_activity_log_data(
        &self,
        agent_name: &str,
        after_seq: Option<u64>,
    ) -> ActivityLogResponse;

    fn get_all_agent_activity_states(&self) -> Vec<(String, String, String)>;

    /// Append a UI-visible activity entry for an agent.
    fn push_activity_entry(&self, agent_name: &str, entry: ActivityEntry);
}

struct NoopAgentLifecycle;

impl AgentLifecycle for NoopAgentLifecycle {
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

    fn push_activity_entry(&self, _agent_name: &str, _entry: ActivityEntry) {}
}

pub fn build_router(store: Arc<Store>) -> Router {
    build_router_with_lifecycle(store, Arc::new(NoopAgentLifecycle))
}

pub fn build_router_with_lifecycle(
    store: Arc<Store>,
    lifecycle: Arc<dyn AgentLifecycle>,
) -> Router {
    use handlers::*;

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let state = AppState {
        store,
        lifecycle,
        transitioning_agents: Arc::new(Mutex::new(HashSet::new())),
    };

    Router::new()
        // Agent bridge endpoints (called by MCP bridge subprocess)
        .route("/internal/agent/{agent_id}/send", post(handle_send))
        .route("/internal/agent/{agent_id}/receive", get(handle_receive))
        .route("/internal/agent/{agent_id}/history", get(handle_history))
        .route("/internal/agent/{agent_id}/server", get(handle_server_info))
        .route(
            "/internal/agent/{agent_id}/resolve-channel",
            post(handle_resolve_channel),
        )
        .route(
            "/internal/agent/{agent_id}/tasks",
            get(handle_list_tasks).post(handle_create_tasks),
        )
        .route(
            "/internal/agent/{agent_id}/tasks/claim",
            post(handle_claim_tasks),
        )
        .route(
            "/internal/agent/{agent_id}/tasks/unclaim",
            post(handle_unclaim_task),
        )
        .route(
            "/internal/agent/{agent_id}/tasks/update-status",
            post(handle_update_task_status),
        )
        .route("/internal/agent/{agent_id}/upload", post(handle_upload))
        .route("/internal/agent/{agent_id}/remember", post(handle_remember))
        .route("/internal/agent/{agent_id}/recall", get(handle_recall))
        .route(
            "/api/attachments/{attachment_id}",
            get(handle_get_attachment),
        )
        // UI / management endpoints
        .route("/api/whoami", get(handle_whoami))
        .route("/api/channels", post(handle_create_channel))
        .route(
            "/api/channels/{channel_id}",
            patch(handle_update_channel).delete(handle_delete_channel),
        )
        .route(
            "/api/channels/{channel_id}/archive",
            post(handle_archive_channel),
        )
        .route("/api/agents", post(handle_create_agent))
        .route(
            "/api/agents/{name}",
            get(handle_get_agent).patch(handle_update_agent),
        )
        .route("/api/agents/{name}/start", post(handle_agent_start))
        .route("/api/agents/{name}/stop", post(handle_agent_stop))
        .route("/api/agents/{name}/restart", post(handle_restart_agent))
        .route("/api/agents/{name}/delete", post(handle_delete_agent))
        .route("/api/agents/{name}/activity", get(handle_agent_activity))
        .route(
            "/api/agents/{name}/activity-log",
            get(handle_agent_activity_log),
        )
        .route("/api/agents/{name}/workspace", get(handle_agent_workspace))
        .route(
            "/api/agents/{name}/workspace/file",
            get(handle_agent_workspace_file),
        )
        .route("/api/server-info", get(handle_ui_server_info))
        .layer(cors)
        .fallback_service(ServeDir::new("ui/dist").fallback(ServeFile::new("ui/dist/index.html")))
        .with_state(state)
}
