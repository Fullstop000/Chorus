//! Task-event system messages emitted on every task mutation.
//!
//! Every event becomes a `sender_type = 'system'` message in the parent channel
//! with a JSON `content` payload. The frontend parses the JSON; agents receive
//! a pre-formatted human sentence via the bridge adapter.

use anyhow::Result;
use rusqlite::Transaction;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::store::channels::Channel;
use crate::store::messages::types::InsertedMessage;
use crate::store::tasks::{TaskInfo, TaskStatus};
use crate::store::Store;

/// What happened to the task. Serialized as snake_case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskEventAction {
    Created,
    Claimed,
    Unclaimed,
    StatusChanged,
}

impl TaskEventAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Claimed => "claimed",
            Self::Unclaimed => "unclaimed",
            Self::StatusChanged => "status_changed",
        }
    }
}

/// Structured payload for one task-event message. Serialized to JSON and stored
/// as the `messages.content` column for the parent channel's system messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskEventPayload {
    pub action: TaskEventAction,
    pub task_number: i64,
    pub title: String,
    pub sub_channel_id: String,
    pub actor: String,
    /// None on Created. Some() on Claimed / Unclaimed / StatusChanged.
    pub prev_status: Option<TaskStatus>,
    pub next_status: TaskStatus,
    /// Current claimer after the event. None when task is unclaimed.
    pub claimed_by: Option<String>,
}

impl TaskEventPayload {
    /// Serialize to a JSON string suitable for `messages.content`. Emits
    /// camelCase field names so the TypeScript frontend can read it without
    /// mapping.
    pub fn to_json_string(&self) -> Result<String, serde_json::Error> {
        let value = json!({
            "kind": "task_event",
            "action": self.action.as_str(),
            "taskNumber": self.task_number,
            "title": self.title,
            "subChannelId": self.sub_channel_id,
            "actor": self.actor,
            "prevStatus": self.prev_status.map(|s| s.as_str()),
            "nextStatus": self.next_status.as_str(),
            "claimedBy": self.claimed_by,
        });
        serde_json::to_string(&value)
    }

    /// Human-readable one-line summary used when serializing for agent
    /// consumers in the bridge layer.
    pub fn as_agent_sentence(&self) -> String {
        match self.action {
            TaskEventAction::Created => format!(
                "[task] {} created #{} \"{}\"",
                self.actor, self.task_number, self.title
            ),
            TaskEventAction::Claimed => format!(
                "[task] {} claimed #{} \"{}\" (now {})",
                self.actor,
                self.task_number,
                self.title,
                self.next_status.as_str()
            ),
            TaskEventAction::Unclaimed => format!(
                "[task] {} unclaimed #{} \"{}\" (now {})",
                self.actor,
                self.task_number,
                self.title,
                self.next_status.as_str()
            ),
            TaskEventAction::StatusChanged => format!(
                "[task] {} → {} on #{} \"{}\"",
                self.actor,
                self.next_status.as_str(),
                self.task_number,
                self.title
            ),
        }
    }
}

/// Wire payload for a `task_card` host message: the parent-channel system
/// message that represents a task in the chat timeline. The frontend reads this
/// JSON directly from `messages.content` and re-renders it on every
/// `task_update` SSE event (status flips, claim/unclaim, dismissal), so the
/// structure carries every field the card needs without a separate `GET /task`
/// round-trip.
///
/// `kind` is an owned `String` (not a `&'static str` literal) so the struct
/// round-trips cleanly through `serde_json::from_str` — a borrowed `&str` would
/// not deserialize from a freshly allocated JSON buffer.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TaskCardWirePayload {
    /// Discriminator: always the literal `"task_card"`. The frontend
    /// routes on this and ignores non-matching system messages.
    pub kind: String,
    pub task_id: String,
    pub task_number: i64,
    pub title: String,
    /// `TaskStatus::as_str()` value — one of proposed/dismissed/todo/
    /// in_progress/in_review/done.
    pub status: String,
    /// Current owner (claimer) handle. `None` when unclaimed or pre-acceptance.
    pub owner: Option<String>,
    pub created_by: String,
    /// Source message id: the chat message this task was carved from.
    pub source_message_id: Option<String>,
    /// Snapshot of the source message's sender at carve time.
    pub snapshot_sender_name: Option<String>,
    pub snapshot_sender_type: Option<String>,
    pub snapshot_content: Option<String>,
    pub snapshot_created_at: Option<String>,
}

