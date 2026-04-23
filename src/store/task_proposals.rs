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

/// Result of a successful `accept_task_proposal`. The caller (HTTP handler)
/// returns this to the client so the UI can deep-link to the sub-channel.
#[derive(Debug, Clone, Serialize)]
pub struct AcceptedTaskProposal {
    pub task_number: i64,
    pub sub_channel_id: String,
    pub sub_channel_name: String,
    /// Internal — message id of the kickoff system message posted in the
    /// sub-channel on acceptance. The HTTP handler uses this to route
    /// `deliver_message_to_agents`. Not exposed in the HTTP response.
    #[serde(skip)]
    pub kickoff_message_id: String,
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

    /// Flip a pending proposal to accepted, creating the backing task +
    /// sub-channel in the same transaction. Auto-claims the task to the
    /// proposer agent. Enrolls the accepter human in the sub-channel so
    /// they can follow along. Does NOT emit a `task_event` create/claim
    /// pair — the proposal card already carries that signal (scope 1b).
    pub fn accept_task_proposal(
        &self,
        id: &str,
        accepter: &str,
    ) -> Result<AcceptedTaskProposal> {
        use rusqlite::TransactionBehavior;

        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

        // Load the proposal under the write lock so a concurrent accept
        // can't race us. Also carry `created_at` so the accepted-snapshot
        // below emits the true proposal time for `proposedAt`, not the
        // resolve time.
        let row = tx
            .query_row(
                "SELECT channel_id, proposed_by, title, status, created_at \
                 FROM task_proposals WHERE id = ?1",
                params![id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                    ))
                },
            )
            .optional()?;
        let Some((channel_id, proposed_by, title, status_str, _proposed_at)) = row else {
            return Err(anyhow!("task proposal not found: {id}"));
        };
        // `_proposed_at` is read so Task 9 can attach it to the resolution
        // snapshot's `proposedAt` field. Kept under the write lock read for
        // free and named with a leading underscore until then.
        if status_str != "pending" {
            return Err(anyhow!("task proposal {id} is not pending"));
        }

        let channel = Self::get_channel_by_id_inner(&tx, &channel_id)?
            .ok_or_else(|| anyhow!("proposal's channel vanished: {channel_id}"))?;

        // Pick task_number via MAX+1 under the write lock, same pattern
        // as `create_tasks`.
        let max_num: i64 = tx.query_row(
            "SELECT COALESCE(MAX(task_number), 0) FROM tasks WHERE channel_id = ?1",
            params![channel.id],
            |r| r.get(0),
        )?;
        let task_number = max_num + 1;

        // Proposer is always an agent (humans don't use the tool); resolve
        // its sender type via the existing helper that handles both kinds.
        let proposer_type = crate::store::resolve_sender_type_inner(&tx, &proposed_by)?;

        let (task_id, sub_channel_id, sub_channel_name) =
            Self::insert_task_and_subchannel_tx(
                &tx,
                &channel,
                &proposed_by,
                proposer_type,
                &title,
                task_number,
            )?;

        // Auto-claim to the proposer (move status Todo → InProgress) and
        // set claimed_by. Mirrors the update_tasks_claim write but without
        // the task_event emission.
        tx.execute(
            "UPDATE tasks SET claimed_by = ?1, status = 'in_progress', \
             updated_at = datetime('now') WHERE id = ?2",
            params![proposed_by, task_id],
        )?;

        // Enroll the accepter human in the sub-channel so they can follow
        // the conversation (the agent was enrolled by
        // insert_task_and_subchannel_tx as the creator).
        let accepter_type = crate::store::resolve_sender_type_inner(&tx, accepter)?;
        tx.execute(
            "INSERT OR IGNORE INTO channel_members \
             (channel_id, member_name, member_type, last_read_seq) \
             VALUES (?1, ?2, ?3, 0)",
            params![sub_channel_id, accepter, accepter_type.as_str()],
        )?;

        // Flip the proposal status.
        let now = chrono::Utc::now().to_rfc3339();
        tx.execute(
            "UPDATE task_proposals \
             SET status = 'accepted', accepted_task_number = ?1, \
                 accepted_sub_channel_id = ?2, resolved_by = ?3, \
                 resolved_at = ?4 \
             WHERE id = ?5",
            params![task_number, sub_channel_id, accepter, now, id],
        )?;

        // Post a kickoff system message in the sub-channel. Plain text,
        // not a structured `task_event` — this is a human-readable marker
        // that the task has been opened and the agent should start. The
        // existing inbox wake-on-new-unread pipeline picks it up and
        // triggers the agent's next run scoped to the sub-channel.
        let sub_channel = Self::get_channel_by_id_inner(&tx, &sub_channel_id)?
            .ok_or_else(|| anyhow!("sub-channel vanished: {sub_channel_id}"))?;
        let kickoff = format!(
            "Task #{task_number} opened: {title}. {proposed_by}, you proposed \
             this — start here and ask any clarifying questions in this channel."
        );
        let kickoff_inserted =
            Self::create_system_message_tx(&tx, &sub_channel, &kickoff)?;
        let kickoff_message_id = kickoff_inserted.id.clone();

        // Collect pending events so we can emit the WS events after commit.
        let pending_sub = vec![(kickoff_inserted, kickoff)];

        tx.commit()?;
        drop(conn);

        self.emit_system_stream_events(&sub_channel, pending_sub)?;

        Ok(AcceptedTaskProposal {
            task_number,
            sub_channel_id,
            sub_channel_name,
            kickoff_message_id,
        })
    }
}
