pub use crate::utils::error;
mod handlers;
pub mod transport;

use std::sync::Arc;
use std::{collections::HashSet, sync::Mutex};

use axum::body::Body;
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post, put};
use axum::Router;
use rust_embed::RustEmbed;
use tower_http::cors::{Any, CorsLayer};

#[derive(RustEmbed)]
#[folder = "ui/dist/"]
struct UiAssets;

async fn serve_ui(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let candidate = if path.is_empty() { "index.html" } else { path };
    let Some(file) = UiAssets::get(candidate).or_else(|| UiAssets::get("index.html")) else {
        return (StatusCode::NOT_FOUND, "UI assets missing").into_response();
    };
    let mime = file.metadata.mimetype();
    let mut response = Response::new(Body::from(file.data.into_owned()));
    match header::HeaderValue::from_str(mime) {
        Ok(value) => {
            response.headers_mut().insert(header::CONTENT_TYPE, value);
            response
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn health() -> &'static str {
    "ok"
}

use crate::agent::runtime_status::SharedRuntimeStatusProvider;
use crate::agent::templates::AgentTemplate;
use crate::agent::AgentLifecycle;
use crate::store::Store;

pub use handlers::dto;
pub use handlers::server_info::{build_server_info, build_ui_shell_info};
pub use handlers::{AgentDetailResponse, AppState, HistoryResponse};

pub fn build_router_with_services(
    store: Arc<Store>,
    lifecycle: Arc<dyn AgentLifecycle>,
    runtime_status_provider: SharedRuntimeStatusProvider,
    templates: Vec<AgentTemplate>,
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
        templates: Arc::new(templates),
    };

    // Agent runtimes and CLI flows still depend on an agent-scoped internal API.
    // In particular, `/internal/agent/{agent_id}/server` is the historical
    // workspace-discovery route for bridge clients. These routes intentionally
    // differ from `/api/*`, which is keyed to the current human viewer and
    // conversation IDs.
    let internal_router = Router::new()
        .route("/agent/{agent_id}/send", post(handle_send))
        .route("/agent/{agent_id}/receive", get(handle_receive))
        .route("/agent/{agent_id}/history", get(handle_history))
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
        .route("/agent/{agent_id}/upload", post(handle_upload));

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
            "/conversations/{conversation_id}/inbox-notification",
            get(handle_public_conversation_inbox_notification),
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
        .route(
            "/runtimes/{runtime}/models",
            get(handle_list_runtime_models),
        )
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
        .route("/templates", get(handle_list_templates))
        .route("/templates/launch-trio", post(handle_launch_trio))
        .route("/server-info", get(handle_ui_server_info))
        .route("/events/ws", get(handle_events_ws))
        .route("/traces/{run_id}", get(handle_trace_events))
        .route("/agents/{name}/runs", get(handle_agent_runs));

    Router::new()
        .route("/health", get(health))
        .nest("/internal", internal_router)
        .nest("/api", api_router)
        .layer(cors)
        // Only GET falls through to the embedded UI — non-GET requests to
        // unmatched paths (e.g. removed `/internal/.../remember`) should
        // return 405/404 rather than silently serving index.html.
        .fallback_service(get(serve_ui))
        .with_state(state)
}
