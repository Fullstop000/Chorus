//! Decision-inbox HTTP handlers.
//!
//! Three endpoints:
//! - `POST /internal/agent/{agent_id}/decisions` — bridge calls this when an
//!   agent invokes the `chorus_create_decision` MCP tool.
//! - `GET  /api/decisions` — UI lists decisions in the active workspace.
//! - `POST /api/decisions/{id}/resolve` — UI resolves a decision; server
//!   builds the envelope and resumes the agent's runtime session.
//!
//! See `docs/DECISIONS.md` for the full lifecycle. v1 ships the minimum
//! mechanism per r7; backoff/retry, terminal `delivery_failed`, per-session
//! FIFO queues, and access control beyond workspace scope are deferred.
//!
//! Wire format note: this module returns and accepts **snake_case** JSON
//! to keep the MCP and UI sides on the same shape. Other `/api/*` routes
//! use camelCase via Public… wrappers; decisions deliberately diverge so
//! `ui/src/data/decisions.ts` mirrors `src/decision/types.rs` directly
//! (drift-tested via `src/decision/fixtures/payload.json`).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use super::{app_err, ApiResult, AppState};
use crate::decision::{validate, DecisionPayload, ResolvePayload};
use crate::store::decisions::{DecisionRow, DecisionStatus};
use crate::utils::error::internal_err;

// ─────────────────────────────────────────────────────────────────────────
// Wire shapes
// ─────────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ListDecisionsQuery {
    /// Filter by status. `"open"` (default) or `"resolved"`.
    pub status: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DecisionView {
    pub id: String,
    pub workspace_id: String,
    pub channel_id: String,
    pub agent_id: String,
    pub session_id: String,
    pub created_at: String,
    pub status: &'static str,
    pub payload: DecisionPayload,
    pub picked_key: Option<String>,
    pub picked_note: Option<String>,
    pub resolved_at: Option<String>,
}

impl DecisionView {
    fn from_row(row: DecisionRow) -> anyhow::Result<Self> {
        let payload = row.payload()?;
        Ok(Self {
            id: row.id,
            workspace_id: row.workspace_id,
            channel_id: row.channel_id,
            agent_id: row.agent_id,
            session_id: row.session_id,
            created_at: row.created_at.to_rfc3339(),
            status: row.status.as_str(),
            payload,
            picked_key: row.picked_key,
            picked_note: row.picked_note,
            resolved_at: row.resolved_at.map(|t| t.to_rfc3339()),
        })
    }
}

#[derive(Debug, Serialize)]
pub struct ListDecisionsResponse {
    pub decisions: Vec<DecisionView>,
}

#[derive(Debug, Serialize)]
pub struct CreateDecisionResponse {
    pub decision_id: String,
}

#[derive(Debug, Serialize)]
pub struct ResolveDecisionResponse {
    pub decision: DecisionView,
}

// ─────────────────────────────────────────────────────────────────────────
// Internal: agent → bridge → server (create)
// ─────────────────────────────────────────────────────────────────────────

/// Called by the MCP bridge when an agent invokes `chorus_create_decision`.
/// The bridge has already run `crate::decision::validate(&payload)` at its
/// boundary; we re-validate here as defense-in-depth in case a future
/// caller hits the route directly.
pub async fn handle_create_decision(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(payload): Json<DecisionPayload>,
) -> ApiResult<CreateDecisionResponse> {
    validate(&payload).map_err(|e| app_err!(StatusCode::BAD_REQUEST, "{e}"))?;

    let store = &state.store;
    let agent = store
        .get_agent(&agent_id)
        .map_err(internal_err)?
        .ok_or_else(|| app_err!(StatusCode::BAD_REQUEST, "agent not found: {agent_id}"))?;

    let workspace_id = agent.workspace_id.clone();

    // Channel inference: r7 contract — the agent must be on a
    // channel-triggered run for chorus_create_decision to know where to
    // file the decision. If no active-run channel, return 400 loudly so
    // the agent's retry path can surface the misconfiguration.
    let channel_id = state
        .lifecycle
        .run_channel_id(&agent.name)
        .ok_or_else(|| {
            app_err!(
                StatusCode::BAD_REQUEST,
                "no active-run channel for agent {agent_id}; chorus_create_decision \
                 requires a channel-triggered agent run (r7 v1 channel-inference contract)"
            )
        })?;

    // Pull the agent's active runtime session so the resolve handler
    // can route the envelope back. v1 requires a live session; if the
    // agent has none yet, we surface that as a 400 — there's nothing
    // useful we can do with a decision that has no session to wake.
    let session = store
        .get_active_session(&agent.id)
        .map_err(internal_err)?
        .ok_or_else(|| {
            app_err!(
                StatusCode::BAD_REQUEST,
                "agent {agent_id} has no active runtime session; cannot route \
                 resolution back without one"
            )
        })?;

    let row = store
        .create_decision(
            &workspace_id,
            &channel_id,
            &agent.id,
            &session.session_id,
            &payload,
        )
        .map_err(internal_err)?;

    info!(
        decision_id = %row.id,
        agent = %agent.name,
        channel = %channel_id,
        "decision created"
    );

    Ok(Json(CreateDecisionResponse {
        decision_id: row.id,
    }))
}

// ─────────────────────────────────────────────────────────────────────────
// Public: human reads the inbox
// ─────────────────────────────────────────────────────────────────────────

