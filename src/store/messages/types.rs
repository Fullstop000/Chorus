use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::store::Store;

// ── Types owned by this module ──

/// Who authored a message or holds channel membership.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SenderType {
    /// Human user row from `humans`.
    Human,
    /// Agent row from `agents`.
    Agent,
}

impl SenderType {
    /// Value stored in `messages.sender_type` / `channel_members.member_type` and in JSON.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::Agent => "agent",
        }
    }

    /// Parse DB / wire string; unknown values default to [`Human`] (matches prior `parse_sender_type`).
    pub fn from_sender_type_str(s: &str) -> Self {
        match s {
            "agent" => Self::Agent,
            _ => Self::Human,
        }
    }
}

/// Provenance metadata attached to a forwarded message, capturing the origin
/// channel and the original sender so recipients can trace where it came from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForwardedFrom {
    /// Source channel slug (no `#`).
    pub channel_name: String,
    /// Original author handle.
    pub sender_name: String,
}

/// In-memory / store representation of one `messages` row plus attachment ids.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// UUID message id.
    pub id: String,
    /// Owning channel id.
    pub channel_id: String,
    /// When set, this message is a thread reply under the parent message id.
    pub thread_parent_id: Option<String>,
    /// Author handle.
    pub sender_name: String,
    /// Author kind.
    pub sender_type: SenderType,
    /// Markdown or plain text body.
    pub content: String,
    /// Wall-clock timestamp from SQLite.
    pub created_at: DateTime<Utc>,
    /// Monotonic per-channel ordering.
    pub seq: i64,
    /// Attachment UUIDs linked via `message_attachments`.
    pub attachment_ids: Vec<String>,
    /// Set when this message was forwarded from another channel.
    pub forwarded_from: Option<ForwardedFrom>,
}

/// Wire shape pushed to agent bridges on receive (names resolved for prompts).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceivedMessage {
    /// Same as `messages.id`.
    pub message_id: String,
    /// Target channel slug.
    pub channel_name: String,
    /// API string for channel kind (`channel`, `dm`, …).
    pub channel_type: String,
    /// Parent channel when this is a thread under another room.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_channel_name: Option<String>,
    /// Parent channel kind string when applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_channel_type: Option<String>,
    /// Author handle.
    pub sender_name: String,
    /// `human` or `agent` string for JSON consumers.
    pub sender_type: String,
    /// Message body.
    pub content: String,
    /// ISO-ish timestamp string for the bridge.
    pub timestamp: String,
    /// Inline attachment metadata when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<AttachmentRef>>,
    /// Forward provenance when this is a cross-post.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forwarded_from: Option<ForwardedFrom>,
}

/// Minimal attachment descriptor embedded in history / receive payloads.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentRef {
    /// Attachment UUID.
    pub id: String,
    /// Original filename for display.
    pub filename: String,
}

/// One message in paginated channel history for the UI.
#[derive(Debug, Serialize, Deserialize)]
pub struct HistoryMessage {
    /// Message UUID.
    pub id: String,
    /// Channel sequence number.
    pub seq: i64,
    /// Body text.
    pub content: String,
    /// Author handle.
    #[serde(rename = "senderName")]
    pub sender_name: String,
    /// `human` or `agent`.
    #[serde(rename = "senderType")]
    pub sender_type: String,
    /// ISO timestamp string.
    #[serde(rename = "createdAt")]
    pub created_at: String,
    /// True when the sender was soft-deleted (tombstone display).
    #[serde(rename = "senderDeleted")]
    pub sender_deleted: bool,
    /// Linked files when any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<AttachmentRef>>,
    /// Number of thread replies when loaded.
    #[serde(rename = "replyCount", skip_serializing_if = "Option::is_none")]
    pub reply_count: Option<i64>,
    /// Set when this message was forwarded from another channel (e.g. via @team mention).
    #[serde(rename = "forwardedFrom", skip_serializing_if = "Option::is_none")]
    pub forwarded_from: Option<ForwardedFrom>,
}

