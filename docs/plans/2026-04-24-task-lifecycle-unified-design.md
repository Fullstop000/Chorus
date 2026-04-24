# Task Lifecycle Unified Design

**Date:** 2026-04-24
**Status:** Approved (brainstorm complete, pending spec review + plan)
**Supersedes:** PR #93 (`feat/task-proposals`), PR #96 (`feat/task-proposals-v2`)

## Summary

Today a task's lifecycle is expressed in three disconnected places: a
`TaskProposalMessage` card in the parent channel (pending / accepted /
dismissed), a `TaskEventMessage` card at the top of the sub-channel (todo /
in_progress / in_review / done), and a separate `TasksPanel` kanban tab. The
proposal row and the task row are stored in two separate tables linked by
acceptance.

This design collapses that split. A task is **one row** in a single `tasks`
table with a six-state enum (`proposed, dismissed, todo, in_progress,
in_review, done`). A task is **one rich message card** in the parent channel
that graduates visually through every state. The sub-channel still exists (it
is where the work happens), but it no longer carries its own duplicate card —
it carries the kickoff message plus an inline event timeline.

The sub-channel semantics from the April 19 Controlled Sessions design stay
intact. Everything PR #96 proved out — snapshot CHECK constraint, RFC3339
normalization, membership preconditions, pointer-vs-truth source-delete
invariant, kickoff ordering contract — survives on the merged `tasks` table.

## Decisions

Six decisions, all locked during the brainstorm:

1. **Single table.** `task_proposals` is dropped. A proposal is a task with
   `status = 'proposed'`. A dismissed proposal is a task with `status =
   'dismissed'` (terminal). This replaces Q1's earlier lean toward two tables.
2. **Owner is a label.** `claimed_by` is renamed to `owner`. It stays
   optional, transferable, not a gate. Any channel member can advance any
   state; owner is display, not authorization.
3. **Parent channel = summary. Sub-channel = detail.** The parent card shows
   status + owner + single primary CTA. The sub-channel shows the full event
   timeline as inline rows (no duplicate card at the top).
4. **One primary CTA per state.** Proposed is the only state with a secondary
   action (dismiss) because it is the only binary-decision moment. Secondary
   actions (reassign, cancel, edit title) live in the task detail overlay.
5. **Forward-only transitions in v1.** No `in_review → in_progress` (reviewer
   says "not yet"), no `done → reopen`, no `dismissed → proposed`. Reverts
   happen by unclaim + reassign or by creating a fresh task.
6. **Agents must propose; humans can direct-create.** Agent-created tasks
   always start `status = 'proposed'` (accountability gate). Human-created
   tasks via the UI can start `status = 'todo'` directly. Matches the
   "agents propose, humans accept" posture from PR #93.

## Data Model

One `tasks` table. `task_proposals` is dropped.

```
tasks
─────
id                      UUID PK
channel_id              UUID  FK (parent channel)
task_number             INT   (per-channel auto-increment for display)
title                   TEXT
status                  ENUM  {proposed, dismissed, todo, in_progress, in_review, done}
owner                   TEXT  NULL   (was claimed_by)
created_by              TEXT
created_at              TIMESTAMP
sub_channel_id          UUID  NULL   (minted on transition out of `proposed`)
-- provenance (migrated from task_proposals, PR #96)
source_message_id       UUID  NULL   FK messages.id ON DELETE SET NULL
snapshot_sender_name    TEXT  NULL
snapshot_sender_type    TEXT  NULL
snapshot_content        TEXT  NULL
snapshot_created_at     TIMESTAMP NULL
```

### Invariants

- **Snapshot CHECK constraint.** The four `snapshot_*` columns must be
  either all-populated or all-null. `source_message_id` is a **separately
  nullable pointer** — not part of the CHECK — because the pointer-vs-truth
  invariant requires it to be nullable independently (ON DELETE SET NULL).
  A task created without a snapshot (human direct-create) has all four
  `snapshot_*` fields null and `source_message_id` null; a task created from
  a message has all four populated plus `source_message_id` populated until
  the source is deleted.
- **Sub-channel lifecycle.** `sub_channel_id` is null while `status =
  'proposed'` and stays null forever if the proposal is dismissed. It is
  minted the first (and only) time a task enters `status = 'todo'` — whether
  via `proposed → todo` or direct-create as `todo`. Once set it never
  becomes null again.
