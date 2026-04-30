pub mod agent_workspace;
pub mod agents;
pub mod attachments;
pub mod channels;
pub mod decisions;
pub mod dto;
pub mod messages;
pub mod path_params;
pub mod server_info;
pub mod tasks;
pub mod teams;
pub mod templates;
pub mod workspaces;

pub use agent_workspace::*;
pub use agents::*;
pub use attachments::*;
pub use channels::*;
pub use decisions::*;
pub use messages::*;
pub use tasks::*;
pub use teams::*;
pub use templates::*;
pub use workspaces::*;

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use tokio::sync::RwLock;
use tracing::debug;

use crate::agent::runtime_status::SharedRuntimeStatusProvider;
use crate::agent::templates::AgentTemplate;
use crate::agent::AgentLifecycle;
use crate::agent::AgentRuntime;
use crate::config::ChorusConfig;
use crate::server::error::{app_err, internal_err, ApiResult, ErrorResponse};
use crate::store::Store;
use dto::ServerInfo;

/// Shared application state injected into every handler via Axum's `State` extractor.
#[derive(Clone)]
pub struct AppState {
    pub store: Arc<Store>,
    pub active_workspace_id: Arc<RwLock<Option<String>>>,
    pub local_human_id: String,
    pub local_human_name: String,
    pub lifecycle: Arc<dyn AgentLifecycle>,
    pub runtime_status_provider: SharedRuntimeStatusProvider,
    pub transitioning_agents: Arc<Mutex<HashSet<String>>>,
    pub templates: Arc<Vec<AgentTemplate>>,
}

impl AppState {
    pub async fn active_workspace_id(&self) -> Option<String> {
        if let Some(workspace_id) = self.active_workspace_id.read().await.clone() {
            return Some(workspace_id);
        }
        if let Ok(Some(workspace)) = self.store.get_active_workspace() {
            self.set_active_workspace_id(Some(workspace.id.clone()))
                .await;
            return Some(workspace.id);
        }
        None
    }

    pub async fn set_active_workspace_id(&self, workspace_id: Option<String>) {
        let mut guard = self.active_workspace_id.write().await;
        *guard = workspace_id;
    }
}

pub(super) struct TransitionGuard {
    agent_name: String,
    transitioning_agents: Arc<Mutex<HashSet<String>>>,
}

impl Drop for TransitionGuard {
    fn drop(&mut self) {
        if let Ok(mut transitioning) = self.transitioning_agents.lock() {
            transitioning.remove(&self.agent_name);
        }
    }
}

pub(super) fn acquire_transition(
    state: &AppState,
    agent_name: &str,
) -> Result<TransitionGuard, (StatusCode, Json<ErrorResponse>)> {
    let mut transitioning = state.transitioning_agents.lock().map_err(|_| {
        app_err!(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to lock transition state",
        )
    })?;
    if !transitioning.insert(agent_name.to_string()) {
        return Err(app_err!(
            StatusCode::CONFLICT,
            "agent lifecycle operation already in progress; retry when the current action completes",
        ));
    }
    Ok(TransitionGuard {
        agent_name: agent_name.to_string(),
        transitioning_agents: state.transitioning_agents.clone(),
    })
}

// ── Whoami ──

pub async fn handle_whoami(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "id": state.local_human_id,
        "name": state.local_human_name,
    }))
}

// ── Agent-Scoped Workspace Snapshot (bridge/CLI compatibility) ──

