use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use axum::extract::{Multipart, Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use tracing::{debug, info};
use uuid::Uuid;

use super::AgentLifecycle;
use crate::agent_workspace::AgentWorkspace;
use crate::models::*;
use crate::store::Store;

pub type ApiResult<T> = Result<Json<T>, (StatusCode, Json<ErrorResponse>)>;

/// Shared application state injected into every handler via Axum's `State` extractor.
#[derive(Clone)]
pub struct AppState {
    pub store: Arc<Store>,
    pub lifecycle: Arc<dyn AgentLifecycle>,
    pub transitioning_agents: Arc<Mutex<HashSet<String>>>,
}

fn api_err(msg: impl Into<String>) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::BAD_REQUEST,
        Json(ErrorResponse { error: msg.into() }),
    )
}

fn internal_err(msg: impl Into<String>) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ErrorResponse { error: msg.into() }),
    )
}

fn conflict_err(msg: impl Into<String>) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::CONFLICT,
        Json(ErrorResponse { error: msg.into() }),
    )
}

struct TransitionGuard {
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

fn acquire_transition(
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

fn strip_channel_prefix(s: &str) -> &str {
    s.strip_prefix('#').unwrap_or(s)
}

/// Build a compact preview suitable for activity log rows and tracing.
fn content_preview(text: &str) -> String {
    let preview: String = text.chars().take(120).collect();
    if text.chars().count() > 120 {
        format!("{preview}…")
    } else {
        preview
    }
}

/// Convert a delivered message into the label shown in the activity timeline.
fn activity_channel_label(message: &ReceivedMessage) -> String {
    match message.channel_type.as_str() {
        "channel" => format!("#{}", message.channel_name),
        "dm" => format!("dm:@{}", message.channel_name),
        "thread" => {
            let parent_type = message.parent_channel_type.as_deref().unwrap_or("channel");
            let parent_name = message
                .parent_channel_name
                .as_deref()
                .unwrap_or(&message.channel_name);
            match parent_type {
                "dm" => format!("dm:@{} thread", parent_name),
                _ => format!("#{} thread", parent_name),
            }
        }
        _ => message.channel_name.clone(),
    }
}

/// Record received messages in the activity log so the UI can show communication flow.
fn push_received_activity(state: &AppState, agent_id: &str, messages: &[ReceivedMessage]) {
    for message in messages {
        state.lifecycle.push_activity_entry(
            agent_id,
            ActivityEntry::MessageReceived {
                channel_label: activity_channel_label(message),
                sender_name: message.sender_name.clone(),
                content: content_preview(&message.content),
            },
        );
    }
}

// ── Whoami ──

pub async fn handle_whoami() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "username": whoami::username() }))
}

// ── Send ──

pub async fn handle_send(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(req): Json<SendRequest>,
) -> ApiResult<SendResponse> {
    let store = &state.store;
    let sender_type = store
        .lookup_sender_type(&agent_id)
        .map_err(|e| api_err(e.to_string()))?
        .unwrap_or(SenderType::Human);

    let (channel_id, thread_parent_id) = store
        .resolve_target(&req.target, &agent_id)
        .map_err(|e| api_err(e.to_string()))?;

    let channel = store
        .find_channel_by_id(&channel_id)
        .map_err(|e| api_err(e.to_string()))?
        .ok_or_else(|| api_err("channel not found"))?;

    // System channels (e.g. #shared-memory) are write-protected.
    // Agents must use mcp_chat_remember instead of send_message to post there.
    if channel.channel_type == ChannelType::System {
        return Err(api_err(
            "Cannot post to system channels directly. Use mcp_chat_remember instead.",
        ));
    }

    let preview = content_preview(&req.content);
    info!(agent = %agent_id, target = %req.target, content = %preview, "send_message");

    let message_id = store
        .send_message(
            &channel.name,
            thread_parent_id.as_deref(),
            &agent_id,
            sender_type,
            &req.content,
            &req.attachment_ids,
        )
        .map_err(|e| api_err(e.to_string()))?;

    let short_id = if message_id.len() >= 8 {
        &message_id[..8]
    } else {
        &message_id
    };
    info!(agent = %agent_id, msg = %short_id, "send_message ok");
    if sender_type == SenderType::Agent {
        state.lifecycle.push_activity_entry(
            &agent_id,
            ActivityEntry::MessageSent {
                target: req.target.clone(),
                content: preview,
            },
        );
    }

    if !req.suppress_agent_delivery {
        deliver_message_to_agents(&state, &channel.id, &agent_id, &message_id)
            .await
            .map_err(|e| internal_err(e.to_string()))?;
    }

    Ok(Json(SendResponse { message_id }))
}

