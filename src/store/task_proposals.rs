use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::store::Store;

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

impl Store {
    /// Create a new pending task proposal and post a companion
    /// `kind: "task_proposal"` chat message into the parent channel so the
    /// UI can render a card. Row + message are inserted in one transaction.
    pub fn create_task_proposal(
        &self,
        channel_id: &str,
        proposed_by: &str,
        title: &str,
    ) -> Result<TaskProposal> {
        let trimmed = title.trim();
        if trimmed.is_empty() {
            return Err(anyhow!("task proposal title must not be empty"));
        }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let channel = Self::get_channel_by_id_inner(&tx, channel_id)?
            .ok_or_else(|| anyhow!("channel not found: {channel_id}"))?;

        let id = Uuid::new_v4().to_string();
        let now = chrono::Utc::now();
        let now_iso = now.to_rfc3339();
        tx.execute(
            "INSERT INTO task_proposals \
             (id, channel_id, proposed_by, title, status, created_at) \
             VALUES (?1, ?2, ?3, ?4, 'pending', ?5)",
            params![id, channel.id, proposed_by, trimmed, now_iso],
        )?;

        let payload = serde_json::json!({
            "kind": "task_proposal",
            "proposalId": id,
            "status": "pending",
            "title": trimmed,
            "proposedBy": proposed_by,
            "proposedAt": now_iso,
            "taskNumber": serde_json::Value::Null,
            "subChannelId": serde_json::Value::Null,
            "subChannelName": serde_json::Value::Null,
        });
        let content = payload.to_string();
        let inserted = Self::create_system_message_tx(&tx, &channel, &content)?;
        let pending = vec![(inserted, content)];
        tx.commit()?;
        drop(conn);

        self.emit_system_stream_events(&channel, pending)?;

        Ok(TaskProposal {
            id,
            channel_id: channel.id,
            proposed_by: proposed_by.to_string(),
            title: trimmed.to_string(),
            status: TaskProposalStatus::Pending,
            created_at: now,
            accepted_task_number: None,
            accepted_sub_channel_id: None,
            resolved_by: None,
            resolved_at: None,
        })
    }

    /// Look up a proposal by id. Returns `Ok(None)` if not found.
    pub fn get_task_proposal_by_id(&self, id: &str) -> Result<Option<TaskProposal>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, channel_id, proposed_by, title, status, created_at, \
                    accepted_task_number, accepted_sub_channel_id, \
                    resolved_by, resolved_at \
             FROM task_proposals WHERE id = ?1",
        )?;
        let row = stmt
            .query_row(params![id], |r| {
                let status_s: String = r.get(4)?;
                let created_at_s: String = r.get(5)?;
                let resolved_at_s: Option<String> = r.get(9)?;
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    status_s,
                    created_at_s,
                    r.get::<_, Option<i64>>(6)?,
                    r.get::<_, Option<String>>(7)?,
                    r.get::<_, Option<String>>(8)?,
                    resolved_at_s,
                ))
            })
            .optional()?;
        let Some(row) = row else { return Ok(None) };
        let status = TaskProposalStatus::from_status_str(&row.4)
            .ok_or_else(|| anyhow!("invalid status in task_proposals row: {}", row.4))?;
        let created_at = chrono::DateTime::parse_from_rfc3339(&row.5)
            .map_err(|e| anyhow!("bad created_at: {e}"))?
            .with_timezone(&chrono::Utc);
        let resolved_at = row
            .9
            .as_deref()
            .map(|s| chrono::DateTime::parse_from_rfc3339(s).map(|dt| dt.with_timezone(&chrono::Utc)))
            .transpose()
            .map_err(|e| anyhow!("bad resolved_at: {e}"))?;
        Ok(Some(TaskProposal {
            id: row.0,
            channel_id: row.1,
            proposed_by: row.2,
            title: row.3,
            status,
            created_at,
            accepted_task_number: row.6,
            accepted_sub_channel_id: row.7,
            resolved_by: row.8,
            resolved_at,
        }))
    }
}
