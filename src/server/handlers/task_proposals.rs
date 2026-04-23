//! HTTP handlers for task proposals — GET, accept, dismiss.
//!
//! The accept handler MUST call `deliver_message_to_agents` after the store
//! commits, because Chorus does NOT auto-wake agents on new messages. The
//! kickoff system message posted inside the acceptance transaction is the
//! trigger that must be dispatched to the proposer agent (now a member of
//! the new sub-channel) to start a run in the task's sub-channel.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};

use super::{ApiResult, AppState};
use crate::server::error::{app_err, AppErrorCode, ErrorResponse};
use crate::store::task_proposals::{AcceptedTaskProposal, TaskProposal};

// ── Request bodies ───────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct AcceptBody {
    /// Member name accepting the proposal. Must be a member of the parent
    /// channel. (v1: enforcement relaxed to any channel member — humans
    /// drive this UI; agents don't click buttons.)
    pub accepter: String,
}

#[derive(Debug, Deserialize)]
pub struct DismissBody {
    pub resolver: String,
}

// ── Response DTOs ────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct AcceptResponse {
    #[serde(rename = "taskNumber")]
    pub task_number: i64,
    #[serde(rename = "subChannelId")]
    pub sub_channel_id: String,
    #[serde(rename = "subChannelName")]
    pub sub_channel_name: String,
}

impl From<AcceptedTaskProposal> for AcceptResponse {
    fn from(a: AcceptedTaskProposal) -> Self {
        Self {
            task_number: a.task_number,
            sub_channel_id: a.sub_channel_id,
            sub_channel_name: a.sub_channel_name,
        }
    }
}

/// HTTP view of a task proposal. Field names align with the chat-message
/// `task_proposal` snapshot payload (`taskNumber`, `subChannelId`,
/// `proposedAt`, etc.) so the frontend can share parsing logic between
/// the HTTP and WebSocket surfaces.
///
/// Known omission: `subChannelName` is NOT in this view (the backing
/// `TaskProposal` row doesn't persist it; snapshots derive it via
/// `format!("{parent}__task-{n}")` during the accept tx). If a frontend
/// surface needs the name from this view, reconstruct it at the call site
/// or add `accepted_sub_channel_name` to the DB row first.
#[derive(Debug, Serialize)]
pub struct ProposalView {
    pub id: String,
    #[serde(rename = "channelId")]
    pub channel_id: String,
    #[serde(rename = "proposedBy")]
    pub proposed_by: String,
    pub title: String,
    pub status: String,
    #[serde(rename = "proposedAt")]
    pub created_at: String,
    #[serde(rename = "taskNumber")]
    pub accepted_task_number: Option<i64>,
    #[serde(rename = "subChannelId")]
    pub accepted_sub_channel_id: Option<String>,
    #[serde(rename = "resolvedBy")]
    pub resolved_by: Option<String>,
    #[serde(rename = "resolvedAt")]
    pub resolved_at: Option<String>,
}

impl From<TaskProposal> for ProposalView {
    fn from(p: TaskProposal) -> Self {
        Self {
            id: p.id,
            channel_id: p.channel_id,
            proposed_by: p.proposed_by,
            title: p.title,
            status: p.status.as_str().to_string(),
            created_at: p.created_at.to_rfc3339(),
            accepted_task_number: p.accepted_task_number,
            accepted_sub_channel_id: p.accepted_sub_channel_id,
            resolved_by: p.resolved_by,
            resolved_at: p.resolved_at.map(|t| t.to_rfc3339()),
        }
    }
}

// ── Handlers ─────────────────────────────────────────────────────────────────

pub async fn get_task_proposal(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<ProposalView> {
    let proposal = state
        .store
        .get_task_proposal_by_id(&id)
        .map_err(|e| app_err!(StatusCode::INTERNAL_SERVER_ERROR, "store error: {e}"))?
        .ok_or_else(|| app_err!(StatusCode::NOT_FOUND, "proposal not found: {id}"))?;
    Ok(Json(proposal.into()))
}

pub async fn accept_task_proposal(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<AcceptBody>,
) -> ApiResult<AcceptResponse> {
    let accepted = state
        .store
        .accept_task_proposal(&id, &body.accepter)
        .map_err(|e| {
            let msg = format!("{e}");
            if msg.contains("not found") {
                app_err!(StatusCode::NOT_FOUND, "{msg}")
            } else if msg.contains("not pending") {
                app_err!(
                    AppErrorCode::TaskProposalAlreadyResolved,
                    "proposal already resolved: {id}"
                )
            } else {
                app_err!(StatusCode::INTERNAL_SERVER_ERROR, "store error: {msg}")
            }
        })?;

    // Chorus does NOT auto-wake agents on new messages — every handler that
    // wants an agent to start a run MUST invoke the dispatcher explicitly.
    // The precedent is `messages.rs:339` and `:405`. Here, the kickoff
    // system message posted inside the acceptance tx is the trigger; we
    // fan it out to the proposer agent (the one member of the new
    // sub-channel who's an agent). Dispatch errors are reported to the
    // caller because a proposal that creates a task but fails to wake the
    // agent leaves the user staring at a silent sub-channel — worse than
    // an error shown up-front.
    crate::server::handlers::messages::deliver_message_to_agents(
        &state,
        &accepted.sub_channel_id,
        "system",
        &accepted.kickoff_message_id,
    )
    .await
    .map_err(|e| app_err!(StatusCode::INTERNAL_SERVER_ERROR, "wake dispatch: {e}"))?;

    Ok(Json(accepted.into()))
}

pub async fn dismiss_task_proposal(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<DismissBody>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    state
        .store
        .dismiss_task_proposal(&id, &body.resolver)
        .map_err(|e| {
            let msg = format!("{e}");
            if msg.contains("not found") {
                app_err!(StatusCode::NOT_FOUND, "{msg}")
            } else if msg.contains("not pending") {
                app_err!(
                    AppErrorCode::TaskProposalAlreadyResolved,
                    "proposal already resolved: {id}"
                )
            } else {
                app_err!(StatusCode::INTERNAL_SERVER_ERROR, "store error: {msg}")
            }
        })?;
    Ok(StatusCode::NO_CONTENT)
}
