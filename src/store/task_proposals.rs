use anyhow::{anyhow, Result};
use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{params, types::Type, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::store::Store;

/// Normalize a timestamp we read out of SQLite into RFC3339 for wire use.
///
/// The `messages.created_at` column uses the SQLite default `datetime('now')`
/// format (`YYYY-MM-DD HH:MM:SS`, UTC implicit, no offset, no `T` separator).
/// Other timestamps in the proposal wire shape (`proposedAt`, `resolvedAt`)
/// are RFC3339 because we write them from chrono. Returning SQLite format to
/// the frontend alongside RFC3339 siblings is a client-parsing footgun
/// (`Date.parse(...)` results differ by browser, and the UI treats all
/// `*At` fields uniformly). Normalize at the store boundary so every wire
/// consumer sees RFC3339 regardless of how the original column was written.
///
/// Falls back to the input string unchanged if it parses as neither format —
/// the CHECK constraint ensures `snapshot_created_at` is never NULL for v2
/// rows, so the parse-failure path is only reachable if a future writer
/// stores a genuinely malformed value, which a test in this module catches.
fn normalize_sqlite_timestamp(ts: &str) -> String {
    if let Ok(dt) = DateTime::parse_from_rfc3339(ts) {
        return dt.with_timezone(&Utc).to_rfc3339();
    }
    if let Ok(ndt) = chrono::NaiveDateTime::parse_from_str(ts, "%Y-%m-%d %H:%M:%S") {
        return Utc.from_utc_datetime(&ndt).to_rfc3339();
    }
    ts.to_string()
}

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

    // v2 additions.
    //
    // Snapshot fields exist for context consistency: the per-task ACP session
    // has a fresh context window and cannot see the parent channel, so the
    // kickoff message must carry the originating user request verbatim.
    // Storing a pointer alone would mean edits/deletes to the source message
    // silently rewrite or erase the task's context after approval. The copy
    // freezes what the user asked for at propose-time and never mutates.
    //
    // The five snapshot_* fields are all-or-nothing (DB CHECK enforces): for
    // any given row they are all Some(..) (v2+) or all None (legacy v1).
    // `source_message_id` is an independent navigation pointer — stays for
    // "jump to source" UX, may be None separately from the snapshot (legacy
    // v1 rows, or after ON DELETE SET NULL fires).
    pub source_message_id: Option<String>,
    pub snapshot_sender_name: Option<String>,
    pub snapshot_sender_type: Option<String>,
    pub snapshot_content: Option<String>,
    pub snapshot_created_at: Option<String>,
    pub snapshotted_at: Option<String>,
}

impl TaskProposal {
    /// Parse the standard 16-column `task_proposals` row. v1 columns (0..10):
    /// id, channel_id, proposed_by, title, status, created_at,
    /// accepted_task_number, accepted_sub_channel_id, resolved_by,
    /// resolved_at. v2 columns (10..16): source_message_id,
    /// snapshot_sender_name, snapshot_sender_type, snapshot_content,
    /// snapshot_created_at, snapshotted_at.
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
            source_message_id: row.get(10)?,
            snapshot_sender_name: row.get(11)?,
            snapshot_sender_type: row.get(12)?,
            snapshot_content: row.get(13)?,
            snapshot_created_at: row.get(14)?,
            snapshotted_at: row.get(15)?,
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

/// Inputs for [`Store::create_task_proposal`]. `source_message_id` is
/// required: v2 proposals always originate from a concrete chat message in the
/// same channel, and the store snapshots its content + sender + timestamp into
/// the proposal row so the per-task ACP session gets immutable context.
pub struct CreateTaskProposalInput<'a> {
    pub channel_id: &'a str,
    pub proposed_by: &'a str,
    pub title: &'a str,
    pub source_message_id: &'a str,
}

/// Shorten `content` to at most 240 Unicode scalar values, appending `'…'`
/// when truncated. Single source of truth for the `snapshotExcerpt` shown on
/// both the pending-card chat-message payload (store-side emission) and the
/// HTTP `ProposalView` (handler-side projection). The full verbatim body lives
/// on the DB row (`snapshot_content`) — the excerpt is a display-only
/// derivation.
///
/// `pub(crate)` so the handler layer can reuse this without duplicating the
/// 240 code-point rule; still crate-local (never reached by downstream crates).
pub(crate) const SNAPSHOT_EXCERPT_LIMIT: usize = 240;

