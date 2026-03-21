use std::sync::Arc;

use axum::extract::{Multipart, Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use tracing::{debug, info};
use uuid::Uuid;

use crate::models::*;
use crate::store::Store;
use super::{AgentLifecycle, home_dir};

pub type ApiResult<T> = Result<Json<T>, (StatusCode, Json<ErrorResponse>)>;

/// Shared application state injected into every handler via Axum's `State` extractor.
#[derive(Clone)]
pub struct AppState {
    pub store: Arc<Store>,
    pub lifecycle: Arc<dyn AgentLifecycle>,
}

fn api_err(msg: impl Into<String>) -> (StatusCode, Json<ErrorResponse>) {
    (StatusCode::BAD_REQUEST, Json(ErrorResponse { error: msg.into() }))
}

fn internal_err(msg: impl Into<String>) -> (StatusCode, Json<ErrorResponse>) {
    (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { error: msg.into() }))
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
            let parent_name = message.parent_channel_name.as_deref().unwrap_or(&message.channel_name);
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

    let short_id = if message_id.len() >= 8 { &message_id[..8] } else { &message_id };
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

    deliver_message_to_agents(&state, &channel.id, &agent_id)
        .await
        .map_err(|e| internal_err(e.to_string()))?;

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
    let timeout_secs = params.timeout.unwrap_or(30);

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

    debug!(agent = %agent_id, timeout_secs, "receive_message: long-polling");
    let mut rx = store.subscribe();
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(timeout_secs);

    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Ok(Json(ReceiveResponse { messages: Vec::new() }));
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
            _ => return Ok(Json(ReceiveResponse { messages: Vec::new() })),
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
        .get_history(&channel_name, thread_parent_id.as_deref(), limit, params.before, params.after)
        .map_err(|e| api_err(e.to_string()))?;

    let last_read_seq = store.get_last_read_seq(&channel_name, &agent_id).unwrap_or(0);

    Ok(Json(HistoryResponse { messages, has_more, last_read_seq }))
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
    let info = state.store.get_server_info(&agent_id).map_err(|e| api_err(e.to_string()))?;
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
    let channel_target = params.channel.ok_or_else(|| api_err("missing channel parameter"))?;
    let channel_name = strip_channel_prefix(&channel_target);
    let status_filter = params.status.as_deref().and_then(TaskStatus::from_str);
    let tasks = state.store.list_tasks(channel_name, status_filter).map_err(|e| api_err(e.to_string()))?;
    Ok(Json(serde_json::json!({ "tasks": tasks })))
}

pub async fn handle_create_tasks(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(req): Json<CreateTasksRequest>,
) -> ApiResult<serde_json::Value> {
    let channel_name = strip_channel_prefix(&req.channel);
    let titles: Vec<&str> = req.tasks.iter().map(|t| t.title.as_str()).collect();
    let tasks = state.store.create_tasks(channel_name, &agent_id, &titles).map_err(|e| api_err(e.to_string()))?;
    Ok(Json(serde_json::json!({ "tasks": tasks })))
}

pub async fn handle_claim_tasks(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(req): Json<ClaimTasksRequest>,
) -> ApiResult<serde_json::Value> {
    let channel_name = strip_channel_prefix(&req.channel);
    let results = state.store.claim_tasks(channel_name, &agent_id, &req.task_numbers).map_err(|e| api_err(e.to_string()))?;
    Ok(Json(serde_json::json!({ "results": results })))
}

