//! Decision Inbox handlers.
//!
//! Lifecycle:
//! - `POST /internal/agent/{id}/decisions` — agent emits via bridge.
//!   Channel is inferred from the agent's active run via
//!   `lifecycle.run_channel_id()`. Validator already ran at the bridge
//!   boundary; this handler stores the row.
//! - `GET /api/decisions?status=open|resolved|all` — UI lists decisions
//!   for the active workspace.
//! - `POST /api/decisions/{id}/resolve` — human picks an option. CAS
//!   updates the row, builds a self-contained envelope, and calls
//!   `lifecycle.resume_with_prompt(agent, envelope)`. On delivery
//!   failure, reverts the row to `open` so the pick isn't lost.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{info, warn};
use uuid::Uuid;

use super::AppState;
use crate::server::error::{app_err, ApiResult};
use crate::store::{DecisionRow, DecisionStatus};

// ── Internal: agent emits a decision ──────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CreateDecisionBody {
    pub headline: String,
    pub question: String,
    pub options: Vec<DecisionOptionDto>,
    pub recommended_key: String,
    #[serde(default)]
    pub context: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DecisionOptionDto {
    pub key: String,
    pub label: String,
    pub body: String,
}

#[derive(Debug, Serialize)]
pub struct CreateDecisionResponse {
    pub decision_id: String,
    pub channel_id: String,
}

