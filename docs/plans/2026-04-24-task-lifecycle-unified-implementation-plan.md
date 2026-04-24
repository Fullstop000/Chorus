# Task Lifecycle Unified — Implementation Plan (R2)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Collapse `task_proposals` + `tasks` into one unified `tasks` table with a six-state enum, replace `TaskProposalMessage` + `TaskEventMessage` with one evolving `TaskCard` component, and retarget all endpoints + MCP tools onto the merged surface. See spec: `docs/plans/2026-04-24-task-lifecycle-unified-design.md`. Review log: `docs/plans/2026-04-24-task-lifecycle-unified-review-log.md`.

**Architecture:** One DB table, two wire kinds (`task_card` host message in the parent channel; `task_event` event rows in the sub-channel), one UI component per message surface, forward-only state machine in the Rust store. Ownership is a label; membership is the only gate.

**Tech stack:** Rust (axum + rusqlite), TypeScript (React 19 + Zustand), SQLite. Existing `anyhow!`-based error shape. Realtime fan-out uses the existing `tokio::sync::broadcast` pattern in `src/server/transport/realtime.rs` plus a new dedicated task-updates sender on `Store`.

**Branch strategy:** Close PR #93 and PR #96. Branch `feat/task-lifecycle` off `main`. Single PR.

**Key convention (locked after R1 review):**
- Tasks are keyed by `(channel_name, task_number)` on every public HTTP route and every MCP tool. UUID `task_id` is an internal DB detail — never surfaced.
- Wire format on `task_event` messages keeps the legacy `claimedBy` field name to preserve message-history compatibility. Only in-memory types rename to `owner`; the boundary maps wire → type.

---

## File Structure

### Backend (Rust)

**Modify:**
- `src/store/schema.sql` — extend `tasks` with snapshot cols + widen status CHECK; drop `task_proposals` table.
- `src/store/migrations.rs` — one new migration `migrate_unify_task_proposals_into_tasks` following the rebuild pattern (`CREATE TABLE tasks_new; INSERT SELECT; migrate task_proposals rows; DROP; RENAME`).
- `src/store/tasks/mod.rs` — extend `TaskStatus` enum; add `create_proposed_task`; rename internal field `claimed_by` → `owner`; rewrite `update_task_status` (drop claimer-only gate, extend transition graph); rewrite `update_tasks_claim` (decouple claim from status advance); extend `load_task_tx` to return snapshot fields; add `update_task_dismiss`.
- `src/store/tasks/events.rs` — add `TaskEventAction::Dismissed`; add `task_card` wire payload struct + helper `post_task_card_message_tx`; keep wire field name `claimedBy` on `TaskEventPayload`.
- `src/store/mod.rs` — add `task_updates_tx: broadcast::Sender<TaskUpdateEvent>` + `subscribe_task_updates()`; remove `pub mod task_proposals`.
- `src/server/handlers/tasks.rs` — add status/claim/unclaim handlers keyed by `(channel, task_number)`; add agent-create + human-create paths; preserve membership preconditions.
- `src/server/handlers/mod.rs` / `src/server/mod.rs` — drop `task_proposals` handler module + routes.
- `src/server/transport/realtime.rs` — add a third `tokio::select!` branch that forwards `TaskUpdateEvent` to every connected viewer (no membership gate — task updates are global).
- `src/bridge/backend.rs` — rename `propose_task` → `create_task`, rename HTTP path `/task-proposals` → `/tasks`, rename `accept_task_proposal` → `accept_task`, `dismiss_task_proposal` → `dismiss_task`; add claim/unclaim/status methods on `Backend` trait; all keyed by `(channel, task_number)`.
- `src/bridge/mod.rs` — tool registrations + docstrings for the six merged tools.

**Delete:**
- `src/store/task_proposals.rs`
- `src/server/handlers/task_proposals.rs`

### Frontend (TypeScript)

**Create:**
- `ui/src/components/chat/TaskCard.tsx` + `.css` + `.test.tsx`
- `ui/src/components/chat/TaskEventRow.tsx` + `.test.tsx`
- `ui/src/hooks/useTask.ts` + `.test.ts`
- `ui/src/hooks/useTaskUpdateStream.ts` (WebSocket subscription for `task_update` events)
- `ui/src/store/tasksStore.ts` — dedicated Zustand slice holding `tasksById: Record<string, TaskInfo>` + `updateTask(patch)`. Separate from `uiStore.ts` so task updates don't rerender unrelated UI.

**Modify:**
- `ui/src/components/chat/MessageList.tsx` — route `task_card` kind → mount `TaskCard`; route `task_event` kind → mount `TaskEventRow`.
- `ui/src/hooks/useTaskEventLog.ts` — simplified reducer: returns `TaskEventRecord[]` (flat, seq-ordered), no card-state derivation.
- `ui/src/data/tasks.ts` — rename `claimed_by` → `owner` on `TaskInfo` + snapshot fields + widen `TaskStatus` to six values; retarget endpoint URLs to `(conversation_id, task_number)`.
- `ui/src/data/taskEvents.ts` — map wire `claimedBy` → type `owner` in `parseTaskEvent`. **Wire shape unchanged.**
- `ui/src/components/tasks/TasksPanel.tsx` — client-side filter to 4-column committed-work view.
- `ui/src/components/tasks/TaskDetail.tsx` — secondary-action surface (reassign, cancel — v2 stubs OK).

**Delete:**
- `ui/src/components/chat/TaskProposalMessage.tsx` + `.css` + `.test.tsx`
- `ui/src/components/chat/TaskEventMessage.tsx` + `.css` + `.test.tsx`
- `ui/src/hooks/useTaskProposalLog.ts` + `.test.ts`

### Tests

**Modify:**
- `tests/store_tests.rs`, `tests/e2e_tests.rs`
- `qa/cases/playwright/TSK-005.spec.ts`

**Create:**
- `qa/cases/playwright/TSK-006.spec.ts`

---

## Task 1: Schema + `TaskStatus` enum extension

**No migration written.** Neither PR #93 nor PR #96 is in prod. Dev-local
SQLite files will be wiped (`rm ~/.chorus/*.db` or equivalent) before the
first run of the new branch. `cargo test` already uses fresh in-memory DBs.
The two proposal-era migrations (`migrate_create_task_proposals_table`,
`migrate_add_task_proposal_snapshot_columns`) and their tests are deleted
outright — no ceremony needed.

**Files:**
- Modify: `src/store/schema.sql:100-112` (tasks table); delete `src/store/schema.sql:232-258` (task_proposals)
- Modify: `src/store/migrations.rs` — delete the two proposal-era migrations + their unit tests
- Modify: `src/store/tasks/mod.rs:44-90` (`TaskStatus` enum + `can_transition_to`)

- [ ] **Step 1: Extend `tasks` table in `schema.sql`.**

Replace the current `tasks` block with the merged shape. Five snapshot columns; `snapshotted_at` is **intentionally dropped** — `tasks.created_at` already captures "when the server minted this row," which equals "when the snapshot was captured" for proposed tasks. One fewer column to maintain.

```sql
CREATE TABLE IF NOT EXISTS tasks (
    id TEXT PRIMARY KEY,
    channel_id TEXT NOT NULL,
    task_number INTEGER NOT NULL,
    title TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'todo'
        CHECK (status IN ('proposed','dismissed','todo','in_progress','in_review','done')),
    owner TEXT,
    created_by TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    sub_channel_id TEXT REFERENCES channels(id),
    source_message_id TEXT REFERENCES messages(id) ON DELETE SET NULL,
    snapshot_sender_name TEXT,
    snapshot_sender_type TEXT,
    snapshot_content TEXT,
    snapshot_created_at TEXT,
    UNIQUE(channel_id, task_number),
    CHECK (
      (source_message_id IS NULL
         AND snapshot_sender_name IS NULL
         AND snapshot_sender_type IS NULL
         AND snapshot_content IS NULL
         AND snapshot_created_at IS NULL)
      OR
      (snapshot_sender_name IS NOT NULL
         AND snapshot_sender_type IS NOT NULL
         AND snapshot_content IS NOT NULL
         AND snapshot_created_at IS NOT NULL)
    )
);
CREATE INDEX IF NOT EXISTS idx_tasks_channel_status ON tasks(channel_id, status);
```

- [ ] **Step 2: Remove `task_proposals` + its index from `schema.sql`.**

Delete lines ~232–258 (the `CREATE TABLE task_proposals` block and `CREATE INDEX idx_task_proposals_channel_status`). Fresh DBs go straight to the unified `tasks` table.

- [ ] **Step 3: Delete the two proposal-era migrations.**

In `src/store/migrations.rs`:
- Remove `migrate_create_task_proposals_table(conn)?;` from `run_migrations`.
- Remove `migrate_add_task_proposal_snapshot_columns(conn)?;` from `run_migrations`.
- Delete the two `fn migrate_*` bodies themselves.
- Delete the associated `#[test]` functions (around line 740+).

Fresh-DB path now goes straight to the merged `tasks` schema via `schema.sql`. No migration code path for the task-lifecycle feature.

- [ ] **Step 3.5: Move `normalize_sqlite_timestamp` out of `src/store/task_proposals.rs`.**

Later tasks (Task 3's `create_proposed_task`, Task 6's agent-create handler) call this helper. It currently lives in `src/store/task_proposals.rs`, which gets deleted in Task 11. Relocate it now — before any caller needs it — to avoid a mid-implementation build break.