/// Explicit read-model row for conversation history while `messages` remains
/// the transitional backing storage for the projected chat view.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationMessageView {
    /// Message UUID.
    pub message_id: String,
    /// Owning conversation UUID.
    pub conversation_id: String,
    /// Conversation slug for the UI/API layer.
    pub conversation_name: String,
    /// `channel`, `dm`, `team`, or `system`.
    pub conversation_type: String,
    /// Parent message id when this row is a thread reply.
    pub thread_parent_id: Option<String>,
    /// Author handle.
    pub sender_name: String,
    /// `human` or `agent`.
    pub sender_type: String,
    /// True when the sender has been soft-deleted.
    pub sender_deleted: bool,
    /// Message body.
    pub content: String,
    /// ISO timestamp string.
    pub created_at: String,
    /// Monotonic per-conversation order.
    pub seq: i64,
    /// Linked files when present.
    pub attachments: Vec<AttachmentRef>,
    /// Reply count for top-level messages.
    pub reply_count: Option<i64>,
    /// Forward provenance when present.
    pub forwarded_from: Option<ForwardedFrom>,
}

/// Explicit read-model row for thread summary state projected from conversation
/// messages while thread semantics remain conversation-local.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadSummaryView {
    /// Owning conversation UUID.
    pub conversation_id: String,
    /// Top-level message id that anchors the thread.
    pub parent_message_id: String,
    /// Number of replies currently in the thread.
    pub reply_count: i64,
    /// Most recent reply id when at least one reply exists.
    pub last_reply_message_id: Option<String>,
    /// Timestamp for the most recent reply when present.
    pub last_reply_at: Option<String>,
    /// Number of unique participants including the parent author.
    pub participant_count: i64,
}

/// Member-specific thread inbox row for one conversation, combining parent
/// preview, thread summary metadata, and unread/read cursor state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelThreadInboxEntry {
    /// Owning conversation UUID.
    #[serde(rename = "conversationId")]
    pub conversation_id: String,
    /// Top-level message id that anchors the thread.
    #[serde(rename = "threadParentId")]
    pub thread_parent_id: String,
    /// Parent message sequence in the conversation stream.
    #[serde(rename = "parentSeq")]
    pub parent_seq: i64,
    /// Parent author handle.
    #[serde(rename = "parentSenderName")]
    pub parent_sender_name: String,
    /// Parent author kind.
    #[serde(rename = "parentSenderType")]
    pub parent_sender_type: String,
    /// Parent message preview/source text.
    #[serde(rename = "parentContent")]
    pub parent_content: String,
    /// Parent message timestamp.
    #[serde(rename = "parentCreatedAt")]
    pub parent_created_at: String,
    /// Current number of replies in the thread.
    #[serde(rename = "replyCount")]
    pub reply_count: i64,
    /// Number of unique participants including the parent author.
    #[serde(rename = "participantCount")]
    pub participant_count: i64,
    /// Latest reply sequence in the conversation stream.
    #[serde(rename = "latestSeq")]
    pub latest_seq: i64,
    /// Highest read reply sequence for this member in this thread.
    #[serde(rename = "lastReadSeq")]
    pub last_read_seq: i64,
    /// Replies newer than `last_read_seq`.
    #[serde(rename = "unreadCount")]
    pub unread_count: i64,
    /// Most recent reply id when present.
    #[serde(rename = "lastReplyMessageId")]
    pub last_reply_message_id: Option<String>,
    /// Most recent reply timestamp when present.
    #[serde(rename = "lastReplyAt")]
    pub last_reply_at: Option<String>,
}

/// Channel-scoped thread inbox payload for one member.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelThreadInbox {
    /// Total unread replies across all listed threads.
    #[serde(rename = "unreadCount")]
    pub unread_count: i64,
    /// Threads sorted unread-first, then newest activity first.
    pub threads: Vec<ChannelThreadInboxEntry>,
}

