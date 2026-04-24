pub mod events;

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, TransactionBehavior};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use self::events::{TaskEventAction, TaskEventPayload};
use super::channels::ChannelType;
use super::Store;

// ── Types owned by this module ──

/// Full task row from the `tasks` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    /// UUID primary key.
    pub id: String,
    /// Owning channel id.
    pub channel_id: String,
    /// Per-channel monotonic task id shown in the UI.
    pub task_number: i64,
    /// Short title line.
    pub title: String,
    /// Workflow column.
    pub status: TaskStatus,
    /// Agent or human name currently holding the task, if any.
    pub claimed_by: Option<String>,
    /// Creator handle (human or agent).
    pub created_by: String,
    /// Insert time.
    pub created_at: DateTime<Utc>,
    /// Last mutation time.
    pub updated_at: DateTime<Utc>,
    /// Child `ChannelType::Task` channel that owns this task's collaboration
    /// scope. Populated on creation (and backfilled for legacy rows).
    pub sub_channel_id: Option<String>,
}

/// Kanban-style state stored in SQLite. Unified lifecycle: Proposed/Dismissed
/// sit alongside the four post-acceptance states. Transitions are forward-only
/// (see `can_transition_to`); there are no reverse edges in v1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// Suggested by an agent; awaiting human accept/dismiss.
    Proposed,
    /// Terminal — the proposal was rejected.
    Dismissed,
    /// Open, not started.
    Todo,
    /// Someone is actively working it.
    InProgress,
    /// Awaiting review.
    InReview,
    /// Terminal — completed.
    Done,
}

impl TaskStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::Dismissed => "dismissed",
            Self::Todo => "todo",
            Self::InProgress => "in_progress",
            Self::InReview => "in_review",
            Self::Done => "done",
        }
    }

    pub fn from_status_str(s: &str) -> Option<Self> {
        match s {
            "proposed" => Some(Self::Proposed),
            "dismissed" => Some(Self::Dismissed),
            "todo" => Some(Self::Todo),
            "in_progress" => Some(Self::InProgress),
            "in_review" => Some(Self::InReview),
            "done" => Some(Self::Done),
            _ => None,
        }
    }

    /// Forward-only transitions. No reverse transitions in v1.
    pub fn can_transition_to(&self, to: Self) -> bool {
        use TaskStatus::*;
        matches!(
            (self, to),
            (Proposed, Todo)
                | (Proposed, Dismissed)
                | (Todo, InProgress)
                | (InProgress, InReview)
                | (InReview, Done)
        )
    }
}

/// Returned by list_tasks and create_tasks — store constructs these directly.
/// Serialized as camelCase JSON for direct consumption by the TypeScript frontend.
///
/// `id` is the task's UUID primary key. Surfaced to the UI for store keying
/// (task_number alone is ambiguous across parent channels). MCP tools use
/// `(channel_name, task_number)` as the agent-facing handle and do NOT see `id`.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TaskInfo {
    /// UUID primary key.
    pub id: String,
    /// Per-channel task number.
    pub task_number: i64,
    /// Title line.
    pub title: String,
    /// Status — the typed enum, so the wire form matches `TaskStatus::as_str()`.
    pub status: TaskStatus,
    /// Current owner (claimer) handle. `None` when unclaimed or pre-acceptance.
    pub owner: Option<String>,
    /// Creator handle (human or agent).
    pub created_by: String,
    /// Insert time — ISO8601 string straight from SQLite.
    pub created_at: String,
    /// Last mutation time — ISO8601 string straight from SQLite.
    pub updated_at: String,
    /// Child `ChannelType::Task` sub-channel id, when the task has one (always
    /// populated for tasks created after Task 2; may be `None` for legacy data).
    pub sub_channel_id: Option<String>,
    /// Child sub-channel name for deep-linking. `None` when `sub_channel_id` is `None`.
    pub sub_channel_name: Option<String>,
    /// Source message id: the chat message this task was carved out of (when
    /// created from a "carve into task" action). `None` for tasks created
    /// directly (no source message).
    pub source_message_id: Option<String>,
    /// Snapshot of the source message's sender display name. Captured at task
    /// creation so the task card remains readable even after the source message
    /// is deleted. `None` when the task has no source message.
    pub snapshot_sender_name: Option<String>,
    /// Snapshot of the source message's sender type (`human` / `agent` / `system`).
    pub snapshot_sender_type: Option<String>,
    /// Snapshot of the source message's content at the time of task creation.
    pub snapshot_content: Option<String>,
    /// Snapshot of the source message's created_at timestamp.
    pub snapshot_created_at: Option<String>,
}

