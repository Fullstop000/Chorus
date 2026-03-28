# Domain Multi-Stream Migration Plan

Date: 2026-03-28
Status: Proposed implementation plan
Related design: `docs/DOMAIN_MULTI_STREAM_ARCHITECTURE.md`

## Objective

Move Chorus from the current global event-backbone direction toward a domain multi-stream architecture without breaking:

- existing agent bridge behavior
- current user-visible messaging flows
- thread reply behavior
- task board behavior
- team room behavior

This plan intentionally keeps tasks relational.

## Constraints

- preserve current HTTP APIs during migration where possible
- keep agent delivery semantics intact
- do not introduce a prolonged dual-canonical-write state
- prefer additive migrations and reversible phases
- verify core messaging behavior in browser QA before claiming the migration is complete

## Migration Invariants

These must remain true in every phase:

1. Sending a channel or DM message remains atomic from the user's perspective.
2. Thread replies remain ordered correctly within their parent conversation.
3. Agent wake/delivery behavior does not regress.
4. A client that reconnects after reading history must not miss committed messages.
5. Team room chat remains chat; team coordination semantics remain layered on top.
6. Tasks remain fully functional and are not coupled to the stream migration.

## Phase 0: Stabilize The Current Branch

### Goal

Fix the most dangerous correctness issues before introducing a new core model.

### Work

- make history bootstrap atomic:
  - return the message window and cursor from the same DB read transaction
  - stop returning a cursor that was fetched after the history snapshot
- reduce dependence on denormalized event payloads for live message truth
- add targeted tests for:
  - bootstrap + subscribe race
  - reconnect after live message arrival
  - thread reply visibility after reconnect

### Deliverable

The current branch becomes safe enough to refactor.

## Phase 1: Introduce Stream Identity

### Goal

Keep the existing event system alive, but start modeling events as belonging to explicit streams rather than only global scope keys.

### Schema

Add to the existing durable event storage:

- `stream_id`
- `stream_kind`
- `stream_pos`

Add a `streams` table:

- `stream_id`
- `stream_kind`
- `aggregate_id`
- `current_pos`

### Initial Mapping

- channel, DM, and team-room message events map to `conversation:<channel_id>`
- team coordination events map to `team:<team_id>`
- agent control-plane events map to `agent:<agent_name>`
- per-user read state events later map to `inbox:<user_name>`

### Work

- assign `stream_id` and increment `stream_pos` inside the same append path as the existing event write
- keep the existing global sequence for projector checkpoints and transport fallback

### Deliverable

Durable events carry both:

- a global append sequence for infrastructure
- a domain stream identity for product correctness

## Phase 2: Make Realtime Replay Stream-Local For Messaging

### Goal

Stop treating replay as "scan globally and filter by scope".

### Work

- add read APIs to fetch stream events by:
  - `stream_id`
  - `after stream_pos`
  - bounded page size
- change conversation subscription logic to use:
  - `stream_id = conversation:<id>`
  - `resume_from_stream_pos`
- keep the current global replay path only as a compatibility fallback for transitional consumers

### Deliverable

Message replay becomes conversation-local rather than workspace-global.

## Phase 3: Split History Projection From Canonical Append

### Goal

Move message writes toward append-first without immediately deleting the current message table.

### Work

- introduce `conversation_messages_view` as an explicit projection target
- treat the current `messages` table as transitional projection storage, not as the forever write model
- ensure all history reads for chat are projection reads
- ensure live updates are derived from stream events and converge to the same projected shape used by history reads

### Important Rule

At the end of this phase, the system should have:

- one canonical append path for messaging
- one projected history shape for messaging

It must not have two canonical message representations.

### Deliverable

Conversation history is clearly a projection over the conversation stream.

## Phase 4: Move Thread State Fully To Projections

### Goal

Remove thread-specific canonical writes beyond normal conversation message append.

### Work

- keep thread replies as `message.posted` events on `conversation:<id>` with `parent_message_id`
- project:
  - reply count
  - last reply metadata
  - thread participant summary
- keep `thread:<parent_message_id>` only as a subscription/read scope, not a canonical stream

### Deliverable

Threads are fully projection-based and conversation-local.

## Phase 5: Move Read State Into Inbox Domain

### Goal

Replace channel membership read cursors with user-private inbox state.

### Work

