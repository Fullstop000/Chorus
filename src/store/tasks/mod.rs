pub mod events;

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use self::events::{
    post_task_card_message_tx, post_task_event_tx, TaskEventAction, TaskEventPayload,
};
use super::channels::{Channel, ChannelType};
use super::messages::types::InsertedMessage;
use super::Store;

/// Typed error returned by [`Store::update_task_status`] when a transition is
/// not permitted by [`TaskStatus::can_transition_to`]. HTTP handlers can
/// `downcast_ref` this to map to 422.
#[derive(Debug, thiserror::Error)]
#[error("invalid task transition: {from:?} -> {to:?}")]
pub struct InvalidTaskTransition {
    pub from: TaskStatus,
    pub to: TaskStatus,
}

/// Normalize a timestamp string to RFC3339 UTC form (`"YYYY-MM-DDTHH:MM:SSZ"`).
///
/// Accepts:
///   - SQLite native: `"YYYY-MM-DD HH:MM:SS"` (no TZ, implicitly UTC) — what
///     `datetime('now')` produces.
///   - RFC3339 already: `"YYYY-MM-DDTHH:MM:SSZ"` or offset-bearing forms
///     like `"2026-04-25T10:30:45+00:00"`.
///
/// The snapshot spec requires canonical RFC3339 UTC for cross-surface
/// consistency (UI, MCP tools, SSE). Callers copy a source message's
/// `created_at` into a task row's snapshot columns, and that source can come
/// from either format — this helper guarantees the stored form is canonical
/// regardless of input shape. Errors when chrono can't parse the input.
pub(crate) fn normalize_sqlite_timestamp(ts: &str) -> Result<String> {
    use chrono::NaiveDateTime;

    // Try RFC3339 first — handles `2026-04-25T00:00:00Z` and offset-bearing forms.
    if let Ok(dt) = DateTime::parse_from_rfc3339(ts) {
        return Ok(dt
            .with_timezone(&Utc)
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string());
    }
    // Fall back to SQLite's native `datetime('now')` shape (implicit UTC).
    if let Ok(naive) = NaiveDateTime::parse_from_str(ts, "%Y-%m-%d %H:%M:%S") {
        return Ok(naive
            .and_utc()
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string());
    }
    Err(anyhow!("timestamp not in known format: {}", ts))
}

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

