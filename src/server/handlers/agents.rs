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
use crate::server::transport::bridge_ws::broadcast_target_update;
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
    /// Runtime owner. `Some(machine_id)` binds the agent to one named
    /// bridge; omitted (or null) = platform-local (runs in `chorus
    /// serve`'s own AgentManager). Every agent has exactly one owner.
    #[serde(default, rename = "machineId")]
    pub machine_id: Option<String>,
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
    #[serde(default, rename = "machineId")]
    pub machine_id: Option<String>,
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
        let ps = state.lifecycle.process_state(&info.id).await;
        info.status = crate::agent::process_status::derive_status(ps.as_ref());
    }

    let activity_states = state.lifecycle.get_all_agent_activity_states();
    for agent in &mut agents {
        if let Some((_, activity, detail)) =
            activity_states.iter().find(|(id, _, _)| id == &agent.id)
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
    // Default an omitted/blank machine_id to the local installation's id
    // so every row has a non-NULL owner. Phase 2's in-process bridge
    // client picks these up by matching on `local_machine_id`.
    let machine_id_resolved: String = req
        .machine_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| state.local_machine_id.clone());
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
            machine_id: Some(&machine_id_resolved),
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
    let ps = state.lifecycle.process_state(&result.id).await;
    let status = crate::agent::process_status::derive_status(ps.as_ref());
    broadcast_target_update(state.store.as_ref(), state.bridge_registry.as_ref());
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
    pub machine_id: Option<&'a str>,
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
            machine_id: params.machine_id,
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
    // Bridge-hosted = `machine_id` differs from the local installation's id.
    // Those agents are started by a remote bridge after the platform pushes
    // a `bridge.target` update; the platform must NOT spawn them locally,
    // or both runtimes contend on the same ACP session.
    let bridge_hosted = params.machine_id != Some(state.local_machine_id.as_str());
    let start_error = if bridge_hosted {
        info!(
            agent = %name,
            machine_id = %params.machine_id.unwrap_or(""),
            "create_and_start_agent: bridge-hosted, skipping platform-side start"
        );
        None
    } else {
        // Reload with env_vars hydrated — the runtime spec inside `start_agent`
        // reads them off the record we hand in.
        let agent = state
            .store
            .get_agent_by_id(&id, true)?
            .ok_or_else(|| anyhow::anyhow!("agent vanished after create: {id}"))?;
        if let Err(err) = state
            .lifecycle
            .start_agent(&agent, None, intro_directive)
            .await
        {
            let error_detail = format_anyhow_error(&err);
            warn!(agent = %name, error = %error_detail, "agent created but failed to start");
            Some(format!("{err}"))
        } else {
            None
        }
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
    let ps = state.lifecycle.process_state(&agent_info.id).await;
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
    let agent_id = existing.id.clone();
    let _transition = acquire_transition(&state, &agent_id)?;

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

    // Preserve the existing owner when the request omits `machineId`.
    // Without this, every PATCH would clobber `machine_id` to NULL — a
    // bridge-hosted agent renamed via the UI (which doesn't echo
    // `machineId` back) would silently lose its owner.
    let machine_id_resolved: String = req
        .machine_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| existing.machine_id.clone())
        .unwrap_or_else(|| state.local_machine_id.clone());
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
            machine_id: Some(&machine_id_resolved),
            env_vars: &env_vars,
        })
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?;

    let ps = state.lifecycle.process_state(&agent_id).await;
    let was_running = crate::agent::process_status::derive_status(ps.as_ref())
        != crate::agent::process_status::Status::Asleep;

    // Bridge-hosted agents are restarted by the remote bridge when it
    // receives the broadcast target update below. The platform does not
    // own the runtime process and must not call start/stop. "Bridge-hosted"
    // here means "owned by a non-local machine_id"; agents with
    // machine_id == local_machine_id still go through the platform's
    // in-process AgentManager until Phase 2 swaps it for the in-process
    // bridge client.
    let bridge_hosted = machine_id_resolved != state.local_machine_id;
    if was_running && requires_restart && !bridge_hosted {
        state
            .lifecycle
            .stop_agent(&agent_id)
            .await
            .map_err(internal_err)?;
        // Reload after the update so spec changes (env_vars, model, runtime)
        // take effect on the restart.
        let refreshed = resolve_public_agent_with_env(&state, &existing.id)?;
        if let Err(err) = state.lifecycle.start_agent(&refreshed, None, None).await {
            return Err(app_err!(
                StatusCode::INTERNAL_SERVER_ERROR,
                "agent updated but restart failed: {err}"
            ));
        }
    }
    broadcast_target_update(state.store.as_ref(), state.bridge_registry.as_ref());
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
    let _transition = acquire_transition(&state, &agent.id)?;
    let agents_dir = state.agents_dir.clone();
    let workspace = AgentWorkspace::new(&agents_dir);

    let bridge_hosted = agent
        .machine_id
        .as_deref()
        .is_some_and(|m| m != state.local_machine_id.as_str());
    if !bridge_hosted {
        state
            .lifecycle
            .stop_agent(&agent.id)
            .await
            .map_err(internal_err)?;
    }

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

    if !bridge_hosted {
        // Reload after RestartMode::ResetSession / FullReset so the start
        // sees post-clear state for env_vars/spec lookups.
        let refreshed = resolve_public_agent_with_env(&state, &agent.id)?;
        if let Err(err) = state.lifecycle.start_agent(&refreshed, None, None).await {
            return Err(app_err!(
                AppErrorCode::AgentRestartFailed,
                "restart failed: {err}"
            ));
        }
    } else {
        // Push fresh target so the bridge stops/starts the runtime locally.
        broadcast_target_update(state.store.as_ref(), state.bridge_registry.as_ref());
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
    let name = agent.name.clone();
    let _transition = acquire_transition(&state, &agent.id)?;

    // Skip local stop for bridge-hosted agents: the bridge stops them when
    // the next `bridge.target` (broadcast below after the row delete) drops
    // the agent from the desired set. Calling stop_agent here is a no-op
    // locally but adds noise to the activity log.
    let bridge_hosted = agent
        .machine_id
        .as_deref()
        .is_some_and(|m| m != state.local_machine_id.as_str());
    if !bridge_hosted {
        state
            .lifecycle
            .stop_agent(&agent.id)
            .await
            .map_err(internal_err)?;
    }

    state
        .store
        .mark_agent_messages_deleted(&name)
        .map_err(internal_err)?;
    state
        .store
        .delete_agent_record(&name)
        .map_err(internal_err)?;
    broadcast_target_update(state.store.as_ref(), state.bridge_registry.as_ref());

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
    // Hydrate env_vars: start_agent now reads the spec directly off the
    // record we pass in, so the lookup happens here rather than inside
    // the manager.
    let agent = resolve_public_agent_with_env(&state, &id)?;
    let _transition = acquire_transition(&state, &agent.id)?;
    let bridge_hosted = agent
        .machine_id
        .as_deref()
        .is_some_and(|m| m != state.local_machine_id.as_str());
    if bridge_hosted {
        // Bridge-hosted: the bridge already keeps the runtime alive per
        // target. Re-broadcast so any reconnected bridge picks it up.
        info!(agent = %agent.name, "agent is bridge-hosted; refreshing target");
        broadcast_target_update(state.store.as_ref(), state.bridge_registry.as_ref());
        return Ok(Json(serde_json::json!({ "ok": true })));
    }
    info!(agent = %agent.name, "starting agent");
    state
        .lifecycle
        .start_agent(&agent, None, None)
        .await
        .map_err(internal_err)?;
    info!(agent = %agent.name, "agent started");
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn handle_agent_stop(
    State(state): State<AppState>,
    Path(PublicResourceIdPath { id }): Path<PublicResourceIdPath>,
) -> ApiResult<serde_json::Value> {
    let agent = resolve_public_agent(&state, &id)?;
    let name = agent.name.clone();
    let _transition = acquire_transition(&state, &agent.id)?;
    let bridge_hosted = agent
        .machine_id
        .as_deref()
        .is_some_and(|m| m != state.local_machine_id.as_str());
    if bridge_hosted {
        // Bridge-hosted: platform doesn't own the runtime. There's no
        // explicit "stop a single bridge-hosted agent" frame yet, so this
        // becomes a no-op until we add an `agent.stop` directive in the
        // protocol. For now, surface a clear status rather than silently
        // succeed with the wrong meaning.
        info!(agent = %name, "agent is bridge-hosted; stop is a no-op (deletion via DELETE removes it)");
        return Ok(Json(serde_json::json!({
            "ok": true,
            "note": "agent is bridge-hosted; the platform does not own the runtime"
        })));
    }
    info!(agent = %name, id = %agent.id, "stopping agent");
    state
        .lifecycle
        .stop_agent(&agent.id)
        .await
        .map_err(internal_err)?;
    info!(agent = %name, id = %agent.id, "agent stopped");
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
        .get_agent_activity(&agent.id, limit)
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
        .get_activity_log_data(&agent.id, params.after);
    Ok(Json(resp))
}
