use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamEvent {
    pub event_type: String,
    pub channel_id: String,
    pub latest_seq: i64,
    pub event_payload: Value,
    pub schema_version: u32,
}

impl StreamEvent {
    pub fn new(channel_id: String, latest_seq: i64, event_payload: Value) -> Self {
        Self {
            event_type: "message.created".to_string(),
            channel_id,
            latest_seq,
            event_payload,
            schema_version: 1,
        }
    }
}

/// Cross-channel task state delta. Fanned out globally to every connected
/// realtime client so the parent-channel `task_card` host message can
/// re-render even when the viewer is not a member of the task's sub-channel.
/// Mutations: create, status transition, claim, unclaim. The frontend keys
/// updates by `task_id` and patches its in-memory `tasksById` slice.
///
/// camelCase serialization matches the existing realtime envelope contract
/// (see `forward_stream_event` payload shape).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskUpdateEvent {
    pub task_id: String,
    pub channel_id: String,
    pub task_number: i64,
    pub status: String,
    pub owner: Option<String>,
    pub sub_channel_id: Option<String>,
    pub updated_at: String,
}