// ── Receive ──

#[derive(Deserialize)]
pub struct ReceiveParams {
    block: Option<String>,
    timeout: Option<u64>,
}

pub async fn handle_receive(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Query(params): Query<ReceiveParams>,
) -> ApiResult<ReceiveResponse> {
    let store = &state.store;
    let blocking = params.block.as_deref() != Some("false");
    let timeout_ms = params.timeout.unwrap_or(30_000);

    let messages = store
        .get_messages_for_agent(&agent_id, true)
        .map_err(|e| api_err(e.to_string()))?;

    if !messages.is_empty() {
        info!(agent = %agent_id, count = messages.len(), "receive_message: got messages immediately");
        for m in &messages {
            info!(agent = %agent_id, target = %format!("{}:{}", m.channel_type, m.channel_name), sender = %m.sender_name, content = %m.content.chars().take(120).collect::<String>(), "  ← message");
        }
        push_received_activity(&state, &agent_id, &messages);
        return Ok(Json(ReceiveResponse { messages }));
    }
    if !blocking {
        debug!(agent = %agent_id, "receive_message: non-blocking, no messages");
        return Ok(Json(ReceiveResponse { messages }));
    }

    debug!(agent = %agent_id, timeout_ms, "receive_message: long-polling");
    let mut rx = store.subscribe();
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_millis(timeout_ms);

    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Ok(Json(ReceiveResponse {
                messages: Vec::new(),
            }));
        }

        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(_)) => {
                let messages = store
                    .get_messages_for_agent(&agent_id, true)
                    .map_err(|e| api_err(e.to_string()))?;
                if !messages.is_empty() {
                    info!(agent = %agent_id, count = messages.len(), "receive_message: woke up with messages");
                    for m in &messages {
                        info!(agent = %agent_id, target = %format!("{}:{}", m.channel_type, m.channel_name), sender = %m.sender_name, content = %m.content.chars().take(120).collect::<String>(), "  ← message");
                    }
                    push_received_activity(&state, &agent_id, &messages);
                    return Ok(Json(ReceiveResponse { messages }));
                }
            }
            _ => {
                return Ok(Json(ReceiveResponse {
                    messages: Vec::new(),
                }))
            }
        }
    }
}

// ── History ──

#[derive(Deserialize)]
pub struct HistoryParams {
    channel: Option<String>,
    limit: Option<i64>,
    before: Option<i64>,
    after: Option<i64>,
}

pub async fn handle_history(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Query(params): Query<HistoryParams>,
) -> ApiResult<HistoryResponse> {
    let channel_target = params
        .channel
        .ok_or_else(|| api_err("missing channel parameter"))?;
    if let Some(ref ch) = Some(&channel_target) {
        debug!(agent = %agent_id, channel = %ch, "read_history");
    }

    let store = &state.store;
    let (channel_name, thread_parent_id) =
        resolve_history_target(store, &agent_id, &channel_target)
            .map_err(|e| api_err(e.to_string()))?;

    let limit = params.limit.unwrap_or(50);
    let (messages, has_more) = store
        .get_history(
            &channel_name,
            thread_parent_id.as_deref(),
            limit,
            params.before,
            params.after,
        )
        .map_err(|e| api_err(e.to_string()))?;

    let last_read_seq = store
        .get_last_read_seq(&channel_name, &agent_id)
        .unwrap_or(0);

    Ok(Json(HistoryResponse {
        messages,
        has_more,
        last_read_seq,
    }))
}

