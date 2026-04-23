use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Lifecycle state of a task proposal. A proposal starts `Pending`, then
/// transitions exactly once to `Accepted` (the user clicked create) or
/// `Dismissed` (the user declined). Terminal in both transitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskProposalStatus {
    Pending,
    Accepted,
    Dismissed,
}

impl TaskProposalStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Accepted => "accepted",
            Self::Dismissed => "dismissed",
        }
    }

    pub fn from_status_str(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "accepted" => Some(Self::Accepted),
            "dismissed" => Some(Self::Dismissed),
            _ => None,
        }
    }
}

/// A persisted task proposal row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskProposal {
    /// UUID primary key.
    pub id: String,
    /// Parent channel the proposal was posted to.
    pub channel_id: String,
    /// Agent or human who proposed the task.
    pub proposed_by: String,
    /// Proposed task title (one-line summary).
    pub title: String,
    pub status: TaskProposalStatus,
    pub created_at: DateTime<Utc>,
    /// Populated when `status == Accepted`. Links to the resulting task's
    /// per-channel number — NOT the task UUID — because the UI uses
    /// numbers everywhere.
    pub accepted_task_number: Option<i64>,
    /// Populated when `status == Accepted`. Sub-channel id of the task
    /// created on acceptance, so the UI can deep-link.
    pub accepted_sub_channel_id: Option<String>,
    /// Member name that accepted or dismissed.
    pub resolved_by: Option<String>,
    pub resolved_at: Option<DateTime<Utc>>,
}
