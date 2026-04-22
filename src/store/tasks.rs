use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

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

impl Store {
    /// Create one or more tasks under `channel_name`, each with its own
    /// `ChannelType::Task` sub-channel and the creator enrolled as the first
    /// member of that sub-channel. All inserts — sub-channel, first member,
    /// and task row — run inside a single `conn.transaction()` so a partial
    /// failure on any task leaves zero orphan channels or membership rows.
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

        // BEGIN IMMEDIATE via `conn.transaction()`. Acquires the SQLite write
        // lock up-front so a concurrent `create_tasks` on the same parent
        // either serializes cleanly or fails fast — and if any INSERT below
        // errors, the channel + member rows roll back with the task row.
        let tx = conn.transaction()?;
        // tx-scoped: MAX under write lock so task_number is race-free across
        // concurrent `create_tasks` calls on the same parent.
        let max_num: i64 = tx.query_row(
            "SELECT COALESCE(MAX(task_number), 0) FROM tasks WHERE channel_id = ?1",
            params![channel.id],
            |row| row.get(0),
        )?;
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
            tx.execute(
                "INSERT INTO tasks (id, channel_id, task_number, title, created_by, sub_channel_id) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    task_id,
                    channel.id,
                    task_number,
                    title,
                    creator_name,
                    sub_channel_id
                ],
            )?;

            result.push(TaskInfo {
                task_number,
                title: title.to_string(),
                status: TaskStatus::Todo.as_str().to_string(),
                claimed_by_name: None,
                created_by_name: Some(creator_name.to_string()),
                sub_channel_id: Some(sub_channel_id),
                sub_channel_name: Some(sub_channel_name),
            });
        }
        tx.commit()?;
        Ok(result)
    }

    pub fn get_tasks(
        &self,
        channel_name: &str,
        status_filter: Option<TaskStatus>,
    ) -> Result<Vec<TaskInfo>> {
        let conn = self.conn.lock().unwrap();
        let channel = Self::get_channel_by_name_inner(&conn, channel_name)?
            .ok_or_else(|| anyhow!("channel not found: {}", channel_name))?;

        let map_row = |row: &rusqlite::Row| -> rusqlite::Result<TaskInfo> {
            Ok(TaskInfo {
                task_number: row.get(0)?,
                title: row.get(1)?,
                status: row.get(2)?,
                claimed_by_name: row.get(3)?,
                created_by_name: row.get(4)?,
                sub_channel_id: row.get(5)?,
                sub_channel_name: row.get(6)?,
            })
        };

        let rows: Vec<TaskInfo> = if let Some(status) = status_filter {
            conn.prepare(
                "SELECT t.task_number, t.title, t.status, t.claimed_by, t.created_by, \
                        t.sub_channel_id, c.name \
                 FROM tasks t \
                 LEFT JOIN channels c ON c.id = t.sub_channel_id \
                 WHERE t.channel_id = ?1 AND t.status = ?2 \
                 ORDER BY t.task_number",
            )?
            .query_map(params![channel.id, status.as_str()], map_row)?
            .filter_map(|r| r.ok())
            .collect()
        } else {
            conn.prepare(
                "SELECT t.task_number, t.title, t.status, t.claimed_by, t.created_by, \
                        t.sub_channel_id, c.name \
                 FROM tasks t \
                 LEFT JOIN channels c ON c.id = t.sub_channel_id \
                 WHERE t.channel_id = ?1 \
                 ORDER BY t.task_number",
            )?
            .query_map(params![channel.id], map_row)?
            .filter_map(|r| r.ok())
            .collect()
        };
        Ok(rows)
    }

    pub fn update_tasks_claim(
        &self,
        channel_name: &str,
        claimer_name: &str,
        task_numbers: &[i64],
    ) -> Result<Vec<ClaimResult>> {
        let conn = self.conn.lock().unwrap();
        let channel = Self::get_channel_by_name_inner(&conn, channel_name)?
            .ok_or_else(|| anyhow!("channel not found: {}", channel_name))?;

        let mut results = Vec::new();
        for &tn in task_numbers {
            let task: Option<(String, Option<String>)> = conn
                .query_row(
                    "SELECT status, claimed_by FROM tasks WHERE channel_id = ?1 AND task_number = ?2",
                    params![channel.id, tn],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .ok();

            match task {
                Some((status, claimed_by)) if status == "todo" && claimed_by.is_none() => {
                    conn.execute(
                        "UPDATE tasks SET claimed_by = ?1, status = 'in_progress', updated_at = datetime('now') WHERE channel_id = ?2 AND task_number = ?3",
                        params![claimer_name, channel.id, tn],
                    )?;
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
        Ok(results)
    }

    pub fn update_task_unclaim(
        &self,
        channel_name: &str,
        claimer_name: &str,
        task_number: i64,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let channel = Self::get_channel_by_name_inner(&conn, channel_name)?
            .ok_or_else(|| anyhow!("channel not found: {}", channel_name))?;

        let claimed_by: Option<String> = conn.query_row(
            "SELECT claimed_by FROM tasks WHERE channel_id = ?1 AND task_number = ?2",
            params![channel.id, task_number],
            |row| row.get(0),
        )?;

        if claimed_by.as_deref() != Some(claimer_name) {
            return Err(anyhow!("task not claimed by {}", claimer_name));
        }

        conn.execute(
            "UPDATE tasks SET claimed_by = NULL, status = 'todo', updated_at = datetime('now') WHERE channel_id = ?1 AND task_number = ?2",
            params![channel.id, task_number],
        )?;
        Ok(())
    }

    pub fn update_task_status(
        &self,
        channel_name: &str,
        task_number: i64,
        requester_name: &str,
        new_status: TaskStatus,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let channel = Self::get_channel_by_name_inner(&conn, channel_name)?
            .ok_or_else(|| anyhow!("channel not found: {}", channel_name))?;

        let (current_status_str, claimed_by): (String, Option<String>) = conn.query_row(
            "SELECT status, claimed_by FROM tasks WHERE channel_id = ?1 AND task_number = ?2",
            params![channel.id, task_number],
            |row| Ok((row.get(0)?, row.get(1)?)),
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

        conn.execute(
            "UPDATE tasks SET status = ?1, updated_at = datetime('now') WHERE channel_id = ?2 AND task_number = ?3",
            params![new_status.as_str(), channel.id, task_number],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod sub_channel_tests {
    use super::*;
    use crate::store::channels::ChannelType;
    use crate::store::Store;

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
        assert_eq!(sub.parent_channel_id.as_deref(), Some(parent_channel_id.as_str()));

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
}
