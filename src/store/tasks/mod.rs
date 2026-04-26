pub mod events;

use anyhow::{anyhow, Result};
use rusqlite::{params, TransactionBehavior};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use self::events::{TaskEventAction, TaskEventPayload};
use super::channels::ChannelType;
use super::messages::SenderType;
use super::Store;

// ── Types owned by this module ──

/// Kanban-style state stored in SQLite.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// Open, not started.
    Todo,
    /// Someone is actively working it.
    InProgress,
    /// Awaiting review.
    InReview,
    /// Completed.
    Done,
}

impl TaskStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Todo => "todo",
            Self::InProgress => "in_progress",
            Self::InReview => "in_review",
            Self::Done => "done",
        }
    }

    pub fn from_status_str(s: &str) -> Option<Self> {
        match s {
            "todo" => Some(Self::Todo),
            "in_progress" => Some(Self::InProgress),
            "in_review" => Some(Self::InReview),
            "done" => Some(Self::Done),
            _ => None,
        }
    }

    pub fn can_transition_to(&self, to: Self) -> bool {
        matches!(
            (self, to),
            (Self::Todo, Self::InProgress)
                | (Self::InProgress, Self::InReview)
                | (Self::InProgress, Self::Done)
                | (Self::InReview, Self::Done)
                | (Self::InReview, Self::InProgress)
        )
    }
}

