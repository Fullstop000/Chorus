use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Multipart, Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use uuid::Uuid;

use crate::models::*;
use crate::store::Store;

type ApiResult<T> = Result<Json<T>, (StatusCode, Json<ErrorResponse>)>;

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

pub fn build_router(store: Arc<Store>) -> Router {
    Router::new()
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
        .with_state(store)
}

// ── Send ──

async fn handle_send(
    State(store): State<Arc<Store>>,
    Path(agent_id): Path<String>,
    Json(req): Json<SendRequest>,
) -> ApiResult<SendResponse> {
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

    Ok(Json(SendResponse { message_id }))
}

// ── Receive ──

#[derive(Deserialize)]
struct ReceiveParams {
    block: Option<String>,
    timeout: Option<u64>,
}

async fn handle_receive(
    State(store): State<Arc<Store>>,
    Path(agent_id): Path<String>,
    Query(params): Query<ReceiveParams>,
) -> ApiResult<ReceiveResponse> {
    let blocking = params.block.as_deref() != Some("false");
    let timeout_secs = params.timeout.unwrap_or(30);

    // Check immediately
    let messages = store
        .get_messages_for_agent(&agent_id, true)
        .map_err(|e| api_err(e.to_string()))?;

    if !messages.is_empty() || !blocking {
        return Ok(Json(ReceiveResponse { messages }));
    }

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
    State(store): State<Arc<Store>>,
    Path(agent_id): Path<String>,
    Query(params): Query<HistoryParams>,
) -> ApiResult<HistoryResponse> {
    let channel_target = params
        .channel
        .ok_or_else(|| api_err("missing channel parameter"))?;

    // The channel param comes URL-encoded, e.g. %23general -> #general
    // axum's Query extractor already decodes it.
    // Strip leading # if present to get the channel name
    let channel_name = channel_target.strip_prefix('#').unwrap_or(&channel_target);

    let limit = params.limit.unwrap_or(50);

    let (messages, has_more) = store
        .get_history(channel_name, None, limit, params.before, params.after)
        .map_err(|e| api_err(e.to_string()))?;

    let last_read_seq = store
        .get_last_read_seq(channel_name, &agent_id)
        .unwrap_or(0);

    Ok(Json(HistoryResponse {
        messages,
        has_more,
        last_read_seq,
    }))
}

// ── Server Info ──

async fn handle_server_info(
    State(store): State<Arc<Store>>,
    Path(agent_id): Path<String>,
) -> ApiResult<ServerInfo> {
    let info = store
        .get_server_info(&agent_id)
        .map_err(|e| api_err(e.to_string()))?;
    Ok(Json(info))
}

// ── Resolve Channel ──

async fn handle_resolve_channel(
    State(store): State<Arc<Store>>,
    Path(agent_id): Path<String>,
    Json(req): Json<ResolveChannelRequest>,
) -> ApiResult<ResolveChannelResponse> {
    let (channel_id, _thread_parent_id) = store
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
    State(store): State<Arc<Store>>,
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

    let tasks = store
        .list_tasks(channel_name, status_filter)
        .map_err(|e| api_err(e.to_string()))?;

    Ok(Json(serde_json::json!({ "tasks": tasks })))
}

async fn handle_create_tasks(
    State(store): State<Arc<Store>>,
    Path(agent_id): Path<String>,
    Json(req): Json<CreateTasksRequest>,
) -> ApiResult<serde_json::Value> {
    let channel_name = req.channel.strip_prefix('#').unwrap_or(&req.channel);
    let titles: Vec<&str> = req.tasks.iter().map(|t| t.title.as_str()).collect();

    let tasks = store
        .create_tasks(channel_name, &agent_id, &titles)
        .map_err(|e| api_err(e.to_string()))?;

    Ok(Json(serde_json::json!({ "tasks": tasks })))
}

async fn handle_claim_tasks(
    State(store): State<Arc<Store>>,
    Path(agent_id): Path<String>,
    Json(req): Json<ClaimTasksRequest>,
) -> ApiResult<serde_json::Value> {
    let channel_name = req.channel.strip_prefix('#').unwrap_or(&req.channel);

    let results = store
        .claim_tasks(channel_name, &agent_id, &req.task_numbers)
        .map_err(|e| api_err(e.to_string()))?;

    Ok(Json(serde_json::json!({ "results": results })))
}

async fn handle_unclaim_task(
    State(store): State<Arc<Store>>,
    Path(agent_id): Path<String>,
    Json(req): Json<UnclaimTaskRequest>,
) -> ApiResult<serde_json::Value> {
    let channel_name = req.channel.strip_prefix('#').unwrap_or(&req.channel);

    store
        .unclaim_task(channel_name, &agent_id, req.task_number)
        .map_err(|e| api_err(e.to_string()))?;

    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn handle_update_task_status(
    State(store): State<Arc<Store>>,
    Path(agent_id): Path<String>,
    Json(req): Json<UpdateTaskStatusRequest>,
) -> ApiResult<serde_json::Value> {
    let channel_name = req.channel.strip_prefix('#').unwrap_or(&req.channel);
    let new_status = TaskStatus::from_str(&req.status)
        .ok_or_else(|| api_err(format!("invalid status: {}", req.status)))?;

    store
        .update_task_status(channel_name, req.task_number, &agent_id, new_status)
        .map_err(|e| api_err(e.to_string()))?;

    Ok(Json(serde_json::json!({ "ok": true })))
}

// ── Upload ──

async fn handle_upload(
    State(store): State<Arc<Store>>,
    Path(_agent_id): Path<String>,
    mut multipart: Multipart,
) -> ApiResult<serde_json::Value> {
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
    State(store): State<Arc<Store>>,
    Path(attachment_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    let attachment = store
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

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}
