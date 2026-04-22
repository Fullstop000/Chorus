use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use super::{app_err, ApiResult, AppState};
use crate::store::channels::Channel;
use crate::store::tasks::{TaskInfo, TaskStatus};

// ── Inline query structs ──

#[derive(Deserialize)]
pub struct ListTasksParams {
    pub channel: Option<String>,
    pub status: Option<String>,
}

// ── API DTOs ──

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct CreateTasksRequest {
    #[serde(default)]
    pub channel: String,
    pub tasks: Vec<CreateTaskItem>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct CreateTaskItem {
    pub title: String,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct ClaimTasksRequest {
    #[serde(default)]
    pub channel: String,
    pub task_numbers: Vec<i64>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct UnclaimTaskRequest {
    #[serde(default)]
    pub channel: String,
    pub task_number: i64,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct UpdateTaskStatusRequest {
    #[serde(default)]
    pub channel: String,
    pub task_number: i64,
    pub status: String,
}

// Internal bridge routes still address task boards by `#channel` target.
fn strip_channel_prefix(s: &str) -> &str {
    s.strip_prefix('#').unwrap_or(s)
}

fn load_channel_by_id(
    state: &AppState,
    conversation_id: &str,
) -> Result<Channel, (axum::http::StatusCode, Json<super::ErrorResponse>)> {
    state
        .store
        .get_channel_by_id(conversation_id)
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?
        .ok_or_else(|| app_err!(StatusCode::BAD_REQUEST, "channel not found"))
}

// ── Agent-scoped compatibility handlers ──

/// Bridge task tools still send `channel: "#name"` and identify the actor by
/// `agent_id`, so these internal routes stay in place until that contract is
/// migrated end-to-end.
pub async fn handle_list_tasks(
    State(state): State<AppState>,
    Path(_agent_id): Path<String>,
    Query(params): Query<ListTasksParams>,
) -> ApiResult<serde_json::Value> {
    let channel_target = params
        .channel
        .ok_or_else(|| app_err!(StatusCode::BAD_REQUEST, "missing channel parameter"))?;
    let channel_name = strip_channel_prefix(&channel_target);
    let status_filter = params
        .status
        .as_deref()
        .and_then(TaskStatus::from_status_str);
    let tasks = state
        .store
        .get_tasks(channel_name, status_filter)
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?;
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
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?;
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
        .update_tasks_claim(channel_name, &agent_id, &req.task_numbers)
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?;
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
        .update_task_unclaim(channel_name, &agent_id, req.task_number)
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn handle_update_task_status(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(req): Json<UpdateTaskStatusRequest>,
) -> ApiResult<serde_json::Value> {
    let channel_name = strip_channel_prefix(&req.channel);
    let new_status = TaskStatus::from_status_str(&req.status)
        .ok_or_else(|| app_err!(StatusCode::BAD_REQUEST, "invalid status: {}", req.status))?;
    state
        .store
        .update_task_status(channel_name, req.task_number, &agent_id, new_status)
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn handle_public_list_tasks(
    State(state): State<AppState>,
    Path(conversation_id): Path<String>,
    Query(params): Query<ListTasksParams>,
) -> ApiResult<serde_json::Value> {
    let channel = load_channel_by_id(&state, &conversation_id)?;
    let status_filter = params
        .status
        .as_deref()
        .and_then(TaskStatus::from_status_str);
    let tasks = state
        .store
        .get_tasks(&channel.name, status_filter)
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?;
    Ok(Json(serde_json::json!({ "tasks": tasks })))
}

pub async fn handle_public_create_tasks(
    State(state): State<AppState>,
    Path(conversation_id): Path<String>,
    Json(req): Json<CreateTasksRequest>,
) -> ApiResult<serde_json::Value> {
    let actor_id = whoami::username();
    let channel = load_channel_by_id(&state, &conversation_id)?;
    let titles: Vec<&str> = req.tasks.iter().map(|t| t.title.as_str()).collect();
    let tasks = state
        .store
        .create_tasks(&channel.name, &actor_id, &titles)
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?;
    Ok(Json(serde_json::json!({ "tasks": tasks })))
}

pub async fn handle_public_claim_tasks(
    State(state): State<AppState>,
    Path(conversation_id): Path<String>,
    Json(req): Json<ClaimTasksRequest>,
) -> ApiResult<serde_json::Value> {
    let actor_id = whoami::username();
    let channel = load_channel_by_id(&state, &conversation_id)?;
    let results = state
        .store
        .update_tasks_claim(&channel.name, &actor_id, &req.task_numbers)
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?;
    Ok(Json(serde_json::json!({ "results": results })))
}

pub async fn handle_public_unclaim_task(
    State(state): State<AppState>,
    Path(conversation_id): Path<String>,
    Json(req): Json<UnclaimTaskRequest>,
) -> ApiResult<serde_json::Value> {
    let actor_id = whoami::username();
    let channel = load_channel_by_id(&state, &conversation_id)?;
    state
        .store
        .update_task_unclaim(&channel.name, &actor_id, req.task_number)
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// `GET /api/conversations/{conversation_id}/tasks/{task_number}` — return one
/// task as `TaskInfo` (already carries `subChannelId` / `subChannelName`).
/// Missing task → 404; missing channel → 400 (same shape as sibling task
/// handlers via `load_channel_by_id`).
pub async fn handle_get_task_detail(
    State(state): State<AppState>,
    Path((conversation_id, task_number)): Path<(String, i64)>,
) -> ApiResult<TaskInfo> {
    let channel = load_channel_by_id(&state, &conversation_id)?;
    let info = state
        .store
        .get_task_info(&channel.name, task_number)
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?
        .ok_or_else(|| app_err!(StatusCode::NOT_FOUND, "task not found"))?;
    Ok(Json(info))
}

pub async fn handle_public_update_task_status(
    State(state): State<AppState>,
    Path(conversation_id): Path<String>,
    Json(req): Json<UpdateTaskStatusRequest>,
) -> ApiResult<serde_json::Value> {
    let actor_id = whoami::username();
    let channel = load_channel_by_id(&state, &conversation_id)?;
    let new_status = TaskStatus::from_status_str(&req.status)
        .ok_or_else(|| app_err!(StatusCode::BAD_REQUEST, "invalid status: {}", req.status))?;
    state
        .store
        .update_task_status(&channel.name, req.task_number, &actor_id, new_status)
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}
