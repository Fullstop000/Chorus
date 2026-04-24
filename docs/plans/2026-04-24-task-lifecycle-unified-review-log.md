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
| 1 | SQLite migration used `ALTER TABLE … ADD CHECK`, which SQLite rejects; Task 13 test asserts DB-level CHECK, so would fail on migrated DBs. | Confirmed: `migrate_add_task_proposal_snapshot_columns` in `src/store/migrations.rs:442` does a `CREATE TABLE _new; INSERT SELECT; DROP; RENAME` rebuild for exactly this reason. | **Resolved by scope decision (post-R1, pre-R2).** User directed: migration is unimportant — neither PR is in prod, dev-local DBs are wiped. Plan was simplified to delete the two proposal-era migrations outright. `schema.sql` is the fresh-DB source of truth for the merged shape; no migration code path. Task 1 Step 3 now just deletes old migration functions. The DB-level CHECK is intact because it's declared in `schema.sql`, not in an ALTER. |
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

## R2 — kimi CLI (2026-04-24, same day)

R2 was run against the revised plan + updated review log. 18 new findings +
one R1 finding (R1 #4) flagged as not fully addressed. All verified against
the code before triage.

### MUST-FIX

| # | Finding | Verification | Disposition |
|---|---------|--------------|-------------|
| R2-1 | `post_task_card_message_tx` return typed as `MessageRow`; `create_system_message_tx` actually returns `InsertedMessage`. `task_card_content(&msg)` helper doesn't exist. | Confirmed at `src/store/messages/posting.rs:119-144` — returns `Result<InsertedMessage>`; the pending-events tuple is `Vec<(InsertedMessage, String)>`. | **Fix inline.** Return `Result<InsertedMessage>`. Capture the serialized `content` string and pass `(msg, content)` to `emit_system_stream_events` — the existing `create_tasks` pattern. |
| R2-2 | `update_task_status` emits a `task_event` on `proposed → todo`; spec says acceptance is marked by kickoff only, not a task_event. | Correct per spec §Wire Model. | **Fix inline.** Task 4 Step 1: skip `task_event` when `(current_status, new_status) == (Proposed, Todo)`. Kickoff is the anchor. |
| R2-3 | Claim/unclaim `task_event` messages still posted in parent channel (inherited pattern from existing code). | Confirmed at `src/store/tasks/mod.rs:374,455` — both call `create_system_message_tx(&tx, &channel, ...)` with parent. | **Fix inline.** Task 5 Step 2 + Step 5: load sub-channel via `get_channel_by_id_inner(&tx, sub_channel_id)`, post via that channel. Covers R1 #4 fully. |
| R2-4 | UI transport can't receive `task_update` frames. `RealtimeFrame`, `RealtimeSession`, `connectRealtime()` — I used fictional APIs. | Confirmed: `RealtimeFrame` has `event/trace/error` variants only; `RealtimeSession` has `subscribe`/`subscribeAll`/`subscribeTraces` — no `subscribeTaskUpdates`. | **Fix inline.** Extend `RealtimeFrame` with `{ type: 'task_update'; event: TaskUpdateEvent }`; add `subscribeTaskUpdates` to `RealtimeSession` following the `subscribeTraces` pattern; rewrite `useTaskUpdateStream` against the existing session singleton. |
| R2-5 | `TaskInfo.id` contradiction — plan says UUID "never surfaced" but UI `tasksStore` keys by `task.id`. | Correct — the "never surfaced" wording was overstated. | **Fix inline.** Correct spec/plan: `task_id` UUID is internal to **agents and MCP tools**; it *is* surfaced to the UI (list responses + `task_card` payload) because the store needs a stable key. Public HTTP responses serialize it. |

### SHOULD-FIX

| # | Finding | Disposition |
|---|---------|-------------|
| R2-6 | Migration silently orphans accepted proposals (`WHERE tp.status IN ('pending','dismissed')`). | **Dismiss — user scoped-out.** Per user directive: migration is unimportant. The whole migration was dropped from the revised plan; accepted proposals simply die with the dev DB wipe. |
| R2-7 | Duplicate of R2-1 (`task_card_content` undefined). | **Fix inline** (same fix). |
| R2-8 | `create_tasks` refactor to post `task_card` is hand-waved; ambiguous whether the old `task_event(Created)` stays for direct-create. | **Fix inline.** Task 3 Step 2: show the explicit `create_tasks` loop modification calling `post_task_card_message_tx`. Remove `task_event(Created)` — the `task_card` replaces it in the parent channel. |
| R2-9 | Agent-create handler jumps from `source_message_id` to `create_proposed_task` without showing message lookup + snapshot assembly + RFC3339 normalize. | **Fix inline.** Task 6 Step 1: concrete handler pseudocode. |
| R2-10 | 422 mapping uses fragile string-prefix match on the error message. | **Fix inline.** Task 6 Step 3: define a typed `InvalidTaskTransition` error; use `downcast_ref` in the handler. |
| R2-11 | `createdByName` rename not included in the scoped list (appears in `TasksPanel.tsx`). | **Fix inline.** Task 12 Step 2: keep `createdBy` on the type and make `createdByName` an alias or just update the callsite. Pick one explicitly. |
| R2-12 | `groupTasksByStatus`'s `Record<TaskStatus, TaskInfo[]>` widens to 6 keys but initializer only makes 4. | **Fix inline.** Task 12 Step 3: initialize all six keys. |
| R2-13 | `MessageList` routing checks `msg.kind` — not a top-level field; `kind` lives in `msg.content` JSON. | **Fix inline.** Task 10 Step 4: parse `msg.content` first with `tryJsonParse`. |
| R2-14 | `TaskEventAction::Dismissed` arms missing in `as_str()` / `as_agent_sentence()`. | **Fix inline.** Task 2 Step 1: add the arms. |
| R2-15 | `Store::open` doesn't initialize `task_updates_tx`. | **Fix inline.** Task 7 Step 2: show the `broadcast::channel(256)` wire-up in the constructor. |
| R2-16 | `archive_sub_channel_tx` referenced but not defined. | **Fix inline.** Task 4 Step 1: inline the `UPDATE channels SET archived = 1` SQL or point at the existing helper in `channels.rs`. |

### NICE-TO-HAVE

| # | Finding | Disposition |
|---|---------|-------------|
| R2-17 | `load_task_by_id_tx` / `load_task_by_number_tx` used as if they exist. | **Fix inline.** Add helper definitions (thin wrappers on the existing `get_task_info` query pattern) or point at the existing callers. |
| R2-18 | Missing `pub use stream::TaskUpdateEvent;` in `src/store/mod.rs`. | **Fix inline.** Task 7 Step 1 addendum. |
| R2-19 | Spec says "five snapshot fields must be all-populated or all-null" but CHECK intentionally excludes `source_message_id` (pointer-vs-truth requires independent nullability). | **Fix in spec.** Correct to "four `snapshot_*` fields all-populated or all-null; `source_message_id` is a separately nullable pointer." |

### R1 findings not fully addressed by R1 triage

- **R1 #4** (task_event routing to sub-channel) — partially addressed for status
  transitions (Task 4 via `post_task_event_tx`), but claim/unclaim in Task 5
  still posted to the parent. Now **closed** by R2-3 above.

## R2 Closeout

18 findings: 16 fix inline, 1 dismiss (R2-6, user scope decision), 1 closes
R1 #4. After applying, ready to commit and ship the plan.

## R3 — kimi CLI (2026-04-24, same day — convergence check)

Verdict: **NOT_CONVERGED**. 4 MUST-FIX + 4 SHOULD-FIX + 2 NICE-TO-HAVE.
Three of the MUST-FIX issues were introduced by the R2 revisions themselves.

### MUST-FIX

| # | Finding | Disposition |
|---|---------|-------------|
| R3-1 | Plan emits a `task_event` in the parent channel on `proposed → dismissed`, contradicting the spec's "no task_event for pre-acceptance transitions" rule. | **Fix inline — honor the spec.** Remove `TaskEventAction::Dismissed`, delete `post_parent_dismissed_event_tx`, strip the Dismissed branch from `update_task_status`, and drop the `dismissed` arm from `TaskEventRow.formatEvent`. The card's state mutation (driven by SSE `task_update`) is sufficient evidence of the dismissal — no separate event log entry. |
| R3-2 | `InvalidTaskTransition` typed error is defined and used by the handler's `downcast_ref`, but `update_task_status` still returns `Err(anyhow!("invalid ..."))` — a plain string error. `downcast_ref` never matches, so every invalid transition 500s. | **Fix inline.** Store returns `Err(InvalidTaskTransition { from, to }.into())`. Overlaps with R2-10, which fixed the handler but left the store untouched. |
| R3-3 | Rust sends `{ "type": "task_update", "payload": ev }`; TypeScript `RealtimeFrame` and `RealtimeSession` dispatcher both read `frame.event`. Task updates drop on the client. | **Fix inline.** Change the Rust `json!(...)` to `"event": ev` — matches the existing trace/event convention. |
| R3-4 | `MessageList` pseudocode calls `useTask(payload.task_id)` inside a conditional — React Rules of Hooks violation. | **Fix inline.** Extract `TaskCardContainer` sub-component that calls `useTask` unconditionally at its top level. |

### SHOULD-FIX

| # | Finding | Disposition |
|---|---------|-------------|
| R3-5 | Claim/unclaim SQL not explicitly shown despite the column rename + fusion break. | **Fix inline.** Show explicit `UPDATE tasks SET owner = ?1 ...` / `SET owner = NULL ...` in Task 5. |
| R3-6 | `create_tasks` pseudocode pushes both parent (`task_card`) and sub-channel (kickoff) events into one `pending_events` vector; `emit_system_stream_events` tags every event with the single `&channel` param, so mixing would misroute the kickoff. | **Fix inline.** Split into `parent_events` + `sub_events` vectors with two separate `emit_system_stream_events` calls. |
| R3-7 | Rust `TaskInfo` struct omits `sub_channel_name`; the UI `TaskCard` uses `task.subChannelName` for the deep-link. | **Fix inline.** Add `pub sub_channel_name: Option<String>` to `TaskInfo` and join `channels` in the SELECTs. |
| R3-8 | `normalize_sqlite_timestamp` is used in Task 3's `create_proposed_task` but the helper move out of `src/store/task_proposals.rs` is scheduled in Task 11. Mid-implementation build break. | **Fix inline.** Move the helper in Task 1 (alongside schema changes), not Task 11. |

### NICE-TO-HAVE

| # | Finding | Disposition |
|---|---------|-------------|
| R3-9 | `MessageList` pseudocode parses `msg.content` twice via `parseTaskEvent`. | **Fix inline** — parse once, reuse. |
| R3-10 | `TaskCardWirePayload` TS interface never explicitly declared in the plan. | **Fix inline** — declare in Task 10 alongside the routing. |

### R3 Closeout

10 findings, all fix inline. If R4 returns zero MUST/SHOULD, we ship the plan.

## R4 — kimi CLI (2026-04-24, same day — convergence confirmed)

Verdict: **CONVERGED**. 0 MUST-FIX, 0 SHOULD-FIX, 3 NICE-TO-HAVE. All 10 R3
fixes verified correct. The three NICE-TO-HAVEs were stale File Structure
overview text (references to a migration that no longer exists, a
`TaskEventAction::Dismissed` variant that was removed, and a missing
`evToRow` helper in MessageList pseudocode). All three applied inline.

### R4 findings

| # | Finding | Disposition |
|---|---------|-------------|
| R4-1 | File Structure bullet for `src/store/migrations.rs` references a migration that Task 1 explicitly deletes. | **Fix inline.** Overview now says "delete the two proposal-era migrations." |
| R4-2 | File Structure bullet for `src/store/tasks/mod.rs` mentions `update_task_dismiss` and `TaskEventAction::Dismissed` — both removed in R3-1. | **Fix inline.** Overview aligned: dismiss routes through generic status endpoint; no separate function. `TaskEventAction` unchanged. |
| R4-3 | `MessageList` pseudocode uses `evToRow(ev, msg)` without defining the helper. | **Fix inline.** Inline the conversion: `{ ...ev, eventId: msg.id, seq: msg.seq, createdAt: msg.createdAt }`. |

### Loop closeout

Four kimi rounds, 47 total findings across R1/R2/R3/R4:
- R1: 16 findings (all applied)
- R2: 18 findings (17 applied, 1 user-dismissed for scope — migration)
- R3: 10 findings (all applied)
- R4: 3 findings (all applied)

Plan is implementable as-is. Ready to ship.