- **Ownership default.** Every create — agent-proposed, human direct-create —
  starts with `owner = null`. A task becomes owned only via explicit
  `claim_task` or `POST /api/conversations/:id/tasks/:number/claim`. This is
  uniform and matches today's `claimed_by` default.
- **Pointer-vs-truth.** `source_message_id` uses `ON DELETE SET NULL`. If the
  originating message is deleted, the pointer nulls but the four `snapshot_*`
  fields stay. Provenance survives source deletion — preserved verbatim from
  PR #96.

### Status enum

```
TaskStatus::Proposed    // awaiting accept/dismiss; no sub-channel yet
TaskStatus::Dismissed   // terminal; no sub-channel ever
TaskStatus::Todo        // accepted; sub-channel minted; unowned or owned-but-not-started
TaskStatus::InProgress
TaskStatus::InReview
TaskStatus::Done        // terminal; card collapses to pill
```

## Wire Model

### Parent channel: one `task_card` system message per task

On task creation, the server posts a `task_card` system message in the parent
channel with payload `{ task_id, task_number, title, status, owner,
snapshot? }`. This message is the **host** for the rendered card. It is the
only `task_card` message ever posted for this task — subsequent state changes
do not post more `task_card` messages.

This replaces PR #93's pattern of posting a new `task_proposal` message on
each accept/dismiss (which the reducer folded by proposal id). Under the
merged model there is one host message, and its rendering reflects live task
state via a store selector.

`task_card` is a breaking rename of `task_proposal`. There is no backwards
compatibility veneer — the old wire kind is dropped.

### Sub-channel: kickoff message + inline event timeline

