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
