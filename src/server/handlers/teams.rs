use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};

use super::path_params::{PublicResourceIdPath, TeamMemberPath};
use super::{app_err, internal_err, ApiResult, AppState};
use crate::agent::workspace::{AgentWorkspace, TeamWorkspace};
use crate::server::error::AppErrorCode;
use crate::store::channels::normalize_channel_name;
use crate::store::messages::SenderType;
use crate::store::teams::{Team, TeamMember};

#[derive(Debug, Deserialize)]
pub struct CreateTeamRequest {
    pub name: String,
    pub display_name: String,
    pub collaboration_model: Option<String>,
    pub leader_agent_name: Option<String>,
    #[serde(default)]
    pub members: Vec<CreateTeamMemberRequest>,
}

#[derive(Debug, Deserialize)]
pub struct CreateTeamMemberRequest {
    pub member_name: String,
    pub member_type: String,
    pub member_id: String,
    pub role: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateTeamRequest {
    pub display_name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AddMemberRequest {
    pub member_name: String,
    pub member_type: String,
    pub member_id: String,
    pub role: String,
}

#[derive(Debug, Serialize)]
pub struct TeamResponse {
    pub team: Team,
    pub members: Vec<TeamMember>,
}

fn load_team_by_public_id(
    state: &AppState,
    id: &str,
) -> Result<Team, (axum::http::StatusCode, Json<super::ErrorResponse>)> {
    state
        .store
        .get_team_by_id(id)
        .map_err(internal_err)?
        .ok_or_else(|| app_err!(StatusCode::BAD_REQUEST, "team not found"))
}

fn parse_member_type(
    member_type: &str,
) -> Result<SenderType, (axum::http::StatusCode, Json<super::ErrorResponse>)> {
    match member_type {
        "agent" => Ok(SenderType::Agent),
        "human" => Ok(SenderType::Human),
        _ => Err(app_err!(
            StatusCode::BAD_REQUEST,
            "member_type must be 'agent' or 'human'"
        )),
    }
}

async fn sync_team_roles_and_agents(
    state: &AppState,
    team: &Team,
    members: &[TeamMember],
) -> Result<(), (axum::http::StatusCode, Json<super::ErrorResponse>)> {
    let agents_dir = state.agents_dir.clone();
    let agent_workspace = AgentWorkspace::new(&agents_dir);

    for member in members {
        if member.member_type == "agent" {
            agent_workspace
                .set_team_role(&member.member_name, &team.name, &member.role)
                .map_err(internal_err)?;
            restart_agent_member(state, &member.member_name).await?;
        }
    }

    Ok(())
}

/// Restart an agent so its system prompt is rebuilt from the latest team state.
///
/// For bridge-hosted agents (`agents.machine_id` set), the platform does not
/// own the runtime; it broadcasts a fresh `bridge.target` instead so the
/// remote bridge can stop/start the local process. Calling `start_agent`
/// here would cause dual-runtime contention.
async fn restart_agent_member(
    state: &AppState,
    agent_name: &str,
) -> Result<(), (axum::http::StatusCode, Json<super::ErrorResponse>)> {
    let agent = match state.store.get_agent(agent_name).map_err(internal_err)? {
        Some(a) => a,
        None => return Ok(()),
    };
    if agent.machine_id.is_some() {
        crate::server::transport::bridge_ws::broadcast_target_update(
            state.store.as_ref(),
            state.bridge_registry.as_ref(),
        );
        return Ok(());
    }
    state
        .lifecycle
        .stop_agent(&agent.id)
        .await
        .map_err(internal_err)?;
    state
        .lifecycle
        .start_agent(&agent, None, None)
        .await
        .map_err(internal_err)?;
    Ok(())
}

pub async fn handle_create_team(
    State(state): State<AppState>,
    Json(req): Json<CreateTeamRequest>,
) -> ApiResult<TeamResponse> {
    let name = normalize_channel_name(&req.name);
    if name.is_empty() {
        return Err(app_err!(StatusCode::BAD_REQUEST, "name is required"));
    }
    let display_name = req.display_name.trim();
    if display_name.is_empty() {
        return Err(app_err!(
            StatusCode::BAD_REQUEST,
            "display_name is required"
        ));
    }

    // Reject duplicate (member_type, member_id) pairs and creator collisions
    // upfront to prevent DB/fs state divergence (PK is (team_id, member_type,
    // member_id)).
    // TODO: This handler is not atomic — late-stage failures (e.g. agent
    // restart) leave DB records behind without rolling back FS state.
    let local_human_id = state.local_human_id.clone();
    let mut seen_member_keys = std::collections::HashSet::new();
    let mut creator_in_members = false;
    for member in &req.members {
        parse_member_type(&member.member_type)?;
        let key = (member.member_type.clone(), member.member_id.clone());
        if !seen_member_keys.insert(key) {
            return Err(app_err!(
                StatusCode::BAD_REQUEST,
                format!(
                    "duplicate member: {} ({})",
                    member.member_name, member.member_id
                )
            ));
        }
        if member.member_type == "human" && member.member_id == local_human_id {
            creator_in_members = true;
        }
    }

    let active_workspace_id = state.active_workspace_id().await;
    let (team_id, team_channel_id) = match active_workspace_id.as_deref() {
        Some(workspace_id) => state.store.create_team_with_channel_in_workspace(
            workspace_id,
            &name,
            display_name,
            req.collaboration_model.as_deref().unwrap_or_default(),
            req.leader_agent_name.as_deref(),
        ),
        None => state.store.create_team_with_channel(
            &name,
            display_name,
            req.collaboration_model.as_deref().unwrap_or_default(),
            req.leader_agent_name.as_deref(),
        ),
    }
    .map_err(|e| {
        let msg = e.to_string();
        if msg.contains("UNIQUE constraint") {
            app_err!(AppErrorCode::TeamNameTaken, "team name already in use")
        } else {
            app_err!(StatusCode::BAD_REQUEST, msg)
        }
    })?;

    // Auto-join the configured local human to the team channel and add them
    // as a team member unless they already included themselves in the
    // explicit members list. Identity is keyed by `humans.id`, not by OS
    // username — the server stops trusting `whoami::username()` for request
    // identity entirely.
    if !creator_in_members {
        let (_, events) = state
            .store
            .join_channel_by_id(&team_channel_id, &local_human_id, SenderType::Human)
            .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?;
        for event in events {
            state.event_bus.publish_stream(event);
        }
        state
            .store
            .create_team_member(&team_id, &local_human_id, "human", "operator")
            .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?;
    }

    let teams_dir = state.teams_dir();
    let agents_dir = state.agents_dir.clone();
    let team_workspace = TeamWorkspace::new(teams_dir);
    let agent_workspace = AgentWorkspace::new(&agents_dir);

    let agent_member_names = req
        .members
        .iter()
        .filter(|member| member.member_type == "agent")
        .map(|member| member.member_name.as_str())
        .collect::<Vec<_>>();
    team_workspace
        .init_team(&name, &agent_member_names)
        .map_err(internal_err)?;

    for member in &req.members {
        let sender_type = parse_member_type(&member.member_type)?;
        state
            .store
            .create_team_member(
                &team_id,
                &member.member_id,
                &member.member_type,
                &member.role,
            )
            .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?;
        let (_, events) = state
            .store
            .join_channel_by_id(&team_channel_id, &member.member_id, sender_type)
            .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?;
        for event in events {
            state.event_bus.publish_stream(event);
        }

        if sender_type == SenderType::Agent {
            agent_workspace
                .init_team_memory(&member.member_name, &name, &member.role)
                .map_err(internal_err)?;
            restart_agent_member(&state, &member.member_name).await?;
        }
    }

    let team = state
        .store
        .get_team_by_id(&team_id)
        .map_err(internal_err)?
        .ok_or_else(|| {
            app_err!(
                StatusCode::INTERNAL_SERVER_ERROR,
                "team not found after create: {name}"
            )
        })?;
    let members = state
        .store
        .get_team_members(&team_id)
        .map_err(internal_err)?;
    Ok(Json(TeamResponse { team, members }))
}

pub async fn handle_list_teams(State(state): State<AppState>) -> ApiResult<Vec<Team>> {
    let active_workspace_id = state.active_workspace_id().await;
    let teams = state
        .store
        .get_teams_for_workspace(active_workspace_id.as_deref())
        .map_err(internal_err)?;
    Ok(Json(teams))
}

pub async fn handle_get_team(
    State(state): State<AppState>,
    Path(PublicResourceIdPath { id }): Path<PublicResourceIdPath>,
) -> ApiResult<TeamResponse> {
    let team = load_team_by_public_id(&state, &id)?;
    let members = state
        .store
        .get_team_members(&team.id)
        .map_err(internal_err)?;
    Ok(Json(TeamResponse { team, members }))
}

pub async fn handle_update_team(
    State(state): State<AppState>,
    Path(PublicResourceIdPath { id }): Path<PublicResourceIdPath>,
    Json(req): Json<UpdateTeamRequest>,
) -> ApiResult<Team> {
    let team = load_team_by_public_id(&state, &id)?;

    let display_name = req
        .display_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(&team.display_name)
        .to_string();
    state
        .store
        .update_team(
            &team.id,
            &display_name,
            &team.collaboration_model,
            team.leader_agent_name.as_deref(),
        )
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?;

    let updated = state
        .store
        .get_team_by_id(&team.id)
        .map_err(internal_err)?
        .ok_or_else(|| {
            app_err!(
                StatusCode::INTERNAL_SERVER_ERROR,
                "team not found after update: {}",
                team.id
            )
        })?;
    let members = state
        .store
        .get_team_members(&team.id)
        .map_err(internal_err)?;
    sync_team_roles_and_agents(&state, &updated, &members).await?;
    Ok(Json(updated))
}

pub async fn handle_delete_team(
    State(state): State<AppState>,
    Path(PublicResourceIdPath { id }): Path<PublicResourceIdPath>,
) -> ApiResult<serde_json::Value> {
    let team = load_team_by_public_id(&state, &id)?;
    let members = state
        .store
        .get_team_members(&team.id)
        .map_err(internal_err)?;
    let agent_members = members
        .iter()
        .filter(|member| member.member_type == "agent")
        .map(|member| member.member_name.clone())
        .collect::<Vec<_>>();

    state.store.delete_team(&team.id).map_err(internal_err)?;

    if let Some(channel) = state
        .store
        .get_channel_by_workspace_and_name(&team.workspace_id, &team.name)
        .map_err(internal_err)?
    {
        state
            .store
            .archive_channel(&channel.id)
            .map_err(internal_err)?;
    }

    let team_workspace = TeamWorkspace::new(state.teams_dir());
    team_workspace
        .delete_team(&team.name)
        .map_err(internal_err)?;

    let agents_dir = state.agents_dir.clone();
    let agent_workspace = AgentWorkspace::new(&agents_dir);
    for agent_name in &agent_members {
        // Agent may have been deleted already — skip cleanup for missing agents.
        if state.store.get_agent(agent_name).ok().flatten().is_none() {
            continue;
        }
        agent_workspace
            .delete_team_memory(agent_name, &team.name)
            .map_err(internal_err)?;
        restart_agent_member(&state, agent_name).await?;
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn handle_add_team_member(
    State(state): State<AppState>,
    Path(PublicResourceIdPath { id }): Path<PublicResourceIdPath>,
    Json(req): Json<AddMemberRequest>,
) -> ApiResult<serde_json::Value> {
    let team = load_team_by_public_id(&state, &id)?;
    let sender_type = parse_member_type(&req.member_type)?;

    state
        .store
        .create_team_member(&team.id, &req.member_id, &req.member_type, &req.role)
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?;
    let (_, events) = state
        .store
        .join_channel_by_id(
            team.channel_id
                .as_deref()
                .ok_or_else(|| app_err!(StatusCode::BAD_REQUEST, "team channel not found"))?,
            &req.member_id,
            sender_type,
        )
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?;
    for event in events {
        state.event_bus.publish_stream(event);
    }

    if sender_type == SenderType::Agent {
        let team_workspace = TeamWorkspace::new(state.teams_dir());
        team_workspace
            .init_member(&team.name, &req.member_name)
            .map_err(internal_err)?;
        let agents_dir = state.agents_dir.clone();
        let agent_workspace = AgentWorkspace::new(&agents_dir);
        agent_workspace
            .init_team_memory(&req.member_name, &team.name, &req.role)
            .map_err(internal_err)?;
    }

    let updated_team = state
        .store
        .get_team_by_id(&team.id)
        .map_err(internal_err)?
        .ok_or_else(|| {
            app_err!(
                StatusCode::INTERNAL_SERVER_ERROR,
                "team not found after add member: {}",
                team.id
            )
        })?;
    let members = state
        .store
        .get_team_members(&team.id)
        .map_err(internal_err)?;
    sync_team_roles_and_agents(&state, &updated_team, &members).await?;

    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn handle_remove_team_member(
    State(state): State<AppState>,
    Path(TeamMemberPath { id, member }): Path<TeamMemberPath>,
) -> ApiResult<serde_json::Value> {
    let team = load_team_by_public_id(&state, &id)?;

    let members = state
        .store
        .get_team_members(&team.id)
        .map_err(internal_err)?;
    // The URL accepts a name as a friendly handle. Resolve it to the stored
    // (member_type, member_id) tuple before issuing any deletes — `member` is
    // a label and may be ambiguous if humans/agents ever share a name (PK is
    // (member_type, member_id), so they technically can).
    let removed_member = members
        .iter()
        .find(|member_item| member_item.member_name == member)
        .cloned()
        .ok_or_else(|| app_err!(StatusCode::BAD_REQUEST, "team member not found: {member}"))?;

    state
        .store
        .delete_team_member(
            &team.id,
            &removed_member.member_id,
            &removed_member.member_type,
        )
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?;
    state
        .store
        .leave_channel(
            &team.name,
            &removed_member.member_id,
            &removed_member.member_type,
        )
        .map_err(internal_err)?;

    if removed_member.member_type == "agent" {
        restart_agent_member(&state, &removed_member.member_name).await?;
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}
