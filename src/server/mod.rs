pub use crate::utils::error;
pub mod auth;
pub mod bridge_auth;
pub mod bridge_registry;
pub mod event_bus;
mod handlers;
/// Re-export so tests + reconnect-replay can rebuild the resume directive
/// from a `DecisionRow` without exposing the full handler surface area.
pub use handlers::decisions::build_resume_envelope_from_row;
pub mod transport;

use std::sync::Arc;
use std::{collections::HashSet, sync::Mutex};

use axum::body::Body;
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post, put};
use axum::Router;
use rust_embed::RustEmbed;
use tokio::sync::RwLock;
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
use crate::config::ChorusConfig;
use crate::server::event_bus::EventBus;
use crate::store::Store;

pub use handlers::dto;
pub use handlers::server_info::{
    build_server_info, build_server_info_for_workspace, build_ui_shell_info,
    build_ui_shell_info_for_workspace,
};
pub use handlers::{AgentDetailResponse, AppState, HistoryResponse};

pub fn build_router_with_services(
    store: Arc<Store>,
    event_bus: Arc<EventBus>,
    data_dir: std::path::PathBuf,
    agents_dir: std::path::PathBuf,
    lifecycle: Arc<dyn AgentLifecycle>,
    runtime_status_provider: SharedRuntimeStatusProvider,
    templates: Vec<AgentTemplate>,
) -> Router {
    build_router_with_services_and_auth(
        store,
        event_bus,
        data_dir,
        agents_dir,
        lifecycle,
        runtime_status_provider,
        templates,
        bridge_auth::BridgeAuth::empty(),
    )
}