impl ThreadSummaryView {
    pub(crate) fn from_projection_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            conversation_id: row.get("conversation_id")?,
            parent_message_id: row.get("parent_message_id")?,
            reply_count: row.get("reply_count")?,
            last_reply_message_id: row.get("last_reply_message_id")?,
            last_reply_at: row.get("last_reply_at")?,
            participant_count: row.get("participant_count")?,
        })
    }
}

impl ConversationMessageView {
    pub(crate) fn from_projection_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        let forwarded_from_raw: Option<String> = row.get("forwarded_from")?;
        let reply_count = row
            .get::<_, Option<i64>>("reply_count")?
            .filter(|count| *count > 0);
        Ok(Self {
            message_id: row.get("message_id")?,
            conversation_id: row.get("conversation_id")?,
            conversation_name: row.get("conversation_name")?,
            conversation_type: row.get("conversation_type")?,
            thread_parent_id: row.get("thread_parent_id")?,
            sender_name: row.get("sender_name")?,
            sender_type: row.get("sender_type")?,
            sender_deleted: row.get::<_, i64>("sender_deleted")? > 0,
            content: row.get("content")?,
            created_at: row.get("created_at")?,
            seq: row.get("seq")?,
            attachments: Vec::new(),
            reply_count,
            forwarded_from: Store::parse_forwarded_from_raw(forwarded_from_raw.as_deref()),
        })
    }

    pub(crate) fn to_history_message(&self) -> HistoryMessage {
        HistoryMessage {
            id: self.message_id.clone(),
            seq: self.seq,
            content: self.content.clone(),
            sender_name: self.sender_name.clone(),
            sender_type: self.sender_type.clone(),
            created_at: self.created_at.clone(),
            sender_deleted: self.sender_deleted,
            attachments: (!self.attachments.is_empty()).then(|| self.attachments.clone()),
            reply_count: self.reply_count,
            forwarded_from: self.forwarded_from.clone(),
        }
    }

    pub(crate) fn to_transport_payload(&self) -> Value {
        let attachment_ids = self
            .attachments
            .iter()
            .map(|attachment| attachment.id.clone())
            .collect::<Vec<_>>();

        json!({
            "messageId": self.message_id,
            "conversationId": self.conversation_id,
            "conversationType": self.conversation_type,
            "threadParentId": self.thread_parent_id,
            "sender": {
                "name": self.sender_name,
                "type": self.sender_type,
            },
            "senderDeleted": self.sender_deleted,
            "content": self.content,
            "attachmentIds": attachment_ids,
            "attachments": self.attachments,
            "seq": self.seq,
            "createdAt": self.created_at,
            "forwardedFrom": self.forwarded_from,
        })
    }
}

/// Consistent history bootstrap payload assembled from one store read.
#[derive(Debug, Serialize, Deserialize)]
pub struct HistorySnapshot {
    /// Page of messages for the requested conversation scope.
    pub messages: Vec<HistoryMessage>,
    /// Whether more history exists beyond this page.
    pub has_more: bool,
    /// Last read sequence for the requesting member in the parent conversation.
    pub last_read_seq: i64,
    /// Latest committed durable event cursor observed alongside this snapshot.
    pub latest_event_id: i64,
    /// Owning conversation stream for this history snapshot.
    pub stream_id: String,
    /// Latest committed position in the owning conversation stream.
    pub stream_pos: i64,
}

/// Compact message row for activity / cross-channel feeds.
#[derive(Debug, Serialize, Deserialize)]
pub struct ActivityMessage {
    /// Message UUID.
    pub id: String,
    /// Channel sequence number.
    pub seq: i64,
    /// Body text.
    pub content: String,
    /// Channel slug where the message lives.
    #[serde(rename = "channelName")]
    pub channel_name: String,
    /// ISO timestamp string.
    #[serde(rename = "createdAt")]
    pub created_at: String,
}

/// Result of inserting a message row before fanout or event derivation.
pub(crate) struct InsertedMessage {
    pub(crate) id: String,
    pub(crate) seq: i64,
}