pub async fn handle_list_decisions(
    State(state): State<AppState>,
    Query(q): Query<ListDecisionsQuery>,
) -> ApiResult<ListDecisionsResponse> {
    let workspace_id = state
        .active_workspace_id()
        .await
        .ok_or_else(|| app_err!(StatusCode::BAD_REQUEST, "no active workspace"))?;

    let status_filter = match q.status.as_deref() {
        Some("open") | None => Some(DecisionStatus::Open),
        Some("resolved") => Some(DecisionStatus::Resolved),
        Some("all") => None,
        Some(other) => {
            return Err(app_err!(
                StatusCode::BAD_REQUEST,
                "unknown status filter: {other}; valid values are 'open', 'resolved', 'all'"
            ))
        }
    };

    let rows = state
        .store
        .list_decisions(&workspace_id, status_filter)
        .map_err(internal_err)?;
    let decisions: Result<Vec<_>, _> = rows.into_iter().map(DecisionView::from_row).collect();
    let decisions = decisions.map_err(internal_err)?;
    Ok(Json(ListDecisionsResponse { decisions }))
}

// ─────────────────────────────────────────────────────────────────────────
// Public: human picks an option
// ─────────────────────────────────────────────────────────────────────────

pub async fn handle_resolve_decision(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(resolve): Json<ResolvePayload>,
) -> ApiResult<ResolveDecisionResponse> {
    // Pre-check: load the row so we can validate the picked_key against
    // the original options before issuing the CAS. A picked_key that
    // doesn't match any option is a 400; CAS races (already resolved)
    // are 409.
    let row = state
        .store
        .get_decision(&id)
        .map_err(internal_err)?
        .ok_or_else(|| app_err!(StatusCode::NOT_FOUND, "decision not found: {id}"))?;

    let payload = row.payload().map_err(internal_err)?;
    if !payload.options.iter().any(|o| o.key == resolve.picked_key) {
        return Err(app_err!(
            StatusCode::BAD_REQUEST,
            "picked_key '{}' does not match any option of decision {id}",
            resolve.picked_key
        ));
    }

    // Snapshot the agent identity + picked option for envelope building
    // before the CAS — the row goes through the resolve transition and
    // we want to send the envelope with the human's picked label/body.
    let agent_id = row.agent_id.clone();
    let session_id = row.session_id.clone();
    let picked_option = payload
        .options
        .iter()
        .find(|o| o.key == resolve.picked_key)
        .cloned()
        .expect("just validated above");
    let headline = payload.headline.clone();
    let question = payload.question.clone();

    // CAS update. Returns None on race (409) and Some(updated_row) on success.
    let updated = state
        .store
        .resolve_decision_cas(&id, &resolve)
        .map_err(internal_err)?;
    let updated = match updated {
        Some(row) => row,
        None => {
            return Err(app_err!(
                StatusCode::CONFLICT,
                "decision {id} already resolved (or row missing)"
            ))
        }
    };

    // Look up the agent name for the resume call. AgentLifecycle keys
    // by name; the store row keeps the agent UUID.
    let agent = state
        .store
        .get_agent_by_id(&agent_id, false)
        .map_err(internal_err)?
        .ok_or_else(|| {
            app_err!(
                StatusCode::INTERNAL_SERVER_ERROR,
                "agent {agent_id} no longer exists; can't resume"
            )
        })?;

    let envelope = build_envelope(
        &updated.id,
        &headline,
        &question,
        &picked_option,
        resolve.note.as_deref(),
    );

    info!(
        decision_id = %updated.id,
        agent = %agent.name,
        session = %session_id,
        picked = %resolve.picked_key,
        "delivering resolution envelope"
    );

    if let Err(err) = state.lifecycle.resume_with_prompt(&agent.name, envelope).await {
        // Resume failure is real: revert the row to Open so the human
        // can re-pick after they fix the underlying issue (driver
        // crashed, runtime not installed, etc.). Loud failure per
        // CLAUDE.md root-cause principle — no silent retries here.
        warn!(
            decision_id = %updated.id,
            agent = %agent.name,
            err = %err,
            "resume_with_prompt failed; reverting decision to Open"
        );
        if let Err(revert_err) = state.store.revert_decision_to_open(&updated.id) {
            warn!(
                decision_id = %updated.id,
                err = %revert_err,
                "follow-up revert also failed; row is now Resolved with no delivery"
            );
        }
        return Err(app_err!(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to deliver envelope to agent: {err}"
        ));
    }

    let view = DecisionView::from_row(updated).map_err(internal_err)?;
    Ok(Json(ResolveDecisionResponse { decision: view }))
}

/// Build the self-contained envelope text per r7. The agent reads this
/// as a regular ACP turn prompt, parses the structured fields, and acts.
fn build_envelope(
    decision_id: &str,
    headline: &str,
    question: &str,
    picked: &crate::decision::OptionPayload,
    note: Option<&str>,
) -> String {
    let note_line = note
        .filter(|n| !n.is_empty())
        .map(|n| format!("  note: {n}\n"))
        .unwrap_or_default();
    format!(
        "[chorus] Decision {decision_id} resolved.\n\
         You created this earlier:\n\
         \x20\x20headline: {headline}\n\
         \x20\x20question: {question}\n\
         The human picked [{key}]:\n\
         \x20\x20label: {label}\n\
         \x20\x20action: {body}\n\
         {note_line}\
         Please proceed.",
        decision_id = decision_id,
        headline = headline,
        question = question,
        key = picked.key,
        label = picked.label,
        body = picked.body,
        note_line = note_line,
    )
}
