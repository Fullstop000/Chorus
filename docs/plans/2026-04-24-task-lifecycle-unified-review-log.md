# Task Lifecycle Unified — Plan Review Log

Pairs with the spec (`2026-04-24-task-lifecycle-unified-design.md`) and plan
(`2026-04-24-task-lifecycle-unified-implementation-plan.md`).

Review loop: draft → kimi review R1 → triage (this log) → revised plan → kimi
review R2 → ship. Kimi is a second-opinion reviewer; every finding is
adjudicated with a reason, not just accepted.

## R1 — kimi CLI (2026-04-24)

Kimi was given the spec + plan plus pointers to the code being superseded
(`src/store/schema.sql`, `src/store/tasks/mod.rs`, `src/store/task_proposals.rs`,
`src/server/handlers/task_proposals.rs`, the two UI components). 16 findings
were returned, graded MUST-FIX / SHOULD-FIX / NICE-TO-HAVE.

All 16 were verified against the code before triage. None dismissed — every
finding pointed at a real gap.

### MUST-FIX

| # | Finding | Verification | Disposition |
|---|---------|--------------|-------------|
| 1 | SQLite migration used `ALTER TABLE … ADD CHECK`, which SQLite rejects; Task 13 test asserts DB-level CHECK, so would fail on migrated DBs. | Confirmed: `migrate_add_task_proposal_snapshot_columns` in `src/store/migrations.rs:442` does a `CREATE TABLE _new; INSERT SELECT; DROP; RENAME` rebuild for exactly this reason. | **Fix inline.** Rewrite Task 1 Step 3 to use the rebuild pattern. Include migrating extant `task_proposals` rows into `tasks` inside the same migration. |
| 2 | `create_proposed_task` posts `task_card` system messages in Task 2 before the `task_card` wire kind is defined (Task 5). | Ordering contradicts itself; Task 2 can't compile. | **Fix inline.** Reorder: make wire-messages the new Task 2, unified create paths become Task 3, shift all downstream. |
| 3 | Existing `Store::update_tasks_claim` fuses claim + status advance in one SQL UPDATE (`SET claimed_by = ?, status = 'in_progress'`). Plan decouples them in the spec but never says to rewrite the SQL. | Confirmed at `src/store/tasks/mod.rs:338`. | **Fix inline.** Task 4 Step 1 explicitly: "Rewrite `update_tasks_claim`'s UPDATE to set only `owner`, not `status`. Decouples claim from start." |
| 4 | `task_event` messages move to sub-channel, but plan doesn't show how to route `create_system_message_tx` (takes `&Channel`, not an id) to the sub-channel, or how `emit_system_stream_events` fans out to sub-channel members. | Confirmed call-site shapes. | **Fix inline.** Task 3 adds `post_task_event_tx` helper that loads the sub-channel `Channel` row and delegates to `create_system_message_tx(&tx, &sub_channel, …)`. Emission also uses sub-channel. |
| 5 | `POST /api/tasks/:id/status` keyed by UUID; `TaskInfo` has no `id` field; existing public API uses `(channel, task_number)`. | Confirmed at `src/store/tasks/mod.rs:296,471`. | **Fix inline.** Change endpoint to `POST /api/conversations/:id/tasks/:number/status`. Same for `/claim`, `/unclaim`. Matches existing convention. |
| 6 | `task_update` SSE hand-waved; existing `StreamEvent` is channel-scoped with member-check gate. | Confirmed at `src/server/transport/realtime.rs`. | **Fix inline.** Task 7 picks option (a): add `broadcast::Sender<TaskUpdateEvent>` on `Store`; `realtime_session` adds a third `tokio::select!` branch that forwards to every connected viewer (no membership check — task updates fan out globally). |

### SHOULD-FIX

| # | Finding | Verification | Disposition |
|---|---------|--------------|-------------|
| 7 | `snapshotted_at` (6th snapshot column on `task_proposals`, the "server captured copy at" audit time) silently dropped. | Confirmed at `schema.sql:248`. | **Fix inline.** Task 1 note: "We intentionally drop `snapshotted_at`. `tasks.created_at` subsumes it — a proposed task's creation time *is* its snapshot capture time." |
| 8 | `ChorusBackend::propose_task` in `src/bridge/backend.rs` POSTs to `/internal/agent/{}/channels/{}/task-proposals`; never retargeted. | | **Fix inline.** Task 8 adds a step to update the HTTP path to `/tasks`. |
| 9 | New MCP tools described as taking `task_id`, but existing `Backend` trait uses `(channel, task_number)`. | | **Fix inline.** Keep `(channel, task_number)` — matches existing bridge + public API. MCP tools surface `task_number`, not `task_id`. |
| 10 | `useTask` reads `tasksById` from Zustand store, but `uiStore.ts` has no such field. | Confirmed. | **Fix inline.** Task 9 extends the store with `tasksById: Record<string, TaskInfo>` + `updateTask(id, patch)` action. Done BEFORE `useTask` is written. |
| 11 | `parseTaskEvent` reads `claimedBy` from wire; renaming it to `owner` breaks the persisted message format. | Confirmed at `src/store/tasks/events.rs:63`, `ui/src/data/taskEvents.ts:18`. | **Fix inline.** **Keep the wire field as `claimedBy`**. Rename happens only in-memory types and UI. Map at the wire→TypeScript boundary in `parseTaskEvent`. Documented in the plan as a deliberate choice. |
| 12 | `update_task_status` enforces `claimed_by == requester_name`; spec says any member can advance. | Confirmed at `src/store/tasks/mod.rs:502`. | **Fix inline.** Task 3 Step 1 explicitly: "Delete the `claimed_by == requester_name` guard. Membership is the only gate." |
| 13 | Kickoff format in plan omits blank line that PR #96 ships (`Task opened: {title}\n\nFrom @…`). | | **Fix inline.** Task 3 Step 3 uses the exact PR #96 format. |
| 14 | Handler pseudocode doesn't map `anyhow!("invalid task transition")` to 422. | | **Fix inline.** Task 6 pseudocode adds error mapping with explicit 422 for transition failures, 403 for membership, etc. |
| 15 | `useTaskEventLog` new return type ambiguous after refactor. | | **Fix inline.** Task 10 Step 4: explicit `TaskEventRecord[]` (flat list, seq-ordered). No more `TaskEventIndex` card state. |

### NICE-TO-HAVE

| # | Finding | Verification | Disposition |
|---|---------|--------------|-------------|
| 16 | Spec data-model diagram says `created_by_name`; existing schema is `created_by`. | Confirmed. | **Fix inline** in the spec. |
| 17 | Internal bridge routes vs public API keyed inconsistently. | | **Fix inline.** Both use `(channel_name, task_number)`. One convention. |

## Not raised by kimi, caught during verification

- **Ordering hazard in Task 12 `sed`.** The Task 12 Step 2 `sed -i` rename of
  `claimedByName` → `owner` will also touch any surviving wire-level
  `claimed_by` string literal in the UI (e.g. JSON serialization), which
  would break wire compat with the preserved `claimedBy` field (finding
  #11). The revised plan uses scoped replacements (type definitions + hook
  returns only), not a repo-wide sed.

## Closeout

All 17 findings addressed in the revised plan (committed separately as
`2026-04-24-task-lifecycle-unified-implementation-plan.md`). Re-running
kimi for R2; continuing the loop until kimi returns no SHOULD-FIX or MUST-FIX.