fn resolve_history_target(
    store: &Store,
    agent_id: &str,
    channel_target: &str,
) -> anyhow::Result<(String, Option<String>)> {
    if channel_target.starts_with('#') || channel_target.starts_with("dm:@") {
        let (channel_id, thread_parent_id) = store.resolve_target(channel_target, agent_id)?;
        let channel = store
            .find_channel_by_id(&channel_id)?
            .ok_or_else(|| anyhow::anyhow!("channel not found: {}", channel_target))?;
        return Ok((channel.name, thread_parent_id));
    }
    Ok((channel_target.to_string(), None))
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

// ── Resolve Channel ──

pub async fn handle_resolve_channel(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(req): Json<ResolveChannelRequest>,
) -> ApiResult<ResolveChannelResponse> {
    let (channel_id, _) = state
        .store
        .resolve_target(&req.target, &agent_id)
        .map_err(|e| api_err(e.to_string()))?;
    Ok(Json(ResolveChannelResponse { channel_id }))
}

// ── Tasks ──

#[derive(Deserialize)]
pub struct ListTasksParams {
    channel: Option<String>,
    status: Option<String>,
}

pub async fn handle_list_tasks(
    State(state): State<AppState>,
    Path(_agent_id): Path<String>,
    Query(params): Query<ListTasksParams>,
) -> ApiResult<serde_json::Value> {
    let channel_target = params
        .channel
        .ok_or_else(|| api_err("missing channel parameter"))?;
    let channel_name = strip_channel_prefix(&channel_target);
    let status_filter = params
        .status
        .as_deref()
        .and_then(TaskStatus::from_status_str);
    let tasks = state
        .store
        .list_tasks(channel_name, status_filter)
        .map_err(|e| api_err(e.to_string()))?;
    Ok(Json(serde_json::json!({ "tasks": tasks })))
}

pub async fn handle_create_tasks(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(req): Json<CreateTasksRequest>,
) -> ApiResult<serde_json::Value> {
    let channel_name = strip_channel_prefix(&req.channel);
    let titles: Vec<&str> = req.tasks.iter().map(|t| t.title.as_str()).collect();
    let tasks = state
        .store
        .create_tasks(channel_name, &agent_id, &titles)
        .map_err(|e| api_err(e.to_string()))?;
    Ok(Json(serde_json::json!({ "tasks": tasks })))
}

pub async fn handle_claim_tasks(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(req): Json<ClaimTasksRequest>,
) -> ApiResult<serde_json::Value> {
    let channel_name = strip_channel_prefix(&req.channel);
    let results = state
        .store
        .claim_tasks(channel_name, &agent_id, &req.task_numbers)
        .map_err(|e| api_err(e.to_string()))?;
    Ok(Json(serde_json::json!({ "results": results })))
}

pub async fn handle_unclaim_task(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(req): Json<UnclaimTaskRequest>,
) -> ApiResult<serde_json::Value> {
    let channel_name = strip_channel_prefix(&req.channel);
    state
        .store
        .unclaim_task(channel_name, &agent_id, req.task_number)
        .map_err(|e| api_err(e.to_string()))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn handle_update_task_status(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(req): Json<UpdateTaskStatusRequest>,
) -> ApiResult<serde_json::Value> {
    let channel_name = strip_channel_prefix(&req.channel);
    let new_status = TaskStatus::from_status_str(&req.status)
        .ok_or_else(|| api_err(format!("invalid status: {}", req.status)))?;
    state
        .store
        .update_task_status(channel_name, req.task_number, &agent_id, new_status)
        .map_err(|e| api_err(e.to_string()))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ── Upload / Attachment ──

pub async fn handle_upload(
    State(state): State<AppState>,
    Path(_agent_id): Path<String>,
    mut multipart: Multipart,
) -> ApiResult<serde_json::Value> {
    let field = multipart
        .next_field()
        .await
        .map_err(|e| api_err(e.to_string()))?
        .ok_or_else(|| api_err("no file uploaded"))?;

    let filename = field.file_name().unwrap_or("upload").to_string();
    let content_type = field
        .content_type()
        .unwrap_or("application/octet-stream")
        .to_string();
    let data = field.bytes().await.map_err(|e| api_err(e.to_string()))?;

    let ext = std::path::Path::new(&filename)
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();

    let file_id = Uuid::new_v4().to_string();
    let attachments_dir = state.store.attachments_dir();
    std::fs::create_dir_all(&attachments_dir).map_err(|e| internal_err(e.to_string()))?;

    let stored_path = attachments_dir.join(format!("{}{}", file_id, ext));
    std::fs::write(&stored_path, &data).map_err(|e| internal_err(e.to_string()))?;

    let size = data.len() as i64;
    let att_id = state
        .store
        .store_attachment(
            &filename,
            &content_type,
            size,
            stored_path.to_string_lossy().as_ref(),
        )
        .map_err(|e| api_err(e.to_string()))?;

    Ok(Json(
        serde_json::json!({ "id": att_id, "filename": filename, "sizeBytes": size }),
    ))
}

pub async fn handle_get_attachment(
    State(state): State<AppState>,
    Path(attachment_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    let attachment = state
        .store
        .get_attachment(&attachment_id)
        .map_err(|e| api_err(e.to_string()))?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: "attachment not found".to_string(),
                }),
            )
        })?;

    let data = std::fs::read(&attachment.stored_path).map_err(|e| internal_err(e.to_string()))?;
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, attachment.mime_type)],
        data,
    ))
}