/// Same as `build_router_with_services`, but accepts a pre-built
/// `BridgeAuth`. The CLI binary calls this with `BridgeAuth::from_env()`
/// so the deployed `chorus serve` enforces tokens; tests inject explicit
/// `BridgeAuth::from_pairs(...)` to exercise the auth path without
/// touching process env vars.
///
/// `clippy::too_many_arguments` is silenced here because this signature
/// is purely additive over `build_router_with_services` (which already
/// hits the 7-arg limit). Folding the existing args into a config
/// struct is the right long-term answer but would touch every test
/// harness + CLI caller in one go — out of scope for this PR.
#[allow(clippy::too_many_arguments)]
pub fn build_router_with_services_and_auth(
    store: Arc<Store>,
    event_bus: Arc<EventBus>,
    data_dir: std::path::PathBuf,
    agents_dir: std::path::PathBuf,
    lifecycle: Arc<dyn AgentLifecycle>,
    runtime_status_provider: SharedRuntimeStatusProvider,
    templates: Vec<AgentTemplate>,
    bridge_auth: Arc<bridge_auth::BridgeAuth>,
) -> Router {
    use handlers::*;
    use transport::bridge_ws::handle_bridge_ws;
    use transport::realtime::handle_events_ws;

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let active_workspace_id = store
        .get_active_workspace()
        .ok()
        .flatten()
        .map(|workspace| workspace.id);
    let (local_human_id, local_human_name) =
        resolve_local_human_identity(store.as_ref(), &data_dir);
    let local_machine_id = resolve_local_machine_id(&data_dir);

    // Built-in channels (`#all`) and the local human's membership are seeded
    // here, after identity resolution: the legacy CLI bootstrap used the OS
    // username as both the human row PK and the `#all` member key, which is
    // incompatible with the ID-first model. Failing here would leave the API
    // running against a database with no `#all` channel, so we log and
    // continue; the server is still useful for diagnostics.
    if let Err(err) = store.ensure_builtin_channels(&local_human_id) {
        tracing::error!(err = %err, "failed to ensure built-in channels for local human");
    }

    let state = AppState {
        store,
        event_bus,
        data_dir,
        agents_dir,
        active_workspace_id: Arc::new(RwLock::new(active_workspace_id)),
        local_human_id,
        local_human_name,
        local_machine_id,
        lifecycle,
        runtime_status_provider,
        transitioning_agents: Arc::new(Mutex::new(HashSet::new())),
        templates: Arc::new(templates),
        bridge_registry: bridge_registry::BridgeRegistry::new(),
        bridge_auth,
    };

    // Spawn a single forwarder task that subscribes once to the event
    // bus and pushes `chat.message.received` frames over the WS to every
    // connected bridge whenever a `message.created` stream event lands.
    // This avoids editing every `publish_stream` call site in the
    // handler tree (~20 of them) and keeps the wire-emit logic in one
    // place. The task runs for the lifetime of the process.
    {
        let bridge_registry = state.bridge_registry.clone();
        let store = state.store.clone();
        let mut rx = state.event_bus.subscribe();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        transport::bridge_ws::forward_chat_event_to_bridges(
                            store.as_ref(),
                            bridge_registry.as_ref(),
                            &event,
                        );
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        // Forwarder fell behind; drop the gap and keep
                        // streaming. Bridge will see the next message
                        // on the next event; reconcile-on-reconnect
                        // catches up the rest.
                        continue;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

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
        .route("/agent/{agent_id}/upload", post(handle_upload))
        .route("/agent/{agent_id}/decisions", post(handle_create_decision))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            bridge_auth::require_bridge_auth,
        ));

    let api_router = Router::new()
        .route("/attachments/{attachment_id}", get(handle_get_attachment))
        .route("/attachments", post(handle_public_upload))
        .route("/decisions", get(handle_list_decisions))
        .route(
            "/decisions/{decision_id}/resolve",
            post(handle_resolve_decision),
        )
        .route("/whoami", get(handle_whoami))
        .route("/humans", get(handle_list_humans))
        .route("/humans/{id}", patch(handle_update_human))
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
            "/conversations/{conversation_id}/read-cursor",
            post(handle_public_update_read_cursor),
        )
        .route(
            "/conversations/{conversation_id}/tasks",
            get(handle_public_list_tasks).post(handle_public_create_tasks),
        )
        .route(
            "/conversations/{conversation_id}/tasks/{task_number}",
            get(handle_get_task_detail),
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
            "/teams/{id}",
            get(handle_get_team)
                .patch(handle_update_team)
                .delete(handle_delete_team),
        )
        .route("/teams/{id}/members", post(handle_add_team_member))
        .route(
            "/workspaces",
            get(handle_list_workspaces).post(handle_create_workspace),
        )
        .route(
            "/workspaces/current",
            get(handle_current_workspace).patch(handle_rename_current_workspace),
        )
        .route("/workspaces/{workspace}", delete(handle_delete_workspace))
        .route("/workspaces/switch", post(handle_switch_workspace))
        .route(
            "/teams/{id}/members/{member}",
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
            "/agents/{id}",
            get(handle_get_agent).patch(handle_update_agent),
        )
        .route("/agents/{id}/start", post(handle_agent_start))
        .route("/agents/{id}/stop", post(handle_agent_stop))
        .route("/agents/{id}/restart", post(handle_restart_agent))
        .route("/agents/{id}/delete", post(handle_delete_agent))
        .route("/agents/{id}/activity", get(handle_agent_activity))
        .route("/agents/{id}/activity-log", get(handle_agent_activity_log))
        .route("/agents/{id}/workspace", get(handle_agent_workspace))
        .route(
            "/agents/{id}/workspace/file",
            get(handle_agent_workspace_file),
        )
        .route("/templates", get(handle_list_templates))
        .route("/templates/launch-trio", post(handle_launch_trio))
        .route("/server-info", get(handle_ui_server_info))
        .route("/system-info", get(handle_system_info))
        .route("/logs", get(handle_logs))
        .route("/traces/{run_id}", get(handle_trace_events))
        .route("/agents/{id}/runs", get(handle_agent_runs));

    // Strict auth on `/api/*`. Every handler reads its actor from the
    // request extension; no fallback to a server-cached identity exists
    // any more. Routes that need to be reachable WITHOUT credentials
    // (the local-session bootstrap, future cloud login callback) are
    // registered as siblings to `/api`, outside this layer.
    let api_router = api_router.route_layer(axum::middleware::from_fn_with_state(
        state.clone(),
        auth::require_auth,
    ));

    // Open endpoints: no auth required for the new auth layer. Each has
    // its own gatekeeper:
    //   /api/auth/local-session — loopback-gated
    //   /api/bridge/ws          — bridge_auth bearer token (its own
    //                             middleware further down the chain)
    //   /api/events/ws          — currently open; revisit when realtime
    //                             grows real client surface
    //
    // These are siblings of `api_router`; they bypass `require_auth`
    // entirely. The handlers (or their dedicated middleware) decide
    // what to accept.
    let api_open_router = Router::new()
        .route(
            "/auth/local-session",
            post(auth::handle_local_session),
        )
        .route("/bridge/ws", get(handle_bridge_ws))
        .route("/events/ws", get(handle_events_ws));

    Router::new()
        .route("/health", get(health))
        .nest("/internal", internal_router)
        .nest("/api", api_open_router.merge(api_router))
        .layer(cors)
        // Only GET falls through to the embedded UI — non-GET requests to
        // unmatched paths (e.g. removed `/internal/.../remember`) should
        // return 405/404 rather than silently serving index.html.
        .fallback_service(get(serve_ui))
        .with_state(state)
}

