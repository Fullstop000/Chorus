use axum::extract::{Path, Query, State};
use axum::Json;
use regex::Regex;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use super::dto::ChannelInfo;
use super::{api_err, format_anyhow_error, internal_err, ApiResult, AppState};
use crate::agent::activity_log::ActivityEntry;
use crate::agent::collaboration::make_collaboration_model;
use crate::store::agents::AgentStatus;
use crate::store::channels::Channel;
use crate::store::channels::ChannelType;
use crate::store::inbox::{InboxConversationNotificationView, ThreadNotificationStateView};
use crate::store::messages::{ForwardedFrom, ReceivedMessage, SenderType};
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

#[derive(Deserialize)]
pub struct PublicConversationMessagesParams {
    pub limit: Option<i64>,
    pub before: Option<i64>,
    pub after: Option<i64>,
    #[serde(rename = "threadParentId")]
    pub thread_parent_id: Option<String>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct PublicConversationSendRequest {
    #[serde(default)]
    pub content: String,
    #[serde(default, rename = "attachmentIds")]
    pub attachment_ids: Vec<String>,
    #[serde(default, rename = "clientNonce")]
    pub client_nonce: Option<String>,
    #[serde(default, rename = "suppressAgentDelivery")]
    pub suppress_agent_delivery: bool,
    #[serde(default, rename = "threadParentId")]
    pub thread_parent_id: Option<String>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct PublicConversationReadCursorRequest {
    #[serde(rename = "lastReadSeq")]
    pub last_read_seq: i64,
    #[serde(default, rename = "threadParentId")]
    pub thread_parent_id: Option<String>,
}

// ── API DTOs ──

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct SendRequest {
    pub target: String,
    #[serde(default)]
    pub content: String,
    #[serde(default, rename = "attachmentIds")]
    pub attachment_ids: Vec<String>,
    #[serde(default, rename = "clientNonce")]
    pub client_nonce: Option<String>,
    /// Skip fan-out to other agents when the caller wants a human-only side effect,
    /// such as "send this message and create one task" without triggering agent replies.
    #[serde(default, rename = "suppressAgentDelivery")]
    pub suppress_agent_delivery: bool,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct SendResponse {
    #[serde(rename = "messageId")]
    pub message_id: String,
    pub seq: i64,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "clientNonce", skip_serializing_if = "Option::is_none")]
    pub client_nonce: Option<String>,
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
pub struct InboxResponse {
    pub conversations: Vec<crate::store::InboxConversationNotificationView>,
}

/// CamelCase inbox row for browser clients (matches `InboxConversationState` in the UI).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicInboxConversationNotification {
    pub conversation_id: String,
    pub conversation_name: String,
    pub conversation_type: String,
    pub latest_seq: i64,
    pub last_read_seq: i64,
    pub unread_count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_read_message_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_message_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_message_at: Option<String>,
}

impl From<&InboxConversationNotificationView> for PublicInboxConversationNotification {
    fn from(v: &InboxConversationNotificationView) -> Self {
        Self {
            conversation_id: v.conversation_id.clone(),
            conversation_name: v.conversation_name.clone(),
            conversation_type: v.conversation_type.clone(),
            latest_seq: v.latest_seq,
            last_read_seq: v.last_read_seq,
            unread_count: v.unread_count,
            last_read_message_id: None,
            last_message_id: v.last_message_id.clone(),
            last_message_at: v.last_message_at.clone(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicThreadNotificationRefresh {
    pub conversation_id: String,
    pub thread_parent_id: String,
    pub latest_seq: i64,
    pub last_read_seq: i64,
    pub unread_count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_reply_message_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_reply_at: Option<String>,
}

impl From<ThreadNotificationStateView> for PublicThreadNotificationRefresh {
    fn from(t: ThreadNotificationStateView) -> Self {
        Self {
            conversation_id: t.conversation_id,
            thread_parent_id: t.thread_parent_id,
            latest_seq: t.latest_seq,
            last_read_seq: t.last_read_seq,
            unread_count: t.unread_count,
            last_reply_message_id: t.last_reply_message_id,
            last_reply_at: t.last_reply_at,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationInboxRefreshResponse {
    pub conversation: PublicInboxConversationNotification,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread: Option<PublicThreadNotificationRefresh>,
}

#[derive(Debug, Deserialize)]
pub struct InboxNotificationQuery {
    #[serde(default, rename = "threadParentId")]
    pub thread_parent_id: Option<String>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct ThreadsResponse {
    #[serde(rename = "unreadCount")]
    pub unread_count: i64,
    pub threads: Vec<crate::store::ChannelThreadInboxEntry>,
}

#[derive(Debug, serde::Serialize)]
pub struct ReadCursorResponse {
    pub ok: bool,
    /// Matches [`crate::store::inbox::InboxConversationNotificationView::unread_count`]
    /// (top-level + thread replies per inbox view).
    #[serde(rename = "conversationUnreadCount")]
    pub conversation_unread_count: i64,
    #[serde(rename = "conversationLastReadSeq")]
    pub conversation_last_read_seq: i64,
    #[serde(rename = "conversationLatestSeq")]
    pub conversation_latest_seq: i64,
    #[serde(rename = "threadParentId", skip_serializing_if = "Option::is_none")]
    pub thread_parent_id: Option<String>,
    #[serde(rename = "threadUnreadCount", skip_serializing_if = "Option::is_none")]
    pub thread_unread_count: Option<i64>,
    #[serde(rename = "threadLastReadSeq", skip_serializing_if = "Option::is_none")]
    pub thread_last_read_seq: Option<i64>,
    #[serde(rename = "threadLatestSeq", skip_serializing_if = "Option::is_none")]
    pub thread_latest_seq: Option<i64>,
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
            .get_channel_by_id(&channel_id)?
            .ok_or_else(|| anyhow::anyhow!("channel not found: {}", channel_target))?;
        return Ok((channel.name, thread_parent_id));
    }
    Ok((channel_target.to_string(), None))
}

fn public_viewer_name() -> String {
    whoami::username()
}

fn sender_type_for_actor(
    store: &Store,
    actor_id: &str,
) -> Result<SenderType, (axum::http::StatusCode, Json<super::ErrorResponse>)> {
    Ok(store
        .lookup_sender_type(actor_id)
        .map_err(|e| api_err(e.to_string()))?
        .unwrap_or(SenderType::Human))
}

fn load_channel_by_id(
    store: &Store,
    channel_id: &str,
) -> Result<Channel, (axum::http::StatusCode, Json<super::ErrorResponse>)> {
    store
        .get_channel_by_id(channel_id)
        .map_err(|e| api_err(e.to_string()))?
        .ok_or_else(|| api_err("channel not found"))
}

#[allow(clippy::too_many_arguments)]
fn history_for_channel(
    state: &AppState,
    actor_id: &str,
    channel: &Channel,
    denied_label: &str,
    thread_parent_id: Option<&str>,
    limit: i64,
    before: Option<i64>,
    after: Option<i64>,
) -> ApiResult<HistoryResponse> {
    if !state
        .store
        .is_member(&channel.name, actor_id)
        .map_err(|e| api_err(e.to_string()))?
    {
        return Err(api_err(format!(
            "you are not a member of channel {}",
            denied_label
        )));
    }

    let snapshot = state
        .store
        .get_history_snapshot(
            &channel.name,
            actor_id,
            thread_parent_id,
            limit,
            before,
            after,
        )
        .map_err(|e| api_err(e.to_string()))?;

    Ok(Json(HistoryResponse {
        messages: snapshot.messages,
        has_more: snapshot.has_more,
        last_read_seq: snapshot.last_read_seq,
    }))
}

fn threads_for_channel(
    state: &AppState,
    actor_id: &str,
    channel: &Channel,
    denied_label: &str,
) -> ApiResult<ThreadsResponse> {
    if !state
        .store
        .is_member(&channel.name, actor_id)
        .map_err(|e| api_err(e.to_string()))?
    {
        return Err(api_err(format!(
            "you are not a member of channel {}",
            denied_label
        )));
    }

    let inbox = state
        .store
        .get_channel_thread_inbox(&channel.name, actor_id)
        .map_err(|e| internal_err(e.to_string()))?;
    Ok(Json(ThreadsResponse {
        unread_count: inbox.unread_count,
        threads: inbox.threads,
    }))
}

fn update_read_cursor_for_channel(
    state: &AppState,
    actor_id: &str,
    channel: &Channel,
    denied_label: &str,
    thread_parent_id: Option<&str>,
    last_read_seq: i64,
) -> ApiResult<ReadCursorResponse> {
    if !state
        .store
        .is_member(&channel.name, actor_id)
        .map_err(|e| api_err(e.to_string()))?
    {
        return Err(api_err(format!(
            "you are not a member of channel {}",
            denied_label
        )));
    }

    let sender_type = sender_type_for_actor(&state.store, actor_id)?;
    state
        .store
        .set_history_read_cursor(
            &channel.name,
            actor_id,
            sender_type,
            thread_parent_id,
            last_read_seq,
        )
        .map_err(|e| api_err(e.to_string()))?;

    let notification = state
        .store
        .get_inbox_conversation_notification_for_member(&channel.id, actor_id)
        .map_err(|e| internal_err(e.to_string()))?
        .ok_or_else(|| internal_err("inbox notification row missing after read cursor"))?;

    let thread_snapshot = if let Some(parent_id) = thread_parent_id {
        state
            .store
            .get_thread_notification_state(&channel.name, parent_id, actor_id)
            .map_err(|e| internal_err(e.to_string()))?
    } else {
        None
    };

    Ok(Json(ReadCursorResponse {
        ok: true,
        conversation_unread_count: notification.unread_count,
        conversation_last_read_seq: notification.last_read_seq,
        conversation_latest_seq: notification.latest_seq,
        thread_parent_id: thread_parent_id.map(str::to_string),
        thread_unread_count: thread_snapshot.as_ref().map(|t| t.unread_count),
        thread_last_read_seq: thread_snapshot.as_ref().map(|t| t.last_read_seq),
        thread_latest_seq: thread_snapshot.as_ref().map(|t| t.latest_seq),
    }))
}

#[allow(clippy::too_many_arguments)]
async fn send_message_to_channel(
    state: &AppState,
    actor_id: &str,
    channel: &Channel,
    thread_parent_id: Option<&str>,
    content: &str,
    attachment_ids: &[String],
    client_nonce: Option<String>,
    suppress_agent_delivery: bool,
) -> ApiResult<SendResponse> {
    let store = &state.store;
    let sender_type = sender_type_for_actor(store, actor_id)?;

    if channel.channel_type == ChannelType::System
        && Store::is_system_channel_read_only(&channel.name)
    {
        return Err(api_err(
            "Cannot post to system channels directly. Use mcp_chat_remember instead.",
        ));
    }

    let preview = content_preview(content);
    let target_label = match thread_parent_id {
        Some(parent_id) => format!("#{}:{parent_id}", channel.name),
        None => format!("#{}", channel.name),
    };
    info!(agent = %actor_id, target = %target_label, content = %preview, "send_message");

    let message_id = store
        .create_message(
            &channel.name,
            thread_parent_id,
            actor_id,
            sender_type,
            content,
            attachment_ids,
        )
        .map_err(|e| api_err(e.to_string()))?;

    let short_id = if message_id.len() >= 8 {
        &message_id[..8]
    } else {
        &message_id
    };
    info!(agent = %actor_id, msg = %short_id, "send_message ok");
    if sender_type == SenderType::Agent {
        state.lifecycle.push_activity_entry(
            actor_id,
            ActivityEntry::MessageSent {
                target: target_label,
                content: preview,
            },
        );
    }

    let mut consensus_message_id = None;
    if sender_type == SenderType::Agent && channel.channel_type == ChannelType::Team {
        if let Some(team) = store
            .get_team(&channel.name)
            .map_err(|e| internal_err(e.to_string()))?
        {
            let collaboration_model = make_collaboration_model(&team.collaboration_model);
            if collaboration_model.is_consensus_signal(content) {
                match store.record_swarm_signal(&team.id, actor_id, content) {
                    Ok(true) => {
                        let system_message_id = store
                            .create_system_message(
                                &channel.id,
                                "[System] All members ready - execution begins.",
                            )
                            .map_err(|e| internal_err(e.to_string()))?;
                        consensus_message_id = Some(system_message_id);
                    }
                    Ok(false) => {}
                    Err(e) => warn!("swarm signal error: {e}"),
                }
            }
        }
    }

    if !suppress_agent_delivery {
        forward_team_mentions(state, &channel.name, actor_id, sender_type, content)
            .await
            .map_err(|e| internal_err(e.to_string()))?;
    }

    if !suppress_agent_delivery {
        if let Err(err) = deliver_message_to_agents(state, &channel.id, actor_id, &message_id).await
        {
            let error_detail = format_anyhow_error(&err);
            warn!(
                channel = %channel.name,
                actor = %actor_id,
                message_id = %message_id,
                error = %error_detail,
                "message persisted but agent delivery failed"
            );
        }
    }
    if let Some(system_message_id) = consensus_message_id {
        if let Err(err) =
            deliver_message_to_agents(state, &channel.id, "system", &system_message_id).await
        {
            let error_detail = format_anyhow_error(&err);
            warn!(
                channel = %channel.name,
                actor = "system",
                message_id = %system_message_id,
                error = %error_detail,
                "system message persisted but agent delivery failed"
            );
        }
    }

    let message_view = store
        .get_conversation_message_view(&message_id)
        .map_err(|e| internal_err(e.to_string()))?
        .ok_or_else(|| internal_err("sent message missing from projection"))?;

    Ok(Json(SendResponse {
        message_id,
        seq: message_view.seq,
        created_at: message_view.created_at,
        client_nonce,
    }))
}

/// Mirror `@team-name` mentions into the corresponding team channel.
async fn forward_team_mentions(
    state: &AppState,
    channel_name: &str,
    sender_name: &str,
    sender_type: SenderType,
    content: &str,
) -> anyhow::Result<()> {
    let mention_re = Regex::new(r"@([A-Za-z0-9_-]+)").expect("team mention regex is valid");
    let mentions = mention_re
        .captures_iter(content)
        .filter_map(|capture| capture.get(1).map(|m| m.as_str().to_string()))
        .collect::<std::collections::BTreeSet<_>>();

    for mention in mentions {
        let Some(team) = state.store.get_team(&mention)? else {
            continue;
        };
        let Some(team_channel) = state.store.get_channel_by_name(&team.name)? else {
            continue;
        };

        let forwarded_message_id = state.store.create_message_with_forwarded_from(
            &team_channel.id,
            sender_name,
            sender_type,
            content,
            &[],
            Some(ForwardedFrom {
                channel_name: channel_name.to_string(),
                sender_name: sender_name.to_string(),
            }),
        )?;
        let collaboration_model = make_collaboration_model(&team.collaboration_model);
        if let Some(prompt) = collaboration_model.deliberation_prompt() {
            state
                .store
                .snapshot_swarm_quorum(&team.id, &forwarded_message_id)?;
            state
                .store
                .create_system_message(&team_channel.id, &prompt)?;
        }

        deliver_message_to_agents(state, &team_channel.id, sender_name, &forwarded_message_id)
            .await?;
    }

    Ok(())
}

// ── Public handlers ──

pub async fn handle_send(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(req): Json<SendRequest>,
) -> ApiResult<SendResponse> {
    let store = &state.store;
    let (channel_id, thread_parent_id) = store
        .resolve_target(&req.target, &agent_id)
        .map_err(|e| api_err(e.to_string()))?;

    let channel = load_channel_by_id(store, &channel_id)?;
    send_message_to_channel(
        &state,
        &agent_id,
        &channel,
        thread_parent_id.as_deref(),
        &req.content,
        &req.attachment_ids,
        req.client_nonce,
        req.suppress_agent_delivery,
    )
    .await
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
    let limit = params.limit.unwrap_or(50);
    let channel = store
        .get_channel_by_name(&channel_name)
        .map_err(|e| api_err(e.to_string()))?
        .ok_or_else(|| api_err("channel not found"))?;
    history_for_channel(
        &state,
        &agent_id,
        &channel,
        &channel_target,
        thread_parent_id.as_deref(),
        limit,
        params.before,
        params.after,
    )
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

pub async fn handle_public_inbox(State(state): State<AppState>) -> ApiResult<InboxResponse> {
    let actor_id = public_viewer_name();
    if state
        .store
        .lookup_sender_type(&actor_id)
        .map_err(|e| api_err(e.to_string()))?
        .is_none()
    {
        return Err(api_err(format!("viewer not found: {}", actor_id)));
    }

    let conversations = state
        .store
        .get_inbox_conversation_notifications(&actor_id)
        .map_err(|e| internal_err(e.to_string()))?;
    Ok(Json(InboxResponse { conversations }))
}

pub async fn handle_public_conversation_inbox_notification(
    State(state): State<AppState>,
    Path(conversation_id): Path<String>,
    Query(query): Query<InboxNotificationQuery>,
) -> ApiResult<ConversationInboxRefreshResponse> {
    let actor_id = public_viewer_name();
    if state
        .store
        .lookup_sender_type(&actor_id)
        .map_err(|e| api_err(e.to_string()))?
        .is_none()
    {
        return Err(api_err(format!("viewer not found: {}", actor_id)));
    }

    let channel = load_channel_by_id(&state.store, &conversation_id)?;
    if !state
        .store
        .is_member(&channel.name, &actor_id)
        .map_err(|e| api_err(e.to_string()))?
    {
        return Err(api_err(format!(
            "you are not a member of channel {}",
            conversation_id
        )));
    }

    let notification = state
        .store
        .get_inbox_conversation_notification_for_member(&channel.id, &actor_id)
        .map_err(|e| internal_err(e.to_string()))?
        .ok_or_else(|| api_err("inbox row not found for this member"))?;

    let conversation = PublicInboxConversationNotification::from(&notification);

    let thread = if let Some(ref parent_id) = query.thread_parent_id {
        state
            .store
            .get_thread_notification_state(&channel.name, parent_id, &actor_id)
            .map_err(|e| internal_err(e.to_string()))?
            .map(PublicThreadNotificationRefresh::from)
    } else {
        None
    };

    Ok(Json(ConversationInboxRefreshResponse {
        conversation,
        thread,
    }))
}

pub async fn handle_public_ensure_dm(
    State(state): State<AppState>,
    Path(peer_name): Path<String>,
) -> ApiResult<ChannelInfo> {
    let actor_id = public_viewer_name();
    if peer_name == actor_id {
        return Err(api_err("cannot create a dm with yourself"));
    }
    if state
        .store
        .lookup_sender_type(&actor_id)
        .map_err(|e| api_err(e.to_string()))?
        .is_none()
    {
        return Err(api_err(format!("viewer not found: {}", actor_id)));
    }
    if state
        .store
        .lookup_sender_type(&peer_name)
        .map_err(|e| api_err(e.to_string()))?
        .is_none()
    {
        return Err(api_err(format!("peer not found: {}", peer_name)));
    }

    let target = format!("dm:@{}", peer_name);
    let (channel_id, _) = state
        .store
        .resolve_target(&target, &actor_id)
        .map_err(|e| api_err(e.to_string()))?;
    let channel = load_channel_by_id(&state.store, &channel_id)?;
    Ok(Json(ChannelInfo::from((&channel, true))))
}

pub async fn handle_public_history(
    State(state): State<AppState>,
    Path(conversation_id): Path<String>,
    Query(params): Query<PublicConversationMessagesParams>,
) -> ApiResult<HistoryResponse> {
    let actor_id = public_viewer_name();
    let channel = load_channel_by_id(&state.store, &conversation_id)?;
    history_for_channel(
        &state,
        &actor_id,
        &channel,
        &conversation_id,
        params.thread_parent_id.as_deref(),
        params.limit.unwrap_or(50),
        params.before,
        params.after,
    )
}

pub async fn handle_public_send(
    State(state): State<AppState>,
    Path(conversation_id): Path<String>,
    Json(req): Json<PublicConversationSendRequest>,
) -> ApiResult<SendResponse> {
    let actor_id = public_viewer_name();
    let channel = load_channel_by_id(&state.store, &conversation_id)?;
    send_message_to_channel(
        &state,
        &actor_id,
        &channel,
        req.thread_parent_id.as_deref(),
        &req.content,
        &req.attachment_ids,
        req.client_nonce,
        req.suppress_agent_delivery,
    )
    .await
}

pub async fn handle_public_threads(
    State(state): State<AppState>,
    Path(conversation_id): Path<String>,
) -> ApiResult<ThreadsResponse> {
    let actor_id = public_viewer_name();
    let channel = load_channel_by_id(&state.store, &conversation_id)?;
    threads_for_channel(&state, &actor_id, &channel, &conversation_id)
}

pub async fn handle_public_update_read_cursor(
    State(state): State<AppState>,
    Path(conversation_id): Path<String>,
    Json(req): Json<PublicConversationReadCursorRequest>,
) -> ApiResult<ReadCursorResponse> {
    let actor_id = public_viewer_name();
    let channel = load_channel_by_id(&state.store, &conversation_id)?;
    update_read_cursor_for_channel(
        &state,
        &actor_id,
        &channel,
        &conversation_id,
        req.thread_parent_id.as_deref(),
        req.last_read_seq,
    )
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