/// Arguments for [`Store::create_proposed_task`] — the agent-driven create
/// path. All five snapshot fields are required together because the schema's
/// CHECK constraint enforces all-or-none snapshot presence (see
/// `schema.sql` `tasks` table). `snapshot_created_at` must already be in
/// canonical RFC3339 UTC form; callers get there via
/// [`normalize_sqlite_timestamp`].
#[derive(Debug, Clone)]
pub struct CreateProposedTaskArgs {
    /// Title line for the proposed task.
    pub title: String,
    /// Creator handle (agent name).
    pub created_by: String,
    /// Chat message the proposal was carved from.
    pub source_message_id: String,
    /// Snapshot of the source message's sender display name.
    pub snapshot_sender_name: String,
    /// Snapshot of the source message's sender type (`human`/`agent`/`system`).
    pub snapshot_sender_type: String,
    /// Snapshot of the source message's body at carve time.
    pub snapshot_content: String,
    /// Snapshot of the source message's created_at, RFC3339 UTC.
    pub snapshot_created_at: String,
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

/// Mint a `ChannelType::Task` sub-channel for a task and seed membership with
/// the task's creator and the accepting actor. Both `create_tasks` (direct
/// create) and `update_task_status` (proposed → todo acceptance) use this so
/// the sub-channel shape stays identical regardless of entry point.
///
/// Membership rules:
///   - `task.created_by` is always seeded. Sender type is resolved inside the
///     same transaction via [`resolve_sender_type_inner`].
///   - `actor` is seeded when it differs from `task.created_by`. `INSERT OR
///     IGNORE` keeps this idempotent if the caller somehow hands us the same
///     name twice.
///
/// Returns the freshly-created `Channel` row so callers can pass it to
/// [`Store::emit_system_stream_events`] without a second lookup.
fn mint_sub_channel_tx(
    tx: &Transaction<'_>,
    parent: &Channel,
    task: &TaskInfo,
    actor: &str,
) -> Result<Channel> {
    let sub_channel_id = Uuid::new_v4().to_string();
    let sub_channel_name = format!("{}__task-{}", parent.name, task.task_number);
    let task_channel_type = ChannelType::Task.as_api_str();

    tx.execute(
        "INSERT INTO channels (id, name, description, channel_type, parent_channel_id) \
         VALUES (?1, ?2, NULL, ?3, ?4)",
        params![sub_channel_id, sub_channel_name, task_channel_type, parent.id],
    )?;

    // Resolve sender types inside the tx so the humans/agents lookup stays
    // consistent with the rest of this transaction's writes.
    let creator_type = crate::store::resolve_sender_type_inner(tx, &task.created_by)?;
    tx.execute(
        "INSERT INTO channel_members (channel_id, member_name, member_type, last_read_seq) \
         VALUES (?1, ?2, ?3, 0)",
        params![sub_channel_id, task.created_by, creator_type.as_str()],
    )?;

    if actor != task.created_by {
        let actor_type = crate::store::resolve_sender_type_inner(tx, actor)?;
        tx.execute(
            "INSERT OR IGNORE INTO channel_members (channel_id, member_name, member_type, last_read_seq) \
             VALUES (?1, ?2, ?3, 0)",
            params![sub_channel_id, actor, actor_type.as_str()],
        )?;
    }

    Store::get_channel_by_id_inner(tx, &sub_channel_id)?
        .ok_or_else(|| anyhow!("sub-channel vanished immediately after insert: {}", sub_channel_id))
}

/// Post the kickoff system message in a task's sub-channel. Two shapes:
///   - With snapshot (proposal acceptance):
///       "Task opened: {title}\n\nFrom @{sender}'s message in #{parent}:\n> {content}"
///   - Without snapshot (direct-created todo):
///       "Task opened: {title}"
///
/// Exact format matches PR #96's contract asserted by Playwright TSK-005.
/// Returns `(InsertedMessage, String)` for the caller's pending-events buffer.
fn post_kickoff_message_tx(
    tx: &Transaction<'_>,
    parent: &Channel,
    sub_channel: &Channel,
    task: &TaskInfo,
) -> Result<(InsertedMessage, String)> {
    let content = match (
        task.snapshot_sender_name.as_deref(),
        task.snapshot_content.as_deref(),
    ) {
        (Some(sender), Some(source)) => format!(
            "Task opened: {}\n\nFrom @{}'s message in #{}:\n> {}",
            task.title, sender, parent.name, source
        ),
        _ => format!("Task opened: {}", task.title),
    };
    let msg = Store::create_system_message_tx(tx, sub_channel, &content)?;
    Ok((msg, content))
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

        // Parent- and sub-channel events live in separate fanout vectors
        // because `emit_system_stream_events` tags every event with the single
        // `&Channel` it receives — mixing sub-channel events into the parent
        // vector would misroute them to parent subscribers. Each vector is
        // keyed to its owning channel and fanned out in its own call below.
        let mut parent_events: Vec<(InsertedMessage, String)> = Vec::new();
        // Sub-channel events carry their owning `Channel` so we can fan out per
        // channel without re-reading the row post-commit. Each direct-create
        // task pushes its kickoff row here.
        let mut sub_events: Vec<(Channel, InsertedMessage, String)> = Vec::new();

        let mut result = Vec::new();
        for (i, title) in titles.iter().enumerate() {
            let task_id = Uuid::new_v4().to_string();
            let task_number = max_num + 1 + i as i64;

            // Build a partial TaskInfo first so `mint_sub_channel_tx` has the
            // task_number + created_by it needs. The sub-channel + membership
            // inserts happen inside the helper.
            let partial = TaskInfo {
                id: task_id.clone(),
                task_number,
                title: title.to_string(),
                status: TaskStatus::Todo,
                owner: None,
                created_by: creator_name.to_string(),
                created_at: String::new(),
                updated_at: String::new(),
                sub_channel_id: None,
                sub_channel_name: None,
                source_message_id: None,
                snapshot_sender_name: None,
                snapshot_sender_type: None,
                snapshot_content: None,
                snapshot_created_at: None,
            };
            let sub_channel = mint_sub_channel_tx(&tx, &channel, &partial, creator_name)?;

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
                    sub_channel.id
                ],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;

            let task = TaskInfo {
                id: task_id,
                task_number,
                title: title.to_string(),
                status: TaskStatus::Todo,
                owner: None,
                created_by: creator_name.to_string(),
                created_at,
                updated_at,
                sub_channel_id: Some(sub_channel.id.clone()),
                sub_channel_name: Some(sub_channel.name.clone()),
                source_message_id: None,
                snapshot_sender_name: None,
                snapshot_sender_type: None,
                snapshot_content: None,
                snapshot_created_at: None,
            };

            // Post the `task_card` host message in the parent channel. This
            // replaces the old `task_event(Created)` emission — the card is
            // the canonical parent-channel surface for a task and re-renders
            // in place on every subsequent `task_update` SSE event.
            parent_events.push(post_task_card_message_tx(&tx, &channel, &task)?);

            // Kickoff is posted unconditionally so the sub-channel's first
            // message is the same "Task opened: {title}" divider regardless
            // of whether the task was accepted from a proposal or created
            // directly. Direct-created tasks have no snapshot → the short
            // form of the kickoff (title only).
            let (kickoff_msg, kickoff_content) =
                post_kickoff_message_tx(&tx, &channel, &sub_channel, &task)?;
            sub_events.push((sub_channel, kickoff_msg, kickoff_content));

            result.push(task);
        }
        tx.commit()?;
        drop(conn); // release the mutex guard before the stream fanout

