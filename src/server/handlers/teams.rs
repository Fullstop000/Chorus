use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};

use super::{app_err, internal_err, ApiResult, AppState};
use crate::agent::workspace::{AgentWorkspace, TeamWorkspace};
use crate::server::error::AppErrorCode;
use crate::server::handlers::channels::normalize_channel_name;
use crate::store::channels::ChannelType;
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
    let agents_dir = state.store.agents_dir();
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
async fn restart_agent_member(
    state: &AppState,
    agent_name: &str,
) -> Result<(), (axum::http::StatusCode, Json<super::ErrorResponse>)> {
    state
        .lifecycle
        .stop_agent(agent_name)
        .await
        .map_err(internal_err)?;
    state
        .lifecycle
        .start_agent(agent_name, None)
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

    let team_id = state
        .store
        .create_team(
            &name,
            display_name,
            req.collaboration_model.as_deref().unwrap_or_default(),
            req.leader_agent_name.as_deref(),
        )
        .map_err(|e| {
            let msg = e.to_string();
            if msg.contains("UNIQUE constraint") {
                app_err!(AppErrorCode::TeamNameTaken, "team name already in use")
            } else {
                app_err!(StatusCode::BAD_REQUEST, msg)
            }
        })?;

    state
        .store
        .create_channel(&name, None, ChannelType::Team)
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?;

    let teams_dir = state.store.teams_dir();
    let agents_dir = state.store.agents_dir();
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
                &member.member_name,
                &member.member_type,
                &member.member_id,
                &member.role,
            )
            .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?;
        state
            .store
            .join_channel(&name, &member.member_name, sender_type)
            .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?;

        if sender_type == SenderType::Agent {
            agent_workspace
                .init_team_memory(&member.member_name, &name, &member.role)
                .map_err(internal_err)?;
            restart_agent_member(&state, &member.member_name).await?;
        }
    }

    let team = state
        .store
        .get_team(&name)
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
    let teams = state.store.get_teams().map_err(internal_err)?;
    Ok(Json(teams))
}

pub async fn handle_get_team(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<TeamResponse> {
    let team = state
        .store
        .get_team(&name)
        .map_err(internal_err)?
        .ok_or_else(|| app_err!(StatusCode::BAD_REQUEST, "team not found: {name}"))?;
    let members = state
        .store
        .get_team_members(&team.id)
        .map_err(internal_err)?;
    Ok(Json(TeamResponse { team, members }))
}

pub async fn handle_update_team(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(req): Json<UpdateTeamRequest>,
) -> ApiResult<Team> {
    let team = state
        .store
        .get_team(&name)
        .map_err(internal_err)?
        .ok_or_else(|| app_err!(StatusCode::BAD_REQUEST, "team not found: {name}"))?;

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
        .get_team(&name)
        .map_err(internal_err)?
        .ok_or_else(|| {
            app_err!(
                StatusCode::INTERNAL_SERVER_ERROR,
                "team not found after update: {name}"
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
    Path(name): Path<String>,
) -> ApiResult<serde_json::Value> {
    let team = state
        .store
        .get_team(&name)
        .map_err(internal_err)?
        .ok_or_else(|| app_err!(StatusCode::BAD_REQUEST, "team not found: {name}"))?;
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
        .get_channel_by_name(&name)
        .map_err(internal_err)?
    {
        state
            .store
            .archive_channel(&channel.id)
            .map_err(internal_err)?;
    }

    let team_workspace = TeamWorkspace::new(state.store.teams_dir());
    team_workspace.delete_team(&name).map_err(internal_err)?;

    let agents_dir = state.store.agents_dir();
    let agent_workspace = AgentWorkspace::new(&agents_dir);
    for agent_name in &agent_members {
        agent_workspace
            .delete_team_memory(agent_name, &name)
            .map_err(internal_err)?;
        restart_agent_member(&state, agent_name).await?;
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn handle_add_team_member(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(req): Json<AddMemberRequest>,
) -> ApiResult<serde_json::Value> {
    let team = state
        .store
        .get_team(&name)
        .map_err(internal_err)?
        .ok_or_else(|| app_err!(StatusCode::BAD_REQUEST, "team not found: {name}"))?;
    let sender_type = parse_member_type(&req.member_type)?;

    state
        .store
        .create_team_member(
            &team.id,
            &req.member_name,
            &req.member_type,
            &req.member_id,
            &req.role,
        )
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?;
    state
        .store
        .join_channel(&name, &req.member_name, sender_type)
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?;

    if sender_type == SenderType::Agent {
        let team_workspace = TeamWorkspace::new(state.store.teams_dir());
        team_workspace
            .init_member(&name, &req.member_name)
            .map_err(internal_err)?;
        let agents_dir = state.store.agents_dir();
        let agent_workspace = AgentWorkspace::new(&agents_dir);
        agent_workspace
            .init_team_memory(&req.member_name, &name, &req.role)
            .map_err(internal_err)?;
    }

    let updated_team = state
        .store
        .get_team(&name)
        .map_err(internal_err)?
        .ok_or_else(|| {
            app_err!(
                StatusCode::INTERNAL_SERVER_ERROR,
                "team not found after add member: {name}"
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
    Path((name, member_name)): Path<(String, String)>,
) -> ApiResult<serde_json::Value> {
    let team = state
        .store
        .get_team(&name)
        .map_err(internal_err)?
        .ok_or_else(|| app_err!(StatusCode::BAD_REQUEST, "team not found: {name}"))?;

    let members = state
        .store
        .get_team_members(&team.id)
        .map_err(internal_err)?;
    let removed_member = members
        .iter()
        .find(|member| member.member_name == member_name)
        .cloned()
        .ok_or_else(|| {
            app_err!(
                StatusCode::BAD_REQUEST,
                "team member not found: {member_name}"
            )
        })?;

    state
        .store
        .delete_team_member(&team.id, &member_name)
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?;
    state
        .store
        .leave_channel(&name, &member_name)
        .map_err(internal_err)?;

    if removed_member.member_type == "agent" {
        restart_agent_member(&state, &member_name).await?;
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}