pub async fn handle_unclaim_task(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(req): Json<UnclaimTaskRequest>,
) -> ApiResult<serde_json::Value> {
    let channel_name = strip_channel_prefix(&req.channel);
    state.store.unclaim_task(channel_name, &agent_id, req.task_number).map_err(|e| api_err(e.to_string()))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn handle_update_task_status(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(req): Json<UpdateTaskStatusRequest>,
) -> ApiResult<serde_json::Value> {
    let channel_name = strip_channel_prefix(&req.channel);
    let new_status = TaskStatus::from_str(&req.status)
        .ok_or_else(|| api_err(format!("invalid status: {}", req.status)))?;
    state.store.update_task_status(channel_name, req.task_number, &agent_id, new_status).map_err(|e| api_err(e.to_string()))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ── Upload / Attachment ──

pub async fn handle_upload(
    State(state): State<AppState>,
    Path(_agent_id): Path<String>,
    mut multipart: Multipart,
) -> ApiResult<serde_json::Value> {
    let field = multipart
        .next_field().await.map_err(|e| api_err(e.to_string()))?
        .ok_or_else(|| api_err("no file uploaded"))?;

    let filename = field.file_name().unwrap_or("upload").to_string();
    let content_type = field.content_type().unwrap_or("application/octet-stream").to_string();
    let data = field.bytes().await.map_err(|e| api_err(e.to_string()))?;

    let ext = std::path::Path::new(&filename)
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();

    let file_id = Uuid::new_v4().to_string();
    let attachments_dir = home_dir().join(".chorus").join("attachments");
    std::fs::create_dir_all(&attachments_dir).map_err(|e| internal_err(e.to_string()))?;

    let stored_path = attachments_dir.join(format!("{}{}", file_id, ext));
    std::fs::write(&stored_path, &data).map_err(|e| internal_err(e.to_string()))?;

    let size = data.len() as i64;
    let att_id = state.store
        .store_attachment(&filename, &content_type, size, stored_path.to_string_lossy().as_ref())
        .map_err(|e| api_err(e.to_string()))?;

    Ok(Json(serde_json::json!({ "id": att_id, "filename": filename, "sizeBytes": size })))
}

pub async fn handle_get_attachment(
    State(state): State<AppState>,
    Path(attachment_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    let attachment = state.store
        .get_attachment(&attachment_id)
        .map_err(|e| api_err(e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, Json(ErrorResponse { error: "attachment not found".to_string() })))?;

    let data = std::fs::read(&attachment.stored_path).map_err(|e| internal_err(e.to_string()))?;
    Ok((StatusCode::OK, [(header::CONTENT_TYPE, attachment.mime_type)], data))
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
    let description = if req.description.is_empty() { None } else { Some(req.description.as_str()) };
    state.store.create_channel(name, description, ChannelType::Channel)
        .map_err(|e| api_err(e.to_string()))?;
    let username = whoami::username();
    let _ = state.store.join_channel(name, &username, SenderType::Human);
    for agent in state.store.list_agents().unwrap_or_default() {
        let _ = state.store.join_channel(name, &agent.name, SenderType::Agent);
    }
    Ok(Json(serde_json::json!({ "name": name })))
}

// ── Agent management ──

#[derive(Deserialize)]
pub struct CreateAgentRequest {
    name: String,
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    description: String,
    #[serde(default = "default_runtime")]
    runtime: String,
    #[serde(default = "default_model")]
    model: String,
}

fn default_runtime() -> String { "claude".to_string() }
fn default_model() -> String { "sonnet".to_string() }

pub async fn handle_create_agent(
    State(state): State<AppState>,
    Json(req): Json<CreateAgentRequest>,
) -> ApiResult<serde_json::Value> {
    let name = req.name.trim().to_string();
    if name.is_empty() {
        return Err(api_err("name is required"));
    }
    let display_name = if req.display_name.is_empty() { name.clone() } else { req.display_name };
    let description = if req.description.is_empty() { None } else { Some(req.description.as_str()) };
    state.store
        .create_agent_record(&name, &display_name, description, &req.runtime, &req.model)
        .map_err(|e| api_err(e.to_string()))?;
    for channel in state.store.list_channels().map_err(|e| internal_err(e.to_string()))? {
        let _ = state.store.join_channel(&channel.name, &name, SenderType::Agent);
    }
    if let Err(err) = state.lifecycle.start_agent(&name).await {
        let _ = state.store.delete_agent_record(&name);
        return Err(internal_err(format!("failed to start agent: {err}")));
    }
    Ok(Json(serde_json::json!({ "name": name })))
}

pub async fn handle_agent_start(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<serde_json::Value> {
    info!(agent = %name, "starting agent");
    state.lifecycle.start_agent(&name).await.map_err(|e| internal_err(e.to_string()))?;
    info!(agent = %name, "agent started");
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn handle_agent_stop(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<serde_json::Value> {
    info!(agent = %name, "stopping agent");
    state.lifecycle.stop_agent(&name).await.map_err(|e| internal_err(e.to_string()))?;
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
    let messages = state.store.get_agent_activity(&name, limit).map_err(|e| api_err(e.to_string()))?;
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

pub async fn handle_ui_server_info(
    State(state): State<AppState>,
) -> ApiResult<serde_json::Value> {
    let username = whoami::username();
    let mut info = state.store.get_server_info(&username).map_err(|e| api_err(e.to_string()))?;

    let activity_states = state.lifecycle.get_all_agent_activity_states();
    for agent in &mut info.agents {
        if let Some((_, activity, detail)) = activity_states.iter().find(|(n, _, _)| n == &agent.name) {
            agent.activity = Some(activity.clone());
            agent.activity_detail = Some(detail.clone());
        }
    }

    Ok(Json(serde_json::to_value(info).unwrap()))
}

// ── Workspace ──

pub async fn handle_agent_workspace(
    Path(name): Path<String>,
) -> ApiResult<serde_json::Value> {
    let workspace_dir = home_dir().join(".chorus").join("agents").join(&name);
    if !workspace_dir.exists() {
        return Ok(Json(serde_json::json!({ "files": [] })));
    }
    let mut files: Vec<String> = Vec::new();
    collect_workspace_files(&workspace_dir, &workspace_dir, &mut files, 0);
    Ok(Json(serde_json::json!({ "files": files })))
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
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut sorted: Vec<_> = entries.filter_map(|e| e.ok()).collect();
    sorted.sort_by_key(|e| e.file_name());
    for entry in sorted {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let rel = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().into_owned();
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
) -> anyhow::Result<()> {
    let members = state.store.get_channel_members(channel_id)?;
    for member in members {
        if member.member_type != SenderType::Agent || member.member_name == sender_name {
            continue;
        }
        let Some(agent) = state.store.get_agent(&member.member_name)? else {
            continue;
        };
        match agent.status {
            AgentStatus::Active => state.lifecycle.notify_agent(&member.member_name).await?,
            AgentStatus::Sleeping | AgentStatus::Inactive => {
                state.lifecycle.start_agent(&member.member_name).await?
            }
        }
    }
    Ok(())
}