// ── Channel management ──

#[derive(Deserialize)]
pub struct CreateChannelRequest {
    name: String,
    #[serde(default)]
    description: String,
}

pub async fn handle_create_channel(
    State(state): State<AppState>,
    Json(req): Json<CreateChannelRequest>,
) -> ApiResult<serde_json::Value> {
    let name = req.name.trim().to_lowercase();
    let name = name.trim_start_matches('#');
    if name.is_empty() {
        return Err(api_err("name is required"));
    }
    let description = if req.description.is_empty() {
        None
    } else {
        Some(req.description.as_str())
    };
    state
        .store
        .create_channel(name, description, ChannelType::Channel)
        .map_err(|e| api_err(e.to_string()))?;
    let username = whoami::username();
    let _ = state.store.join_channel(name, &username, SenderType::Human);
    for agent in state.store.list_agents().unwrap_or_default() {
        let _ = state
            .store
            .join_channel(name, &agent.name, SenderType::Agent);
    }
    Ok(Json(serde_json::json!({ "name": name })))
}

// ── Agent management ──

fn normalize_agent_env_vars(env_vars: &[AgentEnvVarPayload]) -> Result<Vec<AgentEnvVar>, (StatusCode, Json<ErrorResponse>)> {
    if env_vars.len() > 100 {
        return Err(api_err("too many environment variables"));
    }

    let mut seen = std::collections::HashSet::new();
    let mut normalized = Vec::with_capacity(env_vars.len());
    for (index, env_var) in env_vars.iter().enumerate() {
        let key = env_var.key.trim().to_string();
        if key.is_empty() {
            return Err(api_err("environment variable key is required"));
        }
        if key.len() > 8_192 || env_var.value.len() > 8_192 {
            return Err(api_err("environment variable key/value is too large"));
        }
        if !seen.insert(key.clone()) {
            return Err(api_err(format!("duplicate environment variable key: {key}")));
        }
        normalized.push(AgentEnvVar {
            key,
            value: env_var.value.clone(),
            position: index as i64,
        });
    }
    Ok(normalized)
}

