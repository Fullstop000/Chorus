use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use super::{app_err, AppState, ErrorResponse};
use crate::store::agents::Agent;

#[derive(Debug, Deserialize)]
pub struct PublicResourceIdPath {
    pub id: String,
}

#[derive(Debug, Deserialize)]
pub struct TeamMemberPath {
    pub id: String,
    pub member: String,
}

pub fn resolve_public_agent(
    state: &AppState,
    id: &str,
) -> Result<Agent, (StatusCode, Json<ErrorResponse>)> {
    state
        .store
        .get_agent_by_id(id)
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?
        .ok_or_else(|| app_err!(StatusCode::BAD_REQUEST, "agent not found"))
}