/// Returned by list_tasks and create_tasks — store constructs these directly.
#[derive(Debug, Serialize, Deserialize)]
pub struct TaskInfo {
    /// Per-channel task number.
    #[serde(rename = "taskNumber")]
    pub task_number: i64,
    /// Title line.
    pub title: String,
    /// Status string matching `TaskStatus::as_str`.
    pub status: String,
    /// Display name of claimer when set.
    #[serde(rename = "claimedByName")]
    pub claimed_by_name: Option<String>,
    /// Display name of creator.
    #[serde(rename = "createdByName")]
    pub created_by_name: Option<String>,
    /// Child `ChannelType::Task` sub-channel id, when the task has one (always
    /// populated for tasks created after Task 2; may be `None` for legacy data).
    #[serde(rename = "subChannelId")]
    pub sub_channel_id: Option<String>,
    /// Child sub-channel name for deep-linking. `None` when `sub_channel_id` is `None`.
    #[serde(rename = "subChannelName")]
    pub sub_channel_name: Option<String>,
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

// Shared SELECT body for the `tasks` read path. Display names for creator
// and claimer are resolved by joining `humans` or `agents` against the
// stored `created_by_id` / `claimed_by_id`. The CASE expressions return
// NULL when the (type, id) pair has no matching row, which keeps the
// `Option<String>` semantics on `claimed_by_name` / `created_by_name`.
//
// Inlined verbatim into each query rather than concatenated at runtime to
// keep rusqlite's prepare-time SQL a single &'static str.
const TASK_INFO_SELECT_SQL_BY_NUMBER: &str = "\
    SELECT t.task_number, t.title, t.status, \
        CASE t.claimed_by_type \
            WHEN 'human' THEN (SELECT h.name FROM humans h WHERE h.id = t.claimed_by_id) \
            WHEN 'agent' THEN (SELECT a.name FROM agents a WHERE a.id = t.claimed_by_id) \
            ELSE NULL \
        END AS claimed_by_name, \
        CASE t.created_by_type \
            WHEN 'human' THEN (SELECT h.name FROM humans h WHERE h.id = t.created_by_id) \
            WHEN 'agent' THEN (SELECT a.name FROM agents a WHERE a.id = t.created_by_id) \
            ELSE NULL \
        END AS created_by_name, \
        t.sub_channel_id, c.name \
    FROM tasks t \
    LEFT JOIN channels c ON c.id = t.sub_channel_id \
    WHERE t.channel_id = ?1 AND t.task_number = ?2 LIMIT 1";

const TASK_INFO_LIST_SQL_ALL: &str = "\
    SELECT t.task_number, t.title, t.status, \
        CASE t.claimed_by_type \
            WHEN 'human' THEN (SELECT h.name FROM humans h WHERE h.id = t.claimed_by_id) \
            WHEN 'agent' THEN (SELECT a.name FROM agents a WHERE a.id = t.claimed_by_id) \
            ELSE NULL \
        END AS claimed_by_name, \
        CASE t.created_by_type \
            WHEN 'human' THEN (SELECT h.name FROM humans h WHERE h.id = t.created_by_id) \
            WHEN 'agent' THEN (SELECT a.name FROM agents a WHERE a.id = t.created_by_id) \
            ELSE NULL \
        END AS created_by_name, \
        t.sub_channel_id, c.name \
    FROM tasks t \
    LEFT JOIN channels c ON c.id = t.sub_channel_id \
    WHERE t.channel_id = ?1 ORDER BY t.task_number";

const TASK_INFO_LIST_SQL_FILTERED: &str = "\
    SELECT t.task_number, t.title, t.status, \
        CASE t.claimed_by_type \
            WHEN 'human' THEN (SELECT h.name FROM humans h WHERE h.id = t.claimed_by_id) \
            WHEN 'agent' THEN (SELECT a.name FROM agents a WHERE a.id = t.claimed_by_id) \
            ELSE NULL \
        END AS claimed_by_name, \
        CASE t.created_by_type \
            WHEN 'human' THEN (SELECT h.name FROM humans h WHERE h.id = t.created_by_id) \
            WHEN 'agent' THEN (SELECT a.name FROM agents a WHERE a.id = t.created_by_id) \
            ELSE NULL \
        END AS created_by_name, \
        t.sub_channel_id, c.name \
    FROM tasks t \
    LEFT JOIN channels c ON c.id = t.sub_channel_id \
    WHERE t.channel_id = ?1 AND t.status = ?2 ORDER BY t.task_number";

fn map_task_info_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskInfo> {
    Ok(TaskInfo {
        task_number: row.get(0)?,
        title: row.get(1)?,
        status: row.get(2)?,
        claimed_by_name: row.get(3)?,
        created_by_name: row.get(4)?,
        sub_channel_id: row.get(5)?,
        sub_channel_name: row.get(6)?,
    })
}

impl Store {
    /// Create one or more tasks under `channel_name`, each with its own
    /// `ChannelType::Task` sub-channel and the creator enrolled as the first
    /// member of that sub-channel. All inserts — sub-channel, first member,
    /// and task row — run inside a single IMMEDIATE transaction so a partial
    /// failure on any task leaves zero orphan channels or membership rows,
    /// and concurrent `create_tasks` calls on the same parent can't race
    /// on `task_number`.
    ///
    /// Identity is ID-first: callers pass the creator's `humans.id` /
    /// `agents.id` plus a `SenderType`. Display names are resolved via JOIN
    /// when reading task rows back out.
    pub fn create_tasks(
        &self,
        channel_name: &str,
        creator_id: &str,
        creator_type: SenderType,
        titles: &[&str],
    ) -> Result<Vec<TaskInfo>> {
        // `transaction()` needs `&mut Connection`, so bind the guard as `mut`.
        let mut conn = self.conn.lock().unwrap();
        let channel = Self::get_channel_by_name_inner(&conn, channel_name)?
            .ok_or_else(|| anyhow!("channel not found: {}", channel_name))?;

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
        let creator_name = Self::sender_name_for_id_tx(&tx, creator_id, creator_type)?;
        let mut pending_events: Vec<(crate::store::messages::types::InsertedMessage, String)> =
            Vec::new();

        let mut result = Vec::new();
        for (i, title) in titles.iter().enumerate() {
            let task_id = Uuid::new_v4().to_string();
            let task_number = max_num + 1 + i as i64;
            let sub_channel_id = Uuid::new_v4().to_string();
            let sub_channel_name = format!("{}__task-{}", channel.name, task_number);

            tx.execute(
                "INSERT INTO channels (id, workspace_id, name, description, channel_type, parent_channel_id) \
                 VALUES (?1, ?2, ?3, NULL, ?4, ?5)",
                params![
                    sub_channel_id,
                    channel.workspace_id,
                    sub_channel_name,
                    task_channel_type,
                    channel.id
                ],
            )?;
            tx.execute(
                "INSERT INTO channel_members (channel_id, member_id, member_type, last_read_seq) \
                 VALUES (?1, ?2, ?3, 0)",
                params![sub_channel_id, creator_id, creator_type.as_str()],
            )?;
            tx.execute(
                "INSERT INTO tasks (id, channel_id, task_number, title, created_by_id, created_by_type, sub_channel_id) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    task_id,
                    channel.id,
                    task_number,
                    title,
                    creator_id,
                    creator_type.as_str(),
                    sub_channel_id
                ],
            )?;

            // Emit task_event inside the same tx so the task row and its
            // visibility-in-chat commit atomically. The payload's `actor`
            // field carries a display label resolved from the creator's ID.
            let payload = TaskEventPayload {
                action: TaskEventAction::Created,
                task_number,
                title: title.to_string(),
                sub_channel_id: sub_channel_id.clone(),
                actor: creator_name.clone(),
                prev_status: None,
                next_status: TaskStatus::Todo,
                claimed_by: None,
            };
            let content = payload.to_json_string()?;
            let inserted = Self::create_system_message_tx(&tx, &channel, &content)?;
            pending_events.push((inserted, content));

            result.push(TaskInfo {
                task_number,
                title: title.to_string(),
                status: TaskStatus::Todo.as_str().to_string(),
                claimed_by_name: None,
                created_by_name: Some(creator_name.clone()),
                sub_channel_id: Some(sub_channel_id),
                sub_channel_name: Some(sub_channel_name),
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
    ///
    /// Resolves `created_by_id` / `claimed_by_id` to display names by joining
    /// the matching `humans` or `agents` row at read time. The `tasks` table
    /// itself stores only IDs.
    pub fn get_task_info(&self, channel_name: &str, task_number: i64) -> Result<Option<TaskInfo>> {
        let conn = self.conn.lock().unwrap();
        let channel = Self::get_channel_by_name_inner(&conn, channel_name)?
            .ok_or_else(|| anyhow!("channel not found: {}", channel_name))?;

        let row = conn
            .query_row(
                TASK_INFO_SELECT_SQL_BY_NUMBER,
                params![channel.id, task_number],
                map_task_info_row,
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(other),
            })?;
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
            conn.prepare(TASK_INFO_LIST_SQL_FILTERED)?
                .query_map(params![channel.id, status.as_str()], map_task_info_row)?
                .filter_map(|r| r.ok())
                .collect()
        } else {
            conn.prepare(TASK_INFO_LIST_SQL_ALL)?
                .query_map(params![channel.id], map_task_info_row)?
                .filter_map(|r| r.ok())
                .collect()
        };
        Ok(rows)
    }

    pub fn update_tasks_claim(
        &self,
        channel_name: &str,
        claimer_id: &str,
        claimer_type: SenderType,
        task_numbers: &[i64],
    ) -> Result<Vec<ClaimResult>> {
        // `transaction()` needs `&mut Connection`. Every successful claim in
        // this batch must atomically UPDATE the task row AND add the claimer
        // to the task's sub-channel — otherwise a crash between the two
        // writes leaves membership and task state out of sync.
        let mut conn = self.conn.lock().unwrap();
        let channel = Self::get_channel_by_name_inner(&conn, channel_name)?
            .ok_or_else(|| anyhow!("channel not found: {}", channel_name))?;

        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let claimer_name = Self::sender_name_for_id_tx(&tx, claimer_id, claimer_type)?;

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
                    "SELECT status, claimed_by_id, sub_channel_id, title FROM tasks WHERE channel_id = ?1 AND task_number = ?2",
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
                        "UPDATE tasks SET claimed_by_id = ?1, claimed_by_type = ?2, status = 'in_progress', updated_at = datetime('now') \
                         WHERE channel_id = ?3 AND task_number = ?4 AND status = 'todo' AND claimed_by_id IS NULL",
                        params![claimer_id, claimer_type.as_str(), channel.id, tn],
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
                            "INSERT OR IGNORE INTO channel_members (channel_id, member_id, member_type, last_read_seq) \
                             VALUES (?1, ?2, ?3, 0)",
                            params![sub_id, claimer_id, claimer_type.as_str()],
                        )?;
                    }
                    let payload = TaskEventPayload {
                        action: TaskEventAction::Claimed,
                        task_number: tn,
                        title,
                        sub_channel_id: sub_channel_id.unwrap_or_default(),
                        actor: claimer_name.clone(),
                        prev_status: Some(TaskStatus::Todo),
                        next_status: TaskStatus::InProgress,
                        claimed_by: Some(claimer_name.clone()),
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
        claimer_id: &str,
        claimer_type: SenderType,
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
        // reassign the claim between the check and the UPDATE.
        let (claimed_by_id, claimed_by_type, sub_channel_id, current_status_str, title): (
            Option<String>,
            Option<String>,
            Option<String>,
            String,
            String,
        ) = tx.query_row(
            "SELECT claimed_by_id, claimed_by_type, sub_channel_id, status, title \
             FROM tasks WHERE channel_id = ?1 AND task_number = ?2",
            params![channel.id, task_number],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )?;

        let claimer_name = Self::sender_name_for_id_tx(&tx, claimer_id, claimer_type)?;
        if claimed_by_id.as_deref() != Some(claimer_id)
            || claimed_by_type.as_deref() != Some(claimer_type.as_str())
        {
            return Err(anyhow!("task not claimed by {}", claimer_name));
        }

        // Defense in depth: WHERE-guard on the claimer's (id, type) tuple.
        // If `rows != 1` the claim was stolen mid-flight despite the
        // tx-scoped read — surface it.
        let rows = tx.execute(
            "UPDATE tasks SET claimed_by_id = NULL, claimed_by_type = NULL, status = 'todo', updated_at = datetime('now') \
             WHERE channel_id = ?1 AND task_number = ?2 AND claimed_by_id = ?3 AND claimed_by_type = ?4",
            params![channel.id, task_number, claimer_id, claimer_type.as_str()],
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
                "DELETE FROM channel_members WHERE channel_id = ?1 AND member_id = ?2 AND member_type = ?3",
                params![sub_id, claimer_id, claimer_type.as_str()],
            )?;
        }

