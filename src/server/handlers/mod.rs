pub mod agents;
pub mod attachments;
pub mod channels;
pub mod knowledge;
pub mod messages;
pub mod tasks;
pub mod workspace;

pub use agents::*;
pub use attachments::*;
pub use channels::*;
pub use knowledge::*;
pub use messages::*;
pub use tasks::*;
pub use workspace::*;

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use tracing::debug;

use super::AgentLifecycle;
use crate::store::{ServerInfo, Store};

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

// ── Server Info (bridge) ──

pub async fn handle_server_info(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> ApiResult<ServerInfo> {
    debug!(agent = %agent_id, "list_server");
    let info = state
        .store
        .get_server_info(&agent_id)
        .map_err(|e| api_err(e.to_string()))?;
    Ok(Json(info))
}

// ── UI Server Info ──

pub async fn handle_ui_server_info(State(state): State<AppState>) -> ApiResult<serde_json::Value> {
    let username = whoami::username();
    let mut info = state
        .store
        .get_server_info(&username)
        .map_err(|e| api_err(e.to_string()))?;

    let activity_states = state.lifecycle.get_all_agent_activity_states();
    for agent in &mut info.agents {
        if let Some((_, activity, detail)) =
            activity_states.iter().find(|(n, _, _)| n == &agent.name)
        {
            agent.activity = Some(activity.clone());
            agent.activity_detail = Some(detail.clone());
        }
    }

    Ok(Json(serde_json::to_value(info).unwrap()))
}
