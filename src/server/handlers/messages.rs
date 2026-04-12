use std::sync::LazyLock;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use regex::Regex;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use super::dto::ChannelInfo;
use super::{app_err, format_anyhow_error, internal_err, ApiResult, AppState};
use crate::server::error::AppErrorCode;
use crate::store::agents::AgentStatus;
use crate::store::channels::Channel;
use crate::store::inbox::{InboxConversationNotificationView, ThreadNotificationStateView};
use crate::store::messages::{CreateMessage, ForwardedFrom, ReceivedMessage, SenderType};
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
    #[serde(default, rename = "suppressAgentDelivery")]
    pub suppress_agent_delivery: bool,
    #[serde(default, rename = "suppressEvent")]
    pub suppress_event: bool,
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
    /// Skip fan-out to other agents when the caller wants a human-only side effect,
    /// such as "send this message and create one task" without triggering agent replies.
    #[serde(default, rename = "suppressAgentDelivery")]
    pub suppress_agent_delivery: bool,
    /// When true, skip broadcasting the message.created event via WebSocket.
    /// The sender already has an optimistic copy and will promote it on HTTP ack.
    #[serde(default, rename = "suppressEvent")]
    pub suppress_event: bool,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct SendResponse {
    #[serde(rename = "messageId")]
    pub message_id: String,
    pub seq: i64,
    #[serde(rename = "createdAt")]
    pub created_at: String,
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

#[derive(Debug, serde::Serialize)]
pub struct InboxResponse {
    pub conversations: Vec<PublicInboxConversationNotification>,
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
    /// Count of unread top-level messages (excludes thread replies).
    pub unread_count: i64,
    /// Count of unread thread replies across all threads in this conversation.
    pub thread_unread_count: i64,
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
            thread_unread_count: v.thread_unread_count,
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
    /// Count of unread top-level messages in the conversation (excludes thread replies).
    /// Shown in the sidebar channel badge.
    #[serde(rename = "conversationUnreadCount")]
    pub conversation_unread_count: i64,
    #[serde(rename = "conversationLastReadSeq")]
    pub conversation_last_read_seq: i64,
    #[serde(rename = "conversationLatestSeq")]
    pub conversation_latest_seq: i64,
    /// Count of unread thread replies across all threads in this conversation.
    /// Shown in the thread tab badge, not in the sidebar.
    #[serde(rename = "conversationThreadUnreadCount")]
    pub conversation_thread_unread_count: i64,
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
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?
        .unwrap_or(SenderType::Human))
}

fn require_channel_membership(
    state: &AppState,
    actor_id: &str,
    channel: &Channel,
    denied_label: &str,
) -> Result<(), (axum::http::StatusCode, Json<super::ErrorResponse>)> {
    if !state
        .store
        .is_member(&channel.name, actor_id)
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?
    {
        return Err(app_err!(
            AppErrorCode::MessageNotAMember,
            "you are not a member of channel {}",
            denied_label
        ));
    }
    Ok(())
}