/// Build a `TaskInfo` from a SELECT that returns the canonical 15-column shape
/// used by every task-listing query in this module. Keeping one helper prevents
/// per-query drift: adding a field means one SQL column list update + one
/// `row.get(n)` edit here, not four.
///
/// Expected column order (mirrors the SELECT text):
/// 0 `t.id`, 1 `t.task_number`, 2 `t.title`, 3 `t.status`, 4 `t.owner`,
/// 5 `t.created_by`, 6 `t.created_at`, 7 `t.updated_at`, 8 `t.sub_channel_id`,
/// 9 `c.name AS sub_channel_name`, 10 `t.source_message_id`,
/// 11 `t.snapshot_sender_name`, 12 `t.snapshot_sender_type`,
/// 13 `t.snapshot_content`, 14 `t.snapshot_created_at`.
fn task_info_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskInfo> {
    let status_str: String = row.get(3)?;
    // Schema CHECK constraint limits `status` to the six known enum variants,
    // so an unknown string here is a corruption / migration-drift bug, not user
    // input. Surface it as an InvalidColumnType error — same shape rusqlite uses
    // for other parse failures — rather than silently defaulting.
    let status = TaskStatus::from_status_str(&status_str).ok_or_else(|| {
        rusqlite::Error::InvalidColumnType(
            3,
            format!("unknown task status: {status_str}"),
            rusqlite::types::Type::Text,
        )
    })?;
    Ok(TaskInfo {
        id: row.get(0)?,
        task_number: row.get(1)?,
        title: row.get(2)?,
        status,
        owner: row.get(4)?,
        created_by: row.get(5)?,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
        sub_channel_id: row.get(8)?,
        sub_channel_name: row.get(9)?,
        source_message_id: row.get(10)?,
        snapshot_sender_name: row.get(11)?,
        snapshot_sender_type: row.get(12)?,
        snapshot_content: row.get(13)?,
        snapshot_created_at: row.get(14)?,
    })
}

