//! Public platform workspace API.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::config::ChorusConfig;
use crate::server::error::{app_err, internal_err, ApiResult};
use crate::server::handlers::AppState;
use crate::store::{Workspace, WorkspaceMode};

#[derive(Debug, Serialize)]
pub struct WorkspaceResponse {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub mode: WorkspaceMode,
    pub created_by_human: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub active: bool,
}

#[derive(Debug, Serialize)]
pub struct DeleteWorkspaceResponse {
    pub deleted_id: String,
    pub active_workspace: Option<WorkspaceResponse>,
}

impl WorkspaceResponse {
    fn from_workspace(workspace: Workspace, active_workspace_id: Option<&str>) -> Self {
        let active = active_workspace_id.is_some_and(|id| id == workspace.id);
        Self {
            id: workspace.id,
            name: workspace.name,
            slug: workspace.slug,
            mode: workspace.mode,
            created_by_human: workspace.created_by_human,
            created_at: workspace.created_at,
            active,
        }
    }
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
                "no active workspace; run `chorus setup` or `chorus workspace create <name>`"
            )
        })?;
    Ok(Json(WorkspaceResponse::from_workspace(
        workspace.clone(),
        Some(&workspace.id),
    )))
}

pub async fn handle_list_workspaces(
    State(state): State<AppState>,
) -> ApiResult<Vec<WorkspaceResponse>> {
    let active_workspace_id = state.active_workspace_id().map_err(internal_err)?;
    let workspaces = state.store.list_workspaces().map_err(internal_err)?;
    Ok(Json(
        workspaces
            .into_iter()
            .map(|workspace| {
                WorkspaceResponse::from_workspace(workspace, active_workspace_id.as_deref())
            })
            .collect(),
    ))
}

pub async fn handle_create_workspace(
    State(state): State<AppState>,
    Json(req): Json<CreateWorkspaceRequest>,
) -> ApiResult<WorkspaceResponse> {
    let name = req.name.trim();
    if name.is_empty() {
        return Err(app_err!(
            StatusCode::BAD_REQUEST,
            "workspace name is required"
        ));
    }
    let human = local_human_for_store(&state);
    let workspace = state
        .store
        .create_local_workspace(name, &human)
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?;
    state
        .set_active_workspace_id(Some(workspace.id.clone()))
        .map_err(internal_err)?;
    Ok(Json(WorkspaceResponse::from_workspace(
        workspace.clone(),
        Some(&workspace.id),
    )))
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
        .map_err(internal_err)?;
    Ok(Json(WorkspaceResponse::from_workspace(
        workspace.clone(),
        Some(&workspace.id),
    )))
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
                "no active workspace; run `chorus setup` or `chorus workspace create <name>`"
            )
        })?;
    let workspace = state
        .store
        .rename_workspace(&active.id, name)
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?;
    Ok(Json(WorkspaceResponse::from_workspace(
        workspace.clone(),
        Some(&workspace.id),
    )))
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
        .map_err(internal_err)?
        .is_some_and(|id| id == deleted_id);

    state
        .store
        .delete_workspace(&deleted_id)
        .map_err(internal_err)?;

    let active_workspace = if was_active {
        let next = state
            .store
            .list_workspaces()
            .map_err(internal_err)?
            .into_iter()
            .next();
        match next {
            Some(next_workspace) => {
                state
                    .store
                    .set_active_workspace(&next_workspace.id)
                    .map_err(internal_err)?;
                state
                    .set_active_workspace_id(Some(next_workspace.id.clone()))
                    .map_err(internal_err)?;
                Some(WorkspaceResponse::from_workspace(
                    next_workspace.clone(),
                    Some(&next_workspace.id),
                ))
            }
            None => {
                state.set_active_workspace_id(None).map_err(internal_err)?;
                None
            }
        }
    } else {
        state
            .store
            .get_active_workspace()
            .map_err(internal_err)?
            .map(|active| WorkspaceResponse::from_workspace(active.clone(), Some(&active.id)))
    };

    Ok(Json(DeleteWorkspaceResponse {
        deleted_id,
        active_workspace,
    }))
}

fn local_human_for_store(state: &AppState) -> String {
    let data_dir = state
        .store
        .data_dir()
        .parent()
        .unwrap_or_else(|| state.store.data_dir());
    ChorusConfig::load(data_dir)
        .ok()
        .flatten()
        .and_then(|cfg| cfg.local_human.name)
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(whoami::username)
}