/// Insert a `task_card` host message in the task's parent channel. The card
/// is a `sender_type = 'system'` message whose `content` is the
/// [`TaskCardWirePayload`] JSON; the frontend re-renders it in place every time
/// a `task_update` SSE event fires.
///
/// Returns `(InsertedMessage, String)` — caller pushes onto a
/// `pending_events: Vec<(InsertedMessage, String)>` queue and fans out via
/// [`Store::emit_system_stream_events`] AFTER `tx.commit()` to avoid holding
/// the connection mutex across the stream send.
pub(crate) fn post_task_card_message_tx(
    tx: &Transaction<'_>,
    parent_channel: &Channel,
    task: &TaskInfo,
) -> Result<(InsertedMessage, String)> {
    let payload = TaskCardWirePayload {
        kind: "task_card".to_string(),
        task_id: task.id.clone(),
        task_number: task.task_number,
        title: task.title.clone(),
        status: task.status.as_str().to_string(),
        owner: task.owner.clone(),
        created_by: task.created_by.clone(),
        source_message_id: task.source_message_id.clone(),
        snapshot_sender_name: task.snapshot_sender_name.clone(),
        snapshot_sender_type: task.snapshot_sender_type.clone(),
        snapshot_content: task.snapshot_content.clone(),
        snapshot_created_at: task.snapshot_created_at.clone(),
    };
    let content = serde_json::to_string(&payload)?;
    let msg = Store::create_system_message_tx(tx, parent_channel, &content)?;
    Ok((msg, content))
}

/// Post a `task_event` system message into the task's sub-channel. Used for
/// post-acceptance events only (claim / unclaim / status changes); pre-
/// acceptance transitions (Proposed → Todo, Proposed → Dismissed) re-render
/// the parent-channel task card via the `task_update` SSE event instead.
///
/// Returns `(InsertedMessage, String)` on the same fanout contract as
/// [`post_task_card_message_tx`].
pub(crate) fn post_task_event_tx(
    tx: &Transaction<'_>,
    sub_channel_id: &str,
    payload: TaskEventPayload,
) -> Result<(InsertedMessage, String)> {
    // `&Transaction<'_>` derefs to `&Connection`, which is what
    // `get_channel_by_id_inner` accepts — so this stays inside the caller's
    // transaction without opening a second one.
    let sub_channel = Store::get_channel_by_id_inner(tx, sub_channel_id)?
        .ok_or_else(|| anyhow::anyhow!("sub-channel not found: {}", sub_channel_id))?;
    let content = payload.to_json_string()?;
    let msg = Store::create_system_message_tx(tx, &sub_channel, &content)?;
    Ok((msg, content))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_card_payload_roundtrips_json() {
        let p = TaskCardWirePayload {
            kind: "task_card".into(),
            task_id: "abc-123".into(),
            task_number: 7,
            title: "fix login".into(),
            status: "proposed".into(),
            owner: None,
            created_by: "alice".into(),
            source_message_id: Some("msg-1".into()),
            snapshot_sender_name: Some("alice".into()),
            snapshot_sender_type: Some("human".into()),
            snapshot_content: Some("broke on safari".into()),
            snapshot_created_at: Some("2026-04-24T12:00:00Z".into()),
        };
        let json = serde_json::to_string(&p).unwrap();
        let back: TaskCardWirePayload = serde_json::from_str(&json).unwrap();
        assert_eq!(back.task_id, "abc-123");
        assert_eq!(back.kind, "task_card");
        // camelCase discriminator keys are what the TS frontend expects.
        assert!(
            json.contains("\"taskId\":\"abc-123\""),
            "expected camelCase taskId, got: {json}"
        );
        assert!(
            json.contains("\"sourceMessageId\":\"msg-1\""),
            "expected camelCase sourceMessageId, got: {json}"
        );
    }

    #[test]
    fn task_event_wire_field_is_claimed_by_camel_case() {
        // Guard against accidental rename to owner on the wire. The stored
        // chat history carries `claimedBy` — renaming the JSON key would
        // break every persisted task-event message for existing workspaces.
        let payload = TaskEventPayload {
            action: TaskEventAction::Claimed,
            task_number: 1,
            title: "t".into(),
            sub_channel_id: "sub-1".into(),
            actor: "zht".into(),
            prev_status: Some(TaskStatus::Todo),
            next_status: TaskStatus::InProgress,
            claimed_by: Some("zht".into()),
        };
        let json = payload.to_json_string().unwrap();
        assert!(
            json.contains("\"claimedBy\":\"zht\""),
            "expected wire field claimedBy, got: {json}"
        );
        assert!(
            !json.contains("\"owner\""),
            "task_event wire must NOT use owner: {json}"
        );
    }
}
