use std::collections::HashSet;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use tracing::{info, warn};

use super::path_params::{
    resolve_public_agent, resolve_public_agent_with_env, PublicResourceIdPath,
};
use super::{acquire_transition, app_err, ApiResult, AppState};
use crate::agent::activity_log::ActivityLogResponse;
use crate::agent::workspace::AgentWorkspace;
use crate::agent::AgentRuntime;
use crate::store::agents::AgentEnvVar;
use crate::store::messages::SenderType;
use crate::store::AgentRecordUpsert;
use crate::utils::error::internal_err;
use crate::utils::error::{format_anyhow_error, AppErrorCode};
use crate::utils::slug::{random_slug_suffix, slugify_base, MAX_SLUG_ATTEMPTS};

use super::dto::AgentInfo;
use crate::agent::runtime_catalog::{supports_reasoning_effort, supports_reasoning_effort_value};

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
    /// Optional slug hint. When empty or omitted, the server derives
    /// the base slug from `display_name` instead.
    #[serde(default)]
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
    let Some(parsed) = AgentRuntime::parse(runtime) else {
        return Ok(None);
    };
    if !supports_reasoning_effort(parsed) {
        return Ok(None);
    }

    let Some(reasoning_effort) = reasoning_effort.map(str::trim) else {
        return Ok(None);
    };
    if reasoning_effort.is_empty() || reasoning_effort == "default" {
        return Ok(None);
    }

    if supports_reasoning_effort_value(parsed, reasoning_effort) {
        Ok(Some(reasoning_effort.to_string()))
    } else {
        Err(app_err!(
            StatusCode::BAD_REQUEST,
            "unsupported reasoning effort: {reasoning_effort}"
        ))
    }
}

// ── Public handlers ──

pub async fn handle_list_agents(State(state): State<AppState>) -> ApiResult<Vec<AgentInfo>> {
    let active_workspace_id = state.active_workspace_id().await;
    let mut agents: Vec<AgentInfo> = state
        .store
        .get_agents_for_workspace(active_workspace_id.as_deref())
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?
        .iter()
        .map(AgentInfo::from)
        .collect();

    for info in &mut agents {
        let ps = state.lifecycle.process_state(&info.name).await;
        info.status = crate::agent::process_status::derive_status(ps.as_ref());
    }

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
    let name_hint = req.name.trim();
    let display_name_trimmed = req.display_name.trim();
    if name_hint.is_empty() && display_name_trimmed.is_empty() {
        return Err(app_err!(StatusCode::BAD_REQUEST, "name is required"));
    }
    if name_hint.chars().count() > 200 || display_name_trimmed.chars().count() > 200 {
        return Err(app_err!(
            StatusCode::BAD_REQUEST,
            "name/display_name exceeds 200-character limit"
        ));
    }
    // Slug derivation lives on the server so every caller (new UI, old
    // UI, curl users) lands on the same handle shape. Prefer an
    // explicit name hint, fall back to the display name, and finally
    // to `agent` when the input has no ASCII alphanumerics to slug on.
    let base_name = slugify_base(name_hint)
        .or_else(|| slugify_base(display_name_trimmed))
        .unwrap_or_else(|| "agent".to_string());
    let display_name = if display_name_trimmed.is_empty() {
        base_name.clone()
    } else {
        display_name_trimmed.to_string()
    };
    let description = if req.description.is_empty() {
        None
    } else {
        Some(req.description.as_str())
    };
    let reasoning_effort =
        normalize_reasoning_effort(&req.runtime, req.reasoning_effort.as_deref())?;
    let env_vars = normalize_agent_env_vars(&req.env_vars)?;

    // Create the agent record, join auto-join channels, and start it.
    let result = create_and_start_agent(
        &state,
        &CreateAgentParams {
            base_name: &base_name,
            display_name: &display_name,
            description,
            system_prompt: req.system_prompt.as_deref(),
            runtime: &req.runtime,
            model: &req.model,
            reasoning_effort: reasoning_effort.as_deref(),
            env_vars: &env_vars,
        },
    )
    .await
    .map_err(|e| app_err!(StatusCode::INTERNAL_SERVER_ERROR, "{e}"))?;
    if let Some(ref err) = result.start_error {
        return Err(app_err!(
            AppErrorCode::AgentStartFailed,
            "agent @{} created but failed to start: {err}",
            result.name
        ));
    }
    let ps = state.lifecycle.process_state(&result.name).await;
    let status = crate::agent::process_status::derive_status(ps.as_ref());
    Ok(Json(serde_json::json!({
        "id": result.id,
        "name": result.name,
        "status": status,
    })))
}

