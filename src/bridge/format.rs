use serde_json::Value;

use crate::store::tasks::events::{TaskEventAction, TaskEventPayload};
use crate::store::tasks::TaskStatus;

pub(super) fn to_local_time(iso: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(iso)
        .map(|dt| {
            dt.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|_| iso.to_string())
}

pub(super) fn format_attachments(attachments: Option<&Value>) -> String {
    match attachments.and_then(|a| a.as_array()) {
        Some(arr) if !arr.is_empty() => {
            let count = arr.len();
            let details: Vec<String> = arr
                .iter()
                .map(|a| {
                    let filename = a.get("filename").and_then(|v| v.as_str()).unwrap_or("file");
                    let id = a.get("id").and_then(|v| v.as_str()).unwrap_or("?");
                    format!("{} (id:{})", filename, id)
                })
                .collect();
            let plural = if count > 1 { "s" } else { "" };
            format!(
                " [{} image{}: {} \u{2014} use view_file to see]",
                count,
                plural,
                details.join(", ")
            )
        }
        _ => String::new(),
    }
}

/// Format a message `content` string for agent consumption. Task-event messages
/// (`sender_type = "system"` + JSON payload tagged `kind: "task_event"`) are
/// rendered as a one-line human sentence so agents don't see raw JSON in their
/// context. All other sender types pass through unchanged. Non-task system
/// messages (e.g. legacy channel kickoff) also pass through.
///
/// Any malformed or partially-malformed task_event payload falls back to the
/// raw string so agents are never starved of content — consistency with the
/// frontend parser is a nicety, not an invariant here.
pub fn format_message_for_agent(sender_type: &str, content: &str) -> String {
    if sender_type != "system" {
        return content.to_string();
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(content) else {
        return content.to_string();
    };
    if value.get("kind").and_then(|k| k.as_str()) != Some("task_event") {
        return content.to_string();
    }
    match parse_task_event_from_value(&value) {
        Some(payload) => payload.as_agent_sentence(),
        None => content.to_string(),
    }
}

fn parse_task_event_from_value(value: &serde_json::Value) -> Option<TaskEventPayload> {
    let action = match value.get("action")?.as_str()? {
        "created" => TaskEventAction::Created,
        "claimed" => TaskEventAction::Claimed,
        "unclaimed" => TaskEventAction::Unclaimed,
        "status_changed" => TaskEventAction::StatusChanged,
        _ => return None,
    };
    let next_status = TaskStatus::from_status_str(value.get("nextStatus")?.as_str()?)?;
    // prevStatus: mirror the frontend contract — present-but-invalid means
    // producer is broken, so fail the parse and fall back to raw content.
    let prev_status = match value.get("prevStatus") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) => match TaskStatus::from_status_str(s) {
            Some(status) => Some(status),
            None => return None,
        },
        Some(_) => return None,
    };
    let task_number = value.get("taskNumber")?.as_i64()?;
    // claimedBy: same contract — if present and not null, it must be a string.
    let claimed_by = match value.get("claimedBy") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(_) => return None,
    };

    Some(TaskEventPayload {
        action,
        task_number,
        title: value.get("title")?.as_str()?.to_string(),
        sub_channel_id: value.get("subChannelId")?.as_str()?.to_string(),
        actor: value.get("actor")?.as_str()?.to_string(),
        prev_status,
        next_status,
        claimed_by,
    })
}
