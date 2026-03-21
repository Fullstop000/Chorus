use std::path::PathBuf;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tracing::{debug, info};

use axum::extract::{Multipart, Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};
use serde::Deserialize;
use uuid::Uuid;

use crate::models::*;
use crate::store::Store;

type ApiResult<T> = Result<Json<T>, (StatusCode, Json<ErrorResponse>)>;

/// Runtime lifecycle operations the HTTP server can trigger for agents.
pub trait AgentLifecycle: Send + Sync {
    fn start_agent<'a>(
        &'a self,
        agent_name: &'a str,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>>;

    fn notify_agent<'a>(
        &'a self,
        agent_name: &'a str,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>>;

    fn stop_agent<'a>(
        &'a self,
        agent_name: &'a str,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>>;

    /// Get the in-memory activity log for an agent.
    fn get_activity_log_data(&self, agent_name: &str, after_seq: Option<u64>) -> ActivityLogResponse;

    /// Get current activity state for all agents: (name, activity, detail).
    fn get_all_agent_activity_states(&self) -> Vec<(String, String, String)>;
}

struct NoopAgentLifecycle;

impl AgentLifecycle for NoopAgentLifecycle {
    fn start_agent<'a>(
        &'a self,
        _agent_name: &'a str,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }

    fn notify_agent<'a>(
        &'a self,
        _agent_name: &'a str,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }

    fn stop_agent<'a>(
        &'a self,
        _agent_name: &'a str,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }

    fn get_activity_log_data(&self, _agent_name: &str, _after_seq: Option<u64>) -> ActivityLogResponse {
        ActivityLogResponse {
            entries: vec![],
            agent_activity: "offline".to_string(),
            agent_detail: String::new(),
        }
    }

    fn get_all_agent_activity_states(&self) -> Vec<(String, String, String)> {
        vec![]
    }
}

/// Shared application state for HTTP handlers.
#[derive(Clone)]
pub struct AppState {
    pub store: Arc<Store>,
    pub lifecycle: Arc<dyn AgentLifecycle>,
}

fn api_err(msg: impl Into<String>) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::BAD_REQUEST,
        Json(ErrorResponse {
            error: msg.into(),
        }),
    )
}

fn internal_err(msg: impl Into<String>) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ErrorResponse {
            error: msg.into(),
        }),
    )
}

async fn handle_whoami() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "username": whoami::username() }))
}

#[derive(Deserialize)]
struct CreateAgentRequest {
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

async fn handle_create_agent(
    State(state): State<AppState>,
    Json(req): Json<CreateAgentRequest>,
) -> ApiResult<serde_json::Value> {
    let store = &state.store;
    let name = req.name.trim().to_string();
    if name.is_empty() {
        return Err(api_err("name is required"));
    }
    let display_name = if req.display_name.is_empty() { name.clone() } else { req.display_name };
    let description = if req.description.is_empty() { None } else { Some(req.description.as_str()) };
    store
        .create_agent_record(&name, &display_name, description, &req.runtime, &req.model)
        .map_err(|e| api_err(e.to_string()))?;
    for channel in store.list_channels().map_err(|e| internal_err(e.to_string()))? {
        store
            .join_channel(&channel.name, &name, SenderType::Agent)
            .map_err(|e| internal_err(e.to_string()))?;
    }
    if let Err(err) = state.lifecycle.start_agent(&name).await {
        let _ = store.delete_agent_record(&name);
        return Err(internal_err(format!("failed to start agent: {err}")));
    }
    Ok(Json(serde_json::json!({ "name": name })))
}

pub fn build_router(store: Arc<Store>) -> Router {
    build_router_with_lifecycle(store, Arc::new(NoopAgentLifecycle))
}

/// Build the HTTP router with a concrete agent lifecycle implementation.
pub fn build_router_with_lifecycle(
    store: Arc<Store>,
    lifecycle: Arc<dyn AgentLifecycle>,
) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);
    let state = AppState { store, lifecycle };

    Router::new()
        // ── Existing API routes (unchanged) ──
        .route("/internal/agent/{agent_id}/send", post(handle_send))
        .route("/internal/agent/{agent_id}/receive", get(handle_receive))
        .route("/internal/agent/{agent_id}/history", get(handle_history))
        .route("/internal/agent/{agent_id}/server", get(handle_server_info))
        .route(
            "/internal/agent/{agent_id}/resolve-channel",
            post(handle_resolve_channel),
        )
        .route(
            "/internal/agent/{agent_id}/tasks",
            get(handle_list_tasks).post(handle_create_tasks),
        )
        .route(
            "/internal/agent/{agent_id}/tasks/claim",
            post(handle_claim_tasks),
        )
        .route(
            "/internal/agent/{agent_id}/tasks/unclaim",
            post(handle_unclaim_task),
        )
        .route(
            "/internal/agent/{agent_id}/tasks/update-status",
            post(handle_update_task_status),
        )
        .route("/internal/agent/{agent_id}/upload", post(handle_upload))
        .route(
            "/api/attachments/{attachment_id}",
            get(handle_get_attachment),
        )
        // ── New: whoami + agent management ──
        .route("/api/whoami", get(handle_whoami))
        .route("/api/channels", post(handle_create_channel))
        .route("/api/agents", post(handle_create_agent))
        .route("/api/agents/{name}/start", post(handle_agent_start))
        .route("/api/agents/{name}/stop", post(handle_agent_stop))
        .route("/api/agents/{name}/activity", get(handle_agent_activity))
        .route("/api/agents/{name}/activity-log", get(handle_agent_activity_log))
        .route("/api/agents/{name}/workspace", get(handle_agent_workspace))
        .route("/api/server-info", get(handle_ui_server_info))
        // ── CORS middleware ──
        .layer(cors)
        // ── Static file serving (must be last — fallback for all non-API paths) ──
        .fallback_service(
            ServeDir::new("ui/dist")
                .fallback(ServeFile::new("ui/dist/index.html")),
        )
        .with_state(state)
}

