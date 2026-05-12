//! Public platform workspace API.

use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::server::auth::Actor;
use crate::server::error::{app_err, internal_err, ApiResult, ErrorResponse};
use crate::server::handlers::AppState;
use crate::store::{Workspace, WorkspaceCounts, WorkspaceMode};

#[derive(Debug, Serialize)]
pub struct WorkspaceResponse {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub mode: WorkspaceMode,
    pub created_by_human_id: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub active: bool,
    pub channel_count: i64,
    pub agent_count: i64,
    pub human_count: i64,
}

#[derive(Debug, Serialize)]
pub struct DeleteWorkspaceResponse {
    pub deleted_id: String,
    pub active_workspace: Option<WorkspaceResponse>,
}

impl WorkspaceResponse {
    fn from_workspace(
        workspace: Workspace,
        active_workspace_id: Option<&str>,
        counts: WorkspaceCounts,
    ) -> Self {
        let active = active_workspace_id.is_some_and(|id| id == workspace.id);
        Self {
            id: workspace.id,
            name: workspace.name,
            slug: workspace.slug,
            mode: workspace.mode,
            created_by_human_id: workspace.created_by_human_id,
            created_at: workspace.created_at,
            active,
            channel_count: counts.channel_count,
            agent_count: counts.agent_count,
            human_count: counts.human_count,
        }
    }
}

fn workspace_response(
    state: &AppState,
    workspace: Workspace,
    active_workspace_id: Option<&str>,
) -> Result<WorkspaceResponse, (StatusCode, Json<ErrorResponse>)> {
    let counts = state
        .store
        .count_workspace_resources(&workspace.id)
        .map_err(internal_err)?;
    Ok(WorkspaceResponse::from_workspace(
        workspace,
        active_workspace_id,
        counts,
    ))
}

#[derive(Debug, Deserialize)]
pub struct CreateWorkspaceRequest {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct SwitchWorkspaceRequest {
    pub workspace: String,
}

#[derive(Debug, Deserialize)]
pub struct RenameWorkspaceRequest {
    pub name: String,
}

pub async fn handle_current_workspace(
    State(state): State<AppState>,
) -> ApiResult<WorkspaceResponse> {
    let workspace = state
        .store
        .get_active_workspace()
        .map_err(internal_err)?
        .ok_or_else(|| {
            app_err!(
                StatusCode::BAD_REQUEST,
                "no active workspace; run `chorus setup` or `chorus workspace switch <name>`"
            )
        })?;
    Ok(Json(workspace_response(
        &state,
        workspace.clone(),
        Some(&workspace.id),
    )?))
}

pub async fn handle_list_workspaces(
    State(state): State<AppState>,
) -> ApiResult<Vec<WorkspaceResponse>> {
    let active_workspace_id = state.active_workspace_id().await;
    let workspaces = state.store.list_workspaces().map_err(internal_err)?;
    Ok(Json(
        workspaces
            .into_iter()
            .map(|workspace| workspace_response(&state, workspace, active_workspace_id.as_deref()))
            .collect::<Result<Vec<_>, _>>()?,
    ))
}

pub async fn handle_create_workspace(
    State(state): State<AppState>,
    Extension(actor): Extension<Actor>,
    Json(req): Json<CreateWorkspaceRequest>,
) -> ApiResult<WorkspaceResponse> {
    let name = req.name.trim();
    if name.is_empty() {
        return Err(app_err!(
            StatusCode::BAD_REQUEST,
            "workspace name is required"
        ));
    }
    let (workspace, event) = state
        .store
        .create_local_workspace_without_activation(name, &actor.user_id)
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?;
    state.event_bus.publish_stream(event);
    let active_workspace_id = state.active_workspace_id().await;
    Ok(Json(workspace_response(
        &state,
        workspace,
        active_workspace_id.as_deref(),
    )?))
}

pub async fn handle_switch_workspace(
    State(state): State<AppState>,
    Json(req): Json<SwitchWorkspaceRequest>,
) -> ApiResult<WorkspaceResponse> {
    let selector = req.workspace.trim();
    if selector.is_empty() {
        return Err(app_err!(
            StatusCode::BAD_REQUEST,
            "workspace selector is required"
        ));
    }
    let workspace = state
        .store
        .get_workspace_by_selector(selector)
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?
        .ok_or_else(|| app_err!(StatusCode::NOT_FOUND, "workspace not found: {selector}"))?;
    state
        .store
        .set_active_workspace(&workspace.id)
        .map_err(internal_err)?;
    state
        .set_active_workspace_id(Some(workspace.id.clone()))
        .await;
    Ok(Json(workspace_response(
        &state,
        workspace.clone(),
        Some(&workspace.id),
    )?))
}

pub async fn handle_rename_current_workspace(
    State(state): State<AppState>,
    Json(req): Json<RenameWorkspaceRequest>,
) -> ApiResult<WorkspaceResponse> {
    let name = req.name.trim();
    if name.is_empty() {
        return Err(app_err!(
            StatusCode::BAD_REQUEST,
            "workspace name is required"
        ));
    }
    let active = state
        .store
        .get_active_workspace()
        .map_err(internal_err)?
        .ok_or_else(|| {
            app_err!(
                StatusCode::BAD_REQUEST,
                "no active workspace; run `chorus setup` or `chorus workspace switch <name>`"
            )
        })?;
    let workspace = state
        .store
        .rename_workspace(&active.id, name)
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?;
    Ok(Json(workspace_response(
        &state,
        workspace.clone(),
        Some(&workspace.id),
    )?))
}

pub async fn handle_delete_workspace(
    State(state): State<AppState>,
    Path(selector): Path<String>,
) -> ApiResult<DeleteWorkspaceResponse> {
    let selector = selector.trim();
    if selector.is_empty() {
        return Err(app_err!(
            StatusCode::BAD_REQUEST,
            "workspace selector is required"
        ));
    }
    let workspace = state
        .store
        .get_workspace_by_selector(selector)
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?
        .ok_or_else(|| app_err!(StatusCode::NOT_FOUND, "workspace not found: {selector}"))?;
    let deleted_id = workspace.id.clone();
    let was_active = state
        .active_workspace_id()
        .await
        .is_some_and(|id| id == deleted_id);

    state
        .store
        .delete_workspace(&deleted_id)
        .map_err(internal_err)?;

    // Remove workspace-scoped runtime directories. Because paths are now
    // scoped by workspace id, a single directory sweep per workspace is
    // sufficient and safe — same-named agents or teams in other workspaces
    // live under different parent directories.
    let agents_workspace_dir = state.agents_dir.join(&deleted_id);
    let teams_workspace_dir = state.teams_dir().join(&deleted_id);
    if agents_workspace_dir.exists() {
        let _ = tokio::fs::remove_dir_all(&agents_workspace_dir).await;
    }
    if teams_workspace_dir.exists() {
        let _ = tokio::fs::remove_dir_all(&teams_workspace_dir).await;
    }

    let active_workspace = if was_active {
        state.set_active_workspace_id(None).await;
        None
    } else {
        state
            .store
            .get_active_workspace()
            .map_err(internal_err)?
            .map(|active| workspace_response(&state, active.clone(), Some(&active.id)))
            .transpose()?
    };

    Ok(Json(DeleteWorkspaceResponse {
        deleted_id,
        active_workspace,
    }))
}
