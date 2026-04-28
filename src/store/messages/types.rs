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
    /// Server-synthetic sender for system notices (e.g. channel kickoff).
    /// Never appears in `agents` / `humans` / `channel_members`.
    System,
}

impl SenderType {
    /// Value stored in `messages.sender_type` / `channel_members.member_type` and in JSON.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::Agent => "agent",
            Self::System => "system",
        }
    }

    /// Parse DB / wire string; unknown values default to [`Human`] (matches prior `parse_sender_type`).
    pub fn from_sender_type_str(s: &str) -> Self {
        match s {
            "agent" => Self::Agent,
            "system" => Self::System,
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
    /// Set when this message was forwarded from another channel (e.g. via @team mention).
    #[serde(rename = "forwardedFrom", skip_serializing_if = "Option::is_none")]
    pub forwarded_from: Option<ForwardedFrom>,
    /// Telescope trace run id linking to trace_events.
    #[serde(rename = "runId", skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// JSON summary of the trace run for collapsed Telescope.
    #[serde(rename = "traceSummary", skip_serializing_if = "Option::is_none")]
    pub trace_summary: Option<String>,
    /// Kind-discriminated structured payload (member_joined, task_event, …).
    /// Always paired with a human-readable `content` fallback. Storage is
    /// untyped — each renderer narrows by inspecting `payload.kind` at runtime.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
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
    /// Forward provenance when present.
    pub forwarded_from: Option<ForwardedFrom>,
    /// Telescope trace run id.
    pub run_id: Option<String>,
    /// JSON trace summary for collapsed Telescope.
    pub trace_summary: Option<String>,
    /// Kind-discriminated structured payload (parsed from the `payload` JSON column).
    pub payload: Option<Value>,
}

impl ConversationMessageView {
    pub(crate) fn from_projection_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        let forwarded_from_raw: Option<String> = row.get("forwarded_from")?;
        let payload_raw: Option<String> = row.get("payload")?;
        Ok(Self {
            message_id: row.get("message_id")?,
            conversation_id: row.get("conversation_id")?,
            conversation_name: row.get("conversation_name")?,
            conversation_type: row.get("conversation_type")?,
            sender_name: row.get("sender_name")?,
            sender_type: row.get("sender_type")?,
            sender_deleted: row.get::<_, i64>("sender_deleted")? > 0,
            content: row.get("content")?,
            created_at: row.get("created_at")?,
            seq: row.get("seq")?,
            attachments: Vec::new(),
            forwarded_from: Store::parse_forwarded_from_raw(forwarded_from_raw.as_deref()),
            run_id: row.get("run_id")?,
            trace_summary: row.get("trace_summary")?,
            payload: parse_payload_raw(payload_raw.as_deref()),
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
            forwarded_from: self.forwarded_from.clone(),
            run_id: self.run_id.clone(),
            trace_summary: self.trace_summary.clone(),
            payload: self.payload.clone(),
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
            "runId": self.run_id,
            "traceSummary": self.trace_summary,
            "payload": self.payload,
        })
    }
}

/// Parse the raw `messages.payload` column into a JSON value. Malformed JSON
/// returns `None` rather than erroring — the renderer falls back to `content`,
/// so a corrupt payload degrades visibly instead of crashing the history fetch.
fn parse_payload_raw(raw: Option<&str>) -> Option<Value> {
    let raw = raw?;
    match serde_json::from_str::<Value>(raw) {
        Ok(p) => Some(p),
        Err(err) => {
            tracing::warn!(error = %err, raw = %raw, "failed to parse payload — falling back to content");
            None
        }
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

/// Full payload sent over WebSocket for `message.created` events.
/// Serialized as camelCase JSON so the TypeScript frontend can read it directly.
/// The frontend constructs a HistoryMessage from this and appends it instantly,
/// avoiding an extra API round-trip.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MessageCreatedPayload {
    /// Unique message UUID (matches the DB primary key).
    pub message_id: String,
    /// UUID of the channel/DM conversation this message belongs to.
    pub conversation_id: String,
    /// One of "channel", "dm", or "team".
    pub conversation_type: String,
    /// Author identity — name and type ("human" | "agent").
    pub sender: MessageSenderInfo,
    /// True if the sender has been soft-deleted (hide content, show tombstone).
    #[serde(default)]
    pub sender_deleted: bool,
    /// Plain-text body of the message.
    pub content: String,
    /// Attachment UUIDs referenced by this message (empty at creation time).
    #[serde(default)]
    pub attachment_ids: Vec<String>,
    /// Attachment metadata objects (empty at creation time).
    #[serde(default)]
    pub attachments: Vec<serde_json::Value>,
    /// Monotonically increasing sequence number within the conversation.
    pub seq: i64,
    /// ISO-8601 timestamp of when the message was persisted.
    pub created_at: String,
    /// Kind-discriminated structured payload (only set for `senderType == 'system'`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
}

/// Sender identity embedded inside the WebSocket event payload.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct MessageSenderInfo {
    /// Display name of the sender (e.g. "bytedance", "Coder").
    pub name: String,
    /// "human" for user messages; "agent" for bot/agent messages.
    #[serde(rename = "type")]
    pub sender_type: String,
}

impl InsertedMessage {
    pub(crate) fn to_event_payload(
        &self,
        conversation_id: &str,
        conversation_type: &str,
        sender_name: &str,
        sender_type: &str,
        content: &str,
    ) -> MessageCreatedPayload {
        self.to_event_payload_with_payload(
            conversation_id,
            conversation_type,
            sender_name,
            sender_type,
            content,
            None,
        )
    }

    pub(crate) fn to_event_payload_with_payload(
        &self,
        conversation_id: &str,
        conversation_type: &str,
        sender_name: &str,
        sender_type: &str,
        content: &str,
        payload: Option<Value>,
    ) -> MessageCreatedPayload {
        MessageCreatedPayload {
            message_id: self.id.clone(),
            conversation_id: conversation_id.to_string(),
            conversation_type: conversation_type.to_string(),
            sender: MessageSenderInfo {
                name: sender_name.to_string(),
                sender_type: sender_type.to_string(),
            },
            sender_deleted: false,
            content: content.to_string(),
            attachment_ids: Vec::new(),
            attachments: Vec::new(),
            seq: self.seq,
            created_at: chrono::Utc::now().to_rfc3339(),
            payload,
        }
    }
}