/// Returned by claim_tasks — store constructs these directly.
#[derive(Debug, Serialize, Deserialize)]
pub struct ClaimResult {
    /// Task number that was claimed or failed.
    #[serde(rename = "taskNumber")]
    pub task_number: i64,
    /// Whether the claim succeeded.
    pub success: bool,
    /// Error explanation when `success` is false.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl Store {
    /// Create one or more tasks under `channel_name`, each with its own
    /// `ChannelType::Task` sub-channel and the creator enrolled as the first
    /// member of that sub-channel. All inserts — sub-channel, first member,
    /// and task row — run inside a single IMMEDIATE transaction so a partial
    /// failure on any task leaves zero orphan channels or membership rows,
    /// and concurrent `create_tasks` calls on the same parent can't race
    /// on `task_number`.
    pub fn create_tasks(
        &self,
        channel_name: &str,
        creator_name: &str,
        titles: &[&str],
    ) -> Result<Vec<TaskInfo>> {
        // `transaction()` needs `&mut Connection`, so bind the guard as `mut`.
        let mut conn = self.conn.lock().unwrap();
        let channel = Self::get_channel_by_name_inner(&conn, channel_name)?
            .ok_or_else(|| anyhow!("channel not found: {}", channel_name))?;

        // Resolve creator kind up-front so the transaction only does writes.
        let creator_type = crate::store::resolve_sender_type_inner(&conn, creator_name)?;
        let task_channel_type = ChannelType::Task.as_api_str();

        // `transaction_with_behavior(Immediate)` issues BEGIN IMMEDIATE, which
        // acquires the SQLite write lock eagerly. A concurrent `create_tasks`
        // on the same (or any) connection will block until we commit or fail
        // fast with SQLITE_BUSY — so the MAX(task_number) read below is
        // serialized with the INSERTs, and two callers cannot both pick the
        // same task_number. `conn.transaction()` defaults to DEFERRED, which
        // does not give that guarantee.
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        // tx-scoped: MAX under write lock so task_number is race-free across
        // concurrent `create_tasks` calls on the same parent.
        let max_num: i64 = tx.query_row(
            "SELECT COALESCE(MAX(task_number), 0) FROM tasks WHERE channel_id = ?1",
            params![channel.id],
            |row| row.get(0),
        )?;
        let mut pending_events: Vec<(crate::store::messages::types::InsertedMessage, String)> =
            Vec::new();

        let mut result = Vec::new();
        for (i, title) in titles.iter().enumerate() {
            let task_id = Uuid::new_v4().to_string();
            let task_number = max_num + 1 + i as i64;
            let sub_channel_id = Uuid::new_v4().to_string();
            let sub_channel_name = format!("{}__task-{}", channel.name, task_number);

            tx.execute(
                "INSERT INTO channels (id, name, description, channel_type, parent_channel_id) \
                 VALUES (?1, ?2, NULL, ?3, ?4)",
                params![
                    sub_channel_id,
                    sub_channel_name,
                    task_channel_type,
                    channel.id
                ],
            )?;
            tx.execute(
                "INSERT INTO channel_members (channel_id, member_name, member_type, last_read_seq) \
                 VALUES (?1, ?2, ?3, 0)",
                params![sub_channel_id, creator_name, creator_type.as_str()],
            )?;
            // `RETURNING created_at, updated_at` reads the DB-default
            // `datetime('now')` values SQLite applies to the row, so the
            // returned `TaskInfo` carries exact-match timestamps without a
            // second SELECT round-trip.
            let (created_at, updated_at): (String, String) = tx.query_row(
                "INSERT INTO tasks (id, channel_id, task_number, title, created_by, sub_channel_id) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
                 RETURNING created_at, updated_at",
                params![
                    task_id,
                    channel.id,
                    task_number,
                    title,
                    creator_name,
                    sub_channel_id
                ],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;

            // Emit task_event inside the same tx so the task row and its
            // visibility-in-chat commit atomically.
            let payload = TaskEventPayload {
                action: TaskEventAction::Created,
                task_number,
                title: title.to_string(),
                sub_channel_id: sub_channel_id.clone(),
                actor: creator_name.to_string(),
                prev_status: None,
                next_status: TaskStatus::Todo,
                claimed_by: None,
            };
            let content = payload.to_json_string()?;
            let inserted = Self::create_system_message_tx(&tx, &channel, &content)?;
            pending_events.push((inserted, content));

            result.push(TaskInfo {
                id: task_id,
                task_number,
                title: title.to_string(),
                status: TaskStatus::Todo,
                owner: None,
                created_by: creator_name.to_string(),
                created_at,
                updated_at,
                sub_channel_id: Some(sub_channel_id),
                sub_channel_name: Some(sub_channel_name),
                source_message_id: None,
                snapshot_sender_name: None,
                snapshot_sender_type: None,
                snapshot_content: None,
                snapshot_created_at: None,
            });
        }
        tx.commit()?;
        drop(conn); // release the mutex guard before the stream fanout

        self.emit_system_stream_events(&channel, pending_events)?;
        Ok(result)
    }

    /// Fetch a single task by `(channel_name, task_number)`. Returns `Ok(None)`
    /// when the task doesn't exist so the HTTP handler can map it to 404 —
    /// a missing channel still surfaces as an error (real misconfiguration).
    pub fn get_task_info(&self, channel_name: &str, task_number: i64) -> Result<Option<TaskInfo>> {
        let conn = self.conn.lock().unwrap();
        let channel = Self::get_channel_by_name_inner(&conn, channel_name)?
            .ok_or_else(|| anyhow!("channel not found: {}", channel_name))?;

        let row: Option<TaskInfo> = match conn.query_row(
            "SELECT t.id, t.task_number, t.title, t.status, t.owner, t.created_by, \
                    t.created_at, t.updated_at, t.sub_channel_id, c.name AS sub_channel_name, \
                    t.source_message_id, t.snapshot_sender_name, t.snapshot_sender_type, \
                    t.snapshot_content, t.snapshot_created_at \
             FROM tasks t \
             LEFT JOIN channels c ON c.id = t.sub_channel_id \
             WHERE t.channel_id = ?1 AND t.task_number = ?2 \
             LIMIT 1",
            params![channel.id, task_number],
            task_info_from_row,
        ) {
            Ok(row) => Some(row),
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(other) => return Err(other.into()),
        };
        Ok(row)
    }

    pub fn get_tasks(
        &self,
        channel_name: &str,
        status_filter: Option<TaskStatus>,
    ) -> Result<Vec<TaskInfo>> {
        let conn = self.conn.lock().unwrap();
        let channel = Self::get_channel_by_name_inner(&conn, channel_name)?
            .ok_or_else(|| anyhow!("channel not found: {}", channel_name))?;

        let rows: Vec<TaskInfo> = if let Some(status) = status_filter {
            let mut stmt = conn.prepare(
                "SELECT t.id, t.task_number, t.title, t.status, t.owner, t.created_by, \
                        t.created_at, t.updated_at, t.sub_channel_id, c.name AS sub_channel_name, \
                        t.source_message_id, t.snapshot_sender_name, t.snapshot_sender_type, \
                        t.snapshot_content, t.snapshot_created_at \
                 FROM tasks t \
                 LEFT JOIN channels c ON c.id = t.sub_channel_id \
                 WHERE t.channel_id = ?1 AND t.status = ?2 \
                 ORDER BY t.task_number",
            )?;
            let iter = stmt.query_map(params![channel.id, status.as_str()], task_info_from_row)?;
            let mut out = Vec::new();
            for row in iter {
                out.push(row?);
            }
            out
        } else {
            let mut stmt = conn.prepare(
                "SELECT t.id, t.task_number, t.title, t.status, t.owner, t.created_by, \
                        t.created_at, t.updated_at, t.sub_channel_id, c.name AS sub_channel_name, \
                        t.source_message_id, t.snapshot_sender_name, t.snapshot_sender_type, \
                        t.snapshot_content, t.snapshot_created_at \
                 FROM tasks t \
                 LEFT JOIN channels c ON c.id = t.sub_channel_id \
                 WHERE t.channel_id = ?1 \
                 ORDER BY t.task_number",
            )?;
            let iter = stmt.query_map(params![channel.id], task_info_from_row)?;
            let mut out = Vec::new();
            for row in iter {
                out.push(row?);
            }
            out
        };
        Ok(rows)
    }

    pub fn update_tasks_claim(
        &self,
        channel_name: &str,
        claimer_name: &str,
        task_numbers: &[i64],
    ) -> Result<Vec<ClaimResult>> {
        // `transaction()` needs `&mut Connection`. Every successful claim in
        // this batch must atomically UPDATE the task row AND add the claimer
        // to the task's sub-channel — otherwise a crash between the two
        // writes leaves membership and task state out of sync.
        let mut conn = self.conn.lock().unwrap();
        let channel = Self::get_channel_by_name_inner(&conn, channel_name)?
            .ok_or_else(|| anyhow!("channel not found: {}", channel_name))?;

        let claimer_type = crate::store::resolve_sender_type_inner(&conn, claimer_name)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

        let mut results = Vec::new();
        let mut pending_events: Vec<(crate::store::messages::types::InsertedMessage, String)> =
            Vec::new();
        // Batch semantics: all claims commit together or none do. A hard SQL
        // error on claim N rolls back successful claims 1..N-1. "Soft"
        // rejections (task already claimed / not in todo / stolen mid-flight)
        // push a `ClaimResult { success: false, .. }` and continue; they still
        // commit as a no-op batch.
        for &tn in task_numbers {
            let task: Option<(String, Option<String>, Option<String>, String)> = tx
                .query_row(
                    "SELECT status, claimed_by, sub_channel_id, title FROM tasks WHERE channel_id = ?1 AND task_number = ?2",
                    params![channel.id, tn],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .ok();

            match task {
                Some((status, claimed_by, sub_channel_id, title))
                    if status == "todo" && claimed_by.is_none() =>
                {
                    // Defense in depth: WHERE-guard on the precondition we
                    // just read. If another writer won the race between the
                    // SELECT and this UPDATE, `rows == 0` and we soft-fail.
                    let rows = tx.execute(
                        "UPDATE tasks SET claimed_by = ?1, status = 'in_progress', updated_at = datetime('now') \
                         WHERE channel_id = ?2 AND task_number = ?3 AND status = 'todo' AND claimed_by IS NULL",
                        params![claimer_name, channel.id, tn],
                    )?;
                    if rows != 1 {
                        results.push(ClaimResult {
                            task_number: tn,
                            success: false,
                            reason: Some("task was claimed by another writer".to_string()),
                        });
                        continue;
                    }
                    // Sync sub-channel membership: claimer joins. `INSERT OR
                    // IGNORE` keeps the operation idempotent when the claimer
                    // is already a member (e.g. they also created the task).
                    if let Some(ref sub_id) = sub_channel_id {
                        tx.execute(
                            "INSERT OR IGNORE INTO channel_members (channel_id, member_name, member_type, last_read_seq) \
                             VALUES (?1, ?2, ?3, 0)",
                            params![sub_id, claimer_name, claimer_type.as_str()],
                        )?;
                    }
                    let payload = TaskEventPayload {
                        action: TaskEventAction::Claimed,
                        task_number: tn,
                        title,
                        sub_channel_id: sub_channel_id.unwrap_or_default(),
                        actor: claimer_name.to_string(),
                        prev_status: Some(TaskStatus::Todo),
                        next_status: TaskStatus::InProgress,
                        claimed_by: Some(claimer_name.to_string()),
                    };
                    let content = payload.to_json_string()?;
                    let inserted = Self::create_system_message_tx(&tx, &channel, &content)?;
                    pending_events.push((inserted, content));
                    results.push(ClaimResult {
                        task_number: tn,
                        success: true,
                        reason: None,
                    });
                }
                Some(_) => {
                    results.push(ClaimResult {
                        task_number: tn,
                        success: false,
                        reason: Some("task already claimed or not in todo status".to_string()),
                    });
                }
                None => {
                    results.push(ClaimResult {
                        task_number: tn,
                        success: false,
                        reason: Some("task not found".to_string()),
                    });
                }
            }
        }
        tx.commit()?;
        drop(conn);
        self.emit_system_stream_events(&channel, pending_events)?;
        Ok(results)
    }

    pub fn update_task_unclaim(
        &self,
        channel_name: &str,
        claimer_name: &str,
        task_number: i64,
    ) -> Result<()> {
        // Atomic: UPDATE the task row and DELETE the claimer's sub-channel
        // membership in one transaction. The creator is never touched — only
        // the caller's own membership row is removed.
        let mut conn = self.conn.lock().unwrap();
        let channel = Self::get_channel_by_name_inner(&conn, channel_name)?
            .ok_or_else(|| anyhow!("channel not found: {}", channel_name))?;

        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

        // tx-scoped read closes the TOCTOU window — another writer can't
        // reassign `claimed_by` between the check and the UPDATE.
        let (claimed_by, sub_channel_id, current_status_str, title): (Option<String>, Option<String>, String, String) = tx.query_row(
            "SELECT claimed_by, sub_channel_id, status, title FROM tasks WHERE channel_id = ?1 AND task_number = ?2",
            params![channel.id, task_number],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )?;

        if claimed_by.as_deref() != Some(claimer_name) {
            return Err(anyhow!("task not claimed by {}", claimer_name));
        }

        // Defense in depth: WHERE-guard on the claimer. If `rows != 1` the
        // claim was stolen mid-flight despite the tx-scoped read — surface it.
        let rows = tx.execute(
            "UPDATE tasks SET claimed_by = NULL, status = 'todo', updated_at = datetime('now') \
             WHERE channel_id = ?1 AND task_number = ?2 AND claimed_by = ?3",
            params![channel.id, task_number, claimer_name],
        )?;
        if rows != 1 {
            return Err(anyhow!(
                "task {} no longer claimed by {}",
                task_number,
                claimer_name
            ));
        }

        if let Some(sub_id) = sub_channel_id.as_deref() {
            tx.execute(
                "DELETE FROM channel_members WHERE channel_id = ?1 AND member_name = ?2",
                params![sub_id, claimer_name],
            )?;
        }

        let prev_status = TaskStatus::from_status_str(&current_status_str)
            .ok_or_else(|| anyhow!("invalid task status: {}", current_status_str))?;

        let payload = TaskEventPayload {
            action: TaskEventAction::Unclaimed,
            task_number,
            title,
            sub_channel_id: sub_channel_id.unwrap_or_default(),
            actor: claimer_name.to_string(),
            prev_status: Some(prev_status),
            next_status: TaskStatus::Todo,
            claimed_by: None,
        };
        let content = payload.to_json_string()?;
        let inserted = Self::create_system_message_tx(&tx, &channel, &content)?;
        tx.commit()?;
        drop(conn);
        self.emit_system_stream_events(&channel, vec![(inserted, content)])?;
        Ok(())
    }

    pub fn update_task_status(
        &self,
        channel_name: &str,
        task_number: i64,
        requester_name: &str,
        new_status: TaskStatus,
    ) -> Result<()> {
        // `transaction()` needs `&mut Connection`. The status UPDATE and the
        // sub-channel archive (when `new_status == Done`) must commit together
        // so an observer never sees a task marked Done whose sub-channel is
        // still active (or vice versa).
        let mut conn = self.conn.lock().unwrap();
        let channel = Self::get_channel_by_name_inner(&conn, channel_name)?
            .ok_or_else(|| anyhow!("channel not found: {}", channel_name))?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

        let (current_status_str, claimed_by, sub_channel_id, title): (
            String,
            Option<String>,
            Option<String>,
            String,
        ) = tx.query_row(
            "SELECT status, claimed_by, sub_channel_id, title FROM tasks \
             WHERE channel_id = ?1 AND task_number = ?2",
            params![channel.id, task_number],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )?;

        let current_status = TaskStatus::from_status_str(&current_status_str)
            .ok_or_else(|| anyhow!("invalid task status: {}", current_status_str))?;

        if claimed_by.as_deref() != Some(requester_name) {
            return Err(anyhow!("task not claimed by {}", requester_name));
        }
        if !current_status.can_transition_to(new_status) {
            return Err(anyhow!(
                "cannot transition from {} to {}",
                current_status.as_str(),
                new_status.as_str()
            ));
        }

        tx.execute(
            "UPDATE tasks SET status = ?1, updated_at = datetime('now') WHERE channel_id = ?2 AND task_number = ?3",
            params![new_status.as_str(), channel.id, task_number],
        )?;

        // Emit the event inside the tx, BEFORE the optional archive below
        // (which consumes `sub_channel_id`).
        let payload = TaskEventPayload {
            action: TaskEventAction::StatusChanged,
            task_number,
            title,
            sub_channel_id: sub_channel_id.clone().unwrap_or_default(),
            actor: requester_name.to_string(),
            prev_status: Some(current_status),
            next_status: new_status,
            claimed_by: claimed_by.clone(),
        };
        let content = payload.to_json_string()?;
        let inserted = Self::create_system_message_tx(&tx, &channel, &content)?;

        // `Done` is terminal (`can_transition_to` has no outbound edges from
        // `Done`), so archiving here is safe — there is no path back that would
        // need to un-archive. Bypasses the `archive_channel` guard on purpose:
        // that guard rejects direct callers; the task lifecycle is the sole
        // path that may archive a task sub-channel.
        if new_status == TaskStatus::Done {
            if let Some(sub_id) = sub_channel_id {
                tx.execute(
                    "UPDATE channels SET archived = 1 WHERE id = ?1",
                    params![sub_id],
                )?;
            }
        }

        tx.commit()?;
        drop(conn);
        self.emit_system_stream_events(&channel, vec![(inserted, content)])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_status_transitions() {
        use TaskStatus::*;
        assert!(Proposed.can_transition_to(Todo));
        assert!(Proposed.can_transition_to(Dismissed));
        assert!(!Proposed.can_transition_to(InProgress));
        assert!(!Dismissed.can_transition_to(Todo)); // terminal
        assert!(!Done.can_transition_to(InProgress)); // terminal
        assert!(Todo.can_transition_to(InProgress));
        assert!(!InProgress.can_transition_to(Todo)); // no reverse in v1
    }
}

#[cfg(test)]
mod sub_channel_tests {
    use super::*;
    use crate::store::channels::ChannelType;
    use crate::store::{AgentRecordUpsert, Store};

    fn seed_agent(store: &Store, name: &str) {
        store
            .create_agent_record(&AgentRecordUpsert {
                name,
                display_name: name,
                description: None,
                system_prompt: None,
                runtime: "claude",
                model: "sonnet",
                reasoning_effort: None,
                env_vars: &[],
            })
            .unwrap();
    }

    /// Read a task's `(channel_id, sub_channel_id)` without requiring a
    /// high-level accessor. Keeps tests focused on store-layer guarantees.
    fn read_task_channel_ids(
        store: &Store,
        parent_channel_name: &str,
        task_number: i64,
    ) -> (String, Option<String>) {
        let conn = store.conn_for_test();
        conn.query_row(
            "SELECT t.channel_id, t.sub_channel_id \
             FROM tasks t JOIN channels c ON c.id = t.channel_id \
             WHERE c.name = ?1 AND t.task_number = ?2",
            params![parent_channel_name, task_number],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .unwrap()
    }

    #[test]
    fn create_tasks_spawns_sub_channel_with_creator_member() {
        let store = Store::open(":memory:").unwrap();
        store
            .create_channel("eng", None, ChannelType::Channel, None)
            .unwrap();
        store.create_human("alice").unwrap();

        let result = store
            .create_tasks("eng", "alice", &["Ship the feature"])
            .unwrap();
        assert_eq!(result.len(), 1);
        let info = &result[0];
        assert_eq!(info.task_number, 1);
        let sub_id_from_dto = info.sub_channel_id.as_deref().expect("dto carries sub id");
        assert_eq!(
            info.sub_channel_name.as_deref(),
            Some("eng__task-1"),
            "dto carries sub-channel name"
        );

        let (parent_channel_id, sub_id_from_db) = read_task_channel_ids(&store, "eng", 1);
        let sub_id = sub_id_from_db.expect("task row must have sub_channel_id");
        assert_eq!(sub_id, sub_id_from_dto, "dto and db agree on sub id");

        let sub = store
            .get_channel_by_id(&sub_id)
            .unwrap()
            .expect("sub-channel row exists");
        assert_eq!(sub.channel_type, ChannelType::Task);
        assert_eq!(sub.name, "eng__task-1");
        assert_eq!(
            sub.parent_channel_id.as_deref(),
            Some(parent_channel_id.as_str())
        );

        assert!(
            store.channel_member_exists(&sub_id, "alice").unwrap(),
            "creator must be a member of the sub-channel"
        );
    }

    #[test]
    fn create_tasks_atomicity_no_orphan_on_partial_failure() {
        let store = Store::open(":memory:").unwrap();
        store
            .create_channel("eng", None, ChannelType::Channel, None)
            .unwrap();
        store.create_human("alice").unwrap();

        // Pre-squat the name `eng__task-2` so the SECOND iteration's first
        // INSERT (into `channels`) collides on `channels.name`'s UNIQUE
        // constraint. The first iteration writes channel+member+task rows
        // successfully; those plus the second iteration's partial work must
        // all roll back when `tx.commit()` is never reached.
        {
            let conn = store.conn_for_test();
            conn.execute(
                "INSERT INTO channels (id, name, channel_type) \
                 VALUES ('squat', 'eng__task-2', 'channel')",
                [],
            )
            .unwrap();
        }

        let result = store.create_tasks("eng", "alice", &["first", "second"]);
        assert!(
            result.is_err(),
            "create_tasks must fail when a sub-channel name collides"
        );

        // Task rows must roll back — zero tasks on `eng`.
        let task_count: i64 = {
            let conn = store.conn_for_test();
            conn.query_row(
                "SELECT COUNT(*) FROM tasks t \
                 JOIN channels c ON c.id = t.channel_id \
                 WHERE c.name = 'eng'",
                [],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(
            task_count, 0,
            "failed create_tasks must not leave any task rows"
        );

        // Only the pre-existing 'squat' channel may survive — no sub-channels
        // of `ChannelType::Task` created by the aborted transaction.
        let orphan_subs: i64 = {
            let conn = store.conn_for_test();
            conn.query_row(
                "SELECT COUNT(*) FROM channels WHERE channel_type = ?1",
                params![ChannelType::Task.as_api_str()],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(
            orphan_subs, 0,
            "failed create_tasks must not leave an orphan sub-channel"
        );

        // No membership row for alice outside the parent channel (she was
        // never added to `eng` directly in this test, so any membership row
        // for alice is an orphan).
        let orphan_members: i64 = {
            let conn = store.conn_for_test();
            conn.query_row(
                "SELECT COUNT(*) FROM channel_members \
                 WHERE member_name = 'alice' \
                   AND channel_id NOT IN (SELECT id FROM channels WHERE name = 'eng')",
                [],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(
            orphan_members, 0,
            "failed create_tasks must not leave an orphan membership row"
        );
    }

    #[test]
    fn claim_task_adds_claimer_to_sub_channel() {
        let store = Store::open(":memory:").unwrap();
        store
            .create_channel("eng", None, ChannelType::Channel, None)
            .unwrap();
        store.create_human("alice").unwrap();
        seed_agent(&store, "bob");
        store.create_tasks("eng", "alice", &["Ship it"]).unwrap();

        let results = store.update_tasks_claim("eng", "bob", &[1]).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].success, "claim must succeed");

        let (_parent_id, sub_id) = read_task_channel_ids(&store, "eng", 1);
        let sub_id = sub_id.expect("task has sub_channel_id");
        assert!(
            store.channel_member_exists(&sub_id, "bob").unwrap(),
            "claimer must join sub-channel"
        );
        assert!(
            store.channel_member_exists(&sub_id, "alice").unwrap(),
            "creator must remain a member"
        );
    }

    #[test]
    fn unclaim_rejects_when_claim_was_stolen() {
        // A writer outside the unclaim tx reassigns `claimed_by`. The
        // tx-scoped read + WHERE-guarded UPDATE must surface the stolen
        // claim instead of silently NULL-ing out carol's hold.
        let store = Store::open(":memory:").unwrap();
        store
            .create_channel("eng", None, ChannelType::Channel, None)
            .unwrap();
        store.create_human("alice").unwrap();
        seed_agent(&store, "bob");
        seed_agent(&store, "carol");
        store.create_tasks("eng", "alice", &["ship it"]).unwrap();
        store.update_tasks_claim("eng", "bob", &[1]).unwrap();

        // Simulate a write that re-assigns the claim outside of bob's control.
        {
            let conn = store.conn_for_test();
            conn.execute(
                "UPDATE tasks SET claimed_by = 'carol' WHERE task_number = 1",
                [],
            )
            .unwrap();
        }

        let err = store.update_task_unclaim("eng", "bob", 1).unwrap_err();
        assert!(
            err.to_string().contains("not claimed by bob"),
            "expected stolen-claim error, got: {err}"
        );
    }

    #[test]
    fn unclaim_removes_claimer_but_retains_creator() {
        let store = Store::open(":memory:").unwrap();
        store
            .create_channel("eng", None, ChannelType::Channel, None)
            .unwrap();
        store.create_human("alice").unwrap();
        seed_agent(&store, "bob");
        store.create_tasks("eng", "alice", &["Ship it"]).unwrap();
        let claim = store.update_tasks_claim("eng", "bob", &[1]).unwrap();
        assert!(claim[0].success);

        store.update_task_unclaim("eng", "bob", 1).unwrap();

        let (_parent_id, sub_id) = read_task_channel_ids(&store, "eng", 1);
        let sub_id = sub_id.expect("task has sub_channel_id");
        assert!(
            !store.channel_member_exists(&sub_id, "bob").unwrap(),
            "unclaimer must leave sub-channel"
        );
        assert!(
            store.channel_member_exists(&sub_id, "alice").unwrap(),
            "creator stays"
        );
    }

    #[test]
    fn status_done_archives_sub_channel() {
        let store = Store::open(":memory:").unwrap();
        store
            .create_channel("eng", None, ChannelType::Channel, None)
            .unwrap();
        store.create_human("alice").unwrap();
        seed_agent(&store, "bob");
        store.create_tasks("eng", "alice", &["Ship it"]).unwrap();
        store.update_tasks_claim("eng", "bob", &[1]).unwrap();
        store
            .update_task_status("eng", 1, "bob", TaskStatus::InReview)
            .unwrap();

        store
            .update_task_status("eng", 1, "bob", TaskStatus::Done)
            .unwrap();

        let (_parent_id, sub_id) = read_task_channel_ids(&store, "eng", 1);
        let sub_id = sub_id.expect("task has sub_channel_id");
        let archived: i64 = {
            let conn = store.conn_for_test();
            conn.query_row(
                "SELECT archived FROM channels WHERE id = ?1",
                params![&sub_id],
                |row| row.get(0),
            )
            .unwrap()
        };
        assert_eq!(archived, 1, "sub-channel must be archived on Done");
    }

    #[test]
    fn done_sub_channel_drops_out_of_inbox_notifications() {
        // Regression: archived task sub-channels must not linger in the inbox
        // view after Done. Reachable via the task detail page; gone from the
        // active conversation listing.
        let store = Store::open(":memory:").unwrap();
        store
            .create_channel("eng", None, ChannelType::Channel, None)
            .unwrap();
        store.create_human("alice").unwrap();
        seed_agent(&store, "bob");
        store
            .create_tasks("eng", "alice", &["Ship it", "Also ship it"])
            .unwrap();
        store.update_tasks_claim("eng", "bob", &[1, 2]).unwrap();

        // Before Done: both task sub-channels appear in bob's inbox.
        let before: Vec<String> = store
            .get_inbox_conversation_notifications("bob")
            .unwrap()
            .into_iter()
            .map(|r| r.conversation_name)
            .collect();
        assert!(
            before.iter().any(|n| n == "eng__task-1"),
            "active task sub-channel must be visible in inbox before Done: got {:?}",
            before
        );
        assert!(
            before.iter().any(|n| n == "eng__task-2"),
            "second active task sub-channel must be visible: got {:?}",
            before
        );

        // Advance task 1 through InReview → Done. Task 2 stays in progress.
        store
            .update_task_status("eng", 1, "bob", TaskStatus::InReview)
            .unwrap();
        store
            .update_task_status("eng", 1, "bob", TaskStatus::Done)
            .unwrap();

        let after: Vec<String> = store
            .get_inbox_conversation_notifications("bob")
            .unwrap()
            .into_iter()
            .map(|r| r.conversation_name)
            .collect();
        assert!(
            !after.iter().any(|n| n == "eng__task-1"),
            "archived task sub-channel must be hidden from inbox: got {:?}",
            after
        );
        assert!(
            after.iter().any(|n| n == "eng__task-2"),
            "other active task sub-channel must still be visible: got {:?}",
            after
        );
    }

    #[test]
    fn archived_sub_channel_still_resolves_per_member_notification() {
        // Regression: when viewing an archived task sub-channel's history via
        // the task detail page, the UI POSTs a read-cursor. The server then
        // looks up the per-member notification row to return fresh counts.
        // That lookup must NOT apply the "hide archived task sub-channels"
        // filter — that filter is for the sidebar listing only. If the
        // per-member query returned None, `update_read_cursor_for_channel`
        // would 500 and the UI's unread badge would never clear.
        let store = Store::open(":memory:").unwrap();
        store
            .create_channel("eng", None, ChannelType::Channel, None)
            .unwrap();
        store.create_human("alice").unwrap();
        seed_agent(&store, "bob");
        store.create_tasks("eng", "alice", &["Ship it"]).unwrap();
        store.update_tasks_claim("eng", "bob", &[1]).unwrap();
        store
            .update_task_status("eng", 1, "bob", TaskStatus::InReview)
            .unwrap();
        store
            .update_task_status("eng", 1, "bob", TaskStatus::Done)
            .unwrap();

        let (_parent_id, sub_id) = read_task_channel_ids(&store, "eng", 1);
        let sub_id = sub_id.expect("task has sub_channel_id");

        // Archived sub-channel is gone from the sidebar listing.
        let list_names: Vec<String> = store
            .get_inbox_conversation_notifications("alice")
            .unwrap()
            .into_iter()
            .map(|r| r.conversation_name)
            .collect();
        assert!(
            !list_names.iter().any(|n| n == "eng__task-1"),
            "archive filter on list query: got {:?}",
            list_names
        );

        // ...but still resolvable via per-member lookup (alice is the creator,
        // so she's a member). This is the row the read-cursor handler needs.
        let per_member = store
            .get_inbox_conversation_notification_for_member(&sub_id, "alice")
            .unwrap();
        assert!(
            per_member.is_some(),
            "per-member notification lookup must still resolve archived task sub-channels"
        );
        assert_eq!(per_member.unwrap().conversation_name, "eng__task-1");
    }

    #[test]
    fn user_cannot_manually_archive_task_sub_channel() {
        // The existing `archive_channel` guard only allows user/team channels.
        // Task sub-channels must archive exclusively via the `Done` transition
        // in `update_task_status`, never by direct caller request.
        let store = Store::open(":memory:").unwrap();
        store
            .create_channel("eng", None, ChannelType::Channel, None)
            .unwrap();
        store.create_human("alice").unwrap();
        store.create_tasks("eng", "alice", &["x"]).unwrap();

        let (_parent_id, sub_id) = read_task_channel_ids(&store, "eng", 1);
        let sub_id = sub_id.expect("task has sub_channel_id");

        let err = store.archive_channel(&sub_id).unwrap_err();
        assert!(
            err.to_string()
                .contains("only user and team channels can be archived"),
            "expected guard message, got: {err}"
        );
    }
}
