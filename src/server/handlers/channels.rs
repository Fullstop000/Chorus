use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};

use super::{api_err, ApiResult, AppState};
use crate::store::channels::{Channel, ChannelMemberProfile, ChannelType};
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

#[derive(Deserialize)]
pub struct InviteChannelMemberRequest {
    #[serde(rename = "memberName")]
    pub member_name: String,
}

#[derive(Serialize)]
pub struct ChannelMemberInfo {
    #[serde(rename = "memberName")]
    pub member_name: String,
    #[serde(rename = "memberType")]
    pub member_type: SenderType,
    #[serde(rename = "displayName", skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

#[derive(Serialize)]
pub struct ChannelMembersResponse {
    #[serde(rename = "channelId")]
    pub channel_id: String,
    #[serde(rename = "memberCount")]
    pub member_count: usize,
    pub members: Vec<ChannelMemberInfo>,
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

fn channel_member_info(profile: ChannelMemberProfile) -> ChannelMemberInfo {
    ChannelMemberInfo {
        member_name: profile.member_name,
        member_type: profile.member_type,
        display_name: profile.display_name,
    }
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
    Ok(Json(serde_json::json!({ "id": channel_id, "name": name })))
}

pub async fn handle_list_channel_members(
    State(state): State<AppState>,
    Path(channel_id): Path<String>,
) -> ApiResult<ChannelMembersResponse> {
    let channel = state
        .store
        .find_channel_by_id(&channel_id)
        .map_err(|e| api_err(e.to_string()))?
        .ok_or_else(|| api_err("channel not found"))?;
    if channel.channel_type == ChannelType::Dm {
        return Err(api_err("dm channels are not supported by this endpoint"));
    }

    let members = state
        .store
        .get_channel_member_profiles(&channel_id)
        .map_err(|e| api_err(e.to_string()))?
        .into_iter()
        .map(channel_member_info)
        .collect::<Vec<_>>();

    Ok(Json(ChannelMembersResponse {
        channel_id,
        member_count: members.len(),
        members,
    }))
}

pub async fn handle_invite_channel_member(
    State(state): State<AppState>,
    Path(channel_id): Path<String>,
    Json(req): Json<InviteChannelMemberRequest>,
) -> ApiResult<ChannelMembersResponse> {
    let channel = validate_channel_mutation(&state, &channel_id)?;
    let member_name = req.member_name.trim();
    if member_name.is_empty() {
        return Err(api_err("memberName is required"));
    }
    let member_type = state
        .store
        .lookup_sender_type(member_name)
        .map_err(|e| api_err(e.to_string()))?
        .ok_or_else(|| api_err(format!("member not found: {member_name}")))?;

    state
        .store
        .join_channel_by_id(&channel.id, member_name, member_type)
        .map_err(|e| api_err(e.to_string()))?;

    let members = state
        .store
        .get_channel_member_profiles(&channel_id)
        .map_err(|e| api_err(e.to_string()))?
        .into_iter()
        .map(channel_member_info)
        .collect::<Vec<_>>();

    Ok(Json(ChannelMembersResponse {
        channel_id,
        member_count: members.len(),
        members,
    }))
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