        // Two separate fanouts — one per channel the events belong to.
        self.emit_system_stream_events(&channel, parent_events)?;
        for (sub_channel, inserted, content) in sub_events {
            self.emit_system_stream_events(&sub_channel, vec![(inserted, content)])?;
        }
        // Cross-channel task fan-out: every connected client patches their
        // tasksById store, so the parent-channel task_card host re-renders
        // even for non-members of the sub-channel.
        for task in &result {
            self.emit_task_update(task, &channel.id);
        }
        Ok(result)
    }

    /// Agent-driven create path. Always inserts with `status='proposed'` and
    /// no sub-channel — proposals are pre-acceptance and carry no
    /// collaboration scope yet. A `task_card` host message is posted in the
    /// parent channel so the proposal is visible in chat; no `task_event` is
    /// emitted because there's no sub-channel, no claim, and no status
    /// transition at this point.
    ///
    /// The snapshot bundle (`source_message_id` + 4 `snapshot_*` fields) is
    /// required — the schema CHECK constraint enforces all-or-none, and
    /// proposals always originate from a chat message. Callers must
    /// pre-normalize `snapshot_created_at` to RFC3339 UTC using
    /// [`normalize_sqlite_timestamp`].
    pub fn create_proposed_task(
        &self,
        channel_name: &str,
        args: CreateProposedTaskArgs,
    ) -> Result<TaskInfo> {
        let mut conn = self.conn.lock().unwrap();
        let channel = Self::get_channel_by_name_inner(&conn, channel_name)?
            .ok_or_else(|| anyhow!("channel not found: {}", channel_name))?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

        // Race-free allocation of task_number under the write lock — mirrors
        // `create_tasks`. A concurrent proposal on the same parent blocks until
        // this transaction commits or fails.
        let task_number: i64 = tx.query_row(
            "SELECT COALESCE(MAX(task_number), 0) + 1 FROM tasks WHERE channel_id = ?1",
            params![channel.id],
            |r| r.get(0),
        )?;

        let id = Uuid::new_v4().to_string();

        // `status = 'proposed'`, `sub_channel_id IS NULL`, snapshot fully
        // populated. The schema CHECK on snapshot all-or-none rejects
        // partial-snapshot inserts — surfacing a SQLite error back to the
        // caller is the right move (invalid input, not silent fallback).
        tx.execute(
            "INSERT INTO tasks \
               (id, channel_id, task_number, title, status, owner, \
                created_by, sub_channel_id, \
                source_message_id, snapshot_sender_name, snapshot_sender_type, \
                snapshot_content, snapshot_created_at) \
             VALUES (?1, ?2, ?3, ?4, 'proposed', NULL, ?5, NULL, \
                     ?6, ?7, ?8, ?9, ?10)",
            params![
                id,
                channel.id,
                task_number,
                args.title,
                args.created_by,
                args.source_message_id,
                args.snapshot_sender_name,
                args.snapshot_sender_type,
                args.snapshot_content,
                args.snapshot_created_at,
            ],
        )?;

        let task = Self::get_task_info_tx(&tx, &channel.id, task_number)?
            .ok_or_else(|| anyhow!("freshly-inserted proposed task disappeared"))?;

        // The `task_card` host message is the ONLY chat-visible signal for a
        // proposal — there's no sub-channel to post a kickoff in yet, and no
        // `task_event` fires for pre-acceptance transitions per spec.
        let pending = vec![post_task_card_message_tx(&tx, &channel, &task)?];

        tx.commit()?;
        drop(conn);

        self.emit_system_stream_events(&channel, pending)?;
        self.emit_task_update(&task, &channel.id);
        Ok(task)
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

    /// Transaction-scoped variant of `get_task_info`. Used by create paths
    /// that just inserted a task row and need the canonical 15-column
    /// `TaskInfo` (with `sub_channel_name` joined) without releasing the
    /// write lock. Returns `Ok(None)` when the row isn't visible in the tx.
    pub(crate) fn get_task_info_tx(
        tx: &Transaction<'_>,
        channel_id: &str,
        task_number: i64,
    ) -> Result<Option<TaskInfo>> {
        let row: Option<TaskInfo> = match tx.query_row(
            "SELECT t.id, t.task_number, t.title, t.status, t.owner, t.created_by, \
                    t.created_at, t.updated_at, t.sub_channel_id, c.name AS sub_channel_name, \
                    t.source_message_id, t.snapshot_sender_name, t.snapshot_sender_type, \
                    t.snapshot_content, t.snapshot_created_at \
             FROM tasks t \
             LEFT JOIN channels c ON c.id = t.sub_channel_id \
             WHERE t.channel_id = ?1 AND t.task_number = ?2 \
             LIMIT 1",
            params![channel_id, task_number],
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
        // Claim sets `owner = ?` only; it does NOT advance status. Decoupled
        // from start: `[claim] → [start]` are two affordances on the TaskCard,
        // not one. Spec: owner is a label, not permission. Membership is the
        // gate (enforced upstream in the HTTP handler).
        //
        // Claim is allowed on the three live states (Todo, InProgress, InReview)
        // — anyone can re-claim / steal — and rejected on terminal/proposal
        // states (Proposed, Dismissed, Done). The status-IN guard in the
        // UPDATE makes that race-free.
        //
        // Sub-channel membership sync stays: claimer joins the sub-channel so
        // they receive its inbox notifications. `task_event` posts in the
        // sub-channel (not the parent) — sub-channel is where the work happens.
        let mut conn = self.conn.lock().unwrap();
        let channel = Self::get_channel_by_name_inner(&conn, channel_name)?
            .ok_or_else(|| anyhow!("channel not found: {}", channel_name))?;

        let claimer_type = crate::store::resolve_sender_type_inner(&conn, claimer_name)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

        let mut results = Vec::new();
        // Per-sub-channel pending events: each task's claim event goes to its
        // own sub-channel, not a single shared parent. Keyed by sub_channel_id
        // so we can fan them out one channel at a time after commit.
        let mut sub_pending: std::collections::HashMap<
            String,
            Vec<(InsertedMessage, String)>,
        > = std::collections::HashMap::new();

        for &tn in task_numbers {
            let task: Option<(String, Option<String>, Option<String>, String)> = tx
                .query_row(
                    "SELECT status, owner, sub_channel_id, title FROM tasks WHERE channel_id = ?1 AND task_number = ?2",
                    params![channel.id, tn],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .ok();

            match task {
                Some((status_str, _existing_owner, sub_channel_id, title)) => {
                    let prev_status = match TaskStatus::from_status_str(&status_str) {
                        Some(s) => s,
                        None => {
                            results.push(ClaimResult {
                                task_number: tn,
                                success: false,
                                reason: Some(format!("invalid task status: {status_str}")),
                            });
                            continue;
                        }
                    };
                    if !matches!(
                        prev_status,
                        TaskStatus::Todo | TaskStatus::InProgress | TaskStatus::InReview
                    ) {
                        results.push(ClaimResult {
                            task_number: tn,
                            success: false,
                            reason: Some(format!(
                                "cannot claim task in {:?} state",
                                prev_status
                            )),
                        });
                        continue;
                    }

                    let rows = tx.execute(
                        "UPDATE tasks SET owner = ?1, updated_at = datetime('now') \
                         WHERE channel_id = ?2 AND task_number = ?3 \
                           AND status IN ('todo','in_progress','in_review')",
                        params![claimer_name, channel.id, tn],
                    )?;
                    if rows != 1 {
                        // Mid-flight terminal transition stole the task out
                        // from under us (e.g. -> Done before our UPDATE landed).
                        results.push(ClaimResult {
                            task_number: tn,
                            success: false,
                            reason: Some("task left claimable state mid-flight".to_string()),
                        });
                        continue;
                    }

                    let sub_id = sub_channel_id.clone().ok_or_else(|| {
                        anyhow!("task #{tn} in claimable state without sub_channel_id")
                    })?;

                    // Claimer joins the sub-channel. Idempotent.
                    tx.execute(
                        "INSERT OR IGNORE INTO channel_members (channel_id, member_name, member_type, last_read_seq) \
                         VALUES (?1, ?2, ?3, 0)",
                        params![sub_id, claimer_name, claimer_type.as_str()],
                    )?;

                    let payload = TaskEventPayload {
                        action: TaskEventAction::Claimed,
                        task_number: tn,
                        title,
                        sub_channel_id: sub_id.clone(),
                        actor: claimer_name.to_string(),
                        // Status didn't change — claim is now decoupled.
                        prev_status: Some(prev_status),
                        next_status: prev_status,
                        claimed_by: Some(claimer_name.to_string()),
                    };
                    sub_pending
                        .entry(sub_id.clone())
                        .or_default()
                        .push(post_task_event_tx(&tx, &sub_id, payload)?);
                    results.push(ClaimResult {
                        task_number: tn,
                        success: true,
                        reason: None,
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

        // Fan out per sub-channel. Each call needs its own `&Channel` so the
        // event payloads carry the correct channel id/name.
        for (sub_id, events) in sub_pending {
            let sub_channel = self
                .get_channel_by_id(&sub_id)?
                .ok_or_else(|| anyhow!("sub-channel vanished after commit: {}", sub_id))?;
            self.emit_system_stream_events(&sub_channel, events)?;
        }
        // Cross-channel task fan-out for every successful claim. Re-load
        // post-commit so the broadcast carries the freshly-stamped owner +
        // updated_at. Failures here are logged-only.
        for r in results.iter().filter(|r| r.success) {
            if let Ok(Some(t)) = self.get_task_info(channel_name, r.task_number) {
                self.emit_task_update(&t, &channel.id);
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
        // Unclaim clears `owner = NULL` only; status stays put. Decoupled
        // from "go back to todo" — the task remains in whatever live state it
        // was in. Same TOCTOU shape as claim: `WHERE owner = ?` guards
        // against a stolen claim landing between SELECT and UPDATE.
        //
        // `task_event(Unclaimed)` posts in the sub-channel (not the parent).
        let mut conn = self.conn.lock().unwrap();
        let channel = Self::get_channel_by_name_inner(&conn, channel_name)?
            .ok_or_else(|| anyhow!("channel not found: {}", channel_name))?;

        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

        let (owner, sub_channel_id, current_status_str, title): (
            Option<String>,
            Option<String>,
            String,
            String,
        ) = tx.query_row(
            "SELECT owner, sub_channel_id, status, title FROM tasks WHERE channel_id = ?1 AND task_number = ?2",
            params![channel.id, task_number],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )?;

        if owner.as_deref() != Some(claimer_name) {
            return Err(anyhow!("task not claimed by {}", claimer_name));
        }

        let rows = tx.execute(
            "UPDATE tasks SET owner = NULL, updated_at = datetime('now') \
             WHERE channel_id = ?1 AND task_number = ?2 AND owner = ?3",
            params![channel.id, task_number, claimer_name],
        )?;
        if rows != 1 {
            return Err(anyhow!(
                "task {} no longer claimed by {}",
                task_number,
                claimer_name
            ));
        }

        let sub_id = sub_channel_id.ok_or_else(|| {
            anyhow!("task #{task_number} in claimable state without sub_channel_id")
        })?;

        tx.execute(
            "DELETE FROM channel_members WHERE channel_id = ?1 AND member_name = ?2",
            params![sub_id, claimer_name],
        )?;

        let prev_status = TaskStatus::from_status_str(&current_status_str)
            .ok_or_else(|| anyhow!("invalid task status: {}", current_status_str))?;

        let payload = TaskEventPayload {
            action: TaskEventAction::Unclaimed,
            task_number,
            title,
            sub_channel_id: sub_id.clone(),
            actor: claimer_name.to_string(),
            // Status didn't change — unclaim is now decoupled.
            prev_status: Some(prev_status),
            next_status: prev_status,
            claimed_by: None,
        };
        let event = post_task_event_tx(&tx, &sub_id, payload)?;
        tx.commit()?;
        drop(conn);

        let sub_channel = self
            .get_channel_by_id(&sub_id)?
            .ok_or_else(|| anyhow!("sub-channel vanished after commit: {}", sub_id))?;
        self.emit_system_stream_events(&sub_channel, vec![event])?;
        if let Ok(Some(t)) = self.get_task_info(channel_name, task_number) {
            self.emit_task_update(&t, &channel.id);
        }
        Ok(())
    }

    /// Drive a task forward through its state machine. Validates the
    /// transition against [`TaskStatus::can_transition_to`] (forward-only in
    /// v1 — no reverse edges), mints a sub-channel + posts kickoff on
    /// `Proposed → Todo`, fires a `task_event` in the sub-channel on
    /// post-acceptance transitions, and archives the sub-channel on `→ Done`.
    ///
    /// `actor` is the caller's handle — used as the `task_event.actor` field
    /// and as a seeded member of the sub-channel on first acceptance. It is
    /// NOT an ownership gate: per spec, owner is a label, not permission.
    /// Membership is the only authorization gate and lives in the HTTP
    /// handler layer.
    ///
    /// Returns the updated [`TaskInfo`] so the HTTP handler can surface the
    /// new `sub_channel_id` + `status` in the response body.
    pub fn update_task_status(
        &self,
        channel_name: &str,
        task_number: i64,
        actor: &str,
        new_status: TaskStatus,
    ) -> Result<TaskInfo> {
        // `transaction()` needs `&mut Connection`. The status UPDATE and the
        // sub-channel archive (when `new_status == Done`) must commit together
        // so an observer never sees a task marked Done whose sub-channel is
        // still active (or vice versa).
        let mut conn = self.conn.lock().unwrap();
        let channel = Self::get_channel_by_name_inner(&conn, channel_name)?
            .ok_or_else(|| anyhow!("channel not found: {}", channel_name))?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

        let mut task = Self::get_task_info_tx(&tx, &channel.id, task_number)?
            .ok_or_else(|| anyhow!("task not found: {}#{}", channel_name, task_number))?;
        let current_status = task.status;

        if !current_status.can_transition_to(new_status) {
            // Typed error so the HTTP handler can map to 422 via downcast_ref.
            return Err(InvalidTaskTransition {
                from: current_status,
                to: new_status,
            }
            .into());
        }

        // Per-turn sub-channel fanout — populated when we mint a sub-channel
        // this turn OR when we post a task_event into an existing one. Tagged
        // with the owning `Channel` so the final fanout call routes correctly.
        let mut sub_pending: Vec<(InsertedMessage, String)> = Vec::new();
        let mut sub_channel_for_emit: Option<Channel> = None;

        // Proposed → Todo: mint a sub-channel + post the kickoff message.
        // The kickoff carries the snapshot (if any) so the sub-channel's first
        // message is "Task opened: ..." grounded in the proposal's source
        // message. Proposed → Dismissed is pure state mutation — no channel.
        if current_status == TaskStatus::Proposed && new_status == TaskStatus::Todo {
            let sub = mint_sub_channel_tx(&tx, &channel, &task, actor)?;
            task.sub_channel_id = Some(sub.id.clone());
            task.sub_channel_name = Some(sub.name.clone());
            sub_pending.push(post_kickoff_message_tx(&tx, &channel, &sub, &task)?);
            sub_channel_for_emit = Some(sub);
        }

        // Apply status change + (when we minted one this turn) the new
        // sub_channel_id. COALESCE keeps the existing value when we pass NULL
        // so we don't clobber the sub-channel on post-acceptance transitions.
        tx.execute(
            "UPDATE tasks \
             SET status = ?1, sub_channel_id = COALESCE(?2, sub_channel_id), updated_at = datetime('now') \
             WHERE channel_id = ?3 AND task_number = ?4",
            params![
                new_status.as_str(),
                task.sub_channel_id,
                channel.id,
                task_number
            ],
        )?;
        task.status = new_status;

        // Fire the `task_event` only for post-acceptance transitions. Pre-
        // acceptance transitions (Proposed → Todo, Proposed → Dismissed) are
        // signaled by the parent-channel `task_card` re-render via the
        // `task_update` SSE event — no separate system message.
        if current_status != TaskStatus::Proposed {
            let sub_id = task.sub_channel_id.as_deref().ok_or_else(|| {
                anyhow!("post-acceptance transition requires sub-channel: task #{task_number}")
            })?;
            let payload = TaskEventPayload {
                action: TaskEventAction::StatusChanged,
                task_number,
                title: task.title.clone(),
                sub_channel_id: sub_id.to_string(),
                actor: actor.to_string(),
                prev_status: Some(current_status),
                next_status: new_status,
                claimed_by: task.owner.clone(),
            };
            let ev = post_task_event_tx(&tx, sub_id, payload)?;

            // Load the sub-channel for fan-out if we didn't mint it above.
            if sub_channel_for_emit.is_none() {
                sub_channel_for_emit = Some(
                    Self::get_channel_by_id_inner(&tx, sub_id)?
                        .ok_or_else(|| anyhow!("sub-channel vanished: {}", sub_id))?,
                );
            }
            sub_pending.push(ev);
        }

        // `Done` is terminal (`can_transition_to` has no outbound edges from
        // `Done`), so archiving here is safe — there is no path back that would
        // need to un-archive. Bypasses the `archive_channel` guard on purpose:
        // that guard rejects direct callers; the task lifecycle is the sole
        // path that may archive a task sub-channel.
        if new_status == TaskStatus::Done {
            if let Some(sub_id) = task.sub_channel_id.as_deref() {
                tx.execute(
                    "UPDATE channels SET archived = 1 WHERE id = ?1",
                    params![sub_id],
                )?;
            }
        }

        tx.commit()?;
        drop(conn);

        if let Some(sc) = sub_channel_for_emit {
            self.emit_system_stream_events(&sc, sub_pending)?;
        }
        self.emit_task_update(&task, &channel.id);
        Ok(task)
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

    #[test]
    fn normalize_sqlite_timestamp_handles_sqlite_native() {
        // `datetime('now')` returns this exact shape — the default that
        // every task row's created_at/updated_at uses today.
        assert_eq!(
            normalize_sqlite_timestamp("2026-04-25 10:30:45").unwrap(),
            "2026-04-25T10:30:45Z"
        );
    }

    #[test]
    fn normalize_sqlite_timestamp_handles_rfc3339() {
        // Callers may pass already-canonical RFC3339 (passthrough) or
        // offset-bearing forms (normalized to `Z`).
        assert_eq!(
            normalize_sqlite_timestamp("2026-04-25T10:30:45Z").unwrap(),
            "2026-04-25T10:30:45Z"
        );
        assert_eq!(
            normalize_sqlite_timestamp("2026-04-25T10:30:45+00:00").unwrap(),
            "2026-04-25T10:30:45Z"
        );
    }

    #[test]
    fn normalize_sqlite_timestamp_rejects_garbage() {
        // Unknown-format inputs surface as errors rather than silently
        // defaulting — snapshot timestamps are cross-surface identifiers,
        // a wrong value would quietly corrupt history.
        assert!(normalize_sqlite_timestamp("not a date").is_err());
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
                "UPDATE tasks SET owner = 'carol' WHERE task_number = 1",
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
        // Claim no longer auto-advances to InProgress; the state machine is
        // forward-only and step-by-step (Task 5 decoupled claim from start).
        store
            .update_task_status("eng", 1, "bob", TaskStatus::InProgress)
            .unwrap();
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
        // Claim no longer auto-advances; walk task 1 forward step-by-step.
        store
            .update_task_status("eng", 1, "bob", TaskStatus::InProgress)
            .unwrap();

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
        // Claim no longer auto-advances to InProgress; the state machine is
        // forward-only and step-by-step (Task 5 decoupled claim from start).
        store
            .update_task_status("eng", 1, "bob", TaskStatus::InProgress)
            .unwrap();
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

    // ── Task 4: state machine tests (claim-independent) ──
    // Tests that require going through update_tasks_claim (e.g. todo → in_progress
    // → in_review → done end-to-end) live in Task 13 because update_tasks_claim
    // is still broken against the renamed `owner` column until Task 5.

    fn seed_source_message(store: &Store, channel: &str, sender: &str, body: &str) -> String {
        use crate::store::messages::{posting::CreateMessage, types::SenderType};
        store
            .create_message(CreateMessage {
                channel_name: channel,
                sender_name: sender,
                sender_type: SenderType::Human,
                content: body,
                attachment_ids: &[],
                suppress_event: true,
                run_id: None,
            })
            .unwrap()
    }

    fn proposed_task_args(
        store: &Store,
        parent_channel: &str,
        title: &str,
        sender: &str,
        content: &str,
    ) -> CreateProposedTaskArgs {
        let src_id = seed_source_message(store, parent_channel, sender, content);
        CreateProposedTaskArgs {
            title: title.to_string(),
            created_by: "bob".to_string(),
            source_message_id: src_id,
            snapshot_sender_name: sender.to_string(),
            snapshot_sender_type: "human".to_string(),
            snapshot_content: content.to_string(),
            snapshot_created_at: "2026-04-25T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn proposed_to_todo_mints_sub_channel_posts_kickoff_no_task_event_in_parent() {
        let store = Store::open(":memory:").unwrap();
        store
            .create_channel("eng", None, ChannelType::Channel, None)
            .unwrap();
        store.create_human("alice").unwrap();
        seed_agent(&store, "bob");

        let proposed = store
            .create_proposed_task(
                "eng",
                proposed_task_args(
                    &store,
                    "eng",
                    "investigate login 500",
                    "alice",
                    "login breaks on Safari",
                ),
            )
            .unwrap();
        assert_eq!(proposed.status, TaskStatus::Proposed);
        assert!(proposed.sub_channel_id.is_none());

        let accepted = store
            .update_task_status("eng", proposed.task_number, "alice", TaskStatus::Todo)
            .unwrap();
        assert_eq!(accepted.status, TaskStatus::Todo);
        let sub_id = accepted
            .sub_channel_id
            .as_deref()
            .expect("sub-channel minted on proposed -> todo")
            .to_string();
        let (parent_id, _) =
            read_task_channel_ids(&store, "eng", proposed.task_number);

        // Now batch the verification SQL inside one conn-lock acquire to
        // avoid mid-test re-entry into Store methods (which would deadlock
        // on the Mutex<Connection>).
        let (kickoff, task_event_count_in_parent): (String, i64) = {
            let conn = store.conn_for_test();
            let kickoff: String = conn
                .query_row(
                    "SELECT content FROM messages WHERE channel_id = ?1 AND sender_type = 'system' ORDER BY seq ASC LIMIT 1",
                    params![sub_id],
                    |row| row.get(0),
                )
                .unwrap();
            let task_event_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM messages WHERE channel_id = ?1 AND sender_type = 'system' AND content LIKE '%\"action\":\"%'",
                    params![parent_id],
                    |row| row.get(0),
                )
                .unwrap();
            (kickoff, task_event_count)
        };

        assert!(
            kickoff.contains("Task opened: investigate login 500")
                && kickoff.contains("From @alice's message in #eng:")
                && kickoff.contains("> login breaks on Safari"),
            "kickoff missing title/attribution/blockquote: {kickoff}"
        );
        assert_eq!(
            task_event_count_in_parent, 0,
            "acceptance must not post task_event in the parent channel"
        );
    }

    #[test]
    fn proposed_to_dismissed_pure_state_mutation_no_sub_channel_no_events() {
        let store = Store::open(":memory:").unwrap();
        store
            .create_channel("eng", None, ChannelType::Channel, None)
            .unwrap();
        store.create_human("alice").unwrap();
        seed_agent(&store, "bob");

        let proposed = store
            .create_proposed_task("eng", proposed_task_args(&store, "eng", "redo nav", "alice", "fix the nav"))
            .unwrap();
        let dismissed = store
            .update_task_status("eng", proposed.task_number, "alice", TaskStatus::Dismissed)
            .unwrap();
        assert_eq!(dismissed.status, TaskStatus::Dismissed);
        assert!(
            dismissed.sub_channel_id.is_none(),
            "dismissed proposal must never mint a sub-channel"
        );

        // No task_event anywhere — the card mutation (SSE task_update) is the signal.
        let conn = store.conn_for_test();
        let task_event_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE sender_type = 'system' AND content LIKE '%\"action\":\"%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            task_event_count, 0,
            "dismissal must not emit any task_event"
        );
    }

    #[test]
    fn proposed_to_in_progress_rejected_with_invalid_transition_error() {
        let store = Store::open(":memory:").unwrap();
        store
            .create_channel("eng", None, ChannelType::Channel, None)
            .unwrap();
        store.create_human("alice").unwrap();
        seed_agent(&store, "bob");

        let proposed = store
            .create_proposed_task("eng", proposed_task_args(&store, "eng", "skip", "alice", "no"))
            .unwrap();
        let err = store
            .update_task_status("eng", proposed.task_number, "alice", TaskStatus::InProgress)
            .unwrap_err();
        let downcast = err.downcast_ref::<InvalidTaskTransition>();
        assert!(
            downcast.is_some(),
            "expected typed InvalidTaskTransition error, got: {err}"
        );
        let ite = downcast.unwrap();
        assert_eq!(ite.from, TaskStatus::Proposed);
        assert_eq!(ite.to, TaskStatus::InProgress);
    }

    #[test]
    fn reverse_transitions_rejected() {
        // Walk a task to Todo via create_tasks (direct, skips the Proposed state).
        // Then try reverse/invalid transitions and confirm they all return typed errors.
        let store = Store::open(":memory:").unwrap();
        store
            .create_channel("eng", None, ChannelType::Channel, None)
            .unwrap();
        store.create_human("alice").unwrap();
        seed_agent(&store, "bob");
        store.create_tasks("eng", "alice", &["Ship it"]).unwrap();

        // Todo -> Proposed (reverse) rejected.
        let err = store
            .update_task_status("eng", 1, "alice", TaskStatus::Proposed)
            .unwrap_err();
        assert!(err.downcast_ref::<InvalidTaskTransition>().is_some());

        // Todo -> Done (skip in_progress + in_review) rejected.
        let err = store
            .update_task_status("eng", 1, "alice", TaskStatus::Done)
            .unwrap_err();
        assert!(err.downcast_ref::<InvalidTaskTransition>().is_some());

        // Todo -> InReview (skip in_progress) rejected.
        let err = store
            .update_task_status("eng", 1, "alice", TaskStatus::InReview)
            .unwrap_err();
        assert!(err.downcast_ref::<InvalidTaskTransition>().is_some());
    }
}
