use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;

use super::{api_err, ApiResult, AppState};
use crate::store::tasks::TaskStatus;

// ── Inline query structs ──

#[derive(Deserialize)]
pub struct ListTasksParams {
    pub channel: Option<String>,
    pub status: Option<String>,
}

// ── API DTOs ──

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct CreateTasksRequest {
    pub channel: String,
    pub tasks: Vec<CreateTaskItem>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct CreateTaskItem {
    pub title: String,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct ClaimTasksRequest {
    pub channel: String,
    pub task_numbers: Vec<i64>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct UnclaimTaskRequest {
    pub channel: String,
    pub task_number: i64,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct UpdateTaskStatusRequest {
    pub channel: String,
    pub task_number: i64,
    pub status: String,
}

// Duplicate the trivial helper locally to avoid a cross-module dependency.
fn strip_channel_prefix(s: &str) -> &str {
    s.strip_prefix('#').unwrap_or(s)
}

// ── Public handlers ──

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
