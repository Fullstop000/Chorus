use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, types::Type, OptionalExtension};
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

impl TaskProposal {
    /// Parse the standard 10-column `task_proposals` row: id, channel_id,
    /// proposed_by, title, status, created_at, accepted_task_number,
    /// accepted_sub_channel_id, resolved_by, resolved_at.
    ///
    /// Returns a data-integrity error naming the offending proposal id when
    /// the stored `status` is not one of the allowed variants or when a
    /// timestamp does not parse as RFC 3339, so a live-DB debugger can
    /// pinpoint the bad row.
    pub(crate) fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        let id: String = row.get(0)?;
        let status_s: String = row.get(4)?;
        let status = TaskProposalStatus::from_status_str(&status_s).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(
                4,
                Type::Text,
                format!("invalid task_proposals.status for id={id}: {status_s}").into(),
            )
        })?;
        let created_at_s: String = row.get(5)?;
        let created_at = chrono::DateTime::parse_from_rfc3339(&created_at_s)
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    5,
                    Type::Text,
                    format!("invalid task_proposals.created_at for id={id}: {e}").into(),
                )
            })?;
        let resolved_at_s: Option<String> = row.get(9)?;
        let resolved_at = resolved_at_s
            .as_deref()
            .map(|s| {
                chrono::DateTime::parse_from_rfc3339(s)
                    .map(|dt| dt.with_timezone(&chrono::Utc))
                    .map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            9,
                            Type::Text,
                            format!("invalid task_proposals.resolved_at for id={id}: {e}").into(),
                        )
                    })
            })
            .transpose()?;
        Ok(Self {
            id,
            channel_id: row.get(1)?,
            proposed_by: row.get(2)?,
            title: row.get(3)?,
            status,
            created_at,
            accepted_task_number: row.get(6)?,
            accepted_sub_channel_id: row.get(7)?,
            resolved_by: row.get(8)?,
            resolved_at,
        })
    }
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
        stmt.query_row(params![id], TaskProposal::from_row)
            .optional()
            .map_err(anyhow::Error::from)
    }

    /// Mark a pending proposal as dismissed. The proposal row is updated
    /// atomically under a compare-and-set on `status = 'pending'` to avoid
    /// double-resolution races (two users both clicking dismiss on the
    /// card at once). Returns `Err` if the row is missing or already
    /// resolved.
    pub fn dismiss_task_proposal(&self, id: &str, resolver: &str) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let now = chrono::Utc::now().to_rfc3339();
        let rows = tx.execute(
            "UPDATE task_proposals \
             SET status = 'dismissed', resolved_by = ?1, resolved_at = ?2 \
             WHERE id = ?3 AND status = 'pending'",
            params![resolver, now, id],
        )?;
        if rows == 0 {
            let exists: bool = tx
                .query_row(
                    "SELECT 1 FROM task_proposals WHERE id = ?1",
                    params![id],
                    |_| Ok(true),
                )
                .optional()?
                .is_some();
            return if exists {
                Err(anyhow!("task proposal {id} is not pending"))
            } else {
                Err(anyhow!("task proposal not found: {id}"))
            };
        }
        tx.commit()?;
        Ok(())
    }
}
