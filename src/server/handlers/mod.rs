pub mod agents;
pub mod attachments;
pub mod channels;
pub mod dto;
pub mod messages;
pub mod server_info;
pub mod tasks;
pub mod teams;
pub mod workspace;

pub use agents::*;
pub use attachments::*;
pub use channels::*;
pub use messages::*;
pub use tasks::*;
pub use teams::*;
pub use workspace::*;

use std::backtrace::{Backtrace, BacktraceStatus};
use std::collections::HashSet;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use tracing::debug;

use crate::agent::runtime_status::SharedRuntimeStatusProvider;
use crate::agent::AgentLifecycle;
use crate::store::Store;
use dto::ServerInfo;

// ── Core types ──

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct ErrorResponse {
    pub error: String,
}

pub type ApiResult<T> = Result<Json<T>, (StatusCode, Json<ErrorResponse>)>;

/// Shared application state injected into every handler via Axum's `State` extractor.
#[derive(Clone)]
pub struct AppState {
    pub store: Arc<Store>,
    pub lifecycle: Arc<dyn AgentLifecycle>,
    pub runtime_status_provider: SharedRuntimeStatusProvider,
    pub transitioning_agents: Arc<Mutex<HashSet<String>>>,
}

pub(super) fn api_err(msg: impl Into<String>) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::BAD_REQUEST,
        Json(ErrorResponse { error: msg.into() }),
    )
}

pub(super) fn internal_err(msg: impl Into<String>) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ErrorResponse { error: msg.into() }),
    )
}

pub(super) fn conflict_err(msg: impl Into<String>) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::CONFLICT,
        Json(ErrorResponse { error: msg.into() }),
    )
}

pub(super) fn format_anyhow_error(err: &anyhow::Error) -> String {
    let mut rendered = String::new();
    let _ = write!(&mut rendered, "{}", err);

    let mut chain = err.chain();
    let _ = chain.next();
    let causes: Vec<String> = chain.map(ToString::to_string).collect();
    if !causes.is_empty() {
        rendered.push_str("\ncaused by:");
        for (index, cause) in causes.iter().enumerate() {
            let _ = write!(&mut rendered, "\n  {}. {}", index + 1, cause);
        }
    }

    let backtrace = err.backtrace();
    if matches!(backtrace.status(), BacktraceStatus::Captured) {
        let _ = write!(&mut rendered, "\nbacktrace:\n{backtrace}");
    } else {
        let forced_backtrace = Backtrace::force_capture();
        let _ = write!(&mut rendered, "\nbacktrace:\n{forced_backtrace}");
    }

    rendered
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
    let mut transitioning = state
        .transitioning_agents
        .lock()
        .map_err(|_| internal_err("failed to lock transition state"))?;
    if !transitioning.insert(agent_name.to_string()) {
        return Err(conflict_err(
            "agent lifecycle operation already in progress; retry when the current action completes",
        ));
    }
    Ok(TransitionGuard {
        agent_name: agent_name.to_string(),
        transitioning_agents: state.transitioning_agents.clone(),
    })
}

// ── Whoami ──

pub async fn handle_whoami() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "username": whoami::username() }))
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
    let info = server_info::build_server_info(state.store.as_ref(), &agent_id)
        .map_err(|e| api_err(e.to_string()))?;
    Ok(Json(info))
}

// ── UI Server Info ──

pub async fn handle_ui_server_info(State(state): State<AppState>) -> ApiResult<serde_json::Value> {
    let info = server_info::build_ui_shell_info(state.store.as_ref())
        .map_err(|e| api_err(e.to_string()))?;
    Ok(Json(serde_json::to_value(info).unwrap()))
}

pub async fn handle_list_humans(State(state): State<AppState>) -> ApiResult<Vec<dto::HumanInfo>> {
    let humans = state
        .store
        .get_humans()
        .map_err(|e| api_err(e.to_string()))?
        .into_iter()
        .map(dto::HumanInfo::from)
        .collect();
    Ok(Json(humans))
}

pub async fn handle_list_runtime_statuses(
    State(state): State<AppState>,
) -> ApiResult<Vec<dto::RuntimeStatusInfo>> {
    let statuses = state
        .runtime_status_provider
        .list_statuses()
        .map_err(|e| internal_err(e.to_string()))?
        .into_iter()
        .map(|status| dto::RuntimeStatusInfo {
            runtime: status.runtime,
            installed: status.installed,
            auth_status: status.auth_status,
        })
        .collect();
    Ok(Json(statuses))
}

pub async fn handle_list_runtime_models(
    State(state): State<AppState>,
    Path(runtime): Path<String>,
) -> ApiResult<Vec<String>> {
    let models = state
        .runtime_status_provider
        .list_models(&runtime)
        .map_err(|e| api_err(e.to_string()))?;
    Ok(Json(models))
}

#[cfg(test)]
mod tests {
    use anyhow::anyhow;

    use super::format_anyhow_error;

    #[test]
    fn formats_anyhow_error_with_cause_chain() {
        let err = anyhow!("No such file or directory (os error 2)")
            .context("failed to spawn codex runtime")
            .context("failed to start agent");

        let rendered = format_anyhow_error(&err);

        assert!(rendered.contains("failed to start agent"));
        assert!(rendered.contains("caused by:"));
        assert!(rendered.contains("failed to spawn codex runtime"));
        assert!(rendered.contains("No such file or directory (os error 2)"));
        assert!(rendered.contains("backtrace:"));
    }
}
