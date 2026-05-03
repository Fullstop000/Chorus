use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};

use super::{app_err, ApiResult, AppState};
use crate::server::error::AppErrorCode;
use crate::store::channels::{
    is_valid_channel_name, normalize_channel_name, Channel, ChannelMemberProfile, ChannelType,
    INVALID_CHANNEL_NAME_MSG,
};
use crate::store::messages::SenderType;
use crate::store::ChannelListParams;

use super::dto::ChannelInfo;
use super::server_info::channel_infos_for;

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

#[derive(Debug, Deserialize, Default)]
pub struct ListChannelsQuery {
    pub member: Option<String>,
    #[serde(default)]
    pub include_archived: bool,
    #[serde(default)]
    pub include_dm: bool,
    #[serde(default)]
    pub include_system: bool,
    #[serde(default = "default_include_team")]
    pub include_team: bool,
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

fn default_include_team() -> bool {
    true
}

// ── Private helpers ──

pub(super) fn validate_channel_mutation(
    state: &AppState,
    channel_id: &str,
) -> Result<Channel, (StatusCode, Json<super::ErrorResponse>)> {
    let channel = state
        .store
        .get_channel_by_id(channel_id)
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?
        .ok_or_else(|| app_err!(StatusCode::BAD_REQUEST, "channel not found"))?;
    if channel.channel_type != ChannelType::Channel {
        return Err(app_err!(
            AppErrorCode::ChannelOperationUnsupported,
            "only user channels can be modified"
        ));
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

pub async fn handle_list_channels(
    State(state): State<AppState>,
    Query(query): Query<ListChannelsQuery>,
) -> ApiResult<Vec<ChannelInfo>> {
    let member = query.member.unwrap_or_else(|| state.local_human_id.clone());
    let active_workspace_id = state.active_workspace_id().await;
    let channels = channel_infos_for(
        state.store.as_ref(),
        &ChannelListParams {
            workspace_id: active_workspace_id.as_deref(),
            for_member: Some(member.as_str()),
            include_archived: query.include_archived,
            include_dm: query.include_dm,
            include_system: query.include_system,
            include_team: query.include_team,
            include_tasks: false,
        },
    )
    .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?;
    Ok(Json(channels))
}

pub async fn handle_create_channel(
    State(state): State<AppState>,
    Json(req): Json<CreateChannelRequest>,
) -> ApiResult<serde_json::Value> {
    let name = normalize_channel_name(&req.name);
    if !is_valid_channel_name(&name) {
        return Err(app_err!(StatusCode::BAD_REQUEST, INVALID_CHANNEL_NAME_MSG));
    }
    let description = if req.description.trim().is_empty() {
        None
    } else {
        Some(req.description.trim())
    };
    let active_workspace_id = state.active_workspace_id().await;
    let channel_id = match active_workspace_id.as_deref() {
        Some(workspace_id) => state.store.create_channel_in_workspace(
            workspace_id,
            &name,
            description,
            ChannelType::Channel,
            None,
        ),
        None => state
            .store
            .create_channel(&name, description, ChannelType::Channel, None),
    }
    .map_err(|e| {
        let msg = e.to_string();
        if msg.contains("UNIQUE constraint") {
            app_err!(
                AppErrorCode::ChannelNameTaken,
                "channel name already in use"
            )
        } else {
            app_err!(StatusCode::BAD_REQUEST, msg)
        }
    })?;
    let (_, events) = state
        .store
        .join_channel_by_id(&channel_id, &state.local_human_id, SenderType::Human)
        .unwrap_or_default();
    for event in events {
        state.event_bus.publish_stream(event);
    }
    Ok(Json(serde_json::json!({ "id": channel_id, "name": name })))
}

pub async fn handle_list_channel_members(
    State(state): State<AppState>,
    Path(channel_id): Path<String>,
) -> ApiResult<ChannelMembersResponse> {
    let channel = state
        .store
        .get_channel_by_id(&channel_id)
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?
        .ok_or_else(|| app_err!(StatusCode::BAD_REQUEST, "channel not found"))?;
    if channel.channel_type == ChannelType::Dm {
        return Err(app_err!(
            AppErrorCode::ChannelOperationUnsupported,
            "dm channels are not supported by this endpoint"
        ));
    }

    let members = state
        .store
        .get_channel_member_profiles(&channel_id)
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?
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
        return Err(app_err!(StatusCode::BAD_REQUEST, "memberName is required"));
    }
    // Resolve the explicit `memberName` API field into the canonical
    // (id, type) pair; `channel_members` is keyed by immutable id.
    let (member_id, member_type) = state
        .store
        .lookup_sender_by_name(member_name)
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?
        .ok_or_else(|| app_err!(StatusCode::BAD_REQUEST, "member not found: {member_name}"))?;

    let (_, events) = state
        .store
        .join_channel_by_id(&channel.id, &member_id, member_type)
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?;
    for event in events {
        state.event_bus.publish_stream(event);
    }

    let members = state
        .store
        .get_channel_member_profiles(&channel_id)
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?
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
    if !is_valid_channel_name(&name) {
        return Err(app_err!(StatusCode::BAD_REQUEST, INVALID_CHANNEL_NAME_MSG));
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
            .get_channel_by_workspace_and_name(&channel.workspace_id, &name)
            .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?
            .is_some()
    {
        return Err(app_err!(
            AppErrorCode::ChannelNameTaken,
            "channel already exists: {name}"
        ));
    }

    state
        .store
        .update_channel(&channel_id, &name, description)
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?;

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
    let _ = validate_channel_mutation(&state, &channel_id)?;
    state
        .store
        .archive_channel(&channel_id)
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn handle_delete_channel(
    State(state): State<AppState>,
    Path(channel_id): Path<String>,
) -> ApiResult<serde_json::Value> {
    let _ = validate_channel_mutation(&state, &channel_id)?;
    state
        .store
        .delete_channel(&channel_id)
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}