/// Parameters for creating a new agent with auto-slug.
pub(crate) struct CreateAgentParams<'a> {
    pub base_name: &'a str,
    pub display_name: &'a str,
    pub description: Option<&'a str>,
    pub system_prompt: Option<&'a str>,
    pub runtime: &'a str,
    pub model: &'a str,
    pub reasoning_effort: Option<&'a str>,
    pub env_vars: &'a [AgentEnvVar],
}

/// Result of creating and starting an agent.
pub(crate) struct CreateAgentResult {
    pub name: String,
    pub id: String,
    /// `Some` when the agent was created but failed to start.
    pub start_error: Option<String>,
}

/// Creates an agent record with a `{base}-{hex4}` slug, joins it to all
/// auto-join channels, and starts it.
pub(crate) async fn create_and_start_agent(
    state: &AppState,
    params: &CreateAgentParams<'_>,
) -> anyhow::Result<CreateAgentResult> {
    let active_workspace_id = state.active_workspace_id().await;
    let mut last_error: Option<String> = None;
    let mut slug_result: Option<(String, String)> = None;
    for _ in 0..MAX_SLUG_ATTEMPTS {
        let candidate = format!("{}-{}", params.base_name, random_slug_suffix());
        let record = AgentRecordUpsert {
            name: &candidate,
            display_name: params.display_name,
            description: params.description,
            system_prompt: params.system_prompt,
            runtime: params.runtime,
            model: params.model,
            reasoning_effort: params.reasoning_effort,
            env_vars: params.env_vars,
        };
        let create_result = match active_workspace_id.as_deref() {
            Some(workspace_id) => state
                .store
                .create_agent_record_in_workspace_with_events(workspace_id, &record),
            None => state.store.create_agent_record_with_events(&record),
        };
        match create_result {
            Ok((id, events)) => {
                for event in events {
                    state.event_bus.publish_stream(event);
                }
                slug_result = Some((candidate, id));
                break;
            }
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("UNIQUE constraint") {
                    last_error = Some(msg);
                    continue;
                }
                return Err(e);
            }
        }
    }
    let (name, id) = slug_result.ok_or_else(|| {
        anyhow::anyhow!(
            "failed to allocate a unique slug after {MAX_SLUG_ATTEMPTS} attempts: {}",
            last_error.unwrap_or_else(|| "unknown".to_string())
        )
    })?;
    // Track the system-channel join: the intro directive only makes sense
    // if the agent actually became a member of #all. Other auto-join
    // failures are logged but don't block — historically this loop
    // swallowed all errors, and we don't want to gate creation on a
    // non-system channel.
    let mut joined_system_channel = false;
    for channel in state
        .store
        .get_auto_join_channels_for_workspace(active_workspace_id.as_deref())?
    {
        let is_system = channel.name == crate::store::Store::DEFAULT_SYSTEM_CHANNEL;
        match state
            .store
            .join_channel_by_id(&channel.id, &id, SenderType::Agent)
        {
            Ok((_, events)) => {
                for event in events {
                    state.event_bus.publish_stream(event);
                }
                if is_system {
                    joined_system_channel = true;
                }
            }
            Err(err) => warn!(
                agent = %name,
                channel = %channel.name,
                error = %format_anyhow_error(&err),
                "auto-join failed",
            ),
        }
    }
    // Brand-new agent — ask it to introduce itself in the system channel.
    // The directive is delivered as the first prompt; the agent's model
    // picks the right messaging tool from its system prompt and writes
    // the intro itself. Only issued when the system-channel join landed —
    // otherwise we'd send the agent on an impossible errand.
    let intro_directive = joined_system_channel.then(|| {
        format!(
            "You have just been added to #{ch}. Post a brief one-or-two-sentence introduction of yourself in #{ch}, then stop.",
            ch = crate::store::Store::DEFAULT_SYSTEM_CHANNEL,
        )
    });
    let start_error = if let Err(err) = state
        .lifecycle
        .start_agent(&name, None, intro_directive)
        .await
    {
        let error_detail = format_anyhow_error(&err);
        warn!(agent = %name, error = %error_detail, "agent created but failed to start");
        Some(format!("{err}"))
    } else {
        None
    };
    Ok(CreateAgentResult {
        name,
        id,
        start_error,
    })
}

