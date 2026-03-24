use axum::extract::{Path, Query, State};
use axum::Json;
use tracing::{debug, info};

use super::{internal_err, ApiResult, AppState};
use crate::store::knowledge::{RecallQuery, RecallResponse, RememberRequest, RememberResponse};
use crate::store::messages::SenderType;

/// Store a fact in the shared knowledge store and post a breadcrumb to #shared-memory.
/// Both writes happen atomically — if posting to #shared-memory fails the knowledge entry
/// is still retained (best-effort visibility).
pub async fn handle_remember(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(req): Json<RememberRequest>,
) -> ApiResult<RememberResponse> {
    let store = &state.store;

    // Normalise tags: join the vec into space-separated FTS5 tokens.
    let tags = req.tags.join(" ");

    let id = store
        .remember(
            &req.key,
            &req.value,
            &tags,
            &agent_id,
            req.channel_context.as_deref(),
        )
        .map_err(|e| internal_err(e.to_string()))?;

    info!(agent = %agent_id, key = %req.key, id = %id, "knowledge remember");

    // Post a human-readable breadcrumb to #shared-memory.
    // Best-effort: don't fail the remember call if the channel post fails.
    let breadcrumb = if tags.is_empty() {
        format!("[🧠 @{}] {}: {}", agent_id, req.key, req.value)
    } else {
        format!("[🧠 @{}] {} [{}]: {}", agent_id, req.key, tags, req.value)
    };
    let _ = store.send_message(
        "shared-memory",
        None,
        &agent_id,
        SenderType::Agent,
        &breadcrumb,
        &[],
    );

    Ok(Json(RememberResponse { id }))
}

/// Search the shared knowledge store by keyword and/or tags.
pub async fn handle_recall(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Query(q): Query<RecallQuery>,
) -> ApiResult<RecallResponse> {
    let entries = state
        .store
        .recall(q.query.as_deref(), q.tags.as_deref())
        .map_err(|e| internal_err(e.to_string()))?;

    debug!(agent = %agent_id, query = ?q.query, count = entries.len(), "knowledge recall");

    Ok(Json(RecallResponse { entries }))
}
