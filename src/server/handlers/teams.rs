use axum::extract::{Path, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{api_err, internal_err, ApiResult, AppState};
use crate::agent::workspace::{AgentWorkspace, TeamWorkspace};
use crate::server::handlers::channels::normalize_channel_name;
use crate::store::channels::ChannelType;
use crate::store::messages::SenderType;
use crate::store::teams::{Team, TeamMember};

#[derive(Debug, Deserialize)]
pub struct CreateTeamRequest {
    pub name: String,
    pub display_name: String,
    pub collaboration_model: String,
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
    pub collaboration_model: Option<String>,
    pub leader_agent_name: Option<Option<String>>,
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

fn validate_collaboration_model(
    model: &str,
) -> Result<(), (axum::http::StatusCode, Json<super::ErrorResponse>)> {
    if matches!(model, "leader_operators" | "swarm") {
        Ok(())
    } else {
        Err(api_err(
            "collaboration_model must be 'leader_operators' or 'swarm'",
        ))
    }
}

fn validate_team_configuration(
    collaboration_model: &str,
    leader_agent_name: Option<&str>,
) -> Result<(), (axum::http::StatusCode, Json<super::ErrorResponse>)> {
    validate_collaboration_model(collaboration_model)?;
    match (collaboration_model, leader_agent_name) {
        ("leader_operators", None) => Err(api_err(
            "leader_agent_name is required for leader_operators teams",
        )),
        ("swarm", Some(_)) => Err(api_err("leader_agent_name must be null for swarm teams")),
        _ => Ok(()),
    }
}

fn parse_member_type(
    member_type: &str,
) -> Result<SenderType, (axum::http::StatusCode, Json<super::ErrorResponse>)> {
    match member_type {
        "agent" => Ok(SenderType::Agent),
        "human" => Ok(SenderType::Human),
        _ => Err(api_err("member_type must be 'agent' or 'human'")),
    }
}

fn canonical_team_role(
    member_name: &str,
    member_type: &str,
    current_role: &str,
    collaboration_model: &str,
    leader_agent_name: Option<&str>,
) -> String {
    if member_type != "agent" {
        return current_role.to_string();
    }
    if collaboration_model == "leader_operators" && leader_agent_name == Some(member_name) {
        return "leader".to_string();
    }
    "operator".to_string()
}

async fn sync_team_roles_and_agents(
    state: &AppState,
    team: &Team,
    members: &[TeamMember],
) -> Result<(), (axum::http::StatusCode, Json<super::ErrorResponse>)> {
    let agents_dir = state.store.agents_dir();
    let agent_workspace = AgentWorkspace::new(&agents_dir);

    for member in members {
        let desired_role = canonical_team_role(
            &member.member_name,
            &member.member_type,
            &member.role,
            &team.collaboration_model,
            team.leader_agent_name.as_deref(),
        );
        if desired_role != member.role {
            state
                .store
                .update_team_member_role(&team.id, &member.member_name, &desired_role)
                .map_err(|e| api_err(e.to_string()))?;
        }

        if member.member_type == "agent" {
            agent_workspace
                .set_team_role(&member.member_name, &team.name, &desired_role)
                .map_err(|e| internal_err(e.to_string()))?;
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
        .map_err(|e| internal_err(e.to_string()))?;
    state
        .lifecycle
        .start_agent(agent_name, None)
        .await
        .map_err(|e| internal_err(e.to_string()))?;
    Ok(())
}

pub async fn handle_create_team(
    State(state): State<AppState>,
    Json(req): Json<CreateTeamRequest>,
) -> ApiResult<TeamResponse> {
    let name = normalize_channel_name(&req.name);
    if name.is_empty() {
        return Err(api_err("name is required"));
    }
    let display_name = req.display_name.trim();
    if display_name.is_empty() {
        return Err(api_err("display_name is required"));
    }
    validate_team_configuration(&req.collaboration_model, req.leader_agent_name.as_deref())?;

    let team_id = state
        .store
        .create_team(
            &name,
            display_name,
            &req.collaboration_model,
            req.leader_agent_name.as_deref(),
        )
        .map_err(|e| api_err(e.to_string()))?;

    state
        .store
        .create_channel(&name, None, ChannelType::Team)
        .map_err(|e| api_err(e.to_string()))?;

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
        .map_err(|e| internal_err(e.to_string()))?;

    for member in &req.members {
        let sender_type = parse_member_type(&member.member_type)?;
        let effective_role = canonical_team_role(
            &member.member_name,
            &member.member_type,
            &member.role,
            &req.collaboration_model,
            req.leader_agent_name.as_deref(),
        );
        state
            .store
            .create_team_member(
                &team_id,
                &member.member_name,
                &member.member_type,
                &member.member_id,
                &effective_role,
            )
            .map_err(|e| api_err(e.to_string()))?;
        state
            .store
            .join_channel(&name, &member.member_name, sender_type)
            .map_err(|e| api_err(e.to_string()))?;

        if sender_type == SenderType::Agent {
            agent_workspace
                .init_team_memory(&member.member_name, &name, &effective_role)
                .map_err(|e| internal_err(e.to_string()))?;
            restart_agent_member(&state, &member.member_name).await?;
        }
    }

    let team = state
        .store
        .get_team(&name)
        .map_err(|e| internal_err(e.to_string()))?
        .ok_or_else(|| internal_err(format!("team not found after create: {name}")))?;
    let members = state
        .store
        .get_team_members(&team_id)
        .map_err(|e| internal_err(e.to_string()))?;
    let username = whoami::username();
    let _ = state.store.record_workspace_event(
        "team.updated",
        team.channel_id
            .as_deref()
            .map(|channel_id| (channel_id, team.name.as_str())),
        Some(username.as_str()),
        Some(SenderType::Human.as_str()),
        Some("create_team"),
        json!({
            "action": "created",
            "teamId": team.id,
            "teamName": team.name,
            "displayName": team.display_name,
            "channelId": team.channel_id,
        }),
    );
    Ok(Json(TeamResponse { team, members }))
}

pub async fn handle_list_teams(State(state): State<AppState>) -> ApiResult<Vec<Team>> {
    let teams = state
        .store
        .get_teams()
        .map_err(|e| internal_err(e.to_string()))?;
    Ok(Json(teams))
}

pub async fn handle_get_team(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<TeamResponse> {
    let team = state
        .store
        .get_team(&name)
        .map_err(|e| internal_err(e.to_string()))?
        .ok_or_else(|| api_err(format!("team not found: {name}")))?;
    let members = state
        .store
        .get_team_members(&team.id)
        .map_err(|e| internal_err(e.to_string()))?;
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
        .map_err(|e| internal_err(e.to_string()))?
        .ok_or_else(|| api_err(format!("team not found: {name}")))?;

    let display_name = req
        .display_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(&team.display_name)
        .to_string();
    let collaboration_model = req
        .collaboration_model
        .as_deref()
        .unwrap_or(&team.collaboration_model)
        .to_string();
    let leader_agent_name = if collaboration_model == "swarm" {
        None
    } else {
        req.leader_agent_name
            .unwrap_or_else(|| team.leader_agent_name.clone())
    };

    validate_team_configuration(&collaboration_model, leader_agent_name.as_deref())?;

    state
        .store
        .update_team(
            &team.id,
            &display_name,
            &collaboration_model,
            leader_agent_name.as_deref(),
        )
        .map_err(|e| api_err(e.to_string()))?;

    let updated = state
        .store
        .get_team(&name)
        .map_err(|e| internal_err(e.to_string()))?
        .ok_or_else(|| internal_err(format!("team not found after update: {name}")))?;
    let members = state
        .store
        .get_team_members(&team.id)
        .map_err(|e| internal_err(e.to_string()))?;
    sync_team_roles_and_agents(&state, &updated, &members).await?;
    let username = whoami::username();
    let _ = state.store.record_workspace_event(
        "team.updated",
        updated
            .channel_id
            .as_deref()
            .map(|channel_id| (channel_id, updated.name.as_str())),
        Some(username.as_str()),
        Some(SenderType::Human.as_str()),
        Some("update_team"),
        json!({
            "action": "updated",
            "teamId": updated.id,
            "teamName": updated.name,
            "displayName": updated.display_name,
            "channelId": updated.channel_id,
        }),
    );
    Ok(Json(updated))
}

pub async fn handle_delete_team(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<serde_json::Value> {
    let team = state
        .store
        .get_team(&name)
        .map_err(|e| internal_err(e.to_string()))?
        .ok_or_else(|| api_err(format!("team not found: {name}")))?;
    let members = state
        .store
        .get_team_members(&team.id)
        .map_err(|e| internal_err(e.to_string()))?;
    let agent_members = members
        .iter()
        .filter(|member| member.member_type == "agent")
        .map(|member| member.member_name.clone())
        .collect::<Vec<_>>();

    state
        .store
        .delete_team(&team.id)
        .map_err(|e| internal_err(e.to_string()))?;

    if let Some(channel) = state
        .store
        .get_channel_by_name(&name)
        .map_err(|e| internal_err(e.to_string()))?
    {
        state
            .store
            .archive_channel(&channel.id)
            .map_err(|e| internal_err(e.to_string()))?;
    }

    let team_workspace = TeamWorkspace::new(state.store.teams_dir());
    team_workspace
        .delete_team(&name)
        .map_err(|e| internal_err(e.to_string()))?;

    let agents_dir = state.store.agents_dir();
    let agent_workspace = AgentWorkspace::new(&agents_dir);
    for agent_name in &agent_members {
        agent_workspace
            .delete_team_memory(agent_name, &name)
            .map_err(|e| internal_err(e.to_string()))?;
        restart_agent_member(&state, agent_name).await?;
    }
    let username = whoami::username();
    let _ = state.store.record_workspace_event(
        "team.updated",
        team.channel_id
            .as_deref()
            .map(|channel_id| (channel_id, team.name.as_str())),
        Some(username.as_str()),
        Some(SenderType::Human.as_str()),
        Some("delete_team"),
        json!({
            "action": "deleted",
            "teamId": team.id,
            "teamName": team.name,
        }),
    );

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
        .map_err(|e| internal_err(e.to_string()))?
        .ok_or_else(|| api_err(format!("team not found: {name}")))?;
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
        .map_err(|e| api_err(e.to_string()))?;
    state
        .store
        .join_channel(&name, &req.member_name, sender_type)
        .map_err(|e| api_err(e.to_string()))?;

    if sender_type == SenderType::Agent {
        let team_workspace = TeamWorkspace::new(state.store.teams_dir());
        team_workspace
            .init_member(&name, &req.member_name)
            .map_err(|e| internal_err(e.to_string()))?;
        let agents_dir = state.store.agents_dir();
        let agent_workspace = AgentWorkspace::new(&agents_dir);
        agent_workspace
            .init_team_memory(&req.member_name, &name, &req.role)
            .map_err(|e| internal_err(e.to_string()))?;
    }

    let updated_team = state
        .store
        .get_team(&name)
        .map_err(|e| internal_err(e.to_string()))?
        .ok_or_else(|| internal_err(format!("team not found after add member: {name}")))?;
    let members = state
        .store
        .get_team_members(&team.id)
        .map_err(|e| internal_err(e.to_string()))?;
    sync_team_roles_and_agents(&state, &updated_team, &members).await?;
    let username = whoami::username();
    let _ = state.store.record_workspace_event(
        "team.updated",
        updated_team
            .channel_id
            .as_deref()
            .map(|channel_id| (channel_id, updated_team.name.as_str())),
        Some(username.as_str()),
        Some(SenderType::Human.as_str()),
        Some("add_team_member"),
        json!({
            "action": "member_joined",
            "teamId": updated_team.id,
            "teamName": updated_team.name,
            "memberName": req.member_name,
        }),
    );

    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn handle_remove_team_member(
    State(state): State<AppState>,
    Path((name, member_name)): Path<(String, String)>,
) -> ApiResult<serde_json::Value> {
    let team = state
        .store
        .get_team(&name)
        .map_err(|e| internal_err(e.to_string()))?
        .ok_or_else(|| api_err(format!("team not found: {name}")))?;

    let members = state
        .store
        .get_team_members(&team.id)
        .map_err(|e| internal_err(e.to_string()))?;
    let removed_member = members
        .iter()
        .find(|member| member.member_name == member_name)
        .cloned()
        .ok_or_else(|| api_err(format!("team member not found: {member_name}")))?;

    state
        .store
        .delete_team_member(&team.id, &member_name)
        .map_err(|e| api_err(e.to_string()))?;
    state
        .store
        .leave_channel(&name, &member_name)
        .map_err(|e| internal_err(e.to_string()))?;

    if removed_member.member_type == "agent" {
        restart_agent_member(&state, &member_name).await?;
    }
    let username = whoami::username();
    let _ = state.store.record_workspace_event(
        "team.updated",
        team.channel_id
            .as_deref()
            .map(|channel_id| (channel_id, team.name.as_str())),
        Some(username.as_str()),
        Some(SenderType::Human.as_str()),
        Some("remove_team_member"),
        json!({
            "action": "member_left",
            "teamId": team.id,
            "teamName": team.name,
            "memberName": member_name,
        }),
    );

    Ok(Json(serde_json::json!({ "ok": true })))
}