pub async fn handle_get_agent(
    State(state): State<AppState>,
    Path(PublicResourceIdPath { id }): Path<PublicResourceIdPath>,
) -> ApiResult<AgentDetailResponse> {
    let agent = resolve_public_agent_with_env(&state, &id)?;
    let mut agent_info = AgentInfo::from(&agent);
    let ps = state.lifecycle.process_state(&agent_info.name).await;
    agent_info.status = crate::agent::process_status::derive_status(ps.as_ref());
    Ok(Json(AgentDetailResponse {
        agent: agent_info,
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
    Path(PublicResourceIdPath { id }): Path<PublicResourceIdPath>,
    Json(req): Json<UpdateAgentRequest>,
) -> ApiResult<serde_json::Value> {
    let existing = resolve_public_agent(&state, &id)?;
    let name = existing.name.clone();
    let _transition = acquire_transition(&state, &name)?;

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
        .update_agent_record(&AgentRecordUpsert {
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

    let ps = state.lifecycle.process_state(&name).await;
    let was_running = crate::agent::process_status::derive_status(ps.as_ref())
        != crate::agent::process_status::Status::Asleep;

    if was_running && requires_restart {
        state
            .lifecycle
            .stop_agent(&name)
            .await
            .map_err(internal_err)?;
        if let Err(err) = state.lifecycle.start_agent(&name, None, None).await {
            return Err(app_err!(
                StatusCode::INTERNAL_SERVER_ERROR,
                "agent updated but restart failed: {err}"
            ));
        }
    }
    Ok(Json(serde_json::json!({
        "ok": true,
        "restarted": was_running && requires_restart
    })))
}

pub async fn handle_restart_agent(
    State(state): State<AppState>,
    Path(PublicResourceIdPath { id }): Path<PublicResourceIdPath>,
    Json(req): Json<RestartAgentRequest>,
) -> ApiResult<serde_json::Value> {
    let agent = resolve_public_agent(&state, &id)?;
    let name = agent.name.clone();
    let _transition = acquire_transition(&state, &name)?;
    let agents_dir = state.agents_dir.clone();
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
                .clear_active_session(&agent.id)
                .map_err(internal_err)?;
        }
        RestartMode::FullReset => {
            state
                .store
                .clear_active_session(&agent.id)
                .map_err(internal_err)?;
            workspace.delete_if_exists(&name).map_err(|e| {
                app_err!(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to delete workspace: {e}"
                )
            })?;
        }
    }

    if let Err(err) = state.lifecycle.start_agent(&name, None, None).await {
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
    Path(PublicResourceIdPath { id }): Path<PublicResourceIdPath>,
    Json(req): Json<DeleteAgentRequest>,
) -> ApiResult<serde_json::Value> {
    let agent = resolve_public_agent(&state, &id)?;
    let name = agent.name;
    let _transition = acquire_transition(&state, &name)?;

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
        let agents_dir = state.agents_dir.clone();
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
    Path(PublicResourceIdPath { id }): Path<PublicResourceIdPath>,
) -> ApiResult<serde_json::Value> {
    let agent = resolve_public_agent(&state, &id)?;
    let name = agent.name;
    let _transition = acquire_transition(&state, &name)?;
    info!(agent = %name, "starting agent");
    state
        .lifecycle
        .start_agent(&name, None, None)
        .await
        .map_err(internal_err)?;
    info!(agent = %name, "agent started");
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn handle_agent_stop(
    State(state): State<AppState>,
    Path(PublicResourceIdPath { id }): Path<PublicResourceIdPath>,
) -> ApiResult<serde_json::Value> {
    let agent = resolve_public_agent(&state, &id)?;
    let name = agent.name;
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
    Path(PublicResourceIdPath { id }): Path<PublicResourceIdPath>,
    Query(params): Query<ActivityParams>,
) -> ApiResult<serde_json::Value> {
    let agent = resolve_public_agent(&state, &id)?;
    let limit = params.limit.unwrap_or(50).min(200);
    let messages = state
        .store
        .get_agent_activity(&agent.name, limit)
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?;
    Ok(Json(serde_json::json!({ "messages": messages })))
}

pub async fn handle_agent_activity_log(
    State(state): State<AppState>,
    Path(PublicResourceIdPath { id }): Path<PublicResourceIdPath>,
    Query(params): Query<ActivityLogParams>,
) -> ApiResult<ActivityLogResponse> {
    let agent = resolve_public_agent(&state, &id)?;
    let resp = state
        .lifecycle
        .get_activity_log_data(&agent.name, params.after);
    Ok(Json(resp))
}