        let prev_status = TaskStatus::from_status_str(&current_status_str)
            .ok_or_else(|| anyhow!("invalid task status: {}", current_status_str))?;

        let payload = TaskEventPayload {
            action: TaskEventAction::Unclaimed,
            task_number,
            title,
            sub_channel_id: sub_channel_id.unwrap_or_default(),
            actor: claimer_name,
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
        requester_id: &str,
        requester_type: SenderType,
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

        let (current_status_str, claimed_by_id, claimed_by_type, sub_channel_id, title): (
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            String,
        ) = tx.query_row(
            "SELECT status, claimed_by_id, claimed_by_type, sub_channel_id, title FROM tasks \
             WHERE channel_id = ?1 AND task_number = ?2",
            params![channel.id, task_number],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )?;

        let current_status = TaskStatus::from_status_str(&current_status_str)
            .ok_or_else(|| anyhow!("invalid task status: {}", current_status_str))?;

        let requester_name = Self::sender_name_for_id_tx(&tx, requester_id, requester_type)?;
        if claimed_by_id.as_deref() != Some(requester_id)
            || claimed_by_type.as_deref() != Some(requester_type.as_str())
        {
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
        // (which consumes `sub_channel_id`). Resolve the claimer name for
        // the payload directly from the (id, type) we just read so the
        // displayed value tracks the current row, not a stale lookup.
        let claimed_by_label = match (claimed_by_id.as_deref(), claimed_by_type.as_deref()) {
            (Some(id), Some(type_str)) => {
                let kind = SenderType::from_sender_type_str(type_str);
                Some(Self::sender_name_for_id_tx(&tx, id, kind)?)
            }
            _ => None,
        };
        let payload = TaskEventPayload {
            action: TaskEventAction::StatusChanged,
            task_number,
            title,
            sub_channel_id: sub_channel_id.clone().unwrap_or_default(),
            actor: requester_name,
            prev_status: Some(current_status),
            next_status: new_status,
            claimed_by: claimed_by_label,
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
mod sub_channel_tests {
    use super::*;
    use crate::store::channels::ChannelType;
    use crate::store::humans::Human;
    use crate::store::{AgentRecordUpsert, Store};

    fn seed_agent(store: &Store, name: &str) -> String {
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
            .unwrap()
    }

    fn seed_human(store: &Store, name: &str) -> Human {
        store.create_local_human(name).unwrap()
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
        let alice = seed_human(&store, "alice");

        let result = store
            .create_tasks("eng", &alice.id, SenderType::Human, &["Ship the feature"])
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
            store.channel_member_exists(&sub_id, &alice.id).unwrap(),
            "creator must be a member of the sub-channel"
        );
    }

    #[test]
    fn create_tasks_atomicity_no_orphan_on_partial_failure() {
        let store = Store::open(":memory:").unwrap();
        store
            .create_channel("eng", None, ChannelType::Channel, None)
            .unwrap();
        let alice = seed_human(&store, "alice");

        // Pre-squat the name `eng__task-2` so the SECOND iteration's first
        // INSERT (into `channels`) collides on `channels.name`'s UNIQUE
        // constraint. The first iteration writes channel+member+task rows
        // successfully; those plus the second iteration's partial work must
        // all roll back when `tx.commit()` is never reached.
        {
            let conn = store.conn_for_test();
            let workspace_id: String = conn
                .query_row(
                    "SELECT workspace_id FROM channels WHERE name = 'eng'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            conn.execute(
                "INSERT INTO channels (id, workspace_id, name, channel_type) \
                 VALUES ('squat', ?1, 'eng__task-2', 'channel')",
                params![workspace_id],
            )
            .unwrap();
        }

        let result = store.create_tasks("eng", &alice.id, SenderType::Human, &["first", "second"]);
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
                 WHERE member_id = ?1 \
                   AND channel_id NOT IN (SELECT id FROM channels WHERE name = 'eng')",
                params![alice.id],
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
        let alice = seed_human(&store, "alice");
        let bob_id = seed_agent(&store, "bob");
        store
            .create_tasks("eng", &alice.id, SenderType::Human, &["Ship it"])
            .unwrap();

        let results = store
            .update_tasks_claim("eng", &bob_id, SenderType::Agent, &[1])
            .unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].success, "claim must succeed");

        let (_parent_id, sub_id) = read_task_channel_ids(&store, "eng", 1);
        let sub_id = sub_id.expect("task has sub_channel_id");
        assert!(
            store.channel_member_exists(&sub_id, &bob_id).unwrap(),
            "claimer must join sub-channel"
        );
        assert!(
            store.channel_member_exists(&sub_id, &alice.id).unwrap(),
            "creator must remain a member"
        );
    }

    #[test]
    fn unclaim_rejects_when_claim_was_stolen() {
        // A writer outside the unclaim tx reassigns `claimed_by_id`. The
        // tx-scoped read + WHERE-guarded UPDATE must surface the stolen
        // claim instead of silently NULL-ing out carol's hold.
        let store = Store::open(":memory:").unwrap();
        store
            .create_channel("eng", None, ChannelType::Channel, None)
            .unwrap();
        let alice = seed_human(&store, "alice");
        let bob_id = seed_agent(&store, "bob");
        let carol_id = seed_agent(&store, "carol");
        store
            .create_tasks("eng", &alice.id, SenderType::Human, &["ship it"])
            .unwrap();
        store
            .update_tasks_claim("eng", &bob_id, SenderType::Agent, &[1])
            .unwrap();

        // Simulate a write that re-assigns the claim outside of bob's control.
        {
            let conn = store.conn_for_test();
            conn.execute(
                "UPDATE tasks SET claimed_by_id = ?1, claimed_by_type = 'agent' WHERE task_number = 1",
                params![carol_id],
            )
            .unwrap();
        }

        let err = store
            .update_task_unclaim("eng", &bob_id, SenderType::Agent, 1)
            .unwrap_err();
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
        let alice = seed_human(&store, "alice");
        let bob_id = seed_agent(&store, "bob");
        store
            .create_tasks("eng", &alice.id, SenderType::Human, &["Ship it"])
            .unwrap();
        let claim = store
            .update_tasks_claim("eng", &bob_id, SenderType::Agent, &[1])
            .unwrap();
        assert!(claim[0].success);

        store
            .update_task_unclaim("eng", &bob_id, SenderType::Agent, 1)
            .unwrap();

        let (_parent_id, sub_id) = read_task_channel_ids(&store, "eng", 1);
        let sub_id = sub_id.expect("task has sub_channel_id");
        assert!(
            !store.channel_member_exists(&sub_id, &bob_id).unwrap(),
            "unclaimer must leave sub-channel"
        );
        assert!(
            store.channel_member_exists(&sub_id, &alice.id).unwrap(),
            "creator stays"
        );
    }

    #[test]
    fn status_done_archives_sub_channel() {
        let store = Store::open(":memory:").unwrap();
        store
            .create_channel("eng", None, ChannelType::Channel, None)
            .unwrap();
        let alice = seed_human(&store, "alice");
        let bob_id = seed_agent(&store, "bob");
        store
            .create_tasks("eng", &alice.id, SenderType::Human, &["Ship it"])
            .unwrap();
        store
            .update_tasks_claim("eng", &bob_id, SenderType::Agent, &[1])
            .unwrap();
        store
            .update_task_status("eng", 1, &bob_id, SenderType::Agent, TaskStatus::InReview)
            .unwrap();

        store
            .update_task_status("eng", 1, &bob_id, SenderType::Agent, TaskStatus::Done)
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
        let alice = seed_human(&store, "alice");
        let bob_id = seed_agent(&store, "bob");
        store
            .create_tasks(
                "eng",
                &alice.id,
                SenderType::Human,
                &["Ship it", "Also ship it"],
            )
            .unwrap();
        store
            .update_tasks_claim("eng", &bob_id, SenderType::Agent, &[1, 2])
            .unwrap();

        // Before Done: both task sub-channels appear in bob's inbox.
        let before: Vec<String> = store
            .get_inbox_conversation_notifications(&bob_id)
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
            .update_task_status("eng", 1, &bob_id, SenderType::Agent, TaskStatus::InReview)
            .unwrap();
        store
            .update_task_status("eng", 1, &bob_id, SenderType::Agent, TaskStatus::Done)
            .unwrap();

        let after: Vec<String> = store
            .get_inbox_conversation_notifications(&bob_id)
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
        let alice = seed_human(&store, "alice");
        let bob_id = seed_agent(&store, "bob");
        store
            .create_tasks("eng", &alice.id, SenderType::Human, &["Ship it"])
            .unwrap();
        store
            .update_tasks_claim("eng", &bob_id, SenderType::Agent, &[1])
            .unwrap();
        store
            .update_task_status("eng", 1, &bob_id, SenderType::Agent, TaskStatus::InReview)
            .unwrap();
        store
            .update_task_status("eng", 1, &bob_id, SenderType::Agent, TaskStatus::Done)
            .unwrap();

        let (_parent_id, sub_id) = read_task_channel_ids(&store, "eng", 1);
        let sub_id = sub_id.expect("task has sub_channel_id");

        // Archived sub-channel is gone from the sidebar listing.
        let list_names: Vec<String> = store
            .get_inbox_conversation_notifications(&alice.id)
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
            .get_inbox_conversation_notification_for_member(&sub_id, &alice.id)
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
        let alice = seed_human(&store, "alice");
        store
            .create_tasks("eng", &alice.id, SenderType::Human, &["x"])
            .unwrap();

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