- introduce `inbox:<user>` stream
- append `conversation.read_cursor_set` when a user reads a conversation
- build `inbox_conversation_state_view`
- migrate unread/sidebar reads to that projection
- deprecate `channel_members.last_read_seq` for human-facing read state

### Deliverable

Read and unread state become inbox-domain state instead of conversation membership mutation.

## Phase 6: Split Team Coordination From Team Room Chat

### Goal

Separate collaboration protocol semantics from room messaging.

### Work

- keep team room chat on `conversation:<team-room-channel-id>`
- move quorum/delegation protocol events onto `team:<team_id>`
- drive team coordination UI/state from `team_view` and related projections
- keep cross-domain linkage via `causation_id`

### Deliverable

Team room chat and team coordination stop sharing one overloaded event model.

## Phase 7: Clean Up Transport And Scope Model

### Goal

Align websocket subscriptions with the new domain architecture.

### Work

- replace scope-first subscription internals with stream/projection-first subscription internals
- keep public subscription names human-meaningful:
  - `conversation:<id>`
  - `thread:<parent_message_id>`
  - `team:<id>`
  - `agent:<name>`
  - `inbox:<user>`
- treat `thread:<parent_message_id>` as a filtered projection scope over a conversation stream

### Deliverable

Transport semantics match the domain model.

## Phase 8: Freeze The Boundary For Tasks

### Goal

Avoid accidental scope creep and keep tasks simple.

### Work

- keep `tasks` canonical and relational
- if push is needed, emit thin task outbox rows or task changefeed rows
- do not fold tasks into the canonical stream runtime
- do not require task replay for correctness

### Deliverable

Tasks remain independent from the messaging stream core.

## Proposed API Evolution

### Transitional

Keep:

- `GET /history`
- websocket subscribe/resume behavior

But make them stream-aware under the hood.

### Target Shape

Conversation bootstrap should return:

- projected message window
- `conversation_stream_id`
- `conversation_stream_pos`

Inbox bootstrap should return:

- projected inbox/sidebar state
- `inbox_stream_id`
- `inbox_stream_pos`

Thread bootstrap should return:

- projected thread message window
- parent conversation stream identity
- thread projection cursor if needed

## Verification Plan

### Rust Tests

Add or tighten tests for:

- message append assigns monotonic `stream_pos` within a conversation
- replay by `conversation:<id>` after reconnect
- thread reply projection correctness
- read cursor updates through inbox events
- team coordination event separation from room chat
- task behavior unchanged

### Integration Tests

Add end-to-end server tests for:

- history bootstrap plus resume without message loss
- reconnect after several missed conversation events
- team room message plus delegation side effect with causation linkage
- agent lifecycle unaffected by stream model changes

### Browser QA

Required before calling the migration complete for any phase that touches user-visible chat:

- channel send and live receive
- DM send and live receive
- thread reply + reply count update
- refresh + reconnect without duplicate or lost messages
- sidebar unread consistency

## Risk Areas

### Projection Lag

If canonical append and projection reads diverge for too long, users see stale state. The implementation should start with synchronous or near-synchronous local projection updates before introducing heavier async decoupling.

### Dual-Write Regression

The migration must not settle into a permanent state where both:

- message table writes
- stream event writes

are treated as canonical. Transitional phases are acceptable, but the ownership boundary must stay explicit.

### Over-Modeling Threads

Making threads their own canonical stream would complicate ordering and replay. Keep them as conversation-local message projections.

### Scope Creep Into Tasks

Tasks should not be silently absorbed into the event-sourced core just because the infrastructure exists.

## Recommended Immediate Work Items

From the current branch, the next concrete implementation sequence should be:

1. Fix atomic bootstrap for history + cursor.
2. Add `stream_id` and `stream_pos` to the durable event store.
3. Replay conversations by stream instead of global-scan-and-filter.
4. Introduce explicit conversation history projection naming and code boundaries.
5. Move human read state planning toward `inbox:<user>`.
6. Leave tasks untouched except for compatibility testing.

## Completion Criteria

This migration is complete when:

- messaging is append-first on `conversation:<id>`
- history is a projection, not a second canonical write target
- replay is stream-local for messaging
- thread state is projection-only
- read state is owned by `inbox:<user>`
- team coordination is separated from team room chat
- tasks remain relational and unaffected