fn agent_info_from_agent(agent: &Agent) -> AgentInfo {
    AgentInfo {
        name: agent.name.clone(),
        status: match agent.status {
            AgentStatus::Active => "active".to_string(),
            AgentStatus::Sleeping => "sleeping".to_string(),
            AgentStatus::Inactive => "inactive".to_string(),
        },
        display_name: Some(agent.display_name.clone()),
        description: agent.description.clone(),
        runtime: Some(agent.runtime.clone()),
        model: Some(agent.model.clone()),
        session_id: agent.session_id.clone(),
        activity: None,
        activity_detail: None,
    }
}

pub async fn handle_create_agent(
    State(state): State<AppState>,
    Json(req): Json<CreateAgentRequest>,
) -> ApiResult<serde_json::Value> {
    let name = req.name.trim().to_string();
    if name.is_empty() {
        return Err(api_err("name is required"));
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
    let env_vars = normalize_agent_env_vars(&req.env_vars)?;
    state
        .store
        .create_agent_record(
            &name,
            &display_name,
            description,
            &req.runtime,
            &req.model,
            &env_vars,
        )
        .map_err(|e| api_err(e.to_string()))?;
    for channel in state
        .store
        .list_channels()
        .map_err(|e| internal_err(e.to_string()))?
    {
        let _ = state
            .store
            .join_channel(&channel.name, &name, SenderType::Agent);
    }
    if let Err(err) = state.lifecycle.start_agent(&name, None).await {
        let _ = state.store.delete_agent_record(&name);
        return Err(internal_err(format!("failed to start agent: {err}")));
    }
    Ok(Json(serde_json::json!({ "name": name })))
}

pub async fn handle_get_agent(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<AgentDetailResponse> {
    let agent = state
        .store
        .get_agent(&name)
        .map_err(|e| api_err(e.to_string()))?
        .ok_or_else(|| api_err("agent not found"))?;
    Ok(Json(AgentDetailResponse {
        agent: agent_info_from_agent(&agent),
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
        .map_err(|e| api_err(e.to_string()))?
        .ok_or_else(|| api_err("agent not found"))?;

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

    let requires_restart = existing.runtime != req.runtime
        || existing.model != req.model
        || existing.env_vars != env_vars;

    state
        .store
        .update_agent_record(
            &name,
            &display_name,
            description,
            &req.runtime,
            &req.model,
            &env_vars,
        )
        .map_err(|e| api_err(e.to_string()))?;

    if existing.status == AgentStatus::Active && requires_restart {
        state
            .lifecycle
            .stop_agent(&name)
            .await
            .map_err(|e| internal_err(e.to_string()))?;
        if let Err(err) = state.lifecycle.start_agent(&name, None).await {
            let _ = state
                .store
                .update_agent_status(&name, AgentStatus::Inactive);
            return Err(internal_err(format!(
                "agent updated but restart failed: {err}"
            )));
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
        .map_err(|e| api_err(e.to_string()))?
        .ok_or_else(|| api_err("agent not found"))?;
    let agents_dir = state.store.agents_dir();
    let workspace = AgentWorkspace::new(&agents_dir);

    state
        .lifecycle
        .stop_agent(&name)
        .await
        .map_err(|e| internal_err(e.to_string()))?;

    match req.mode {
        RestartMode::Restart => {}
        RestartMode::ResetSession => {
            state
                .store
                .update_agent_session(&name, None)
                .map_err(|e| internal_err(e.to_string()))?;
        }
        RestartMode::FullReset => {
            state
                .store
                .update_agent_session(&name, None)
                .map_err(|e| internal_err(e.to_string()))?;
            workspace
                .delete_if_exists(&name)
                .map_err(|e| internal_err(format!("failed to delete workspace: {e}")))?;
        }
    }

    if let Err(err) = state.lifecycle.start_agent(&name, None).await {
        let _ = state
            .store
            .update_agent_status(&name, AgentStatus::Inactive);
        return Err(internal_err(format!("restart failed: {err}")));
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
        .map_err(|e| api_err(e.to_string()))?
        .ok_or_else(|| api_err("agent not found"))?;

    state
        .lifecycle
        .stop_agent(&name)
        .await
        .map_err(|e| internal_err(e.to_string()))?;

    state
        .store
        .mark_agent_messages_deleted(&name)
        .map_err(|e| internal_err(e.to_string()))?;
    state
        .store
        .delete_agent_record(&name)
        .map_err(|e| internal_err(e.to_string()))?;

    if matches!(req.mode, DeleteMode::DeleteWorkspace) {
        let agents_dir = state.store.agents_dir();
        let workspace = AgentWorkspace::new(&agents_dir);
        workspace
            .delete_if_exists(&name)
            .map_err(|e| internal_err(format!("agent deleted but failed to delete workspace: {e}")))?;
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
        .map_err(|e| internal_err(e.to_string()))?;
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
        .map_err(|e| internal_err(e.to_string()))?;
    info!(agent = %name, "agent stopped");
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ── Activity ──

#[derive(Deserialize)]
pub struct ActivityParams {
    limit: Option<i64>,
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
        .map_err(|e| api_err(e.to_string()))?;
    Ok(Json(serde_json::json!({ "messages": messages })))
}

#[derive(Deserialize)]
pub struct ActivityLogParams {
    after: Option<u64>,
}

pub async fn handle_agent_activity_log(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(params): Query<ActivityLogParams>,
) -> ApiResult<ActivityLogResponse> {
    let resp = state.lifecycle.get_activity_log_data(&name, params.after);
    Ok(Json(resp))
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

// ── Workspace ──

#[derive(Deserialize)]
pub struct WorkspaceFileParams {
    path: String,
}

pub async fn handle_agent_workspace(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<serde_json::Value> {
    let workspace_dir = state.store.agents_dir().join(&name);
    if !workspace_dir.exists() {
        return Ok(Json(serde_json::json!({
            "path": workspace_dir.to_string_lossy(),
            "files": []
        })));
    }
    let mut files: Vec<String> = Vec::new();
    collect_workspace_files(&workspace_dir, &workspace_dir, &mut files, 0);
    Ok(Json(serde_json::json!({
        "path": workspace_dir.to_string_lossy(),
        "files": files
    })))
}

pub async fn handle_agent_workspace_file(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(params): Query<WorkspaceFileParams>,
) -> ApiResult<serde_json::Value> {
    let workspace_dir = state.store.agents_dir().join(&name);
    let relative = sanitize_workspace_path(&params.path)?;
    let file_path = workspace_dir.join(&relative);

    if !file_path.is_file() {
        return Err(api_err("workspace file not found"));
    }

    let metadata = std::fs::metadata(&file_path).map_err(|e| internal_err(e.to_string()))?;
    let bytes = std::fs::read(&file_path).map_err(|e| internal_err(e.to_string()))?;
    let limit = 100_000usize;
    let truncated = bytes.len() > limit;
    let content = if truncated {
        String::from_utf8_lossy(&bytes[..limit]).into_owned()
    } else {
        String::from_utf8_lossy(&bytes).into_owned()
    };
    let modified_ms = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as u64);

    Ok(Json(serde_json::json!({
        "path": relative.to_string_lossy(),
        "content": content,
        "truncated": truncated,
        "sizeBytes": metadata.len(),
        "modifiedMs": modified_ms
    })))
}

fn sanitize_workspace_path(
    path: &str,
) -> Result<std::path::PathBuf, (StatusCode, Json<ErrorResponse>)> {
    use std::path::{Component, PathBuf};

    let candidate = std::path::Path::new(path);
    let mut cleaned = PathBuf::new();
    for component in candidate.components() {
        match component {
            Component::Normal(part) => cleaned.push(part),
            Component::CurDir => {}
            _ => return Err(api_err("invalid workspace path")),
        }
    }

    if cleaned.as_os_str().is_empty() {
        return Err(api_err("invalid workspace path"));
    }

    Ok(cleaned)
}

fn collect_workspace_files(
    root: &std::path::Path,
    dir: &std::path::Path,
    out: &mut Vec<String>,
    depth: usize,
) {
    if depth > 5 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut sorted: Vec<_> = entries.filter_map(|e| e.ok()).collect();
    sorted.sort_by_key(|e| e.file_name());
    for entry in sorted {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();
        if path.is_dir() {
            out.push(format!("{}/", rel));
            collect_workspace_files(root, &path, out, depth + 1);
        } else {
            out.push(rel);
        }
    }
}

// ── Message delivery ──

pub async fn deliver_message_to_agents(
    state: &AppState,
    channel_id: &str,
    sender_name: &str,
    message_id: &str,
) -> anyhow::Result<()> {
    // Thread messages are scoped to implicit thread participants rather than
    // every agent in the parent channel.
    let recipients =
        state
            .store
            .get_agent_message_recipients(channel_id, message_id, sender_name)?;
    for recipient_name in recipients {
        let Some(agent) = state.store.get_agent(&recipient_name)? else {
            continue;
        };
        match agent.status {
            AgentStatus::Active => state.lifecycle.notify_agent(&recipient_name).await?,
            AgentStatus::Sleeping | AgentStatus::Inactive => {
                let wake_message = state
                    .store
                    .get_received_message_for_agent(&recipient_name, message_id)?;
                state
                    .lifecycle
                    .start_agent(&recipient_name, wake_message)
                    .await?
            }
        }
    }
    Ok(())
}

// ── Knowledge store (remember / recall) ──

/// Store a fact in the shared knowledge store and post a breadcrumb to #shared-memory.
/// Both writes happen atomically — if posting to #shared-memory fails the knowledge entry
/// is still retained (best-effort visibility).
pub async fn handle_remember(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(req): Json<RememberRequest>,
) -> ApiResult<RememberResponse> {
    let store = &state.store;

    // Normalise tags: join the vec into space-separated FTS5 tokens.
    let tags = req.tags.join(" ");

    let id = store
        .remember(
            &req.key,
            &req.value,
            &tags,
            &agent_id,
            req.channel_context.as_deref(),
        )
        .map_err(|e| internal_err(e.to_string()))?;

    info!(agent = %agent_id, key = %req.key, id = %id, "knowledge remember");

    // Post a human-readable breadcrumb to #shared-memory.
    // Best-effort: don't fail the remember call if the channel post fails.
    let breadcrumb = if tags.is_empty() {
        format!("[🧠 @{}] {}: {}", agent_id, req.key, req.value)
    } else {
        format!("[🧠 @{}] {} [{}]: {}", agent_id, req.key, tags, req.value)
    };
    let _ = store.send_message(
        "shared-memory",
        None,
        &agent_id,
        SenderType::Agent,
        &breadcrumb,
        &[],
    );

    Ok(Json(RememberResponse { id }))
}

/// Search the shared knowledge store by keyword and/or tags.
pub async fn handle_recall(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Query(q): Query<RecallQuery>,
) -> ApiResult<RecallResponse> {
    let entries = state
        .store
        .recall(q.query.as_deref(), q.tags.as_deref())
        .map_err(|e| internal_err(e.to_string()))?;

    debug!(agent = %agent_id, query = ?q.query, count = entries.len(), "knowledge recall");

    Ok(Json(RecallResponse { entries }))
}
