use std::collections::HashSet;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use tracing::{info, warn};

use super::{acquire_transition, app_err, format_anyhow_error, internal_err, ApiResult, AppState};
use crate::agent::activity_log::ActivityLogResponse;
use crate::agent::workspace::AgentWorkspace;
use crate::agent::AgentRuntime;
use crate::server::error::AppErrorCode;
use crate::store::agents::{AgentEnvVar, AgentStatus};
use crate::store::messages::SenderType;
use crate::store::AgentRecordUpsert;

use super::dto::AgentInfo;

// ── Activity query params ──

#[derive(Deserialize)]
pub struct ActivityParams {
    pub limit: Option<i64>,
}

#[derive(Deserialize)]
pub struct ActivityLogParams {
    pub after: Option<u64>,
}

// ── API DTOs ──

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct AgentDetailResponse {
    pub agent: AgentInfo,
    #[serde(rename = "envVars")]
    pub env_vars: Vec<AgentEnvVarPayload>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct AgentEnvVarPayload {
    pub key: String,
    pub value: String,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct CreateAgentRequest {
    pub name: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, rename = "systemPrompt")]
    pub system_prompt: Option<String>,
    #[serde(default = "default_runtime")]
    pub runtime: String,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default, rename = "reasoningEffort")]
    pub reasoning_effort: Option<String>,
    #[serde(default, rename = "envVars")]
    pub env_vars: Vec<AgentEnvVarPayload>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct UpdateAgentRequest {
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, rename = "systemPrompt")]
    pub system_prompt: Option<String>,
    #[serde(default = "default_runtime")]
    pub runtime: String,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default, rename = "reasoningEffort")]
    pub reasoning_effort: Option<String>,
    #[serde(default, rename = "envVars")]
    pub env_vars: Vec<AgentEnvVarPayload>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct RestartAgentRequest {
    pub mode: RestartMode,
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestartMode {
    Restart,
    ResetSession,
    FullReset,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct DeleteAgentRequest {
    pub mode: DeleteMode,
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeleteMode {
    PreserveWorkspace,
    DeleteWorkspace,
}

pub(super) fn default_runtime() -> String {
    AgentRuntime::Claude.as_str().to_string()
}

pub(super) fn default_model() -> String {
    "sonnet".to_string()
}

// ── Private helpers ──

pub(super) fn normalize_agent_env_vars(
    env_vars: &[AgentEnvVarPayload],
) -> Result<Vec<AgentEnvVar>, (StatusCode, Json<super::ErrorResponse>)> {
    if env_vars.len() > 100 {
        return Err(app_err!(
            StatusCode::BAD_REQUEST,
            "too many environment variables",
        ));
    }

    let mut seen = HashSet::new();
    let mut normalized = Vec::with_capacity(env_vars.len());
    for (index, env_var) in env_vars.iter().enumerate() {
        let key = env_var.key.trim().to_string();
        if key.is_empty() {
            return Err(app_err!(
                StatusCode::BAD_REQUEST,
                "environment variable key is required",
            ));
        }
        if key.len() > 8_192 || env_var.value.len() > 8_192 {
            return Err(app_err!(
                StatusCode::BAD_REQUEST,
                "environment variable key/value is too large",
            ));
        }
        if !seen.insert(key.clone()) {
            return Err(app_err!(
                StatusCode::BAD_REQUEST,
                "duplicate environment variable key: {key}"
            ));
        }
        normalized.push(AgentEnvVar {
            key,
            value: env_var.value.clone(),
            position: index as i64,
        });
    }
    Ok(normalized)
}

pub(super) fn normalize_reasoning_effort(
    runtime: &str,
    reasoning_effort: Option<&str>,
) -> Result<Option<String>, (StatusCode, Json<super::ErrorResponse>)> {
    let parsed = AgentRuntime::parse(runtime);
    if parsed != Some(AgentRuntime::Codex) && parsed != Some(AgentRuntime::Opencode) {
        return Ok(None);
    }

    let Some(reasoning_effort) = reasoning_effort.map(str::trim) else {
        return Ok(None);
    };
    if reasoning_effort.is_empty() || reasoning_effort == "default" {
        return Ok(None);
    }

    match reasoning_effort {
        "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max" => {
            Ok(Some(reasoning_effort.to_string()))
        }
        _ => Err(app_err!(
            StatusCode::BAD_REQUEST,
            "unsupported reasoning effort: {reasoning_effort}"
        )),
    }
}

// ── Public handlers ──

pub async fn handle_list_agents(State(state): State<AppState>) -> ApiResult<Vec<AgentInfo>> {
    let mut agents: Vec<AgentInfo> = state
        .store
        .get_agents()
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?
        .iter()
        .map(AgentInfo::from)
        .collect();
    let activity_states = state.lifecycle.get_all_agent_activity_states();
    for agent in &mut agents {
        if let Some((_, activity, detail)) = activity_states
            .iter()
            .find(|(name, _, _)| name == &agent.name)
        {
            agent.activity = Some(activity.clone());
            agent.activity_detail = Some(detail.clone());
        }
    }
    Ok(Json(agents))
}

pub async fn handle_create_agent(
    State(state): State<AppState>,
    Json(req): Json<CreateAgentRequest>,
) -> ApiResult<serde_json::Value> {
    let name = req.name.trim().to_string();
    if name.is_empty() {
        return Err(app_err!(StatusCode::BAD_REQUEST, "name is required"));
    }
    let display_name = if req.display_name.is_empty() {
        name.clone()
    } else {
        req.display_name
    };
    let description = if req.description.is_empty() {
        None
    } else {
        Some(req.description.as_str())
    };
    let reasoning_effort =
        normalize_reasoning_effort(&req.runtime, req.reasoning_effort.as_deref())?;
    let env_vars = normalize_agent_env_vars(&req.env_vars)?;
    state
        .store
        .create_agent_record_with_reasoning(&AgentRecordUpsert {
            name: &name,
            display_name: &display_name,
            description,
            system_prompt: req.system_prompt.as_deref(),
            runtime: &req.runtime,
            model: &req.model,
            reasoning_effort: reasoning_effort.as_deref(),
            env_vars: &env_vars,
        })
        .map_err(|e| {
            let msg = e.to_string();
            if msg.contains("UNIQUE constraint") {
                app_err!(AppErrorCode::AgentNameTaken, "agent name already in use")
            } else {
                app_err!(StatusCode::BAD_REQUEST, msg)
            }
        })?;
    for channel in state.store.get_auto_join_channels().map_err(internal_err)? {
        let _ = state
            .store
            .join_channel(&channel.name, &name, SenderType::Agent);
    }
    let mut created_status = AgentStatus::Active;
    let mut start_warning = None;
    if let Err(err) = state.lifecycle.start_agent(&name, None).await {
        let error_detail = format_anyhow_error(&err);
        warn!(agent = %name, error = %error_detail, "agent created but failed to start");
        created_status = AgentStatus::Inactive;
        start_warning = Some(format!("failed to start agent: {err}"));
    }
    Ok(Json(serde_json::json!({
        "name": name,
        "status": created_status.as_str(),
        "warning": start_warning,
    })))
}

pub async fn handle_get_agent(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<AgentDetailResponse> {
    let agent = state
        .store
        .get_agent(&name)
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?
        .ok_or_else(|| app_err!(StatusCode::BAD_REQUEST, "agent not found"))?;
    Ok(Json(AgentDetailResponse {
        agent: AgentInfo::from(&agent),
        env_vars: agent
            .env_vars
            .iter()
            .map(|env_var| AgentEnvVarPayload {
                key: env_var.key.clone(),
                value: env_var.value.clone(),
            })
            .collect(),
    }))
}

pub async fn handle_update_agent(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(req): Json<UpdateAgentRequest>,
) -> ApiResult<serde_json::Value> {
    let _transition = acquire_transition(&state, &name)?;
    let existing = state
        .store
        .get_agent(&name)
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?
        .ok_or_else(|| app_err!(StatusCode::BAD_REQUEST, "agent not found"))?;

    let env_vars = normalize_agent_env_vars(&req.env_vars)?;
    let display_name = if req.display_name.trim().is_empty() {
        existing.name.clone()
    } else {
        req.display_name.trim().to_string()
    };
    let description = if req.description.trim().is_empty() {
        None
    } else {
        Some(req.description.trim())
    };
    let reasoning_effort =
        normalize_reasoning_effort(&req.runtime, req.reasoning_effort.as_deref())?;

    let requires_restart = existing.runtime != req.runtime
        || existing.model != req.model
        || existing.reasoning_effort != reasoning_effort
        || existing.env_vars != env_vars;

    state
        .store
        .update_agent_record_with_reasoning(&AgentRecordUpsert {
            name: &name,
            display_name: &display_name,
            description,
            system_prompt: req.system_prompt.as_deref(),
            runtime: &req.runtime,
            model: &req.model,
            reasoning_effort: reasoning_effort.as_deref(),
            env_vars: &env_vars,
        })
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?;

    if existing.status == AgentStatus::Active && requires_restart {
        state
            .lifecycle
            .stop_agent(&name)
            .await
            .map_err(internal_err)?;
        if let Err(err) = state.lifecycle.start_agent(&name, None).await {
            let _ = state
                .store
                .update_agent_status(&name, AgentStatus::Inactive);
            return Err(app_err!(
                StatusCode::INTERNAL_SERVER_ERROR,
                "agent updated but restart failed: {err}"
            ));
        }
    }
    Ok(Json(serde_json::json!({
        "ok": true,
        "restarted": existing.status == AgentStatus::Active && requires_restart
    })))
}

pub async fn handle_restart_agent(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(req): Json<RestartAgentRequest>,
) -> ApiResult<serde_json::Value> {
    let _transition = acquire_transition(&state, &name)?;
    let agent = state
        .store
        .get_agent(&name)
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?
        .ok_or_else(|| app_err!(StatusCode::BAD_REQUEST, "agent not found"))?;
    let agents_dir = state.store.agents_dir();
    let workspace = AgentWorkspace::new(&agents_dir);

    state
        .lifecycle
        .stop_agent(&name)
        .await
        .map_err(internal_err)?;

    match req.mode {
        RestartMode::Restart => {}
        RestartMode::ResetSession => {
            state
                .store
                .update_agent_session(&name, None)
                .map_err(internal_err)?;
        }
        RestartMode::FullReset => {
            state
                .store
                .update_agent_session(&name, None)
                .map_err(internal_err)?;
            workspace.delete_if_exists(&name).map_err(|e| {
                app_err!(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to delete workspace: {e}"
                )
            })?;
        }
    }

    if let Err(err) = state.lifecycle.start_agent(&name, None).await {
        let _ = state
            .store
            .update_agent_status(&name, AgentStatus::Inactive);
        return Err(app_err!(
            AppErrorCode::AgentRestartFailed,
            "restart failed: {err}"
        ));
    }
    Ok(Json(serde_json::json!({
        "ok": true,
        "mode": req.mode,
        "agent": agent.name
    })))
}

pub async fn handle_delete_agent(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(req): Json<DeleteAgentRequest>,
) -> ApiResult<serde_json::Value> {
    let _transition = acquire_transition(&state, &name)?;
    state
        .store
        .get_agent(&name)
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?
        .ok_or_else(|| app_err!(StatusCode::BAD_REQUEST, "agent not found"))?;

    state
        .lifecycle
        .stop_agent(&name)
        .await
        .map_err(internal_err)?;

    state
        .store
        .mark_agent_messages_deleted(&name)
        .map_err(internal_err)?;
    state
        .store
        .delete_agent_record(&name)
        .map_err(internal_err)?;

    if matches!(req.mode, DeleteMode::DeleteWorkspace) {
        let agents_dir = state.store.agents_dir();
        let workspace = AgentWorkspace::new(&agents_dir);
        if let Err(e) = workspace.delete_if_exists(&name) {
            return Ok(Json(serde_json::json!({
                "ok": true,
                "warning": format!("agent deleted but workspace cleanup failed: {e}"),
                "code": "AGENT_DELETE_WORKSPACE_CLEANUP_FAILED"
            })));
        }
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn handle_agent_start(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<serde_json::Value> {
    let _transition = acquire_transition(&state, &name)?;
    info!(agent = %name, "starting agent");
    state
        .lifecycle
        .start_agent(&name, None)
        .await
        .map_err(internal_err)?;
    info!(agent = %name, "agent started");
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn handle_agent_stop(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<serde_json::Value> {
    let _transition = acquire_transition(&state, &name)?;
    info!(agent = %name, "stopping agent");
    state
        .lifecycle
        .stop_agent(&name)
        .await
        .map_err(internal_err)?;
    info!(agent = %name, "agent stopped");
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn handle_agent_activity(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(params): Query<ActivityParams>,
) -> ApiResult<serde_json::Value> {
    let limit = params.limit.unwrap_or(50).min(200);
    let messages = state
        .store
        .get_agent_activity(&name, limit)
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?;
    Ok(Json(serde_json::json!({ "messages": messages })))
}

pub async fn handle_agent_activity_log(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(params): Query<ActivityLogParams>,
) -> ApiResult<ActivityLogResponse> {
    let resp = state.lifecycle.get_activity_log_data(&name, params.after);
    Ok(Json(resp))
}