fn load_channel_by_id(
    store: &Store,
    channel_id: &str,
) -> Result<Channel, (axum::http::StatusCode, Json<super::ErrorResponse>)> {
    store
        .get_channel_by_id(channel_id)
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?
        .ok_or_else(|| app_err!(StatusCode::BAD_REQUEST, "channel not found"))
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
    require_channel_membership(state, actor_id, channel, denied_label)?;

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
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?;

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
    require_channel_membership(state, actor_id, channel, denied_label)?;

    let inbox = state
        .store
        .get_channel_thread_inbox(&channel.name, actor_id)
        .map_err(internal_err)?;
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
    require_channel_membership(state, actor_id, channel, denied_label)?;

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
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?;

    let notification = state
        .store
        .get_inbox_conversation_notification_for_member(&channel.id, actor_id)
        .map_err(internal_err)?
        .ok_or_else(|| {
            app_err!(
                StatusCode::INTERNAL_SERVER_ERROR,
                "inbox notification row missing after read cursor"
            )
        })?;

    let thread_snapshot = if let Some(parent_id) = thread_parent_id {
        state
            .store
            .get_thread_notification_state(&channel.name, parent_id, actor_id)
            .map_err(internal_err)?
    } else {
        None
    };

    Ok(Json(ReadCursorResponse {
        ok: true,
        conversation_unread_count: notification.unread_count,
        conversation_last_read_seq: notification.last_read_seq,
        conversation_latest_seq: notification.latest_seq,
        conversation_thread_unread_count: notification.thread_unread_count,
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
    suppress_agent_delivery: bool,
    suppress_event: bool,
) -> ApiResult<SendResponse> {
    let store = &state.store;
    let sender_type = sender_type_for_actor(store, actor_id)?;

    // Look up active trace run_id for agent senders.
    let run_id = if sender_type == SenderType::Agent {
        state.lifecycle.active_run_id(actor_id)
    } else {
        None
    };

    let preview = content_preview(content);
    let target_label = match thread_parent_id {
        Some(parent_id) => format!("#{}:{parent_id}", channel.name),
        None => format!("#{}", channel.name),
    };
    info!(agent = %actor_id, target = %target_label, content = %preview, "send_message");

    let message_id = store
        .create_message(CreateMessage {
            channel_name: &channel.name,
            thread_parent_id,
            sender_name: actor_id,
            sender_type,
            content,
            attachment_ids,
            suppress_event,
            run_id: run_id.as_deref(),
        })
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?;

    info!(agent = %actor_id, msg = %message_id, content=%content, "send_message ok");

    if !suppress_agent_delivery {
        forward_team_mentions(state, &channel.name, actor_id, sender_type, content)
            .await
            .map_err(internal_err)?;
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

    let message_view = store
        .get_conversation_message_view(&message_id)
        .map_err(internal_err)?
        .ok_or_else(|| {
            app_err!(
                StatusCode::INTERNAL_SERVER_ERROR,
                "sent message missing from projection"
            )
        })?;

    Ok(Json(SendResponse {
        message_id,
        seq: message_view.seq,
        created_at: message_view.created_at,
    }))
}

static MENTION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"@([A-Za-z0-9_-]+)").expect("team mention regex is valid"));

/// Mirror `@team-name` mentions into the corresponding team channel.
async fn forward_team_mentions(
    state: &AppState,
    channel_name: &str,
    sender_name: &str,
    sender_type: SenderType,
    content: &str,
) -> anyhow::Result<()> {
    let mentions = MENTION_RE
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
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?;

    let channel = load_channel_by_id(store, &channel_id)?;
    send_message_to_channel(
        &state,
        &agent_id,
        &channel,
        thread_parent_id.as_deref(),
        &req.content,
        &req.attachment_ids,
        req.suppress_agent_delivery,
        req.suppress_event,
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
    let timeout_ms = params.timeout.unwrap_or(30_000).min(60_000);

    let messages = store
        .get_messages_for_agent(&agent_id, true)
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?;

    if !messages.is_empty() {
        info!(agent = %agent_id, count = messages.len(), "receive_message: got messages immediately");
        for m in &messages {
            info!(agent = %agent_id, target = %format!("{}:{}", m.channel_type, m.channel_name), sender = %m.sender_name, content = %m.content.chars().take(120).collect::<String>(), "  ← message");
        }

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
                    .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?;
                if !messages.is_empty() {
                    info!(agent = %agent_id, count = messages.len(), "receive_message: woke up with messages");
                    for m in &messages {
                        info!(agent = %agent_id, target = %format!("{}:{}", m.channel_type, m.channel_name), sender = %m.sender_name, content = %m.content.chars().take(120).collect::<String>(), "  ← message");
                    }

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
        .ok_or_else(|| app_err!(StatusCode::BAD_REQUEST, "missing channel parameter"))?;
    if let Some(ref ch) = Some(&channel_target) {
        debug!(agent = %agent_id, channel = %ch, "read_history");
    }

    let store = &state.store;
    let (channel_name, thread_parent_id) =
        resolve_history_target(store, &agent_id, &channel_target)
            .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?;
    let limit = params.limit.unwrap_or(50);
    let channel = store
        .get_channel_by_name(&channel_name)
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?
        .ok_or_else(|| app_err!(StatusCode::BAD_REQUEST, "channel not found"))?;
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
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?;
    Ok(Json(ResolveChannelResponse { channel_id }))
}

pub async fn handle_public_inbox(State(state): State<AppState>) -> ApiResult<InboxResponse> {
    let actor_id = public_viewer_name();
    if state
        .store
        .lookup_sender_type(&actor_id)
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?
        .is_none()
    {
        return Err(app_err!(
            StatusCode::BAD_REQUEST,
            "viewer not found: {}",
            actor_id
        ));
    }

    let conversations: Vec<PublicInboxConversationNotification> = state
        .store
        .get_inbox_conversation_notifications(&actor_id)
        .map_err(internal_err)?
        .iter()
        .map(PublicInboxConversationNotification::from)
        .collect();
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
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?
        .is_none()
    {
        return Err(app_err!(
            StatusCode::BAD_REQUEST,
            "viewer not found: {}",
            actor_id
        ));
    }

    let channel = load_channel_by_id(&state.store, &conversation_id)?;
    if !state
        .store
        .is_member(&channel.name, &actor_id)
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?
    {
        return Err(app_err!(
            StatusCode::BAD_REQUEST,
            "you are not a member of channel {}",
            conversation_id
        ));
    }

    let notification = state
        .store
        .get_inbox_conversation_notification_for_member(&channel.id, &actor_id)
        .map_err(internal_err)?
        .ok_or_else(|| {
            app_err!(
                StatusCode::BAD_REQUEST,
                "inbox row not found for this member"
            )
        })?;

    let conversation = PublicInboxConversationNotification::from(&notification);

    let thread = if let Some(ref parent_id) = query.thread_parent_id {
        state
            .store
            .get_thread_notification_state(&channel.name, parent_id, &actor_id)
            .map_err(internal_err)?
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
        return Err(app_err!(
            StatusCode::BAD_REQUEST,
            "cannot create a dm with yourself"
        ));
    }
    if state
        .store
        .lookup_sender_type(&actor_id)
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?
        .is_none()
    {
        return Err(app_err!(
            StatusCode::BAD_REQUEST,
            "viewer not found: {}",
            actor_id
        ));
    }
    if state
        .store
        .lookup_sender_type(&peer_name)
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?
        .is_none()
    {
        return Err(app_err!(
            StatusCode::BAD_REQUEST,
            "peer not found: {}",
            peer_name
        ));
    }

    let target = format!("dm:@{}", peer_name);
    let (channel_id, _) = state
        .store
        .resolve_target(&target, &actor_id)
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?;
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
        req.suppress_agent_delivery,
        req.suppress_event,
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
        // Associate the channel with the agent's trace run before notifying/starting.
        state.lifecycle.set_run_channel(&recipient_name, channel_id);
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

// ── Trace history ──

#[derive(Serialize)]
pub struct TraceEventsResponse {
    pub events: Vec<serde_json::Value>,
}

pub async fn handle_trace_events(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
) -> ApiResult<TraceEventsResponse> {
    let viewer = public_viewer_name();
    // Check that the viewer is a member of the channel this run belongs to.
    let run_channel_id = state
        .store
        .get_run_channel_id(&run_id)
        .map_err(internal_err)?;
    match run_channel_id {
        Some(ch_id) => {
            if !state
                .store
                .channel_member_exists(&ch_id, &viewer)
                .map_err(internal_err)?
            {
                return Err(app_err!(
                    StatusCode::BAD_REQUEST,
                    "not a member of the channel for this run"
                ));
            }
        }
        None => {
            return Err(app_err!(StatusCode::BAD_REQUEST, "run not found"));
        }
    }

    let events = state
        .store
        .get_trace_events(&run_id)
        .map_err(internal_err)?;
    Ok(Json(TraceEventsResponse { events }))
}

// ── Agent runs ──

#[derive(Serialize)]
pub struct AgentRunsResponse {
    pub runs: Vec<serde_json::Value>,
}

pub async fn handle_agent_runs(
    State(state): State<AppState>,
    Path(agent_name): Path<String>,
) -> ApiResult<AgentRunsResponse> {
    let viewer = public_viewer_name();
    let runs = state
        .store
        .get_agent_runs(&agent_name, &viewer, 20)
        .map_err(internal_err)?;
    Ok(Json(AgentRunsResponse { runs }))
}