pub(crate) fn truncate_excerpt(content: &str) -> String {
    let mut chars = content.chars();
    let head: String = chars.by_ref().take(SNAPSHOT_EXCERPT_LIMIT).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

impl Store {
    /// Create a new pending task proposal and post a companion
    /// `kind: "task_proposal"` chat message into the parent channel so the
    /// UI can render a card. Row + message are inserted in one transaction.
    ///
    /// v2: the proposal captures an immutable snapshot of the source message
    /// (content, sender, original timestamp) so the per-task ACP session —
    /// which has a fresh context window and cannot see the parent channel —
    /// gets the originating user request verbatim via the kickoff message.
    /// Edits or deletes to the source message after propose-time do not
    /// mutate the agreed context. The DB CHECK constraint enforces that all
    /// five snapshot fields are populated together.
    pub fn create_task_proposal(&self, input: CreateTaskProposalInput<'_>) -> Result<TaskProposal> {
        use rusqlite::TransactionBehavior;

        let trimmed = input.title.trim();
        if trimmed.is_empty() {
            return Err(anyhow!("task proposal title must not be empty"));
        }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let channel = Self::get_channel_by_id_inner(&tx, input.channel_id)?
            .ok_or_else(|| anyhow!("channel not found: {}", input.channel_id))?;

        // Fetch the source message within the same channel. `messages` uses
        // sender_name + sender_type as identity (no separate sender_id
        // column), so we snapshot both — deleted source messages never
        // orphan authorship type. Scoping to `channel_id = ?2` rejects
        // cross-channel references up-front with a clear error.
        let Some((src_content, src_sender_name, src_sender_type, src_created_at)) = tx
            .query_row(
                "SELECT content, sender_name, sender_type, created_at \
                 FROM messages WHERE id = ?1 AND channel_id = ?2",
                params![input.source_message_id, channel.id],
                |r| {
                    Ok::<_, rusqlite::Error>((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()?
        else {
            return Err(anyhow!(
                "source message not found in channel: {}",
                input.source_message_id
            ));
        };

        let id = Uuid::new_v4().to_string();
        let now = chrono::Utc::now();
        let now_iso = now.to_rfc3339();
        // Normalize the source message's created_at to RFC3339 BEFORE
        // persisting. `messages.created_at` uses SQLite's default datetime
        // format (`YYYY-MM-DD HH:MM:SS`), which would drift the wire shape
        // from sibling `proposedAt`/`resolvedAt` fields (RFC3339). Normalize
        // at the store boundary so every downstream reader — ProposalView,
        // pending payload, accept/dismiss payloads — gets a single format.
        let src_created_at_normalized = normalize_sqlite_timestamp(&src_created_at);
        // All five snapshot fields and the pointer are written together —
        // the DB CHECK constraint rejects partial snapshots, so partial
        // writes are impossible even under a misbehaving caller.
        tx.execute(
            "INSERT INTO task_proposals (
                id, channel_id, proposed_by, title, status, created_at,
                source_message_id, snapshot_sender_name, snapshot_sender_type,
                snapshot_content, snapshot_created_at, snapshotted_at
             ) VALUES (?1, ?2, ?3, ?4, 'pending', ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                id,
                channel.id,
                input.proposed_by,
                trimmed,
                now_iso,
                input.source_message_id,
                src_sender_name,
                src_sender_type,
                src_content,
                src_created_at_normalized,
                now_iso,
            ],
        )?;

        // Pending-card payload: carry the snapshot on the wire so the
        // frontend reducer can render an excerpt block without a second
        // round-trip. `snapshotted_at` and `snapshot_sender_type` are
        // DB-only — not in the payload — per the v2 wire contract.
        let excerpt = truncate_excerpt(&src_content);
        let payload = serde_json::json!({
            "kind": "task_proposal",
            "proposalId": id,
            "status": "pending",
            "title": trimmed,
            "proposedBy": input.proposed_by,
            "proposedAt": now_iso,
            "taskNumber": serde_json::Value::Null,
            "subChannelId": serde_json::Value::Null,
            "subChannelName": serde_json::Value::Null,
            "sourceMessageId": input.source_message_id,
            "snapshotSenderName": src_sender_name,
            "snapshotExcerpt": excerpt,
            "snapshotCreatedAt": src_created_at_normalized,
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
            proposed_by: input.proposed_by.to_string(),
            title: trimmed.to_string(),
            status: TaskProposalStatus::Pending,
            created_at: now,
            accepted_task_number: None,
            accepted_sub_channel_id: None,
            resolved_by: None,
            resolved_at: None,
            source_message_id: Some(input.source_message_id.to_string()),
            snapshot_sender_name: Some(src_sender_name),
            snapshot_sender_type: Some(src_sender_type),
            snapshot_content: Some(src_content),
            snapshot_created_at: Some(src_created_at_normalized),
            snapshotted_at: Some(now_iso),
        })
    }

    /// Look up a proposal by id. Returns `Ok(None)` if not found.
    pub fn get_task_proposal_by_id(&self, id: &str) -> Result<Option<TaskProposal>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, channel_id, proposed_by, title, status, created_at, \
                    accepted_task_number, accepted_sub_channel_id, \
                    resolved_by, resolved_at, \
                    source_message_id, snapshot_sender_name, \
                    snapshot_sender_type, snapshot_content, \
                    snapshot_created_at, snapshotted_at \
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

        // After the status flip, append a `status=dismissed` snapshot so
        // the chat log reflects the terminal state and the UI fold picks
        // it up. Load fields from the updated row first — including the
        // four v2 wire-contract snapshot fields so the dismissed payload
        // matches the pending/accepted shape (see `accept_task_proposal`
        // for the same rationale).
        // Types inferred from turbofish in the closure (matches the
        // `accept_task_proposal` pattern above); an explicit 8-tuple type
        // annotation here would trip clippy::type_complexity.
        let (
            channel_id,
            proposed_by,
            title,
            created_at,
            source_message_id,
            snapshot_sender_name,
            snapshot_content,
            snapshot_created_at,
        ) = tx.query_row(
            "SELECT channel_id, proposed_by, title, created_at, \
                    source_message_id, snapshot_sender_name, \
                    snapshot_content, snapshot_created_at \
             FROM task_proposals WHERE id = ?1",
            params![id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, Option<String>>(6)?,
                    r.get::<_, Option<String>>(7)?,
                ))
            },
        )?;
        let channel = Self::get_channel_by_id_inner(&tx, &channel_id)?
            .ok_or_else(|| anyhow!("channel vanished: {channel_id}"))?;
        let snapshot_excerpt = snapshot_content.as_deref().map(truncate_excerpt);
        let snapshot = serde_json::json!({
            "kind": "task_proposal",
            "proposalId": id,
            "status": "dismissed",
            "title": title,
            "proposedBy": proposed_by,
            "proposedAt": created_at,
            "taskNumber": serde_json::Value::Null,
            "subChannelId": serde_json::Value::Null,
            "subChannelName": serde_json::Value::Null,
            "resolvedBy": resolver,
            "resolvedAt": now,
            "sourceMessageId": source_message_id,
            "snapshotSenderName": snapshot_sender_name,
            "snapshotExcerpt": snapshot_excerpt,
            "snapshotCreatedAt": snapshot_created_at,
        });
        let snapshot_content = snapshot.to_string();
        let snapshot_inserted = Self::create_system_message_tx(&tx, &channel, &snapshot_content)?;
        let pending = vec![(snapshot_inserted, snapshot_content)];

        tx.commit()?;
        drop(conn);
        self.emit_system_stream_events(&channel, pending)?;
        Ok(())
    }

    /// Flip a pending proposal to accepted, creating the backing task +
    /// sub-channel in the same transaction. Auto-claims the task to the
    /// proposer agent. Enrolls the accepter human in the sub-channel so
    /// they can follow along. Does NOT emit a `task_event` create/claim
    /// pair — the proposal card already carries that signal (scope 1b).
    pub fn accept_task_proposal(&self, id: &str, accepter: &str) -> Result<AcceptedTaskProposal> {
        use rusqlite::TransactionBehavior;

        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

        // Load the proposal under the write lock so a concurrent accept
        // can't race us. Also carry `created_at` so the accepted-snapshot
        // below emits the true proposal time for `proposedAt`, not the
        // resolve time. Snapshot sender + content are pulled too so the
        // kickoff body can embed the originating user request verbatim as
        // a blockquote (v2 context-carrying kickoff).
        // Also load the two extra wire-contract snapshot fields
        // (`source_message_id`, `snapshot_created_at`) so the terminal-state
        // snapshot payload carries the same four v2 fields the pending
        // payload does. Without them, users joining mid-flight would see the
        // accepted/dismissed card with null snapshot metadata — the
        // frontend reducer's `?? prev` fallback masks the gap only for
        // clients that loaded the pending snapshot first.
        let row = tx
            .query_row(
                "SELECT channel_id, proposed_by, title, status, created_at, \
                        snapshot_sender_name, snapshot_content, \
                        source_message_id, snapshot_created_at \
                 FROM task_proposals WHERE id = ?1",
                params![id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, Option<String>>(5)?,
                        r.get::<_, Option<String>>(6)?,
                        r.get::<_, Option<String>>(7)?,
                        r.get::<_, Option<String>>(8)?,
                    ))
                },
            )
            .optional()?;
        let Some((
            channel_id,
            proposed_by,
            title,
            status_str,
            proposed_at,
            snapshot_sender_name,
            snapshot_content,
            source_message_id,
            snapshot_created_at,
        )) = row
        else {
            return Err(anyhow!("task proposal not found: {id}"));
        };
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

        let (task_id, sub_channel_id, sub_channel_name) = Self::insert_task_and_subchannel_tx(
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
        // Two named sections in one message: the task handoff (title) and
        // the immutable source evidence (provenance + verbatim blockquote).
        // Splitting into separate kickoff + pinned-source messages is
        // deferred until multi-message snapshots exist; for v2 a single
        // concatenated body keeps ordering, delivery, and per-task agent
        // reads deterministic.
        //
        // Legacy fallback: pre-v2 rows that were pending at deploy time and
        // accepted afterward have NULL snapshot fields. They get the v1
        // title-only body. New rows always have snapshot data by
        // construction (store insert populates all five fields atomically;
        // the DB CHECK rejects partial writes).
        let kickoff = match (&snapshot_content, &snapshot_sender_name) {
            (Some(content), Some(sender)) => {
                let quoted: String = content
                    .lines()
                    .map(|line| format!("> {line}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                format!(
                    "Task opened: {title}\n\nFrom @{sender}'s message in #{parent}:\n{quoted}",
                    title = title,
                    sender = sender,
                    parent = channel.name,
                    quoted = quoted,
                )
            }
            _ => format!("Task opened: {title}"),
        };
        let kickoff_inserted = Self::create_system_message_tx(&tx, &sub_channel, &kickoff)?;
        let kickoff_message_id = kickoff_inserted.id.clone();

        // Collect pending events so we can emit the WS events after commit.
        let pending_sub = vec![(kickoff_inserted, kickoff)];

        // Emit a snapshot message reflecting the new accepted state so the
        // chat log is a faithful record of the lifecycle and the UI
        // reducer (Task 14) can fold the two snapshots by proposalId.
        // `proposed_at` is the ORIGINAL creation time loaded from the row
        // above; `now` is the resolve time.
        //
        // Carry the four v2 snapshot wire fields alongside the terminal
        // state so the payload stays a complete proposal snapshot — clients
        // that first see the accepted card (joined mid-flight, re-hydrated
        // from history) don't have to cross-reference the pending payload
        // to render the excerpt/source/sender. Legacy v1 rows have all
        // snapshot columns NULL and therefore emit `null` for each wire
        // field, which is what the UI already expects for those rows.
        //
        // Known limitation (deferred, not in v2 scope): an attachment-only
        // source message has `snapshot_content = Some("")`. The kickoff
        // body's `content.lines()` splitter then emits no `> ` lines, so
        // the blockquote section renders visually empty. Low severity —
        // attachment-only proposals are rare, and the other two sections
        // (title + provenance line) still carry the task identity. A
        // future pass could emit `> (attachment-only message)` or pre-
        // reject empty content at propose time.
        let snapshot_excerpt = snapshot_content.as_deref().map(truncate_excerpt);
        let snapshot = serde_json::json!({
            "kind": "task_proposal",
            "proposalId": id,
            "status": "accepted",
            "title": title,
            "proposedBy": proposed_by,
            "proposedAt": proposed_at,
            "taskNumber": task_number,
            "subChannelId": sub_channel_id,
            "subChannelName": sub_channel_name,
            "resolvedBy": accepter,
            "resolvedAt": now,
            "sourceMessageId": source_message_id,
            "snapshotSenderName": snapshot_sender_name,
            "snapshotExcerpt": snapshot_excerpt,
            "snapshotCreatedAt": snapshot_created_at,
        });
        let snapshot_body = snapshot.to_string();
        let parent_snapshot = Self::create_system_message_tx(&tx, &channel, &snapshot_body)?;
        let pending_parent = vec![(parent_snapshot, snapshot_body)];

        tx.commit()?;
        drop(conn);

        self.emit_system_stream_events(&sub_channel, pending_sub)?;
        self.emit_system_stream_events(&channel, pending_parent)?;

        Ok(AcceptedTaskProposal {
            task_number,
            sub_channel_id,
            sub_channel_name,
            kickoff_message_id,
        })
    }
}

#[cfg(test)]
mod truncation_tests {
    use super::*;

    /// Under the limit: the helper must not append `'…'` — the card shows the
    /// verbatim content. Guards against an off-by-one that would
    /// unconditionally append the ellipsis.
    #[test]
    fn under_limit_unchanged() {
        let input = "hello world";
        let out = truncate_excerpt(input);
        assert_eq!(out, "hello world");
        assert!(!out.ends_with('…'));
    }

    /// Exactly at the limit (240 code points): still unchanged, still no
    /// ellipsis. This pins the boundary — the helper appends only when input
    /// has a 241st code point.
    #[test]
    fn exactly_at_limit_unchanged() {
        let input: String = "a".repeat(SNAPSHOT_EXCERPT_LIMIT);
        assert_eq!(input.chars().count(), SNAPSHOT_EXCERPT_LIMIT);
        let out = truncate_excerpt(&input);
        assert_eq!(out, input);
        assert!(!out.ends_with('…'));
    }

    /// Over the limit by one multi-byte char: the helper must slice on a
    /// Unicode scalar boundary (NOT a byte boundary) and append `'…'`. This
    /// is the case that would panic or corrupt UTF-8 if the implementation
    /// used `&s[..240]` on multi-byte input.
    #[test]
    fn over_limit_multibyte_trailing_char() {
        let mut input: String = "a".repeat(SNAPSHOT_EXCERPT_LIMIT);
        input.push('🦀');
        assert_eq!(input.chars().count(), SNAPSHOT_EXCERPT_LIMIT + 1);

        let out = truncate_excerpt(&input);

        // Valid UTF-8 by construction (String), but also assert the crab is
        // NOT present — the 241st code point must have been dropped.
        assert!(!out.contains('🦀'));
        assert!(out.ends_with('…'));
        // Head = first 240 scalar values; plus the appended ellipsis = 241.
        assert_eq!(out.chars().count(), SNAPSHOT_EXCERPT_LIMIT + 1);
        let head: String = "a".repeat(SNAPSHOT_EXCERPT_LIMIT);
        assert_eq!(out, format!("{head}…"));
    }

    /// All-multibyte input over the limit: ensures the helper counts code
    /// points, not bytes. 241 Japanese characters ≈ 723 bytes; a byte-indexed
    /// implementation would slice in the middle of a scalar and either panic
    /// or produce invalid UTF-8.
    #[test]
    fn over_limit_all_multibyte() {
        let input: String = "あ".repeat(SNAPSHOT_EXCERPT_LIMIT + 1);
        assert_eq!(input.chars().count(), SNAPSHOT_EXCERPT_LIMIT + 1);

        let out = truncate_excerpt(&input);

        assert!(out.ends_with('…'));
        assert_eq!(out.chars().count(), SNAPSHOT_EXCERPT_LIMIT + 1);
        let head: String = "あ".repeat(SNAPSHOT_EXCERPT_LIMIT);
        assert_eq!(out, format!("{head}…"));
    }
}
