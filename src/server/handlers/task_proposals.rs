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
use crate::store::task_proposals::{
    truncate_excerpt, AcceptedTaskProposal, CreateTaskProposalInput, TaskProposal,
};

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

    // v2 snapshot fields. These mirror the pending-card chat-message payload
    // so the frontend can share parsing logic between HTTP reads and WS
    // events. Only four of the six DB snapshot columns cross the wire:
    // `snapshot_sender_type` and `snapshotted_at` are internal/audit.
    #[serde(rename = "sourceMessageId")]
    pub source_message_id: Option<String>,
    #[serde(rename = "snapshotSenderName")]
    pub snapshot_sender_name: Option<String>,
    #[serde(rename = "snapshotExcerpt")]
    pub snapshot_excerpt: Option<String>,
    #[serde(rename = "snapshotCreatedAt")]
    pub snapshot_created_at: Option<String>,
}

impl From<TaskProposal> for ProposalView {
    fn from(p: TaskProposal) -> Self {
        // The excerpt derives from `snapshot_content` ALONE — independent of
        // `source_message_id`. A proposal whose source message was deleted
        // (pointer NULL after ON DELETE SET NULL, snapshot still populated)
        // must still serialize `snapshotExcerpt` so the UI can render the
        // frozen evidence even when the jump-to-source link is gone.
        let snapshot_excerpt = p.snapshot_content.as_deref().map(truncate_excerpt);
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
            source_message_id: p.source_message_id,
            snapshot_sender_name: p.snapshot_sender_name,
            snapshot_excerpt,
            snapshot_created_at: p.snapshot_created_at,
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
    // Gate on parent-channel membership BEFORE mutating. Without this,
    // anyone who knows a pending proposal id can resolve it as any
    // identity they like — materializing a task + sub-channel in a
    // channel they don't belong to, and routing the kickoff to a
    // wake-dispatch they shouldn't trigger. Load the proposal once to
    // resolve its channel, then reuse the same membership helper the
    // message-send path uses so the failure mode is uniform across
    // agent-authored and human-authored writes in this subsystem.
    let proposal = state
        .store
        .get_task_proposal_by_id(&id)
        .map_err(|e| app_err!(StatusCode::INTERNAL_SERVER_ERROR, "store error: {e}"))?
        .ok_or_else(|| app_err!(StatusCode::NOT_FOUND, "proposal not found: {id}"))?;
    let channel = state
        .store
        .get_channel_by_id(&proposal.channel_id)
        .map_err(|e| app_err!(StatusCode::INTERNAL_SERVER_ERROR, "store error: {e}"))?
        .ok_or_else(|| {
            app_err!(
                StatusCode::INTERNAL_SERVER_ERROR,
                "proposal's channel vanished: {}",
                proposal.channel_id
            )
        })?;
    crate::server::handlers::messages::require_channel_membership(
        &state,
        &body.accepter,
        &channel,
        &channel.name,
    )?;

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

#[derive(Debug, Deserialize)]
pub struct InternalProposeBody {
    pub title: String,
    /// v2: id of the user message the agent read when deciding to propose.
    /// Required — the store snapshots this message's content + sender into
    /// the proposal row so the per-task session gets the originating
    /// request as immutable kickoff context. A missing/foreign message id
    /// returns 400 with `TASK_PROPOSAL_SOURCE_MESSAGE_NOT_FOUND`.
    #[serde(rename = "sourceMessageId")]
    pub source_message_id: String,
}

/// Internal endpoint the MCP bridge hits from the `propose_task` tool.
///
/// The bridge addresses the channel by name because that's what the agent
/// sees in the user's message — we resolve to an id here. Error shape
/// mirrors sibling handlers: store errors surface as 500, a missing channel
/// is 404, and title validation failures (e.g. empty title) bubble up as
/// 400 by string-sniffing the anyhow message, same pattern as
/// `accept_task_proposal`.
pub async fn internal_agent_propose(
    State(state): State<AppState>,
    Path((agent, channel_name)): Path<(String, String)>,
    Json(body): Json<InternalProposeBody>,
) -> ApiResult<ProposalView> {
    let channel = state
        .store
        .get_channel_by_name(&channel_name)
        .map_err(|e| app_err!(StatusCode::INTERNAL_SERVER_ERROR, "store error: {e}"))?
        .ok_or_else(|| app_err!(StatusCode::NOT_FOUND, "channel not found: {channel_name}"))?;
    // Mirror the membership precondition every other agent-authored write
    // enforces. Without this, an agent could mint a proposal (and its
    // kickoff side-effects on accept) in any channel merely by naming it —
    // bypassing the core invariant that agents only act inside channels
    // they're members of. Same error code as the messaging path so clients
    // can handle both surfaces uniformly.
    crate::server::handlers::messages::require_channel_membership(
        &state,
        &agent,
        &channel,
        &channel_name,
    )?;
    let proposal = state
        .store
        .create_task_proposal(CreateTaskProposalInput {
            channel_id: &channel.id,
            proposed_by: &agent,
            title: &body.title,
            source_message_id: &body.source_message_id,
        })
        .map_err(|e| {
            let msg = format!("{e}");
            // Tightened from "source message" to the full phrase the store
            // emits — "source message" alone could accidentally match future
            // unrelated errors (e.g. a "source message type mismatch" 500).
            if msg.contains("source message not found") {
                app_err!(
                    AppErrorCode::TaskProposalSourceMessageNotFound,
                    "source message not found in channel"
                )
            } else if msg.contains("title") {
                app_err!(StatusCode::BAD_REQUEST, "{msg}")
            } else {
                app_err!(StatusCode::INTERNAL_SERVER_ERROR, "{msg}")
            }
        })?;
    Ok(Json(proposal.into()))
}

pub async fn dismiss_task_proposal(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<DismissBody>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    // Same membership precondition as accept: a resolver who isn't in
    // the parent channel shouldn't be able to terminate a pending
    // proposal and post its dismissed-state snapshot to a conversation
    // they don't belong to. See the accept path for the full rationale.
    let proposal = state
        .store
        .get_task_proposal_by_id(&id)
        .map_err(|e| app_err!(StatusCode::INTERNAL_SERVER_ERROR, "store error: {e}"))?
        .ok_or_else(|| app_err!(StatusCode::NOT_FOUND, "proposal not found: {id}"))?;
    let channel = state
        .store
        .get_channel_by_id(&proposal.channel_id)
        .map_err(|e| app_err!(StatusCode::INTERNAL_SERVER_ERROR, "store error: {e}"))?
        .ok_or_else(|| {
            app_err!(
                StatusCode::INTERNAL_SERVER_ERROR,
                "proposal's channel vanished: {}",
                proposal.channel_id
            )
        })?;
    crate::server::handlers::messages::require_channel_membership(
        &state,
        &body.resolver,
        &channel,
        &channel.name,
    )?;

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

#[cfg(test)]
mod view_tests {
    use super::*;
    use crate::store::task_proposals::TaskProposalStatus;
    use chrono::TimeZone;

    /// Covers the deleted-source edge case: the DB's `ON DELETE SET NULL`
    /// on `source_message_id` nulls the navigation pointer, but the five
    /// snapshot fields survive untouched (DB CHECK demands all-or-nothing,
    /// and the delete touches only the pointer). The projected view must
    /// still emit `snapshotExcerpt` — deriving it from `snapshot_content`
    /// without consulting `source_message_id`.
    #[test]
    fn proposal_view_emits_excerpt_when_pointer_is_null() {
        let p = TaskProposal {
            id: "p1".into(),
            channel_id: "c1".into(),
            proposed_by: "claude".into(),
            title: "t".into(),
            status: TaskProposalStatus::Pending,
            created_at: chrono::Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            accepted_task_number: None,
            accepted_sub_channel_id: None,
            resolved_by: None,
            resolved_at: None,
            source_message_id: None, // pointer was NULL'd after ON DELETE SET NULL
            snapshot_sender_name: Some("alice".into()),
            snapshot_sender_type: Some("human".into()),
            snapshot_content: Some("please fix login".into()),
            snapshot_created_at: Some("2026-01-01T00:00:00Z".into()),
            snapshotted_at: Some("2026-01-01T00:00:00Z".into()),
        };
        let view: ProposalView = p.into();
        assert!(view.source_message_id.is_none());
        assert_eq!(view.snapshot_sender_name.as_deref(), Some("alice"));
        assert_eq!(view.snapshot_excerpt.as_deref(), Some("please fix login"));
        assert_eq!(
            view.snapshot_created_at.as_deref(),
            Some("2026-01-01T00:00:00Z")
        );
    }
}
