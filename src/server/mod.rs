mod handlers;
pub mod transport;

use std::sync::Arc;
use std::{collections::HashSet, sync::Mutex};

use axum::routing::{get, patch, post, put};
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

    let internal_router = Router::new()
        .route("/agent/{agent_id}/send", post(handle_send))
        .route("/agent/{agent_id}/receive", get(handle_receive))
        .route("/agent/{agent_id}/history", get(handle_history))
        .route("/agent/{agent_id}/inbox", get(handle_inbox))
        .route("/agent/{agent_id}/threads", get(handle_threads))
        .route(
            "/agent/{agent_id}/read-cursor",
            post(handle_update_read_cursor),
        )
        .route("/agent/{agent_id}/server", get(handle_server_info))
        .route(
            "/agent/{agent_id}/resolve-channel",
            post(handle_resolve_channel),
        )
        .route(
            "/agent/{agent_id}/tasks",
            get(handle_list_tasks).post(handle_create_tasks),
        )
        .route("/agent/{agent_id}/tasks/claim", post(handle_claim_tasks))
        .route("/agent/{agent_id}/tasks/unclaim", post(handle_unclaim_task))
        .route(
            "/agent/{agent_id}/tasks/update-status",
            post(handle_update_task_status),
        )
        .route("/agent/{agent_id}/upload", post(handle_upload))
        .route("/agent/{agent_id}/remember", post(handle_remember))
        .route("/agent/{agent_id}/recall", get(handle_recall));

    let api_router = Router::new()
        .route("/attachments/{attachment_id}", get(handle_get_attachment))
        .route("/attachments", post(handle_public_upload))
        .route("/whoami", get(handle_whoami))
        .route("/humans", get(handle_list_humans))
        .route("/inbox", get(handle_public_inbox))
        .route("/dms/{peer_name}", put(handle_public_ensure_dm))
        .route(
            "/conversations/{conversation_id}/messages",
            get(handle_public_history).post(handle_public_send),
        )
        .route(
            "/conversations/{conversation_id}/threads",
            get(handle_public_threads),
        )
        .route(
            "/conversations/{conversation_id}/read-cursor",
            post(handle_public_update_read_cursor),
        )
        .route(
            "/conversations/{conversation_id}/tasks",
            get(handle_public_list_tasks).post(handle_public_create_tasks),
        )
        .route(
            "/conversations/{conversation_id}/tasks/claim",
            post(handle_public_claim_tasks),
        )
        .route(
            "/conversations/{conversation_id}/tasks/unclaim",
            post(handle_public_unclaim_task),
        )
        .route(
            "/conversations/{conversation_id}/tasks/update-status",
            post(handle_public_update_task_status),
        )
        .route(
            "/channels",
            get(handle_list_channels).post(handle_create_channel),
        )
        .route("/agents", get(handle_list_agents).post(handle_create_agent))
        .route("/runtimes", get(handle_list_runtime_statuses))
        .route("/teams", get(handle_list_teams).post(handle_create_team))
        .route(
            "/teams/{name}",
            get(handle_get_team)
                .patch(handle_update_team)
                .delete(handle_delete_team),
        )
        .route("/teams/{name}/members", post(handle_add_team_member))
        .route(
            "/teams/{name}/members/{member}",
            axum::routing::delete(handle_remove_team_member),
        )
        .route(
            "/channels/{channel_id}/members",
            get(handle_list_channel_members).post(handle_invite_channel_member),
        )
        .route(
            "/channels/{channel_id}",
            patch(handle_update_channel).delete(handle_delete_channel),
        )
        .route(
            "/channels/{channel_id}/archive",
            post(handle_archive_channel),
        )
        .route(
            "/agents/{name}",
            get(handle_get_agent).patch(handle_update_agent),
        )
        .route("/agents/{name}/start", post(handle_agent_start))
        .route("/agents/{name}/stop", post(handle_agent_stop))
        .route("/agents/{name}/restart", post(handle_restart_agent))
        .route("/agents/{name}/delete", post(handle_delete_agent))
        .route("/agents/{name}/activity", get(handle_agent_activity))
        .route(
            "/agents/{name}/activity-log",
            get(handle_agent_activity_log),
        )
        .route("/agents/{name}/workspace", get(handle_agent_workspace))
        .route(
            "/agents/{name}/workspace/file",
            get(handle_agent_workspace_file),
        )
        .route("/server-info", get(handle_ui_server_info))
        .route("/events/ws", get(handle_events_ws));

    Router::new()
        .nest("/internal", internal_router)
        .nest("/api", api_router)
        .layer(cors)
        .fallback_service(ServeDir::new("ui/dist").fallback(ServeFile::new("ui/dist/index.html")))
        .with_state(state)
}