Destination: `src/store/tasks/mod.rs` (colocated with callers) or `src/utils/time.rs` if a shared util module exists. Update any existing call site (there's exactly one, inside `task_proposals.rs`) to the new path.

- [ ] **Step 4: Extend `TaskStatus` enum in `src/store/tasks/mod.rs`.**

Add two variants + a transition validator:

```rust
pub enum TaskStatus {
    Proposed,
    Dismissed,
    Todo,
    InProgress,
    InReview,
    Done,
}

impl TaskStatus {
    pub fn as_str(&self) -> &'static str { /* as today, + proposed/dismissed */ }
    pub fn from_status_str(s: &str) -> Option<Self> { /* as today, + proposed/dismissed */ }

    /// Forward-only transitions. No reverse transitions in v1.
    pub fn can_transition_to(&self, to: Self) -> bool {
        use TaskStatus::*;
        matches!(
            (self, to),
            (Proposed,   Todo)
          | (Proposed,   Dismissed)
          | (Todo,       InProgress)
          | (InProgress, InReview)
          | (InReview,   Done)
        )
    }
}
```

- [ ] **Step 5: Unit tests for the transition graph.**

```rust
#[test]
fn task_status_transitions() {
    use TaskStatus::*;
    assert!(Proposed.can_transition_to(Todo));
    assert!(Proposed.can_transition_to(Dismissed));
    assert!(!Proposed.can_transition_to(InProgress));
    assert!(!Dismissed.can_transition_to(Todo));       // terminal
    assert!(!Done.can_transition_to(InProgress));       // terminal
    assert!(Todo.can_transition_to(InProgress));
    assert!(!InProgress.can_transition_to(Todo));       // no reverse in v1
}
```

- [ ] **Step 6: Run `cargo test -p chorus --lib`. Confirm pass.**

- [ ] **Step 7: Commit.**

```bash
git commit -am "feat(schema): unify task_proposals into tasks with six-state enum"
```

---

## Task 2: Wire messages — `task_card` payload + `post_task_event_tx` helper

(Moved ahead of create paths per R1 finding #2 — create_proposed_task needs the wire helper to exist.)

**Files:**
- Modify: `src/store/tasks/events.rs`

- [ ] **Step 1: `TaskEventAction` stays unchanged.**

Per the spec, `proposed → dismissed` does **not** emit a `task_event` — the parent-channel `task_card` host message re-renders to the `dismissed` state via SSE `task_update`, and that's the only signal. No enum changes needed.

Keep the existing variants as-is:

```rust
pub enum TaskEventAction {
    Created,        // today; becomes unused under the new model (parent-channel task_card replaces it).
    Claimed,
    Unclaimed,
    StatusChanged,
}
```

The `Created` variant stays for now (used by the existing `create_tasks` code path) but is removed in Task 3 Step 2 when `create_tasks` switches to posting `task_card` instead.

- [ ] **Step 2: Define `TaskCardWirePayload`.**

```rust
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TaskCardWirePayload {
    pub kind: &'static str,      // always "task_card" — serialized as a discriminator
    pub task_id: String,
    pub task_number: i64,
    pub title: String,
    pub status: String,
    pub owner: Option<String>,
    pub created_by: String,
    pub source_message_id: Option<String>,
    pub snapshot_sender_name: Option<String>,
    pub snapshot_sender_type: Option<String>,
    pub snapshot_content: Option<String>,
    pub snapshot_created_at: Option<String>,
}
```

- [ ] **Step 3: Add `post_task_card_message_tx` helper.**

`Store::create_system_message_tx` is at `src/store/messages/posting.rs:119` — returns `Result<InsertedMessage>`, not a generic `MessageRow`. The existing `create_tasks` code pairs `(InsertedMessage, String)` tuples for `emit_system_stream_events`, so match that pattern: return both so the caller can emit.

```rust
pub fn post_task_card_message_tx(
    tx: &Transaction<'_>,
    parent_channel: &Channel,
    task: &TaskInfo,
) -> Result<(InsertedMessage, String)> {
    let payload = TaskCardWirePayload {
        kind: "task_card",
        task_id: task.id.clone(),
        task_number: task.task_number,
        title: task.title.clone(),
        status: task.status.as_str().to_string(),
        owner: task.owner.clone(),
        created_by: task.created_by.clone(),
        source_message_id: task.source_message_id.clone(),
        snapshot_sender_name: task.snapshot_sender_name.clone(),
        snapshot_sender_type: task.snapshot_sender_type.clone(),
        snapshot_content: task.snapshot_content.clone(),
        snapshot_created_at: task.snapshot_created_at.clone(),
    };
    let content = serde_json::to_string(&payload)?;
    let msg = Store::create_system_message_tx(tx, parent_channel, &content)?;
    Ok((msg, content))
}
```

Callers push the returned tuple into the existing `pending_events: Vec<(InsertedMessage, String)>` buffer and fan out via `emit_system_stream_events` after `tx.commit()`.

- [ ] **Step 4: Add `post_task_event_tx` helper that loads the sub-channel.**

```rust
pub fn post_task_event_tx(
    tx: &Transaction<'_>,
    sub_channel_id: &str,
    payload: TaskEventPayload,
) -> Result<(InsertedMessage, String)> {
    // Load the sub-channel row by id. `create_system_message_tx` takes &Channel.
    // `get_channel_by_id_inner` lives in `src/store/channels.rs:437`; a `&Transaction`
    // derefs to `&Connection`, so it works from inside a tx.
    let sub_channel = Store::get_channel_by_id_inner(tx, sub_channel_id)?
        .ok_or_else(|| anyhow!("sub-channel not found: {}", sub_channel_id))?;
    let content = payload.to_json_string()?;
    let msg = Store::create_system_message_tx(tx, &sub_channel, &content)?;
    Ok((msg, content))
}
```

- [ ] **Step 5: Keep wire field name `claimedBy` on `TaskEventPayload`.**

`TaskEventPayload::claimed_by` field stays. Serde renames to `claimedBy` on the wire. **Do not rename this field to `owner` in the wire payload** — it would break persisted chat history compat.

- [ ] **Step 6: Unit tests.**

```rust
#[test]
fn task_card_payload_roundtrips_json() { ... }

#[test]
fn task_event_wire_field_is_claimed_by_camel_case() {
    // Guard against accidental rename to owner.
    let p = TaskEventPayload { claimed_by: Some("zht".into()), ... };
    let json = p.to_json_string().unwrap();
    assert!(json.contains("\"claimedBy\":\"zht\""));
}
```

- [ ] **Step 7: Commit.**

---

## Task 3: Rust store — unified create paths

**Files:**
- Modify: `src/store/tasks/mod.rs` — add `create_proposed_task`, refactor existing `create_tasks` to share insert helper
- Modify: `src/store/mod.rs` — add `subscribe_task_updates()` (wired up in Task 7)

- [ ] **Step 1: Add `create_proposed_task`.**

Agent-path create: always `status='proposed'`, snapshot required, no sub-channel yet. Posts `task_card` in the parent channel. Does NOT fire `task_event` (no sub-channel, no claim, no status change).

```rust
pub fn create_proposed_task(
    &self,
    channel_name: &str,
    args: CreateProposedTaskArgs,
) -> Result<TaskInfo> {
    let mut conn = self.conn.lock().unwrap();
    let channel = Self::get_channel_by_name_inner(&conn, channel_name)?
        .ok_or_else(|| anyhow!("channel not found: {}", channel_name))?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

    // 1. Allocate next task_number for this channel.
    let task_number: i64 = tx.query_row(
        "SELECT COALESCE(MAX(task_number), 0) + 1 FROM tasks WHERE channel_id = ?1",
        params![channel.id], |r| r.get(0),
    )?;

    // 2. Insert row: status='proposed', sub_channel_id=NULL, snapshot populated.
    let id = uuid::Uuid::new_v4().to_string();
    tx.execute(
        "INSERT INTO tasks
           (id, channel_id, task_number, title, status, owner,
            created_by, sub_channel_id,
            source_message_id, snapshot_sender_name, snapshot_sender_type,
            snapshot_content, snapshot_created_at)
         VALUES (?1, ?2, ?3, ?4, 'proposed', NULL, ?5, NULL,
                 ?6, ?7, ?8, ?9, ?10)",
        params![
            id, channel.id, task_number, args.title, args.created_by,
            args.source_message_id, args.snapshot_sender_name,
            args.snapshot_sender_type, args.snapshot_content,
            args.snapshot_created_at,
        ],
    )?;

    // 3. Load the freshly-inserted TaskInfo. Helper wraps the existing
    //    `get_task_info`-style SELECT keyed by id — add it alongside
    //    `get_task_info` if it doesn't already exist.
    let task = load_task_by_id_tx(&tx, &id)?;

    // 4. Post task_card in the parent channel. Returns (InsertedMessage, content)
    //    tuple so we can fan out the stream event after tx.commit().
    let pending = vec![events::post_task_card_message_tx(&tx, &channel, &task)?];

    tx.commit()?;
    drop(conn);

    self.emit_system_stream_events(&channel, pending)?;
    self.emit_task_update(&task);  // see Task 7
    Ok(task)
}
```

- [ ] **Step 2: Refactor existing `create_tasks` (human direct-create path).**

Keep today's sub-channel mint + kickoff. **Remove** the existing `task_event(Created)` emission in the parent channel — the new `task_card` host message replaces it. Direct-created tasks have `status='todo'`, all snapshot fields null, and an immediately-minted sub-channel.

Concretely, inside the existing `create_tasks` per-task loop — keep parent and sub-channel events in **separate** vectors because `emit_system_stream_events` tags every event with the single `&channel` argument it receives, so mixing would misroute the kickoff (sub-channel event) to parent-channel subscribers:

```rust
// (existing code allocates task_number, INSERTs the row, mints sub_channel)

// REMOVED: parent-channel `task_event(Created)` emission — task_card replaces it.

// ADDED: post task_card in the parent channel.
let task = load_task_by_id_tx(&tx, &id)?;
parent_events.push(events::post_task_card_message_tx(&tx, &channel, &task)?);

// Existing: post kickoff in the sub-channel. Push onto the sub-channel vector.
let sub_channel = /* load the newly-minted sub-channel */;
sub_events.push(events::post_kickoff_message_tx(&tx, &channel, &sub_channel.id, &task)?);
```

```rust
// After tx.commit():
self.emit_system_stream_events(&channel, parent_events)?;
self.emit_system_stream_events(&sub_channel, sub_events)?;
```

Both vectors are `Vec<(InsertedMessage, String)>` — the helpers return tuples shaped exactly for them. Two separate fan-out calls, one per channel.

- [ ] **Step 3: Extend `TaskInfo` struct with snapshot fields + rename `claimed_by` → `owner`.**

```rust
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TaskInfo {
    pub id: String,                            // UUID — surfaced to UI for store keying; NOT surfaced to MCP tools
    pub task_number: i64,
    pub title: String,
    pub status: TaskStatus,
    pub owner: Option<String>,                 // renamed from claimed_by
    pub created_by: String,
    pub created_at: String,
    pub updated_at: String,
    pub sub_channel_id: Option<String>,
    pub sub_channel_name: Option<String>,      // joined from channels.name — UI uses this for the deep-link button
    pub source_message_id: Option<String>,
    pub snapshot_sender_name: Option<String>,
    pub snapshot_sender_type: Option<String>,
    pub snapshot_content: Option<String>,
    pub snapshot_created_at: Option<String>,
}
```

**`id` serialization policy:** this field *is* serialized in public HTTP responses — the UI store keys by `task.id`. It is *not* surfaced to agents/MCP tools — those key by `(channel, task_number)`. The bridge backend strips it when formatting tool output.

- [ ] **Step 4: Update all existing `SELECT` statements.**

Every `SELECT task_number, title, status, claimed_by, created_by, …` becomes:

```sql
SELECT t.id, t.task_number, t.title, t.status, t.owner, t.created_by,
       t.created_at, t.updated_at, t.sub_channel_id, c.name AS sub_channel_name,
       t.source_message_id, t.snapshot_sender_name, t.snapshot_sender_type,
       t.snapshot_content, t.snapshot_created_at
FROM tasks t
LEFT JOIN channels c ON c.id = t.sub_channel_id
WHERE t.channel_id = ?1 AND t.task_number = ?2
```

The `LEFT JOIN channels` to resolve `sub_channel_name` matches the existing
pattern at `src/store/tasks/mod.rs:220-236`. Locations to update:
`grep -n "SELECT .* FROM tasks" src/store/tasks/mod.rs`.

- [ ] **Step 5: Store tests.**

```rust
#[test]
fn create_proposed_task_inserts_snapshot_and_posts_task_card() { ... }

#[test]
fn create_proposed_task_does_not_mint_sub_channel() { ... }

#[test]
fn create_task_direct_mints_sub_channel_and_posts_both_kickoff_and_task_card() { ... }

#[test]
fn create_task_direct_has_null_snapshot_fields() { ... }

#[test]
fn create_proposed_task_rejects_empty_snapshot() { ... }  // CHECK violation
```

- [ ] **Step 6: Run `cargo test --test store_tests`. Pass.**

- [ ] **Step 7: Commit.**

---

## Task 4: Rust store — transition state machine + sub-channel mint

**Files:**
- Modify: `src/store/tasks/mod.rs` (`update_task_status`), add `mint_sub_channel_tx`, add `post_kickoff_message_tx`
- Modify: `src/store/tasks/mod.rs:502` — **remove the `claimed_by == requester_name` gate** (R1 finding #12)

- [ ] **Step 1: Rewrite `update_task_status` as a state machine; drop the owner-gate.**

The existing implementation at line 471 enforces `claimed_by == requester_name`. Per spec, owner is a label, not a gate. Membership is the only gate — and that's enforced in the HTTP handler, not here.

```rust
pub fn update_task_status(
    &self,
    channel_name: &str,
    task_number: i64,
    actor: &str,                 // was `requester_name`; now used only for the task_event actor field
    new_status: TaskStatus,
) -> Result<TaskInfo> {
    let mut conn = self.conn.lock().unwrap();
    let channel = Self::get_channel_by_name_inner(&conn, channel_name)?
        .ok_or_else(|| anyhow!("channel not found: {}", channel_name))?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

    let mut task = load_task_by_number_tx(&tx, &channel.id, task_number)?;
    let current_status = task.status;

    if !current_status.can_transition_to(new_status) {
        // Typed error so the HTTP handler can map to 422 via downcast_ref.
        return Err(InvalidTaskTransition {
            from: current_status,
            to: new_status,
        }.into());
    }

    // REMOVED: `if claimed_by.as_deref() != Some(requester_name) { ... }`
    // Owner is a label. Any channel member can advance any transition.

    // proposed -> todo: mint sub-channel + post kickoff.
    if current_status == TaskStatus::Proposed && new_status == TaskStatus::Todo {
        let sub_id = mint_sub_channel_tx(&tx, &channel, &task, actor)?;
        post_kickoff_message_tx(&tx, &channel, &sub_id, &task)?;
        task.sub_channel_id = Some(sub_id);
    }

    // Apply status change.
    tx.execute(
        "UPDATE tasks
         SET status = ?1, sub_channel_id = COALESCE(?2, sub_channel_id), updated_at = datetime('now')
         WHERE channel_id = ?3 AND task_number = ?4",
        params![new_status.as_str(), task.sub_channel_id, channel.id, task_number],
    )?;
    task.status = new_status;

    // Post task_event — only for post-acceptance transitions. The spec
    // explicitly forbids task_events on pre-acceptance transitions
    // (proposed → todo, proposed → dismissed). For those two cases, the
    // parent-channel task_card host message re-renders via SSE task_update
    // and that is the sole signal.
    let pending_event: Option<(InsertedMessage, String)> =
        if current_status == TaskStatus::Proposed {
            None   // both proposed → todo and proposed → dismissed: no event
        } else {
            let event_payload = TaskEventPayload {
                action: TaskEventAction::StatusChanged,
                task_number, title: task.title.clone(),
                sub_channel_id: task.sub_channel_id.clone().unwrap_or_default(),
                actor: actor.to_string(),
                prev_status: Some(current_status),
                next_status: new_status,
                claimed_by: task.owner.clone(),   // keep wire field name!
            };
            let sub_id = task.sub_channel_id.as_deref()
                .ok_or_else(|| anyhow!("post-acceptance transition requires sub-channel"))?;
            Some(events::post_task_event_tx(&tx, sub_id, event_payload)?)
        };

    // Done -> archive the sub-channel (existing behavior preserved). The
    // archive helper is at `src/store/channels.rs` (inline if not present):
    //   UPDATE channels SET archived = 1 WHERE id = ?1
    if new_status == TaskStatus::Done {
        if let Some(sub_id) = task.sub_channel_id.as_deref() {
            tx.execute("UPDATE channels SET archived = 1 WHERE id = ?1", params![sub_id])?;
        }
    }

    tx.commit()?;
    drop(conn);

    // Fan out: post-acceptance task_event in the sub-channel; task_update globally.
    if let Some(ev) = pending_event {
        let sub_id = task.sub_channel_id.as_deref().unwrap();
        let sub_channel = self.get_channel_by_id(sub_id)?
            .ok_or_else(|| anyhow!("sub-channel vanished after tx commit"))?;
        self.emit_system_stream_events(&sub_channel, vec![ev])?;
    }
    self.emit_task_update(&task);  // global fan-out (Task 7)
    Ok(task)
}
```

Required helpers referenced above — if not present in `src/store/tasks/mod.rs`, add them as thin wrappers on the existing `get_task_info`-style SELECT:

```rust
fn load_task_by_number_tx(tx: &Transaction<'_>, channel_id: &str, task_number: i64) -> Result<TaskInfo> { ... }
fn load_task_by_id_tx(tx: &Transaction<'_>, id: &str) -> Result<TaskInfo> { ... }
```

These wrap a `SELECT id, task_number, title, status, owner, created_by, created_at, updated_at, sub_channel_id, source_message_id, snapshot_sender_name, snapshot_sender_type, snapshot_content, snapshot_created_at FROM tasks WHERE ...` and populate `TaskInfo`. Single source of truth for the column list.

- [ ] **Step 2: Add `mint_sub_channel_tx` helper.**

Creates a `channel_type='task'` channel named `{parent_slug}__task-{task_number}`, with `parent_channel_id = parent.id`. Adds the task's `created_by` + `actor` as members. Returns the new channel id.

Extract from today's `create_tasks` inline code (look for `INSERT INTO channels ... 'task' ...` and membership seed).

- [ ] **Step 3: Add `post_kickoff_message_tx` helper.**

**Exact format** matching PR #96's existing kickoff (found at `src/store/task_proposals.rs` ~line 620):

```
Task opened: {title}
<blank line>
From @{snapshot_sender_name}'s message in #{parent_slug}:
> {snapshot_content}
```

If snapshot fields are null (direct-created `todo`), post only:

```
Task opened: {title}
```

Single line; no `From` line, no blockquote. Keep the `system-message-divider__label` pattern the UI already renders.

- [ ] **Step 4: Store tests.**

```rust
#[test]
fn proposed_to_todo_mints_sub_channel_posts_kickoff_emits_task_event() { ... }

#[test]
fn proposed_to_dismissed_posts_parent_task_event_no_sub_channel() { ... }

#[test]
fn todo_to_in_progress_does_not_require_owner() {
    // Seed task with owner=NULL; advance by non-owner actor.
    // Assert: success, task_event.actor = caller, task.owner still NULL.
}

#[test]
fn in_review_to_done_archives_sub_channel_fires_task_update() { ... }

#[test]
fn proposed_to_in_progress_rejected_422() { ... }

#[test]
fn done_is_terminal() { ... }

#[test]
fn reverse_transition_rejected() { ... }
```

- [ ] **Step 5: Run `cargo test --test store_tests`. Pass.**

- [ ] **Step 6: Commit.**

---

## Task 5: Rust store — claim / unclaim decoupled

(Explicitly addresses R1 finding #3: existing `update_tasks_claim` fuses claim + status advance.)

**Files:**
- Modify: `src/store/tasks/mod.rs` (`update_tasks_claim`, `update_task_unclaim`)

- [ ] **Step 1: Rewrite `update_tasks_claim` — set owner only, do not advance status.**

The existing SQL at `src/store/tasks/mod.rs:338` is:
```sql
UPDATE tasks SET claimed_by = ?1, status = 'in_progress', updated_at = datetime('now')
  WHERE channel_id = ?2 AND task_number = ?3 AND status = 'todo' AND claimed_by IS NULL
```

New SQL — set `owner` only, drop the status advance and the `claimed_by IS NULL` guard (re-claim is permissive):

```sql
UPDATE tasks SET owner = ?1, updated_at = datetime('now')
  WHERE channel_id = ?2 AND task_number = ?3 AND status IN ('todo','in_progress','in_review')
```

The `status IN (...)` guard enforces the claimable-state precondition at the SQL level — a check-then-UPDATE race between "status still Todo" and "someone just moved it to InReview" resolves to the UPDATE simply matching zero rows, and we return a "cannot claim terminal task" error.

Rust-side precondition check before the UPDATE (for 422 clarity at the handler):

```rust
if !matches!(task.status, TaskStatus::Todo | TaskStatus::InProgress | TaskStatus::InReview) {
    return Err(anyhow!("cannot claim task in {:?} state", task.status));
}
```

- [ ] **Step 2: Emit `Claimed` task_event in the SUB-CHANNEL.**

(Closes R1 #4 for claim/unclaim.) The existing `update_tasks_claim` at line 374 calls `Self::create_system_message_tx(&tx, &channel, &content)` where `channel` is the **parent**. The spec says claim/unclaim happen inside the sub-channel, so the event timeline lives there too. Route via the new helper:

```rust
// Inside the claim loop, after the UPDATE succeeds:
let payload = TaskEventPayload {
    action: TaskEventAction::Claimed,
    task_number: tn,
    title,
    sub_channel_id: sub_channel_id.clone().unwrap_or_default(),
    actor: claimer_name.to_string(),
    prev_status: Some(task.status),   // same as next
    next_status: task.status,
    claimed_by: Some(claimer_name.to_string()),
};
let sub_id = sub_channel_id.as_deref()
    .ok_or_else(|| anyhow!("claim on task without sub-channel"))?;   // should be unreachable given the status precondition
let event = events::post_task_event_tx(&tx, sub_id, payload)?;
pending_events.push(event);
```

`prev_status == next_status == task.status`. The UI renders this as a `"@zht claimed"` row. Then fan out via `emit_system_stream_events(&sub_channel, pending_events)` after `tx.commit()` — **the sub-channel, not the parent**.

- [ ] **Step 3: Keep the sub-channel-membership side effect.**

Claimer joins the sub-channel (existing `INSERT OR IGNORE INTO channel_members`). Unclaimer leaves (existing `DELETE FROM channel_members`). Both behaviors preserved.

- [ ] **Step 4: `update_task_unclaim` — clear owner only.**

Existing code at `src/store/tasks/mod.rs:431` fuses `claimed_by = NULL, status = 'todo'`:

```sql
UPDATE tasks SET claimed_by = NULL, status = 'todo', updated_at = datetime('now')
  WHERE channel_id = ?1 AND task_number = ?2 AND claimed_by = ?3
```

New SQL — clear `owner`, leave `status` alone, keep the TOCTOU guard on the caller:

```sql
UPDATE tasks SET owner = NULL, updated_at = datetime('now')
  WHERE channel_id = ?1 AND task_number = ?2 AND owner = ?3
```

The `owner = ?3` guard (where `?3` is the caller's name) prevents unclaiming a claim that was already stolen by a concurrent writer — if `rows != 1`, surface "no longer claimed by you" as today.

Unclaim is allowed on the same three states (Todo, InProgress, InReview). Add the same Rust-side guard as Step 1.

- [ ] **Step 5: Unclaim `task_event` also posts to the SUB-CHANNEL.**

Same routing as claim: load the sub-channel via `events::post_task_event_tx(&tx, sub_id, payload)`, fan out via `emit_system_stream_events(&sub_channel, ...)`. `claimed_by` in the payload is `None` (wire-field means "new owner after this event"); `prev_status == next_status == task.status`.

- [ ] **Step 6: Store tests.**

```rust
#[test]
fn claim_sets_owner_does_not_advance_status() {
    // seed: task status=todo, owner=NULL
    // claim by @zht
    // assert: owner="zht", status still todo
}

#[test]
fn claim_allowed_on_in_progress_replaces_owner() { ... }

#[test]
fn claim_rejected_on_proposed() { ... }    // 422

#[test]
fn unclaim_clears_owner_does_not_advance_status() {
    // seed: task status=in_progress, owner="zht"
    // unclaim as zht
    // assert: owner=NULL, status still in_progress
}

#[test]
fn unclaim_by_non_claimer_rejected() { ... }  // TOCTOU guard
```

- [ ] **Step 7: Commit.**

---

## Task 6: HTTP handlers — unified surface, (channel, task_number) keying

**Files:**
- Modify: `src/server/handlers/tasks.rs`
- Modify: `src/server/handlers/mod.rs` — remove `pub mod task_proposals;`
- Modify: `src/server/mod.rs` — route table cleanup
- Delete: `src/server/handlers/task_proposals.rs`

- [ ] **Step 1: Agent-create route.**

```
POST /internal/agent/:agent/channels/:channel/tasks
     body: { title, source_message_id }
```

Handler loads the source message, assembles the snapshot, normalizes the
timestamp, and calls `store.create_proposed_task`:

```rust
pub async fn agent_create_proposed_task_handler(
    Path((agent_name, channel_name)): Path<(String, String)>,
    State(state): State<AppState>,
    Json(body): Json<CreateProposedTaskBody>,
) -> Result<Json<TaskInfo>, ApiError> {
    // Membership precondition — carried over from PR #96.
    state.store.require_channel_membership(&channel_name, &agent_name)?;

    // Load the source message for snapshot capture. 404 if missing.
    let src = state.store.get_message_by_id(&body.source_message_id)?
        .ok_or_else(|| app_err!(StatusCode::NOT_FOUND, "source message not found"))?;
    if src.channel_name != channel_name {
        return Err(app_err!(StatusCode::CONFLICT, "source message belongs to a different channel"));
    }

    // Assemble snapshot. normalize_sqlite_timestamp lives in the tasks module
    // after being moved out of the deleted task_proposals.rs.
    let args = CreateProposedTaskArgs {
        title: body.title.clone(),
        created_by: agent_name.clone(),
        source_message_id: body.source_message_id.clone(),
        snapshot_sender_name: src.sender_name.clone(),
        snapshot_sender_type: src.sender_type.clone(),
        snapshot_content: src.content.clone(),
        snapshot_created_at: normalize_sqlite_timestamp(&src.created_at)?,
    };

    let task = state.store.create_proposed_task(&channel_name, args)
        .map_err(|e| app_err!(StatusCode::INTERNAL_SERVER_ERROR, "{}", e))?;
    Ok(Json(task))
}
```

Preserve the membership precondition (`require_channel_membership` at the agent level).

Returns `TaskInfo` as JSON; `id` surfaces to the UI but agent-facing bridge formatting strips it (see Task 8).

- [ ] **Step 2: Human-create route.**

```
POST /api/conversations/:id/tasks
     body: { titles: string[] }        (existing plural form — unchanged)
```

Calls into the direct `create_tasks` path. Rejects snapshot fields. Sets membership preconditions on the caller.

- [ ] **Step 3: Status transition route, keyed by (channel, task_number).**

```
POST /api/conversations/:id/tasks/:number/status
     body: { status: "todo" | "in_progress" | "in_review" | "done" | "dismissed" }
```

Define a typed error in the store so the handler maps cleanly without
string-prefix matching:

```rust
// In src/store/tasks/mod.rs
#[derive(Debug, thiserror::Error)]
#[error("invalid task transition: {from:?} -> {to:?}")]
pub struct InvalidTaskTransition {
    pub from: TaskStatus,
    pub to: TaskStatus,
}
// Returned via anyhow::Error::new(InvalidTaskTransition { ... })
```

```rust
pub async fn update_task_status_handler(
    Path((conversation_id, task_number)): Path<(String, i64)>,
    State(state): State<AppState>,
    WithAuth(actor): WithAuth,
    Json(body): Json<UpdateTaskStatusBody>,
) -> Result<Json<TaskInfo>, ApiError> {
    let new_status = TaskStatus::from_status_str(&body.status)
        .ok_or_else(|| app_err!(StatusCode::BAD_REQUEST, "unknown status: {}", body.status))?;
    let channel_name = state.store.resolve_channel_name(&conversation_id)?;
    state.store.require_channel_membership(&channel_name, &actor.name)?;
    match state.store.update_task_status(&channel_name, task_number, &actor.name, new_status) {
        Ok(task) => Ok(Json(task)),
        Err(e) => {
            if e.downcast_ref::<InvalidTaskTransition>().is_some() {
                Err(app_err!(StatusCode::UNPROCESSABLE_ENTITY, "{}", e))
            } else {
                Err(app_err!(StatusCode::INTERNAL_SERVER_ERROR, "{}", e))
            }
        }
    }
}
```

Note the typed-error `downcast_ref` — more durable than the string-prefix match kimi flagged in R2.

- [ ] **Step 4: Claim + unclaim routes (same keying).**

```
POST /api/conversations/:id/tasks/:number/claim
POST /api/conversations/:id/tasks/:number/unclaim
```

No body. Caller identity comes from auth. Membership precondition applies. Rejects claim on proposed/dismissed/done (422).

- [ ] **Step 5: List + detail routes.**

Already exist — confirm they return the new `TaskInfoPublic` DTO with snapshot fields + `owner` (not `claimed_by`).

- [ ] **Step 6: Delete `src/server/handlers/task_proposals.rs` + its mod declaration + routes.**

```bash
git rm src/server/handlers/task_proposals.rs
```

Update `src/server/handlers/mod.rs` (remove `pub mod task_proposals;`). Update `src/server/mod.rs` to delete the proposal routes.

- [ ] **Step 7: E2E tests.**

In `tests/e2e_tests.rs`:

```rust
#[tokio::test]
async fn http_create_proposed_task_then_accept() { ... }

#[tokio::test]
async fn http_update_task_status_rejects_non_member_403() { ... }

#[tokio::test]
async fn http_update_task_status_invalid_transition_returns_422() { ... }

#[tokio::test]
async fn http_update_task_status_by_non_owner_succeeds() {
    // Owner is zht; alice (also a member) sends-for-review. Expected: 200.
    // Regression test for removed owner-gate.
}

#[tokio::test]
async fn http_claim_on_proposed_returns_422() { ... }

#[tokio::test]
async fn http_dismiss_proposal_does_not_mint_sub_channel() { ... }
```

- [ ] **Step 8: Run `cargo test --test e2e_tests`. Pass.**

- [ ] **Step 9: Commit.**

---

## Task 7: Realtime — `TaskUpdateEvent` global fan-out

(R1 finding #6: no hand-waving. Concrete diff.)

**Files:**
- Modify: `src/store/mod.rs` — add `task_updates_tx: broadcast::Sender<TaskUpdateEvent>`
- Modify: `src/server/transport/realtime.rs` — add third `tokio::select!` branch
- Modify: `src/store/tasks/mod.rs` — call `self.emit_task_update(&task)` after every mutation

- [ ] **Step 1: Define `TaskUpdateEvent` next to `StreamEvent`.**

```rust
// src/store/stream.rs (or wherever StreamEvent lives)
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TaskUpdateEvent {
    pub task_id: String,
    pub channel_id: String,              // parent channel — useful for clients that care
    pub task_number: i64,
    pub status: String,
    pub owner: Option<String>,
    pub sub_channel_id: Option<String>,
    pub updated_at: String,
}
```

Re-export alongside the existing `StreamEvent`:

```rust
// src/store/mod.rs
pub use stream::{StreamEvent, TaskUpdateEvent};
```

- [ ] **Step 2: Add a dedicated broadcaster on `Store`. Initialize in `Store::open`.**

```rust
pub struct Store {
    // ...existing fields (conn, stream_tx, trace_tx, ...)
    task_updates_tx: broadcast::Sender<TaskUpdateEvent>,
}

impl Store {
    pub fn open(path: &str) -> Result<Self> {
        // ...existing init (conn, run_migrations, stream_tx, trace_tx)
        let (task_updates_tx, _) = broadcast::channel(256);
        Ok(Store {
            // ...existing field assignments
            task_updates_tx,
        })
    }

    pub fn subscribe_task_updates(&self) -> broadcast::Receiver<TaskUpdateEvent> {
        self.task_updates_tx.subscribe()
    }

    pub(crate) fn emit_task_update(&self, task: &TaskInfo) {
        let ev = TaskUpdateEvent { /* map from task */ };
        let _ = self.task_updates_tx.send(ev);   // ignore NoReceivers error
    }
}
```

- [ ] **Step 3: Wire into `realtime_session`.**

```rust
async fn realtime_session(mut socket: WebSocket, store: Arc<Store>, viewer: String) {
    let mut stream_rx = store.subscribe();
    let mut trace_rx = store.subscribe_traces();
    let mut task_update_rx = store.subscribe_task_updates();   // NEW

    loop {
        tokio::select! {
            // ...existing branches unchanged
            task_update = task_update_rx.recv() => {
                match task_update {
                    Ok(ev) => {
                        // Key is "event" — matches the existing trace/event wrapper
                        // convention used elsewhere in realtime.rs. The TypeScript
                        // `RealtimeFrame` dispatcher expects `frame.event`.
                        let msg = Message::Text(
                            serde_json::to_string(&json!({ "type": "task_update", "event": ev })).unwrap()
                        );
                        if socket.send(msg).await.is_err() { break; }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
}
```

No membership check — task updates are global. Every connected viewer gets every task update.

- [ ] **Step 4: Call `emit_task_update` from every mutating store method.**

Locations: `create_proposed_task`, `create_tasks`, `update_task_status`, `update_tasks_claim`, `update_task_unclaim`. Fire **after** `tx.commit()` and `drop(conn)` (the Rust borrow checker enforces this already — the conn lock must release before we touch the broadcaster).

- [ ] **Step 5: Rust integration test.**

```rust
#[tokio::test]
async fn task_mutations_fire_task_update_events() {
    let store = test_store();
    let mut rx = store.subscribe_task_updates();
    let task = store.create_proposed_task(...);
    let ev = rx.recv().await.unwrap();
    assert_eq!(ev.task_id, task.id);
    assert_eq!(ev.status, "proposed");
}
```

- [ ] **Step 6: Commit.**

---

## Task 8: MCP tools — rename + retarget bridge backend

**Files:**
- Modify: `src/bridge/backend.rs` (tool dispatch + HTTP client)
- Modify: `src/bridge/mod.rs` (tool definitions + docstrings)

- [ ] **Step 1: Rename bridge HTTP client paths.**

`ChorusBackend::propose_task` currently POSTs to `/internal/agent/{}/channels/{}/task-proposals`. Change to `/internal/agent/{}/channels/{}/tasks`. Rename the method to `create_task`.

Same renames:
- `ChorusBackend::accept_task_proposal` → `accept_task`; path stays under the new `/tasks` family: `POST /internal/agent/:agent/channels/:channel/tasks/:number/status` with `body: {status: "todo"}`.
- `dismiss_task_proposal` → `dismiss_task`; same endpoint, `body: {status: "dismissed"}`.

- [ ] **Step 2: Add `claim_task_by_number`, `unclaim_task_by_number`, `advance_task_status_by_number` to the `Backend` trait.**

Keep `(channel, task_number)` signatures — matches existing `claim_tasks` shape. Implement against the new HTTP endpoints.

- [ ] **Step 3: Rename MCP tool registrations + update docstrings.**

In `src/bridge/mod.rs` (or wherever `tool_definitions()` lives):

```rust
// BEFORE: "propose_task" -> "create_task"
Tool::new("create_task")
    .description(
        "Propose a new task from a specific chat message. \
         The task starts in `proposed` state and stays there until a human accepts or dismisses it. \
         Sub-channel is minted only on acceptance. Any channel member can accept or dismiss.",
    )
    .input_schema(...)
```

Each tool's docstring is the agent's tuning surface — be explicit per `docs/KNOWLEDGE.md`.

- [ ] **Step 4: Bridge smoke tests.**

`src/bridge/smoke_test.rs` — confirm:
- `create_task` is callable, posts to `/tasks`.
- `propose_task` (old name) returns "unknown tool" — don't silently fall through.
- `accept_task` + `dismiss_task` hit the status endpoint.
- `claim_task` / `unclaim_task` / `advance_task_status` work with `(channel, task_number)`.

- [ ] **Step 5: Commit.**

---

## Task 9: UI — tasks store slice + `TaskCard` + `useTask`

**Files:**
- Create: `ui/src/store/tasksStore.ts` — dedicated Zustand slice
- Create: `ui/src/hooks/useTask.ts` + `.test.ts`
- Create: `ui/src/components/chat/TaskCard.tsx` + `.css` + `.test.tsx`

- [ ] **Step 1: Create `tasksStore.ts`.**

(R1 finding #10: the existing `uiStore.ts` has no `tasksById`. Adding one separate store so task churn doesn't re-render unrelated UI.)

```ts
// ui/src/store/tasksStore.ts
import { create } from 'zustand'
import type { TaskInfo } from '../data/tasks'

interface TasksState {
  tasksById: Record<string, TaskInfo>
  setAll: (tasks: TaskInfo[]) => void
  upsert: (task: TaskInfo) => void
  applyUpdate: (update: {
    task_id: string
    status: string
    owner: string | null
    sub_channel_id: string | null
    updated_at: string
  }) => void
}

export const useTasksStore = create<TasksState>((set) => ({
  tasksById: {},
  setAll: (tasks) => set({
    tasksById: Object.fromEntries(tasks.map((t) => [t.id, t])),
  }),
  upsert: (task) => set((state) => ({
    tasksById: { ...state.tasksById, [task.id]: task },
  })),
  applyUpdate: (u) => set((state) => {
    const prev = state.tasksById[u.task_id]
    if (!prev) return state   // SSE for a task we haven't loaded — ignore; list-refresh will catch it.
    return {
      tasksById: {
        ...state.tasksById,
        [u.task_id]: {
          ...prev,
          status: u.status as any,
          owner: u.owner,
          subChannelId: u.sub_channel_id,
          updatedAt: u.updated_at,
        },
      },
    }
  }),
}))
```

- [ ] **Step 2: `useTask(taskId)` hook.**

```ts
export function useTask(taskId: string): TaskInfo | null {
  return useTasksStore((s) => s.tasksById[taskId] ?? null)
}
```

- [ ] **Step 3: Write failing `TaskCard` tests — six state coverage.**

```tsx
// ui/src/components/chat/TaskCard.test.tsx
import { render, screen } from '@testing-library/react'
import { TaskCard } from './TaskCard'

const base = {
  id: 'u1', taskNumber: 1, title: 'fix login', owner: null,
  createdBy: 'zht', createdAt: '...', updatedAt: '...',
  subChannelId: null, subChannelName: null,
  sourceMessageId: null, snapshotSenderName: null,
  snapshotSenderType: null, snapshotContent: null, snapshotCreatedAt: null,
} as any

test('proposed renders provenance + [create]/[dismiss]', () => {
  render(<TaskCard task={{
    ...base, status: 'proposed',
    snapshotSenderName: 'alice', snapshotContent: 'safari broken',
  }} onAction={jest.fn()} busy={false} />)
  expect(screen.getByTestId('task-card-accept-btn')).toBeInTheDocument()
  expect(screen.getByTestId('task-card-dismiss-btn')).toBeInTheDocument()
  expect(screen.getByText(/safari broken/)).toBeInTheDocument()
})

test('todo unowned shows [claim]', () => {
  render(<TaskCard task={{ ...base, status: 'todo' }} onAction={jest.fn()} busy={false} />)
  expect(screen.getByTestId('task-card-claim-btn')).toBeInTheDocument()
})

test('todo owned shows [start]', () => {
  render(<TaskCard task={{ ...base, status: 'todo', owner: 'zht' }} onAction={jest.fn()} busy={false} />)
  expect(screen.getByTestId('task-card-start-btn')).toBeInTheDocument()
})

test('in_progress shows [send for review]', () => { ... })
test('in_review shows [mark done]', () => { ... })
test('done collapses to pill, no primary action', () => { ... })
test('dismissed renders muted with no actions', () => { ... })
```

- [ ] **Step 4: Run — expected FAIL: component does not exist.**

- [ ] **Step 5: Implement `TaskCard.tsx`.**

```tsx
import type { TaskInfo, TaskStatus } from '../../data/tasks'
import './TaskCard.css'

type TaskAction =
  | { kind: 'accept' } | { kind: 'dismiss' }
  | { kind: 'claim' } | { kind: 'unclaim' }
  | { kind: 'start' } | { kind: 'sendForReview' } | { kind: 'markDone' }
  | { kind: 'openSubChannel' }

interface TaskCardProps {
  task: TaskInfo
  onAction: (a: TaskAction) => void
  busy: boolean
}

export function TaskCard({ task, onAction, busy }: TaskCardProps) {
  const { status, taskNumber, title, owner, snapshotContent, snapshotSenderName } = task
  return (
    <div className="task-card" data-status={status} data-testid={`task-card-${taskNumber}`}>
      <div className="task-card__head">
        <span className="task-card__num">#{taskNumber}</span>
        <span className="task-card__status" data-status={status}>{status}</span>
        {owner && <span className="task-card__owner">@{owner}</span>}
      </div>

      {status === 'proposed' && snapshotContent && (
        <blockquote className="task-card__excerpt">
          {snapshotSenderName && <span className="task-card__excerpt-author">@{snapshotSenderName}:</span>}
          <span>{snapshotContent}</span>
        </blockquote>
      )}

      <div className="task-card__title">{title}</div>

      {renderPrimaryCta({ status, owner, onAction, busy })}
      {status === 'proposed' && (
        <button data-testid="task-card-dismiss-btn" onClick={() => onAction({ kind: 'dismiss' })} disabled={busy}>
          dismiss
        </button>
      )}
      {(status === 'in_progress' || status === 'in_review' || status === 'done') && task.subChannelName && (
        <button className="task-card__link" onClick={() => onAction({ kind: 'openSubChannel' })}>
          {task.subChannelName}
        </button>
      )}
    </div>
  )
}

function renderPrimaryCta(args): JSX.Element | null {
  const { status, owner, onAction, busy } = args
  switch (status) {
    case 'proposed':
      return <button data-testid="task-card-accept-btn" onClick={() => onAction({ kind: 'accept' })} disabled={busy}>create</button>
    case 'dismissed':
      return null
    case 'todo':
      return owner
        ? <button data-testid="task-card-start-btn"  onClick={() => onAction({ kind: 'start' })} disabled={busy}>start</button>
        : <button data-testid="task-card-claim-btn"  onClick={() => onAction({ kind: 'claim' })} disabled={busy}>claim</button>
    case 'in_progress':
      return <button data-testid="task-card-review-btn" onClick={() => onAction({ kind: 'sendForReview' })} disabled={busy}>send for review</button>
    case 'in_review':
      return <button data-testid="task-card-done-btn"   onClick={() => onAction({ kind: 'markDone' })} disabled={busy}>mark done</button>
    case 'done':
      return <span data-testid="task-card-done-pill">done</span>
    default:
      return null
  }
}
```

- [ ] **Step 6: Write CSS following zero-radius + mono conventions in `docs/DESIGN.md`.**

- [ ] **Step 7: Run vitest. Pass.**

- [ ] **Step 8: Commit.**

---

## Task 10: UI — MessageList routing + `TaskEventRow` + `useTaskUpdateStream`

**Files:**
- Modify: `ui/src/components/chat/MessageList.tsx`
- Create: `ui/src/components/chat/TaskEventRow.tsx` + `.test.tsx`
- Create: `ui/src/hooks/useTaskUpdateStream.ts`
- Modify: `ui/src/hooks/useTaskEventLog.ts` — return `TaskEventRecord[]`, drop card state
- Modify: `ui/src/data/taskEvents.ts` — map wire `claimedBy` → type `owner`

- [ ] **Step 1: Map wire `claimedBy` → type `owner` in `parseTaskEvent`.**

(R1 finding #11: don't rename the wire; map at the boundary.)

```ts
// ui/src/data/taskEvents.ts
interface TaskEventPayloadWire {
  action: string
  taskNumber: number
  title: string
  subChannelId: string
  actor: string
  prevStatus?: TaskStatus
  nextStatus: TaskStatus
  claimedBy?: string | null   // wire name unchanged
}

export interface TaskEventPayload {
  action: string
  taskNumber: number
  title: string
  subChannelId: string
  actor: string
  prevStatus?: TaskStatus
  nextStatus: TaskStatus
  owner?: string | null       // in-memory: renamed
}

export function parseTaskEvent(content: string): TaskEventPayload | null {
  const wire = tryJsonParse<TaskEventPayloadWire>(content)
  if (!wire || wire.action === undefined) return null
  return {
    action: wire.action,
    taskNumber: wire.taskNumber,
    title: wire.title,
    subChannelId: wire.subChannelId,
    actor: wire.actor,
    prevStatus: wire.prevStatus,
    nextStatus: wire.nextStatus,
    owner: wire.claimedBy,
  }
}
```

- [ ] **Step 2: Refactor `useTaskEventLog`.**

(R1 finding #15: explicit return type.)

```ts
export interface TaskEventRow {
  eventId: string
  seq: number
  action: TaskEventPayload['action']
  actor: string
  prevStatus?: TaskStatus
  nextStatus: TaskStatus
  createdAt: string
  taskNumber: number
}

export function useTaskEventLog(messages: HistoryMessage[]): TaskEventRow[] {
  return useMemo(() => {
    const rows: TaskEventRow[] = []
    for (const m of messages) {
      if (m.senderType !== 'system') continue
      const ev = parseTaskEvent(m.content)
      if (!ev) continue
      rows.push({
        eventId: m.id,
        seq: m.seq,
        action: ev.action,
        actor: ev.actor,
        prevStatus: ev.prevStatus,
        nextStatus: ev.nextStatus,
        createdAt: m.createdAt,
        taskNumber: ev.taskNumber,
      })
    }
    rows.sort((a, b) => a.seq - b.seq)
    return rows
  }, [messages])
}
```

No more `TaskEventIndex`, no more `byTaskNumber: Map`. Flat list; consumers render rows inline.

- [ ] **Step 3: Implement `TaskEventRow`.**

```tsx
interface TaskEventRowProps { event: TaskEventRow }
export function TaskEventRow({ event }: TaskEventRowProps) {
  return (
    <div className="task-event-row" data-action={event.action}>
      {formatEvent(event)}
    </div>
  )
}
function formatEvent(e: TaskEventRow): string {
  switch (e.action) {
    case 'claimed':        return `@${e.actor} claimed`
    case 'unclaimed':      return `@${e.actor} unclaimed`
    case 'status_changed': return `→ ${e.nextStatus}`
    // No `dismissed` or `created` arms — per spec, proposed → dismissed does
    // not post a task_event (the task_card mutation is the signal), and
    // direct-create posts a task_card instead of a task_event(Created).
    default:               return `${e.actor} ${e.action}`
  }
}
```

- [ ] **Step 4: Route `task_card` + `task_event` in MessageList.**

The `HistoryMessage` type has no top-level `kind` field — `kind` is a discriminator inside the JSON `content` string (pattern used by the existing `task_proposal` and `task_event` rendering).

Because `useTask` is a hook (it subscribes to `useTasksStore`), it **cannot** be called conditionally inside the `MessageList` render loop — that would violate React's Rules of Hooks. Extract a sub-component that calls the hook unconditionally at its top level:

```tsx
// ui/src/components/chat/TaskCardContainer.tsx
import { useTask } from '../../hooks/useTask'
import { TaskCard } from './TaskCard'

export interface TaskCardWirePayload {
  kind: 'task_card'
  task_id: string
  task_number: number
  title: string
  status: string
  owner: string | null
  created_by: string
  source_message_id: string | null
  snapshot_sender_name: string | null
  snapshot_sender_type: string | null
  snapshot_content: string | null
  snapshot_created_at: string | null
}

interface TaskCardContainerProps {
  payload: TaskCardWirePayload
  onAction: (action: TaskAction) => void
  busy: boolean
}

export function TaskCardContainer({ payload, onAction, busy }: TaskCardContainerProps) {
  const task = useTask(payload.task_id)
  return task ? <TaskCard task={task} onAction={onAction} busy={busy} /> : null
}
```

Then the `MessageList` dispatch calls the sub-component, not the hook:

```tsx
// Parse once — the same result serves both routes.
const parsed = tryJsonParse<{ kind?: string }>(msg.content)

if (parsed?.kind === 'task_card') {
  return <TaskCardContainer payload={parsed as TaskCardWirePayload} onAction={...} busy={...} />
}

// `parseTaskEvent` already returns null on non-event system messages, so
// using it as the `task_event` detector is stable. Parse once, reuse.
const ev = parseTaskEvent(msg.content)
if (ev != null) {
  return <TaskEventRow event={evToRow(ev, msg)} />
}
```

- [ ] **Step 5: Extend the realtime transport for `task_update` frames.**

The existing UI transport at `ui/src/transport/types.ts` defines `RealtimeFrame` as a discriminated union (`event | trace | error`). Extend it with a new variant, then add a matching subscription method on `RealtimeSession` — same pattern as `subscribeTraces`.

```ts
// ui/src/transport/types.ts
export interface TaskUpdateFrame {
  type: 'task_update'
  taskId: string
  channelId: string
  taskNumber: number
  status: string
  owner: string | null
  subChannelId: string | null
  updatedAt: string
}

export type RealtimeFrame =
  | { type: 'event'; event: ServerEvent }
  | { type: 'trace'; event: TraceFrame }
  | { type: 'task_update'; event: TaskUpdateFrame }   // NEW
  | { type: 'error'; code: string; message: string }
```

```ts
// ui/src/transport/session.ts
export class RealtimeSession {
  // ...existing fields
  private taskUpdateSubscribers = new Map<string, (f: TaskUpdateFrame) => void>()

  subscribeTaskUpdates(onUpdate: (f: TaskUpdateFrame) => void): () => void {
    const id = `task-update-${nextSubId++}`
    this.taskUpdateSubscribers.set(id, onUpdate)
    this.ensureSocket()
    return () => { this.taskUpdateSubscribers.delete(id) }
  }

  // In the frame dispatch switch (near where traces fan out):
  //   case 'task_update':
  //     for (const onUpdate of this.taskUpdateSubscribers.values()) {
  //       onUpdate(frame.event)
  //     }
  //     break
}
```

Then the hook:

```ts
// ui/src/hooks/useTaskUpdateStream.ts
import { useEffect } from 'react'
import { useTasksStore } from '../store/tasksStore'
import { getRealtimeSession } from '../transport'   // existing singleton accessor

export function useTaskUpdateStream() {
  const applyUpdate = useTasksStore(s => s.applyUpdate)
  useEffect(() => {
    const session = getRealtimeSession()
    return session.subscribeTaskUpdates((frame) => {
      applyUpdate({
        task_id: frame.taskId,
        status: frame.status,
        owner: frame.owner,
        sub_channel_id: frame.subChannelId,
        updated_at: frame.updatedAt,
      })
    })
  }, [applyUpdate])
}
```

Mount once at the app root. If the transport doesn't expose a singleton accessor today, add one (it should — the trace hook uses the same session). No separate WebSocket connection.

- [ ] **Step 6: Initial load into `tasksStore`.**

When a user opens a channel, the existing list-tasks call populates `tasksStore.setAll([...])`. Every subsequent `task_update` SSE applies a patch; list stays in sync.

- [ ] **Step 7: Run vitest. Pass.**

- [ ] **Step 8: Commit.**

---

## Task 11: UI — delete dead components + Rust sibling

**Files:**
- Delete: `ui/src/components/chat/TaskProposalMessage.tsx` + `.css` + `.test.tsx`
- Delete: `ui/src/components/chat/TaskEventMessage.tsx` + `.css` + `.test.tsx`
- Delete: `ui/src/hooks/useTaskProposalLog.ts` + `.test.ts`
- Delete: `src/store/task_proposals.rs`

- [ ] **Step 1: Grep for remaining references.**

```bash
git grep -l "TaskProposalMessage\|TaskEventMessage\|useTaskProposalLog\|task_proposals\b" -- 'ui/src/**' 'src/**' 'tests/**'
```

Expected output: only the files being deleted. Anything else indicates a missed call site.

- [ ] **Step 2: Delete files + update `src/store/mod.rs`.**

```bash
git rm ui/src/components/chat/TaskProposalMessage.{tsx,css,test.tsx}
git rm ui/src/components/chat/TaskEventMessage.{tsx,css,test.tsx}
git rm ui/src/hooks/useTaskProposalLog.ts ui/src/hooks/useTaskProposalLog.test.ts
git rm src/store/task_proposals.rs
# remove `pub mod task_proposals;` from src/store/mod.rs
```

- [ ] **Step 3: `normalize_sqlite_timestamp` already moved in Task 1 Step 3.5.** Confirm no stragglers with `git grep normalize_sqlite_timestamp -- 'src/**'`.

- [ ] **Step 4: Run the full stack.**

```bash
cargo test
cd ui && npx tsc --noEmit && npm run test
```

- [ ] **Step 5: Commit.**

---

## Task 12: UI — kanban filter + detail stubs + field rename

**Files:**
- Modify: `ui/src/data/tasks.ts`
- Modify: `ui/src/components/tasks/TasksPanel.tsx`
- Modify: `ui/src/components/tasks/TaskDetail.tsx`

- [ ] **Step 1: Update `TaskInfo` + `TaskStatus` types.**

```ts
export type TaskStatus =
  | 'proposed' | 'dismissed'
  | 'todo' | 'in_progress' | 'in_review' | 'done'

export interface TaskInfo {
  id: string
  taskNumber: number
  title: string
  status: TaskStatus
  owner?: string | null                // was claimedByName
  createdBy: string
  createdAt: string
  updatedAt: string
  subChannelId?: string | null
  subChannelName?: string | null
  sourceMessageId?: string | null
  snapshotSenderName?: string | null
  snapshotSenderType?: string | null
  snapshotContent?: string | null
  snapshotCreatedAt?: string | null
}
```

- [ ] **Step 2: Scoped rename `claimedByName` → `owner` AND `createdByName` → `createdBy`.**

Only in these files (avoid a repo-wide sed that could touch wire `claimedBy`):
- `ui/src/data/tasks.ts`
- `ui/src/components/tasks/TasksPanel.tsx` — updates both the `task.createdByName` reader at line ~60 and the `task.claimedByName` reader
- `ui/src/components/tasks/TaskDetail.tsx`
- `ui/src/components/tasks/TaskCard` references (already `owner` from Task 9)

Do NOT touch `ui/src/data/taskEvents.ts`'s wire-level `claimedBy` string — the boundary map is already in place from Task 10 Step 1. The wire is immutable for chat-history compat; only in-memory types rename.

- [ ] **Step 3: Kanban 4-column filter + six-key `groupTasksByStatus`.**

```tsx
const COMMITTED: TaskStatus[] = ['todo', 'in_progress', 'in_review', 'done']
const committedTasks = tasks.filter(t => COMMITTED.includes(t.status))
const groupedByStatus = groupTasksByStatus(committedTasks)
// Read only the four committed-work columns; the other two (proposed, dismissed) are initialized empty.
```

`groupTasksByStatus` must initialize **all six** keys — `Record<TaskStatus, TaskInfo[]>` requires them, and TypeScript will fail to compile otherwise:

```ts
export function groupTasksByStatus(tasks: TaskInfo[]): Record<TaskStatus, TaskInfo[]> {
  const result: Record<TaskStatus, TaskInfo[]> = {
    proposed: [], dismissed: [],
    todo: [], in_progress: [], in_review: [], done: [],
  }
  for (const t of tasks) result[t.status].push(t)
  return result
}
```

The UI only reads four. The extra two keys are harmless when empty.

- [ ] **Step 4: Endpoint URLs.**

```ts
export function updateTaskStatus(conversationId: string, taskNumber: number, status: TaskStatus) {
  return post(`/api/conversations/${encodeURIComponent(conversationId)}/tasks/${taskNumber}/status`, { status })
}
export function claimTask(conversationId: string, taskNumber: number) {
  return post(`/api/conversations/${encodeURIComponent(conversationId)}/tasks/${taskNumber}/claim`, {})
}
export function unclaimTask(conversationId: string, taskNumber: number) {
  return post(`/api/conversations/${encodeURIComponent(conversationId)}/tasks/${taskNumber}/unclaim`, {})
}
```

- [ ] **Step 5: `TaskDetail` — primary CTA mirrors the card; secondary actions are v2 stubs.**

Click handlers call the same endpoints. Secondary buttons ("reassign", "cancel") can render disabled with a "coming soon" tooltip; fine for v1.

- [ ] **Step 6: Typecheck + test.**

```bash
cd ui && npx tsc --noEmit && npm run test
```

- [ ] **Step 7: Commit.**

---

## Task 13: Rust store tests — coverage

**Files:**
- Modify: `tests/store_tests.rs`

- [ ] **Step 1: Audit PR #93/#96 tests; rename/retain/delete.**

Keep any test that still maps to the new model (snapshot CHECK, pointer-vs-truth, RFC3339 normalization). Delete proposal-specific tests that the merged model invalidates. Rewrite accept/dismiss tests as transition tests.

- [ ] **Step 2: Transition-graph coverage — one test per valid + one per representative invalid.**

Valid forward: proposed→todo, proposed→dismissed, todo→in_progress, in_progress→in_review, in_review→done.
Representative invalid: proposed→in_progress (skip), todo→proposed (reverse), done→in_progress (reverse from terminal).

- [ ] **Step 3: DB-level CHECK constraint tests.**

```rust
#[test]
fn snapshot_partial_insert_rejected_by_check() {
    // INSERT INTO tasks (..., source_message_id) values (..., 'mid') -- no sender_name
    // Expect rusqlite error mentioning CHECK constraint.
}
```

Valid on both migrated and fresh DBs (the migration uses rebuild pattern so CHECK is live).

- [ ] **Step 4: Pointer-vs-truth.**

```rust
#[test]
fn source_message_delete_nulls_pointer_preserves_snapshot() {
    // 1. Create proposed task with snapshot.
    // 2. DELETE FROM messages WHERE id = source_message_id.
    // 3. SELECT source_message_id, snapshot_sender_name FROM tasks.
    // 4. Expect source_message_id = NULL; snapshot_sender_name still set.
}
```

- [ ] **Step 5: Owner-label regression test.**

```rust
#[test]
fn non_owner_can_advance_status() {
    // Seed: task status=in_progress, owner=alice
    // update_task_status(channel, task_number, "bob", InReview)  // bob != owner
    // Expect success.
}
```

- [ ] **Step 6: Claim decoupling regression test.**

```rust
#[test]
fn claim_does_not_advance_status() {
    // Seed: task status=todo, owner=NULL
    // update_tasks_claim(...)
    // Expect owner=alice, status=todo (not in_progress!)
}
```

- [ ] **Step 7: Run `cargo test --test store_tests`. Pass.**

- [ ] **Step 8: Commit.**

---

## Task 14: Rust e2e tests — HTTP flow

**Files:**
- Modify: `tests/e2e_tests.rs`

- [ ] **Step 1: Happy-path lifecycle test.**

```rust
#[tokio::test]
async fn full_lifecycle_proposed_to_done() {
    // 1. Seed source message in parent channel.
    // 2. Agent creates proposed task -> 200, task_card in parent channel.
    // 3. Human POSTs status=todo -> 200. Expect sub-channel minted + kickoff.
    // 4. Human claims -> 200. Expect owner set, status still todo.
    // 5. Human POSTs status=in_progress -> 200.
    // 6. status=in_review -> 200.
    // 7. status=done -> 200. Expect sub-channel archived.
    // Assert on task_update WebSocket events at each step.
}
```

- [ ] **Step 2: 403 tests for every mutating endpoint.**

- [ ] **Step 3: 422 tests for invalid transitions + claim on terminal states.**

- [ ] **Step 4: Dismiss invariant.**

```rust
#[tokio::test]
async fn dismiss_from_proposed_does_not_mint_sub_channel() { ... }
```

- [ ] **Step 5: Source-delete invariant via HTTP.**

- [ ] **Step 6: Non-owner-can-advance regression.**

```rust
#[tokio::test]
async fn non_owner_status_advance_succeeds_via_http() { ... }
```

- [ ] **Step 7: Run `cargo test --test e2e_tests`. Pass.**

- [ ] **Step 8: Commit.**

---

## Task 15: Vitest — `TaskCard` + hooks + wire parsing

**Files:**
- (tests mostly created in Tasks 9, 10; extend here)

- [ ] **Step 1: `useTask` — subscription + re-render on store update.**

- [ ] **Step 2: `useTaskUpdateStream` — mock the realtime connection, emit `task_update`, assert store mutation.**

- [ ] **Step 3: `parseTaskEvent` — wire `claimedBy` surfaces as type `owner`.**

```ts
test('parseTaskEvent maps wire claimedBy to owner', () => {
  const wire = JSON.stringify({
    action: 'claimed', taskNumber: 1, title: 't', subChannelId: 's',
    actor: 'zht', nextStatus: 'todo', claimedBy: 'zht',
  })
  const ev = parseTaskEvent(wire)!
  expect(ev.owner).toBe('zht')
  expect((ev as any).claimedBy).toBeUndefined()
})
```

- [ ] **Step 4: MessageList routing tests.**

- [ ] **Step 5: Run vitest. Pass.**

- [ ] **Step 6: Commit.**

---

## Task 16: Playwright — rewire TSK-005, add TSK-006

**Files:**
- Modify: `qa/cases/playwright/TSK-005.spec.ts`
- Create: `qa/cases/playwright/TSK-006.spec.ts`
- Modify: `qa/cases/tasks.md`

- [ ] **Step 1: TSK-005 endpoint + testid rewire.**

```
/internal/agent/:agent/channels/:channel/task-proposals
  → /internal/agent/:agent/channels/:channel/tasks

[data-testid^="task-proposal-"]  →  [data-testid^="task-card-"]
[data-testid="task-proposal-accept-btn"]  →  [data-testid="task-card-accept-btn"]
```

Deep-link + kickoff ordering assertions stay as-is (spec preserves the format).

Accept button click now POSTs `/api/conversations/:id/tasks/:number/status` with `{status:"todo"}` — but the UI wraps this, so the test clicks the button; only the network mock/interceptor in the spec needs the URL update.

- [ ] **Step 2: TSK-006 — full-lifecycle smoke.**

```ts
test('Full Task Lifecycle @case TSK-006', async ({ page, request }) => {
  // Reuse TSK-005 setup through accept.
  // 3. Click create -> card flips to accepted
  // 4. Click claim -> card shows [start] primary + owner line
  // 5. Click start -> in_progress, [send for review] primary
  // 6. Click send for review -> in_review, [mark done] primary
  // 7. Click mark done -> card collapses to [done pill]
  // 8. Click pill -> sub-channel opens
})
```

- [ ] **Step 3: Run Playwright.**

```bash
npx playwright test qa/cases/playwright/TSK-00{5,6}.spec.ts
```

- [ ] **Step 4: Commit.**

---

## Task 17: Docs + cleanup + final verification

**Files:**
- Modify: `docs/BACKEND.md`, `docs/KNOWLEDGE.md`

- [ ] **Step 1: `docs/BACKEND.md`** — document the six-state `TaskStatus` enum, CHECK constraint, `task_card`/`task_event` wire kinds, forward-only transition graph, owner-is-a-label, `(channel, task_number)` keying everywhere.

- [ ] **Step 2: `docs/KNOWLEDGE.md`** — append a decision entry:

```
## 2026-04-24: Unified task lifecycle

Merged `task_proposals` into `tasks` with a six-state enum. Proposal is a task
in status `proposed`. Wire format for `task_event` keeps `claimedBy` (pre-merge)
to preserve chat history — in-memory renamed to `owner`. Tasks keyed by
`(channel, task_number)` everywhere (HTTP + MCP); task_id UUID is internal.
See: docs/plans/2026-04-24-task-lifecycle-unified-design.md.
```

- [ ] **Step 3: Final full-stack test.**

```bash
cargo test
cd ui && npx tsc --noEmit && npm run test
npx playwright test qa/cases/playwright/TSK-00{5,6}.spec.ts
```

- [ ] **Step 4: Commit.**

---

## Done

- [ ] Close PR #93 and PR #96 with a comment pointing to this branch.
- [ ] Open PR from `feat/task-lifecycle` → `main`.
- [ ] Run `/gstack-qa` for visual + flow verification before merge.