fn resolve_local_human_identity(store: &Store, data_dir: &std::path::Path) -> (String, String) {
    let config_root = data_dir;
    let configured = ChorusConfig::load(config_root)
        .ok()
        .flatten()
        .map(|cfg| cfg.local_human);
    if let Some(local_human) = configured.as_ref() {
        if let (Some(id), Some(name)) = (&local_human.id, &local_human.name) {
            if !id.trim().is_empty() && !name.trim().is_empty() {
                return (id.clone(), name.clone());
            }
        }
    }
    if let Ok(Some(human)) = store.get_humans().map(|mut humans| humans.pop()) {
        return (human.id, human.name);
    }
    let name = configured
        .and_then(|local_human| local_human.name)
        .filter(|name| !name.trim().is_empty())
        // This is only a fresh local label suggestion; it is not identity.
        .unwrap_or_else(whoami::username);
    match store.create_local_human(&name) {
        Ok(human) => (human.id, human.name),
        Err(err) => {
            tracing::error!(err = %err, "failed to create local human identity; refusing to start with an unbacked id");
            panic!("unable to resolve persisted local human identity: {err}");
        }
    }
}

/// Public re-export of `resolve_local_machine_id` so `cli/serve.rs` can
/// read the same id (from disk) it embeds into `AppState`. The function
/// is idempotent — both calls land on the same UUID.
pub fn resolve_local_machine_id_for_serve(data_dir: &std::path::Path) -> String {
    resolve_local_machine_id(data_dir)
}

/// Resolve the local installation's `machine_id`, generating and persisting
/// one to `config.toml` on first call. Every agent created on this server
/// inherits this id when the request omits `machine_id`, so the bridge
/// client knows to pick it up. Persistence makes the id stable across
/// restarts; without persistence, every restart would re-orphan every
/// local agent's `machine_id`.
fn resolve_local_machine_id(data_dir: &std::path::Path) -> String {
    let mut cfg = ChorusConfig::load(data_dir)
        .ok()
        .flatten()
        .unwrap_or_default();
    if let Some(id) = cfg.machine_id.clone() {
        return id;
    }
    let id = cfg.ensure_machine_id().to_string();
    if let Err(err) = cfg.save(data_dir) {
        tracing::warn!(err = %err, "failed to persist generated machine_id; will regenerate on next restart");
    }
    id
}
