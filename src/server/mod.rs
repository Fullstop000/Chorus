mod handlers;
pub mod transport;

use std::sync::Arc;
use std::{collections::HashSet, sync::Mutex};

use axum::routing::{get, patch, post};
use axum::Router;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};

use crate::agent::runtime_status::{SharedRuntimeStatusProvider, SystemRuntimeStatusProvider};
use crate::agent::{AgentLifecycle, NoopAgentLifecycle};
use crate::store::Store;

pub use handlers::dto;
pub use handlers::server_info::{build_server_info, build_ui_shell_info};
pub use handlers::{AgentDetailResponse, AppState, HistoryResponse};

pub fn build_router(store: Arc<Store>) -> Router {
    build_router_with_lifecycle(store, Arc::new(NoopAgentLifecycle))
}

pub fn build_router_with_lifecycle(
    store: Arc<Store>,
    lifecycle: Arc<dyn AgentLifecycle>,
) -> Router {
    build_router_with_services(
        store,
        lifecycle,
        Arc::new(SystemRuntimeStatusProvider) as SharedRuntimeStatusProvider,
    )
}

pub fn build_router_with_services(
    store: Arc<Store>,
    lifecycle: Arc<dyn AgentLifecycle>,
    runtime_status_provider: SharedRuntimeStatusProvider,
) -> Router {
    use handlers::*;
    use transport::realtime::handle_events_ws;

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let state = AppState {
        store,
        lifecycle,
        runtime_status_provider,
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
        .route(
            "/api/channels",
            get(handle_list_channels).post(handle_create_channel),
        )
        .route(
            "/api/agents",
            get(handle_list_agents).post(handle_create_agent),
        )
        .route("/api/runtimes", get(handle_list_runtime_statuses))
        .route(
            "/api/teams",
            get(handle_list_teams).post(handle_create_team),
        )
        .route(
            "/api/teams/{name}",
            get(handle_get_team)
                .patch(handle_update_team)
                .delete(handle_delete_team),
        )
        .route("/api/teams/{name}/members", post(handle_add_team_member))
        .route(
            "/api/teams/{name}/members/{member}",
            axum::routing::delete(handle_remove_team_member),
        )
        .route(
            "/api/channels/{channel_id}/members",
            get(handle_list_channel_members).post(handle_invite_channel_member),
        )
        .route(
            "/api/channels/{channel_id}",
            patch(handle_update_channel).delete(handle_delete_channel),
        )
        .route(
            "/api/channels/{channel_id}/archive",
            post(handle_archive_channel),
        )
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
        .route("/api/events/ws", get(handle_events_ws))
        .layer(cors)
        .fallback_service(ServeDir::new("ui/dist").fallback(ServeFile::new("ui/dist/index.html")))
        .with_state(state)
}
