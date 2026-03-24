use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use super::{api_err, ApiResult, AppState};
use crate::store::channels::{Channel, ChannelType};
use crate::store::messages::SenderType;

// ── Inline structs ──

#[derive(Deserialize)]
pub struct CreateChannelRequest {
    pub name: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Deserialize)]
pub struct UpdateChannelRequest {
    pub name: String,
    #[serde(default)]
    pub description: String,
}

// ── Private helpers ──

pub(super) fn normalize_channel_name(raw: &str) -> String {
    raw.trim().trim_start_matches('#').trim().to_lowercase()
}

pub(super) fn validate_channel_mutation(
    state: &AppState,
    channel_id: &str,
) -> Result<Channel, (StatusCode, Json<super::ErrorResponse>)> {
    let channel = state
        .store
        .find_channel_by_id(channel_id)
        .map_err(|e| api_err(e.to_string()))?
        .ok_or_else(|| api_err("channel not found"))?;
    if channel.channel_type != ChannelType::Channel {
        return Err(api_err("only user channels can be modified"));
    }
    Ok(channel)
}

// ── Public handlers ──

pub async fn handle_create_channel(
    State(state): State<AppState>,
    Json(req): Json<CreateChannelRequest>,
) -> ApiResult<serde_json::Value> {
    let name = normalize_channel_name(&req.name);
    if name.is_empty() {
        return Err(api_err("name is required"));
    }
    let description = if req.description.trim().is_empty() {
        None
    } else {
        Some(req.description.trim())
    };
    let channel_id = state
        .store
        .create_channel(&name, description, ChannelType::Channel)
        .map_err(|e| api_err(e.to_string()))?;
    let username = whoami::username();
    let _ = state
        .store
        .join_channel(&name, &username, SenderType::Human);
    for agent in state.store.list_agents().unwrap_or_default() {
        let _ = state
            .store
            .join_channel(&name, &agent.name, SenderType::Agent);
    }
    Ok(Json(serde_json::json!({ "id": channel_id, "name": name })))
}

pub async fn handle_update_channel(
    State(state): State<AppState>,
    Path(channel_id): Path<String>,
    Json(req): Json<UpdateChannelRequest>,
) -> ApiResult<serde_json::Value> {
    let channel = validate_channel_mutation(&state, &channel_id)?;
    let name = normalize_channel_name(&req.name);
    if name.is_empty() {
        return Err(api_err("name is required"));
    }
    let description = if req.description.trim().is_empty() {
        None
    } else {
        Some(req.description.trim())
    };

    // Avoid spurious unique-key failures when keeping the same logical name.
    if name != channel.name
        && state
            .store
            .find_channel_by_name(&name)
            .map_err(|e| api_err(e.to_string()))?
            .is_some()
    {
        return Err(api_err(format!("channel already exists: {name}")));
    }

    state
        .store
        .update_channel(&channel_id, &name, description)
        .map_err(|e| api_err(e.to_string()))?;

    Ok(Json(serde_json::json!({
        "id": channel_id,
        "name": name,
        "description": description,
    })))
}

pub async fn handle_archive_channel(
    State(state): State<AppState>,
    Path(channel_id): Path<String>,
) -> ApiResult<serde_json::Value> {
    validate_channel_mutation(&state, &channel_id)?;
    state
        .store
        .archive_channel(&channel_id)
        .map_err(|e| api_err(e.to_string()))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn handle_delete_channel(
    State(state): State<AppState>,
    Path(channel_id): Path<String>,
) -> ApiResult<serde_json::Value> {
    validate_channel_mutation(&state, &channel_id)?;
    state
        .store
        .delete_channel(&channel_id)
        .map_err(|e| api_err(e.to_string()))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}