pub async fn handle_create_decision(
    State(state): State<AppState>,
    Path(agent_name): Path<String>,
    Json(body): Json<CreateDecisionBody>,
) -> ApiResult<CreateDecisionResponse> {
    if agent_name.trim().is_empty() {
        return Err(app_err!(StatusCode::BAD_REQUEST, "agent_name is empty"));
    }

    // Look up the agent row (validator at the bridge boundary already
    // checked payload shape; this is the persistence step).
    let agent = state
        .store
        .get_agent(&agent_name)
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?
        .ok_or_else(|| app_err!(StatusCode::NOT_FOUND, "agent not found: {agent_name}"))?;

    // Channel inference: v1 contract is that the agent is in an active
    // channel-triggered run. The trace store records the channel for the
    // current run via `set_run_channel`. Fail loudly if not set rather
    // than silently picking #all.
    let channel_id = state
        .lifecycle
        .run_channel_id(&agent_name)
        .await
        .ok_or_else(|| {
            app_err!(
                StatusCode::BAD_REQUEST,
                "no active-run channel for agent {agent_name}; \
                 dispatch_decision requires a channel-triggered agent run"
            )
        })?;

    let workspace_id = state
        .active_workspace_id()
        .await
        .ok_or_else(|| app_err!(StatusCode::BAD_REQUEST, "no active workspace"))?;

    // Session id of the agent's current run, for the resume_with_prompt
    // round-trip later. We don't enforce that the agent must still be on
    // the same session at resolve time — claude in particular spawns a
    // new session per turn — but recording it here is useful for trace
    // and audit.
    let session_id = state
        .store
        .get_active_session(&agent.id)
        .ok()
        .flatten()
        .map(|s| s.session_id)
        .unwrap_or_default();

    let decision_id = Uuid::new_v4().to_string();
    let payload_json = serde_json::to_string(&serde_json::json!({
        "headline": body.headline,
        "question": body.question,
        "options": body.options,
        "recommended_key": body.recommended_key,
        "context": body.context,
    }))
    .map_err(|e| app_err!(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    state
        .store
        .create_decision(
            &decision_id,
            &workspace_id,
            &channel_id,
            &agent.id,
            &session_id,
            &payload_json,
        )
        .map_err(|e| app_err!(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    info!(
        target: "chorus_decision",
        agent = %agent_name,
        decision_id = %decision_id,
        channel_id = %channel_id,
        "decision created"
    );

    Ok(Json(CreateDecisionResponse {
        decision_id,
        channel_id,
    }))
}

// ── Public: list decisions ────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ListDecisionsParams {
    /// "open" | "resolved" | "all" (default "open")
    #[serde(default)]
    pub status: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DecisionView {
    pub id: String,
    pub agent_id: String,
    pub agent_name: String,
    pub channel_id: String,
    pub channel_name: String,
    pub created_at: String,
    pub status: DecisionStatus,
    pub payload: Value,
    pub picked_key: Option<String>,
    pub picked_note: Option<String>,
    pub resolved_at: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ListDecisionsResponse {
    pub decisions: Vec<DecisionView>,
}

pub async fn handle_list_decisions(
    State(state): State<AppState>,
    Query(params): Query<ListDecisionsParams>,
) -> ApiResult<ListDecisionsResponse> {
    let workspace_id = state
        .active_workspace_id()
        .await
        .ok_or_else(|| app_err!(StatusCode::BAD_REQUEST, "no active workspace"))?;

    let status_filter = match params.status.as_deref() {
        None | Some("open") => Some(DecisionStatus::Open),
        Some("resolved") => Some(DecisionStatus::Resolved),
        Some("all") => None,
        Some(other) => {
            return Err(app_err!(
                StatusCode::BAD_REQUEST,
                "invalid status filter: {other}"
            ))
        }
    };

    let rows = state
        .store
        .list_decisions(&workspace_id, status_filter)
        .map_err(|e| app_err!(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let decisions = rows
        .into_iter()
        .map(|row| row_to_view(&state, row))
        .collect();

    Ok(Json(ListDecisionsResponse { decisions }))
}

fn row_to_view(state: &AppState, row: DecisionRow) -> DecisionView {
    let payload: Value =
        serde_json::from_str(&row.payload_json).unwrap_or_else(|_| serde_json::json!({}));
    let agent_name = state
        .store
        .get_agent_by_id(&row.agent_id, false)
        .ok()
        .flatten()
        .map(|a| a.name)
        .unwrap_or_else(|| row.agent_id.clone());
    let channel_name = state
        .store
        .get_channel_by_id(&row.channel_id)
        .ok()
        .flatten()
        .map(|c| c.name)
        .unwrap_or_else(|| row.channel_id.clone());
    DecisionView {
        id: row.id,
        agent_id: row.agent_id,
        agent_name,
        channel_id: row.channel_id,
        channel_name,
        created_at: row.created_at,
        status: row.status,
        payload,
        picked_key: row.picked_key,
        picked_note: row.picked_note,
        resolved_at: row.resolved_at,
    }
}

// ── Public: resolve a decision ────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ResolveDecisionBody {
    pub picked_key: String,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ResolveDecisionResponse {
    pub decision_id: String,
    pub status: DecisionStatus,
}

pub async fn handle_resolve_decision(
    State(state): State<AppState>,
    Path(decision_id): Path<String>,
    Json(body): Json<ResolveDecisionBody>,
) -> ApiResult<ResolveDecisionResponse> {
    // Fetch the row first so we can build the envelope and validate the
    // picked_key against the stored options.
    let row = state
        .store
        .get_decision(&decision_id)
        .map_err(|e| app_err!(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| app_err!(StatusCode::NOT_FOUND, "decision not found: {decision_id}"))?;

    let payload: Value = serde_json::from_str(&row.payload_json)
        .map_err(|e| app_err!(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Locate the picked option so we can splice its full body into the
    // envelope, and to validate that picked_key is one of the offered
    // option keys (race protection if the agent submitted a malformed
    // payload that bypassed bridge validation somehow).
    let options = payload
        .get("options")
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            app_err!(
                StatusCode::INTERNAL_SERVER_ERROR,
                "decision payload missing options"
            )
        })?;
    let picked = options
        .iter()
        .find(|o| o.get("key").and_then(|k| k.as_str()) == Some(&body.picked_key))
        .ok_or_else(|| {
            app_err!(
                StatusCode::BAD_REQUEST,
                "picked_key '{}' is not one of the decision's option keys",
                body.picked_key
            )
        })?;

    // CAS update: if the row is no longer open, return 409 so the UI can
    // refresh. Two simultaneous picks must not both succeed.
    let updated = state
        .store
        .resolve_decision_cas(&decision_id, &body.picked_key, body.note.as_deref())
        .map_err(|e| app_err!(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if !updated {
        return Err(app_err!(StatusCode::CONFLICT, "decision is no longer open"));
    }

    // Locate the agent name for the lifecycle call.
    let agent = state
        .store
        .get_agent_by_id(&row.agent_id, false)
        .map_err(|e| app_err!(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| {
            app_err!(
                StatusCode::INTERNAL_SERVER_ERROR,
                "decision references a deleted agent"
            )
        })?;

    let envelope = build_resume_envelope(&payload, picked, body.note.as_deref());

    if let Err(e) = state
        .lifecycle
        .resume_with_prompt(&agent.id, envelope)
        .await
    {
        // Roll back the resolve so the human's pick isn't silently lost.
        warn!(
            agent = %agent.name,
            agent_id = %agent.id,
            decision_id = %decision_id,
            error = %e,
            "resume_with_prompt failed; reverting decision to open"
        );
        if let Err(revert_err) = state.store.revert_decision_to_open(&decision_id) {
            warn!(
                decision_id = %decision_id,
                error = %revert_err,
                "failed to revert decision after resume failure"
            );
        }
        return Err(app_err!(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to deliver pick to agent: {e}"
        ));
    }

    info!(
        target: "chorus_decision",
        decision_id = %decision_id,
        agent = %agent.name,
        picked = %body.picked_key,
        "decision resolved + envelope delivered"
    );

    Ok(Json(ResolveDecisionResponse {
        decision_id,
        status: DecisionStatus::Resolved,
    }))
}

fn build_resume_envelope(payload: &Value, picked: &Value, note: Option<&str>) -> String {
    let headline = payload
        .get("headline")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let question = payload
        .get("question")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let picked_key = picked.get("key").and_then(|v| v.as_str()).unwrap_or("");
    let picked_label = picked.get("label").and_then(|v| v.as_str()).unwrap_or("");
    let picked_body = picked.get("body").and_then(|v| v.as_str()).unwrap_or("");

    let mut envelope = String::with_capacity(2_048);
    envelope.push_str(
        "Your decision has been resolved. The human picked an option in their inbox; \
         this prompt is your follow-up. Read the picked option's body for what to do next, \
         then proceed with that work — do not re-ask via send_message.\n\n",
    );
    envelope.push_str(&format!("**Original headline:** {headline}\n"));
    envelope.push_str(&format!("**Original question:** {question}\n\n"));
    envelope.push_str(&format!(
        "**Picked option ({picked_key}): {picked_label}**\n\n{picked_body}\n"
    ));
    if let Some(n) = note {
        if !n.trim().is_empty() {
            envelope.push_str(&format!("\n**Human note:** {n}\n"));
        }
    }
    envelope
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_includes_headline_question_picked_body_and_note() {
        let payload = serde_json::json!({
            "headline": "PR #120 retrospective",
            "question": "Was that the right call?",
        });
        let picked = serde_json::json!({
            "key": "A",
            "label": "Keep the merge",
            "body": "The merge stands. CI is green.",
        });
        let env = build_resume_envelope(&payload, &picked, Some("looks good"));
        assert!(env.contains("PR #120 retrospective"));
        assert!(env.contains("Was that the right call?"));
        assert!(env.contains("Picked option (A): Keep the merge"));
        assert!(env.contains("The merge stands. CI is green."));
        assert!(env.contains("**Human note:** looks good"));
        // The envelope must explicitly tell the agent NOT to re-ask via
        // send_message, otherwise it will reply conversationally and skip
        // the work.
        assert!(env.contains("do not re-ask via send_message"));
    }

    #[test]
    fn envelope_omits_blank_note() {
        let payload = serde_json::json!({"headline": "h", "question": "q"});
        let picked = serde_json::json!({"key": "A", "label": "L", "body": "B"});
        let env = build_resume_envelope(&payload, &picked, Some("   "));
        assert!(!env.contains("Human note"));
    }
}