// ── Send ──

async fn handle_send(
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

    let preview: String = req.content.chars().take(120).collect();
    let preview = if req.content.chars().count() > 120 { format!("{preview}…") } else { preview };
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

    deliver_message_to_agents(&state, &channel.id, &agent_id)
        .await
        .map_err(|e| internal_err(e.to_string()))?;

    Ok(Json(SendResponse { message_id }))
}

// ── Receive ──

#[derive(Deserialize)]
struct ReceiveParams {
    block: Option<String>,
    timeout: Option<u64>,
}

async fn handle_receive(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Query(params): Query<ReceiveParams>,
) -> ApiResult<ReceiveResponse> {
    let store = &state.store;
    let blocking = params.block.as_deref() != Some("false");
    let timeout_secs = params.timeout.unwrap_or(30);

    // Check immediately
    let messages = store
        .get_messages_for_agent(&agent_id, true)
        .map_err(|e| api_err(e.to_string()))?;

    if !messages.is_empty() {
        info!(agent = %agent_id, count = messages.len(), "receive_message: got messages immediately");
        for m in &messages {
            let target = format!("{}:{}", m.channel_type, m.channel_name);
            info!(agent = %agent_id, target = %target, sender = %m.sender_name, content = %m.content.chars().take(120).collect::<String>(), "  ← message");
        }
        return Ok(Json(ReceiveResponse { messages }));
    }
    if !blocking {
        debug!(agent = %agent_id, "receive_message: non-blocking, no messages");
        return Ok(Json(ReceiveResponse { messages }));
    }

    debug!(agent = %agent_id, timeout_secs, "receive_message: long-polling");
    // Long-poll: subscribe and wait
    let mut rx = store.subscribe();
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(timeout_secs);

    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Ok(Json(ReceiveResponse {
                messages: Vec::new(),
            }));
        }

        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(_notification)) => {
                let messages = store
                    .get_messages_for_agent(&agent_id, true)
                    .map_err(|e| api_err(e.to_string()))?;
                if !messages.is_empty() {
                    info!(agent = %agent_id, count = messages.len(), "receive_message: woke up with messages");
                    for m in &messages {
                        let target = format!("{}:{}", m.channel_type, m.channel_name);
                        info!(agent = %agent_id, target = %target, sender = %m.sender_name, content = %m.content.chars().take(120).collect::<String>(), "  ← message");
                    }
                    return Ok(Json(ReceiveResponse { messages }));
                }
                // Not for us, keep waiting
            }
            Ok(Err(_)) => {
                // Broadcast channel closed
                return Ok(Json(ReceiveResponse {
                    messages: Vec::new(),
                }));
            }
            Err(_) => {
                // Timeout
                return Ok(Json(ReceiveResponse {
                    messages: Vec::new(),
                }));
            }
        }
    }
}

// ── History ──

