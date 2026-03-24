use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use tracing::{debug, info};

use super::{api_err, internal_err, ApiResult, AppState};
use crate::agent::activity_log::ActivityEntry;
use crate::store::agents::AgentStatus;
use crate::store::channels::ChannelType;
use crate::store::messages::{ReceivedMessage, SenderType};
use crate::store::Store;

// ── Inline query structs ──

#[derive(Deserialize)]
pub struct ReceiveParams {
    pub block: Option<String>,
    pub timeout: Option<u64>,
}

#[derive(Deserialize)]
pub struct HistoryParams {
    pub channel: Option<String>,
    pub limit: Option<i64>,
    pub before: Option<i64>,
    pub after: Option<i64>,
}

// ── API DTOs ──

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct SendRequest {
    pub target: String,
    #[serde(default)]
    pub content: String,
    #[serde(default, rename = "attachmentIds")]
    pub attachment_ids: Vec<String>,
    /// Skip fan-out to other agents when the caller wants a human-only side effect,
    /// such as "send this message and create one task" without triggering agent replies.
    #[serde(default, rename = "suppressAgentDelivery")]
    pub suppress_agent_delivery: bool,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct SendResponse {
    #[serde(rename = "messageId")]
    pub message_id: String,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct ReceiveResponse {
    pub messages: Vec<ReceivedMessage>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct HistoryResponse {
    pub messages: Vec<crate::store::messages::HistoryMessage>,
    pub has_more: bool,
    pub last_read_seq: i64,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct ResolveChannelRequest {
    pub target: String,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct ResolveChannelResponse {
    #[serde(rename = "channelId")]
    pub channel_id: String,
}

// ── Private helpers ──

/// Build a compact preview suitable for activity log rows and tracing.
fn content_preview(text: &str) -> String {
    let preview: String = text.chars().take(120).collect();
    if text.chars().count() > 120 {
        format!("{preview}…")
    } else {
        preview
    }
}

/// Convert a delivered message into the label shown in the activity timeline.
fn activity_channel_label(message: &ReceivedMessage) -> String {
    match message.channel_type.as_str() {
        "channel" => format!("#{}", message.channel_name),
        "dm" => format!("dm:@{}", message.channel_name),
        "thread" => {
            let parent_type = message.parent_channel_type.as_deref().unwrap_or("channel");
            let parent_name = message
                .parent_channel_name
                .as_deref()
                .unwrap_or(&message.channel_name);
            match parent_type {
                "dm" => format!("dm:@{} thread", parent_name),
                _ => format!("#{} thread", parent_name),
            }
        }
        _ => message.channel_name.clone(),
    }
}

/// Record received messages in the activity log so the UI can show communication flow.
fn push_received_activity(state: &AppState, agent_id: &str, messages: &[ReceivedMessage]) {
    for message in messages {
        state.lifecycle.push_activity_entry(
            agent_id,
            ActivityEntry::MessageReceived {
                channel_label: activity_channel_label(message),
                sender_name: message.sender_name.clone(),
                content: content_preview(&message.content),
            },
        );
    }
}

fn resolve_history_target(
    store: &Store,
    agent_id: &str,
    channel_target: &str,
) -> anyhow::Result<(String, Option<String>)> {
    if channel_target.starts_with('#') || channel_target.starts_with("dm:@") {
        let (channel_id, thread_parent_id) = store.resolve_target(channel_target, agent_id)?;
        let channel = store
            .find_channel_by_id(&channel_id)?
            .ok_or_else(|| anyhow::anyhow!("channel not found: {}", channel_target))?;
        return Ok((channel.name, thread_parent_id));
    }
    Ok((channel_target.to_string(), None))
}

// ── Public handlers ──

pub async fn handle_send(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(req): Json<SendRequest>,
) -> ApiResult<SendResponse> {
    let store = &state.store;
    let sender_type = store
        .lookup_sender_type(&agent_id)
        .map_err(|e| api_err(e.to_string()))?
        .unwrap_or(SenderType::Human);

    let (channel_id, thread_parent_id) = store
        .resolve_target(&req.target, &agent_id)
        .map_err(|e| api_err(e.to_string()))?;

    let channel = store
        .find_channel_by_id(&channel_id)
        .map_err(|e| api_err(e.to_string()))?
        .ok_or_else(|| api_err("channel not found"))?;

    // Protected system channels (e.g. #shared-memory) are write-protected.
    // Agents must use mcp_chat_remember instead of send_message to post there.
    if channel.channel_type == ChannelType::System
        && Store::is_system_channel_read_only(&channel.name)
    {
        return Err(api_err(
            "Cannot post to system channels directly. Use mcp_chat_remember instead.",
        ));
    }

    let preview = content_preview(&req.content);
    info!(agent = %agent_id, target = %req.target, content = %preview, "send_message");

    let message_id = store
        .send_message(
            &channel.name,
            thread_parent_id.as_deref(),
            &agent_id,
            sender_type,
            &req.content,
            &req.attachment_ids,
        )
        .map_err(|e| api_err(e.to_string()))?;

    let short_id = if message_id.len() >= 8 {
        &message_id[..8]
    } else {
        &message_id
    };
    info!(agent = %agent_id, msg = %short_id, "send_message ok");
    if sender_type == SenderType::Agent {
        state.lifecycle.push_activity_entry(
            &agent_id,
            ActivityEntry::MessageSent {
                target: req.target.clone(),
                content: preview,
            },
        );
    }

    if !req.suppress_agent_delivery {
        deliver_message_to_agents(&state, &channel.id, &agent_id, &message_id)
            .await
            .map_err(|e| internal_err(e.to_string()))?;
    }

    Ok(Json(SendResponse { message_id }))
}

pub async fn handle_receive(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Query(params): Query<ReceiveParams>,
) -> ApiResult<ReceiveResponse> {
    let store = &state.store;
    let blocking = params.block.as_deref() != Some("false");
    let timeout_ms = params.timeout.unwrap_or(30_000);

    let messages = store
        .get_messages_for_agent(&agent_id, true)
        .map_err(|e| api_err(e.to_string()))?;

    if !messages.is_empty() {
        info!(agent = %agent_id, count = messages.len(), "receive_message: got messages immediately");
        for m in &messages {
            info!(agent = %agent_id, target = %format!("{}:{}", m.channel_type, m.channel_name), sender = %m.sender_name, content = %m.content.chars().take(120).collect::<String>(), "  ← message");
        }
        push_received_activity(&state, &agent_id, &messages);
        return Ok(Json(ReceiveResponse { messages }));
    }
    if !blocking {
        debug!(agent = %agent_id, "receive_message: non-blocking, no messages");
        return Ok(Json(ReceiveResponse { messages }));
    }

    debug!(agent = %agent_id, timeout_ms, "receive_message: long-polling");
    let mut rx = store.subscribe();
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_millis(timeout_ms);

    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Ok(Json(ReceiveResponse {
                messages: Vec::new(),
            }));
        }

        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(_)) => {
                let messages = store
                    .get_messages_for_agent(&agent_id, true)
                    .map_err(|e| api_err(e.to_string()))?;
                if !messages.is_empty() {
                    info!(agent = %agent_id, count = messages.len(), "receive_message: woke up with messages");
                    for m in &messages {
                        info!(agent = %agent_id, target = %format!("{}:{}", m.channel_type, m.channel_name), sender = %m.sender_name, content = %m.content.chars().take(120).collect::<String>(), "  ← message");
                    }
                    push_received_activity(&state, &agent_id, &messages);
                    return Ok(Json(ReceiveResponse { messages }));
                }
            }
            _ => {
                return Ok(Json(ReceiveResponse {
                    messages: Vec::new(),
                }))
            }
        }
    }
}

pub async fn handle_history(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Query(params): Query<HistoryParams>,
) -> ApiResult<HistoryResponse> {
    let channel_target = params
        .channel
        .ok_or_else(|| api_err("missing channel parameter"))?;
    if let Some(ref ch) = Some(&channel_target) {
        debug!(agent = %agent_id, channel = %ch, "read_history");
    }

    let store = &state.store;
    let (channel_name, thread_parent_id) =
        resolve_history_target(store, &agent_id, &channel_target)
            .map_err(|e| api_err(e.to_string()))?;
    if !store
        .is_member(&channel_name, &agent_id)
        .map_err(|e| api_err(e.to_string()))?
    {
        return Ok(Json(HistoryResponse {
            messages: vec![],
            has_more: false,
            last_read_seq: 0,
        }));
    }

    let limit = params.limit.unwrap_or(50);
    let (messages, has_more) = store
        .get_history(
            &channel_name,
            thread_parent_id.as_deref(),
            limit,
            params.before,
            params.after,
        )
        .map_err(|e| api_err(e.to_string()))?;

    let last_read_seq = store
        .get_last_read_seq(&channel_name, &agent_id)
        .unwrap_or(0);

    Ok(Json(HistoryResponse {
        messages,
        has_more,
        last_read_seq,
    }))
}

pub async fn handle_resolve_channel(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(req): Json<ResolveChannelRequest>,
) -> ApiResult<ResolveChannelResponse> {
    let (channel_id, _) = state
        .store
        .resolve_target(&req.target, &agent_id)
        .map_err(|e| api_err(e.to_string()))?;
    Ok(Json(ResolveChannelResponse { channel_id }))
}

/// Fan-out a newly posted message to all relevant agent recipients.
pub(crate) async fn deliver_message_to_agents(
    state: &AppState,
    channel_id: &str,
    sender_name: &str,
    message_id: &str,
) -> anyhow::Result<()> {
    // Thread messages are scoped to implicit thread participants rather than
    // every agent in the parent channel.
    let recipients =
        state
            .store
            .get_agent_message_recipients(channel_id, message_id, sender_name)?;
    for recipient_name in recipients {
        let Some(agent) = state.store.get_agent(&recipient_name)? else {
            continue;
        };
        match agent.status {
            AgentStatus::Active => state.lifecycle.notify_agent(&recipient_name).await?,
            AgentStatus::Sleeping | AgentStatus::Inactive => {
                let wake_message = state
                    .store
                    .get_received_message_for_agent(&recipient_name, message_id)?;
                state
                    .lifecycle
                    .start_agent(&recipient_name, wake_message)
                    .await?
            }
        }
    }
    Ok(())
}
