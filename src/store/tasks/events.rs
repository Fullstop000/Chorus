//! Task-event system messages emitted on every task mutation.
//!
//! Every event becomes a `sender_type = 'system'` message in the parent channel
//! with a JSON `content` payload. The frontend parses the JSON; agents receive
//! a pre-formatted human sentence via the bridge adapter.

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::store::tasks::TaskStatus;

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