#[derive(Deserialize)]
struct HistoryParams {
    channel: Option<String>,
    limit: Option<i64>,
    before: Option<i64>,
    after: Option<i64>,
}

async fn handle_history(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Query(params): Query<HistoryParams>,
) -> ApiResult<HistoryResponse> {
    if let Some(ref ch) = params.channel {
        debug!(agent = %agent_id, channel = %ch, "read_history");
    }
    let store = &state.store;
    let channel_target = params
        .channel
        .ok_or_else(|| api_err("missing channel parameter"))?;
    let (channel_name, thread_parent_id) =
        resolve_history_target(&store, &agent_id, &channel_target)
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

/// Resolve a history target into the persisted channel name and optional thread parent ID.
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

// ── Server Info ──

async fn handle_server_info(
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

async fn handle_resolve_channel(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(req): Json<ResolveChannelRequest>,
) -> ApiResult<ResolveChannelResponse> {
    let (channel_id, _thread_parent_id) = state
        .store
        .resolve_target(&req.target, &agent_id)
        .map_err(|e| api_err(e.to_string()))?;
    Ok(Json(ResolveChannelResponse { channel_id }))
}

// ── Tasks ──

#[derive(Deserialize)]
struct ListTasksParams {
    channel: Option<String>,
    status: Option<String>,
}

async fn handle_list_tasks(
    State(state): State<AppState>,
    Path(_agent_id): Path<String>,
    Query(params): Query<ListTasksParams>,
) -> ApiResult<serde_json::Value> {
    let channel_target = params
        .channel
        .ok_or_else(|| api_err("missing channel parameter"))?;
    let channel_name = channel_target.strip_prefix('#').unwrap_or(&channel_target);

    let status_filter = params
        .status
        .as_deref()
        .and_then(TaskStatus::from_str);

    let tasks = state
        .store
        .list_tasks(channel_name, status_filter)
        .map_err(|e| api_err(e.to_string()))?;

    Ok(Json(serde_json::json!({ "tasks": tasks })))
}

async fn handle_create_tasks(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(req): Json<CreateTasksRequest>,
) -> ApiResult<serde_json::Value> {
    let channel_name = req.channel.strip_prefix('#').unwrap_or(&req.channel);
    let titles: Vec<&str> = req.tasks.iter().map(|t| t.title.as_str()).collect();

    let tasks = state
        .store
        .create_tasks(channel_name, &agent_id, &titles)
        .map_err(|e| api_err(e.to_string()))?;

    Ok(Json(serde_json::json!({ "tasks": tasks })))
}

async fn handle_claim_tasks(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(req): Json<ClaimTasksRequest>,
) -> ApiResult<serde_json::Value> {
    let channel_name = req.channel.strip_prefix('#').unwrap_or(&req.channel);

    let results = state
        .store
        .claim_tasks(channel_name, &agent_id, &req.task_numbers)
        .map_err(|e| api_err(e.to_string()))?;

    Ok(Json(serde_json::json!({ "results": results })))
}

async fn handle_unclaim_task(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(req): Json<UnclaimTaskRequest>,
) -> ApiResult<serde_json::Value> {
    let channel_name = req.channel.strip_prefix('#').unwrap_or(&req.channel);

    state
        .store
        .unclaim_task(channel_name, &agent_id, req.task_number)
        .map_err(|e| api_err(e.to_string()))?;

    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn handle_update_task_status(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(req): Json<UpdateTaskStatusRequest>,
) -> ApiResult<serde_json::Value> {
    let channel_name = req.channel.strip_prefix('#').unwrap_or(&req.channel);
    let new_status = TaskStatus::from_str(&req.status)
        .ok_or_else(|| api_err(format!("invalid status: {}", req.status)))?;

    state
        .store
        .update_task_status(channel_name, req.task_number, &agent_id, new_status)
        .map_err(|e| api_err(e.to_string()))?;

    Ok(Json(serde_json::json!({ "ok": true })))
}

// ── Upload ──

async fn handle_upload(
    State(state): State<AppState>,
    Path(_agent_id): Path<String>,
    mut multipart: Multipart,
) -> ApiResult<serde_json::Value> {
    let store = &state.store;
    let field = multipart
        .next_field()
        .await
        .map_err(|e| api_err(e.to_string()))?
        .ok_or_else(|| api_err("no file uploaded"))?;

    let filename = field
        .file_name()
        .unwrap_or("upload")
        .to_string();
    let content_type = field
        .content_type()
        .unwrap_or("application/octet-stream")
        .to_string();
    let data = field
        .bytes()
        .await
        .map_err(|e| api_err(e.to_string()))?;

    let ext = std::path::Path::new(&filename)
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();

    let file_id = Uuid::new_v4().to_string();
    let attachments_dir = home_dir().join(".chorus").join("attachments");
    std::fs::create_dir_all(&attachments_dir).map_err(|e| internal_err(e.to_string()))?;

    let stored_filename = format!("{}{}", file_id, ext);
    let stored_path = attachments_dir.join(&stored_filename);
    std::fs::write(&stored_path, &data).map_err(|e| internal_err(e.to_string()))?;

    let size = data.len() as i64;
    let att_id = store
        .store_attachment(
            &filename,
            &content_type,
            size,
            stored_path.to_string_lossy().as_ref(),
        )
        .map_err(|e| api_err(e.to_string()))?;

    Ok(Json(serde_json::json!({
        "id": att_id,
        "filename": filename,
        "sizeBytes": size,
    })))
}

// ── Get Attachment ──

async fn handle_get_attachment(
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

    let data =
        std::fs::read(&attachment.stored_path).map_err(|e| internal_err(e.to_string()))?;

    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, attachment.mime_type)],
        data,
    ))
}