/// Return the full workspace snapshot as seen by one specific agent process.
///
/// This remains on `/internal/agent/{agent_id}/server` because bridge tools and
/// CLI commands still need an agent-scoped discovery payload. The public
/// `/api/server-info` route is intentionally a smaller shell bootstrap for the
/// human UI and is not a drop-in replacement.
pub async fn handle_server_info(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> ApiResult<ServerInfo> {
    debug!(agent = %agent_id, "list_server");
    let active_workspace_id = state.active_workspace_id().await;
    let mut info = server_info::build_server_info_for_workspace(
        state.store.as_ref(),
        &agent_id,
        active_workspace_id.as_deref(),
    )
    .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?;
    for agent_info in &mut info.agents {
        let ps = state.lifecycle.process_state(&agent_info.name).await;
        agent_info.status = crate::agent::process_status::derive_status(ps.as_ref());
    }
    Ok(Json(info))
}

// ── UI Server Info ──

pub async fn handle_ui_server_info(State(state): State<AppState>) -> ApiResult<serde_json::Value> {
    let active_workspace_id = state.active_workspace_id().await;
    let info = server_info::build_ui_shell_info_for_workspace(
        state.store.as_ref(),
        active_workspace_id.as_deref(),
    )
    .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?;
    Ok(Json(serde_json::to_value(info).unwrap()))
}

pub async fn handle_system_info(State(state): State<AppState>) -> ApiResult<dto::SystemInfo> {
    let data_dir = state
        .store
        .data_dir()
        .parent()
        .unwrap_or_else(|| state.store.data_dir())
        .to_string_lossy()
        .into_owned();
    let data_dir_path = state
        .store
        .data_dir()
        .parent()
        .unwrap_or_else(|| state.store.data_dir())
        .to_path_buf();
    let db_size_bytes = std::fs::metadata(state.store.db_path())
        .map(|m| m.len())
        .ok();

    let config = ChorusConfig::load(&data_dir_path)
        .ok()
        .flatten()
        .map(|cfg| {
            let mut runtimes = Vec::new();
            let runtime_entries = [
                ("claude", &cfg.claude),
                ("codex", &cfg.codex),
                ("kimi", &cfg.kimi),
                ("opencode", &cfg.opencode),
                ("gemini", &cfg.gemini),
            ];
            for (name, rt) in runtime_entries {
                if rt.binary_path.is_some() || rt.acp_adaptor.is_some() {
                    runtimes.push(dto::RuntimeInfo {
                        name: name.to_string(),
                        binary_path: rt.binary_path.clone(),
                        acp_adaptor: rt.acp_adaptor.clone(),
                    });
                }
            }
            dto::ConfigInfo {
                machine_id: cfg.machine_id,
                local_human: cfg
                    .local_human
                    .id
                    .zip(cfg.local_human.name)
                    .filter(|(id, name)| !id.trim().is_empty() && !name.trim().is_empty())
                    .map(|(id, name)| dto::LocalHumanInfo { id, name }),
                agent_template: dto::AgentTemplateInfo {
                    dir: cfg.agent_template.dir,
                    default: cfg.agent_template.default,
                },
                logs: dto::LogsInfo {
                    level: cfg.logs.level,
                    rotation: cfg.logs.rotation,
                    retention: cfg.logs.retention,
                },
                runtimes,
            }
        });

    Ok(Json(dto::SystemInfo {
        data_dir,
        db_size_bytes,
        config,
    }))
}

#[derive(Debug, Deserialize)]
pub struct LogsParams {
    /// Number of lines to return from the end of the log. Default 200, max 2000.
    pub tail: Option<usize>,
}

pub async fn handle_logs(
    State(state): State<AppState>,
    axum::extract::Query(params): axum::extract::Query<LogsParams>,
) -> ApiResult<serde_json::Value> {
    let tail = params.tail.unwrap_or(200).min(2000);
    let logs_dir = state
        .store
        .data_dir()
        .parent()
        .unwrap_or_else(|| state.store.data_dir())
        .join("logs");
    let log_path = logs_dir.join("chorus.log");
    let lines = match tokio::fs::read_to_string(&log_path).await {
        Ok(content) => {
            let all: Vec<&str> = content.lines().collect();
            let start = all.len().saturating_sub(tail);
            all[start..]
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
        }
        Err(_) => vec![],
    };
    Ok(Json(serde_json::json!({ "lines": lines })))
}

pub async fn handle_list_humans(State(state): State<AppState>) -> ApiResult<Vec<dto::HumanInfo>> {
    let humans = state
        .store
        .get_humans()
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?
        .into_iter()
        .map(dto::HumanInfo::from)
        .collect();
    Ok(Json(humans))
}

pub async fn handle_update_human(Path(_id): Path<String>) -> ApiResult<dto::HumanInfo> {
    Err(app_err!(
        StatusCode::NOT_FOUND,
        "human profile updates are not supported"
    ))
}

pub async fn handle_list_runtime_statuses(
    State(state): State<AppState>,
) -> ApiResult<Vec<dto::RuntimeCatalogEntry>> {
    let statuses = state
        .runtime_status_provider
        .list_statuses()
        .await
        .map_err(internal_err)?;
    Ok(Json(statuses))
}

pub async fn handle_list_runtime_models(
    State(state): State<AppState>,
    Path(runtime): Path<String>,
) -> ApiResult<Vec<String>> {
    let rt = AgentRuntime::parse(&runtime)
        .ok_or_else(|| app_err!(StatusCode::BAD_REQUEST, "unknown runtime: {runtime}"))?;
    let models = state
        .runtime_status_provider
        .list_models(rt)
        .await
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?;
    Ok(Json(models))
}