When a task transitions `proposed → todo` (or is direct-created as
`status = 'todo'` via the human route), the server:
1. Mints a sub-channel named `{parent_slug}__task-{task_number}`.
2. Posts the kickoff system message **inside the newly-minted sub-channel** —
   title + author attribution + verbatim blockquote, in that order (preserved
   from PR #96's ordering contract). Direct-created tasks with no snapshot
   skip the blockquote and author-attribution lines but still post a title
   line as kickoff.

On every subsequent status-transitioning action that happens inside the
sub-channel (`claim`, `unclaim`, `send for review`, `mark done`), the server
posts a `task_event` system message in that sub-channel. These render as
inline event rows — `"@zht claimed"`, `"→ in review"`, `"→ done"` — not as a
card. The card is in the parent channel; the sub-channel is the operational
record.

Transitions that happen while `status = 'proposed'` (`→ dismissed` or
`→ todo`) do **not** post `task_event` messages: pre-acceptance there is no
sub-channel to host them. Acceptance itself is marked by the kickoff message.

### SSE: `task_update` event

The server emits a `task_update` SSE event whenever a `tasks` row mutates.
Payload: `{ task_id, status, owner, sub_channel_id }`. The client subscribes
**globally** (not per-channel), keys updates by `task_id`, and mutates the
tasks store. The `TaskCard` component and the `TasksPanel` kanban both read
from this store, so both stay live without per-surface polling.

This is a small widening of today's channel-scoped SSE. Implementation is a
separate stream or a well-known channel id — deferred to the plan.

## UI Components

### New: `TaskCard` component

Replaces `TaskProposalMessage` and `TaskEventMessage`. One file, one state
machine, six renderings.

Inputs:

```ts
interface TaskCardProps {
  task: TaskInfo          // from tasks store; live via SSE
  onAction: (action: TaskAction) => void
  busy: boolean           // mutation-in-flight
}
```

Per-state render:

| Status | Body | Primary CTA | Secondary |
|--------|------|-------------|-----------|
| `proposed` | title + provenance blockquote | [create] | [dismiss] |
| `dismissed` | muted card, "dismissed" tag | — | — |
| `todo` (unowned) | title + "unowned" | [claim] | — |
| `todo` (owned) | title + "claimed by @X" | [start] | — |
| `in_progress` | title + owner + sub-channel deep-link | [send for review] | — |
| `in_review` | title + owner + sub-channel deep-link | [mark done] | — |
| `done` | collapsed pill, strikethrough title, "open →" | — | — |

Proposed is the only state with a secondary action because it is the only
binary-decision moment. All other secondary actions (reassign, cancel, edit
title, return to in_progress) live in the task detail overlay.

### Store wire-up

- `useTask(taskId)` hook subscribes to the tasks store keyed by id. Returns
  live `TaskInfo`. Re-renders on SSE `task_update`.
- `MessageList` detects a `task_card` system message, extracts `task_id` from
  payload, and mounts `<TaskCard task={useTask(task_id)} ...>`. Host message
  id is the React key: the card graduates in place across all six statuses —
  no remounting, no CSS animation fight.

### Sub-channel: `TaskEventRow`

`task_event` system messages render as `TaskEventRow` — a single-line event
(`"@zht claimed"`, `"→ in review"`). No card-at-the-top; the parent channel
owns the card. `useTaskEventLog` is simplified to emit per-event rows instead
of deriving a card state.

### Task detail overlay + kanban (`TasksPanel`)

Both stay. The detail overlay is the home for secondary actions and full
event history. The kanban stays 4-column (`todo / in_progress / in_review /
done`): **proposed and dismissed tasks do not appear on the board**. The
board is for committed work. `getTasks(conversationId, status)` filter
defaults to `todo|in_progress|in_review|done` for kanban; feed reads and
detail reads use the full enum.

## Action Surface

### HTTP endpoints (post-merge)

```
POST  /api/conversations/:id/tasks           create (human, direct)   → status=todo
POST  /internal/agent/:agent/channels/:ch/tasks  create (agent)       → status=proposed (snapshot required)
GET   /api/conversations/:id/tasks           list (filterable by status)
GET   /api/conversations/:id/tasks/:number   detail
POST  /api/conversations/:id/tasks/:number/status   body: { status }   advance lifecycle
POST  /api/conversations/:id/tasks/:number/claim    sets owner = caller
POST  /api/conversations/:id/tasks/:number/unclaim  clears owner
```

One status endpoint with server-side transition validation, not six
action-named endpoints. Matches today's `updateTaskStatus` precedent and the
existing convention of keying tasks by `(conversation_id, task_number)` —
not UUID. The state machine lives in the Rust store.

### Transition graph (forward-only v1)

```
proposed ──┬── todo ── in_progress ── in_review ── done
           └── dismissed
```

- `proposed → todo`: mints sub-channel, posts kickoff, no `task_event`.
- `proposed → dismissed`: pure state mutation, no sub-channel, no event
  message. Parent card re-renders via SSE.
- `todo → in_progress → in_review → done`: each fires a `task_event` system
  message in the sub-channel. Done triggers parent-card pill-collapse.

No reverse transitions in v1. Reverts happen via unclaim + reassign or via
creating a fresh task.

### Permissions

Every endpoint requires channel membership of the caller (preserved from PR
#96's `require_channel_membership`). Beyond that, any member can advance any
transition — owner is a label, not a gate.

### MCP tools (agent surface)

```
create_task          { channel, title, source_message_id } → status=proposed
accept_task          { channel, task_number }              → proposed → todo
dismiss_task         { channel, task_number }              → proposed → dismissed
claim_task           { channel, task_number }
unclaim_task         { channel, task_number }
advance_task_status  { channel, task_number, status }
```

All MCP tools key tasks by `(channel, task_number)` — matches existing
`Backend` trait in `src/bridge/backend.rs` and the public HTTP API. The UUID
`task_id` is surfaced to the **UI** (list responses, `task_card` wire payload)
because the client store needs a stable primary key. It is **not** surfaced
to **agents / MCP tools** — they use `(channel, task_number)` exclusively.

One tool per action (CLAUDE.md: "one thing, done well"). Agents always go
through the proposal gate — `create_task` always produces `status =
'proposed'`. Humans-via-UI can direct-create in `todo` via the non-internal
HTTP route.

## Error Model

| Code | Condition | Preserved from |
|------|-----------|----------------|
| 400  | Invalid status in body; snapshot fields not all-or-none | PR #96 CHECK |
| 403  | Caller not a channel member | PR #96 R1 |
| 404  | Task id not found | new |
| 409  | Source message id points to another channel's message | PR #96 |
| 422  | Invalid state transition (e.g. `proposed → in_review`) | new |

Concurrency: last-writer-wins. SSE bounces authoritative state back to every
client. No OCC in v1.

## PR #93 / PR #96 Impact

### Survives (moves onto the merged `tasks` surface)

- Snapshot CHECK constraint (all-or-null on five fields)
- RFC3339 normalization helper (`normalize_sqlite_timestamp`)
- Membership precondition (`require_channel_membership`) on every mutating
  endpoint
- `ON DELETE SET NULL` pointer-vs-truth invariant on `source_message_id`
- Kickoff ordering contract (title + attribution + blockquote)
- TSK-005 Playwright smoke (rewired endpoints, same end-to-end assertions)

### Rewritten / renamed

- `task_proposals` table: dropped (data migrated into `tasks`).
- `task_proposal` wire kind: renamed to `task_card`.
- `TaskProposalMessage.tsx` + `useTaskProposalLog.ts`: replaced by
  `TaskCard.tsx` + `useTask(id)`.
- `TaskEventMessage.tsx` + `useTaskEventLog.ts` card-deriving reducer:
  replaced by `TaskEventRow.tsx` + per-event reducer.
- Action-named HTTP routes (`POST /task-proposals/:id/accept`, `/dismiss`):
  collapsed into `POST /api/conversations/:id/tasks/:number/status`.
- MCP tools: `propose_task` → `create_task`; `accept_task_proposal` →
  `accept_task`; `dismiss_task_proposal` → `dismiss_task`.

### Data migration

None required. Neither PR #93 nor PR #96 is in prod. Dogfood SQLite files
will be wiped. Dev-local only.

## Ship Strategy

- Close PR #93 and PR #96 with a pointer to the new branch.
- Branch `feat/task-lifecycle` off `main`, **not** stacked on either old
  branch.
- Single PR. Splitting the rename + table collapse into two PRs creates
  intermediate broken states that would require a throwaway compatibility
  layer.

## Testing Strategy

| Layer | What to test | Status |
|-------|--------------|--------|
| `tests/store_tests.rs` | Create in `proposed` (with snapshot) + `todo` (direct); every forward transition; invalid transitions → 422; CHECK constraint; pointer-vs-truth; claim/unclaim | Migrated + extended |
| `tests/e2e_tests.rs` | Full `proposed → done` happy path; `proposed → dismissed`; non-member 403 on every mutation; sub-channel mint only on `→ todo`; concurrent mutations resolve | Migrated + extended |
| Vitest | `TaskCard` renders each of 6 statuses; done → pill; dismissed → muted; `useTask` subscribes to SSE; `MessageList` routes `task_card` kind | Mostly new |
| TSK-005 Playwright | Rewire to `/tasks`; same propose → accept → deep-link → kickoff-ordering assertions | Migrated |
| TSK-006 Playwright (new) | Full lifecycle: claim → start → send for review → mark done → pill collapse | New |

## Out of Scope (v1)

Explicit non-goals, each queued as a v2 ticket driven by its own incident:

- Reverse transitions (`in_review → in_progress`, `done → reopen`,
  `dismissed → proposed`)
- Concurrent-mutation OCC (today: last-writer-wins + SSE rebroadcast)
- Permission gates beyond channel membership (today: any member can advance
  any state; owner is display)
- Cross-channel task aggregation on the kanban (today: per-channel)
- "Task from message" UI for humans (today: only agents propose from a
  source message; humans direct-create without provenance)
- Backfill for any production data (none exists)

## Open Questions (resolve during plan)

- **SSE widening shape.** Separate `/events/tasks` stream vs. a well-known
  global channel id piggy-backed on the existing SSE endpoint. Both work;
  the plan picks one and justifies it in one line.
- **`task_update` payload minimum fields.** `status + owner + sub_channel_id`
  is the minimum for re-render. Do we also send `updated_at` for client-side
  clock-skew ordering? Nice-to-have, not load-bearing — defer unless clients
  need ordering.
- **CTA wording.** `[create]` / `[dismiss]` stay (already shipped). New
  verbs: `[claim]`, `[start]`, `[send for review]`, `[mark done]`. Reviewable
  during implementation; not load-bearing for the data model.
