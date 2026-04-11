use std::collections::VecDeque;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

// ── Types owned by this module ──

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ActivityEntry {
    Start {
        is_resume: bool,
    },
    Thinking {
        text: String,
    },
    ToolCall {
        tool_name: String,
        tool_input: String,
    },
    ToolResult {
        tool_name: String,
        content: String,
    },
    Text {
        text: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityLogEntry {
    pub seq: u64,
    pub timestamp_ms: u64,
    pub entry: ActivityEntry,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ActivityLogResponse {
    pub entries: Vec<ActivityLogEntry>,
    pub agent_activity: String,
    pub agent_detail: String,
}

pub const ACTIVITY_LOG_MAX: usize = 500;

/// Per-agent in-memory activity log (ring buffer, up to ACTIVITY_LOG_MAX entries).
#[derive(Default)]
pub struct AgentActivityLog {
    entries: VecDeque<ActivityLogEntry>,
    next_seq: u64,
    /// Current activity state: online | thinking | working | offline
    pub activity: String,
    pub detail: String,
}

impl AgentActivityLog {
    pub fn push(&mut self, entry: ActivityEntry) {
        let timestamp_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        self.entries.push_back(ActivityLogEntry {
            seq: self.next_seq,
            timestamp_ms,
            entry,
        });
        self.next_seq += 1;
        if self.entries.len() > ACTIVITY_LOG_MAX {
            self.entries.pop_front();
        }
    }

    /// Update the last `ToolResult` entry in-place if it has the same `tool_name`,
    /// otherwise push a new entry. This prevents streaming chunks from flooding
    /// the log with dozens of near-identical entries.
    pub fn upsert_tool_result(&mut self, tool_name: String, content: String) {
        if let Some(last) = self.entries.back_mut() {
            if let ActivityEntry::ToolResult {
                tool_name: ref existing_name,
                content: ref mut existing_content,
            } = last.entry
            {
                if *existing_name == tool_name {
                    *existing_content = content;
                    last.timestamp_ms = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64;
                    return;
                }
            }
        }
        self.push(ActivityEntry::ToolResult { tool_name, content });
    }

    /// Update the most recent `ToolCall` entry's input in-place.
    /// Called when ACP runtimes deliver the real args in a deferred `tool_call_update`.
    pub fn update_last_tool_call_input(&mut self, new_input: String) {
        for entry in self.entries.iter_mut().rev() {
            if let ActivityEntry::ToolCall {
                tool_input: ref mut existing_input,
                ..
            } = entry.entry
            {
                *existing_input = new_input;
                entry.timestamp_ms = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                return;
            }
        }
    }

    pub fn set_state(&mut self, activity: &str, detail: &str) {
        self.activity = activity.to_string();
        self.detail = detail.to_string();
    }

    pub fn entries_since(&self, after_seq: u64) -> Vec<ActivityLogEntry> {
        self.entries
            .iter()
            .filter(|e| e.seq > after_seq)
            .cloned()
            .collect()
    }

    pub fn all_entries(&self) -> Vec<ActivityLogEntry> {
        self.entries.iter().cloned().collect()
    }
}

/// Thread-safe map of activity logs keyed by agent name.
pub type ActivityLogMap = std::sync::Mutex<std::collections::HashMap<String, AgentActivityLog>>;

/// Push a new entry for an agent (creates the log if absent).
pub fn push_activity(logs: &ActivityLogMap, agent_name: &str, entry: ActivityEntry) {
    logs.lock()
        .unwrap()
        .entry(agent_name.to_string())
        .or_default()
        .push(entry);
}

/// Upsert a ToolResult for an agent: update the last entry in-place if it
/// matches the same tool_name, otherwise push a new entry.
pub fn upsert_tool_result_activity(
    logs: &ActivityLogMap,
    agent_name: &str,
    tool_name: String,
    content: String,
) {
    logs.lock()
        .unwrap()
        .entry(agent_name.to_string())
        .or_default()
        .upsert_tool_result(tool_name, content);
}

/// Update the most recent ToolCall's input for an agent.
/// Used when ACP runtimes deliver args in a deferred `tool_call_update`.
pub fn update_tool_call_input(logs: &ActivityLogMap, agent_name: &str, new_input: String) {
    logs.lock()
        .unwrap()
        .entry(agent_name.to_string())
        .or_default()
        .update_last_tool_call_input(new_input);
}

/// Update the activity state for an agent (also appends a Status entry).
pub fn set_activity_state(logs: &ActivityLogMap, agent_name: &str, activity: &str, detail: &str) {
    logs.lock()
        .unwrap()
        .entry(agent_name.to_string())
        .or_default()
        .set_state(activity, detail);
}

/// Read the activity log for a single agent.
pub fn get_activity_log(
    logs: &ActivityLogMap,
    agent_name: &str,
    after_seq: Option<u64>,
) -> ActivityLogResponse {
    let map = logs.lock().unwrap();
    match map.get(agent_name) {
        Some(log) => {
            let entries = match after_seq {
                Some(seq) => log.entries_since(seq),
                None => log.all_entries(),
            };
            ActivityLogResponse {
                entries,
                agent_activity: log.activity.clone(),
                agent_detail: log.detail.clone(),
            }
        }
        None => ActivityLogResponse {
            entries: vec![],
            agent_activity: "offline".to_string(),
            agent_detail: String::new(),
        },
    }
}

/// Snapshot of all agents' current activity states: `(name, activity, detail)`.
pub fn all_activity_states(logs: &ActivityLogMap) -> Vec<(String, String, String)> {
    logs.lock()
        .unwrap()
        .iter()
        .map(|(name, log)| (name.clone(), log.activity.clone(), log.detail.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_set_activity_state_does_not_emit_log_entries() {
        let logs = ActivityLogMap::default();

        set_activity_state(&logs, "bot1", "online", "Idle");
        set_activity_state(&logs, "bot1", "online", "Idle");

        let resp = get_activity_log(&logs, "bot1", None);
        assert_eq!(
            resp.entries.len(),
            0,
            "set_activity_state should not create log entries — only updates activity state fields"
        );
        assert_eq!(resp.agent_activity, "online");
        assert_eq!(resp.agent_detail, "Idle");
    }
}