/// Start sleeping/inactive agents or notify active ones when a new message arrives.
async fn deliver_message_to_agents(
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

// ── Agent Activity ──

#[derive(Deserialize)]
struct ActivityParams {
    limit: Option<i64>,
}

async fn handle_agent_activity(
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

// ── Create Channel ──

#[derive(Deserialize)]
struct CreateChannelRequest {
    name: String,
    #[serde(default)]
    description: String,
}

async fn handle_create_channel(
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
    // Auto-join all agents
    let username = whoami::username();
    let _ = state.store.join_channel(name, &username, SenderType::Human);
    for agent in state.store.list_agents().unwrap_or_default() {
        let _ = state.store.join_channel(name, &agent.name, SenderType::Agent);
    }
    Ok(Json(serde_json::json!({ "name": name })))
}

// ── Agent Start/Stop ──

async fn handle_agent_start(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<serde_json::Value> {
    info!(agent = %name, "starting agent");
    state.lifecycle.start_agent(&name).await.map_err(|e| internal_err(e.to_string()))?;
    info!(agent = %name, "agent started");
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn handle_agent_stop(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<serde_json::Value> {
    info!(agent = %name, "stopping agent");
    state.lifecycle.stop_agent(&name).await.map_err(|e| internal_err(e.to_string()))?;
    info!(agent = %name, "agent stopped");
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ── Agent Activity Log (living log) ──

#[derive(Deserialize)]
struct ActivityLogParams {
    after: Option<u64>,
}

async fn handle_agent_activity_log(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(params): Query<ActivityLogParams>,
) -> ApiResult<ActivityLogResponse> {
    let resp = state.lifecycle.get_activity_log_data(&name, params.after);
    Ok(Json(resp))
}

// ── UI Server Info (enriched with activity states) ──

async fn handle_ui_server_info(
    State(state): State<AppState>,
) -> ApiResult<serde_json::Value> {
    let username = whoami::username();
    let mut info = state
        .store
        .get_server_info(&username)
        .map_err(|e| api_err(e.to_string()))?;

    // Enrich agent list with live activity states
    let activity_states = state.lifecycle.get_all_agent_activity_states();
    for agent in &mut info.agents {
        if let Some((_, activity, detail)) = activity_states.iter().find(|(n, _, _)| n == &agent.name) {
            agent.activity = Some(activity.clone());
            agent.activity_detail = Some(detail.clone());
        }
    }

    Ok(Json(serde_json::to_value(info).unwrap()))
}

// ── Agent Workspace ──

async fn handle_agent_workspace(
    Path(name): Path<String>,
) -> ApiResult<serde_json::Value> {
    let workspace_dir = home_dir().join(".chorus").join("agents").join(&name);
    if !workspace_dir.exists() {
        return Ok(Json(serde_json::json!({ "files": serde_json::json!([]) })));
    }
    let mut files: Vec<String> = Vec::new();
    collect_workspace_files(&workspace_dir, &workspace_dir, &mut files, 0);
    Ok(Json(serde_json::json!({ "files": files })))
}

fn collect_workspace_files(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<String>, depth: usize) {
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

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}
