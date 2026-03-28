# Domain Multi-Stream Architecture Design

Date: 2026-03-28
Status: Proposed design

## Goal

Replace the current global IM event-backbone direction with a cleaner long-term architecture:

- no single global workspace stream as the product backbone
- no dual canonical truth between message rows and event payloads
- per-domain durable streams with domain-local ordering
- projections for chat history, sidebar state, thread summaries, team state, and agent state
- tasks remain relational state, not part of the canonical event-sourced core

This design assumes Chorus is evolving into a realtime collaboration system with channels, DMs, threads, team coordination, and long-lived agent sessions, rather than a polling chat UI with some event push on top.

## Current Problem

The current branch introduces a durable `events` table and websocket replay, but it still keeps `messages` as a separate canonical-looking store. That creates the wrong long-term shape:

- message history reads from the message table
- live updates materialize from denormalized event payloads
- replay is global-scan-and-filter rather than stream-local
- global event order is overused where only conversation-local order is actually needed

The core issue is not "transactions are bad". The core issue is "two truths and one global stream".

## Decision Summary

The long-term model should be:

- canonical durable streams by bounded context
- projections for read-side views
- ephemeral realtime signals for presence/typing/activity
- tasks kept out of the canonical event-sourced core

Canonical streams:

- `conversation:<id>`
- `team:<id>`
- `agent:<name>`
- `inbox:<user>`

Not canonical streams:

- tasks
- typing
- presence
- transient agent activity
- raw tool traces

## Design Principles

1. Strong ordering should exist only where the product actually needs it.
2. Chat ordering is conversation-local, not workspace-global.
3. Threads are a projection over conversation messages, not a separate source of truth.
4. Read/unread state is user-private state and should not be mixed into membership rows.
5. A global event sequence may still exist for observability and projector checkpoints, but it is not the product's canonical ordering model.
6. Cross-domain workflows should use causation and correlation ids instead of synchronous dual canonical writes.

## Bounded Contexts

### Conversation Domain

Owns:

- channels
- DMs
- team room messaging
- message lifecycle
- membership changes

Does not own:

- per-user unread state
- team coordination logic
- tasks

### Team Domain

Owns:

- team metadata
- team membership and roles
- team coordination model
- delegation/quorum semantics

Does not own:

- team room chat history

Team room chat belongs to the team's conversation stream.

### Agent Domain

Owns durable control-plane state:

- agent creation
- runtime selection
- session lifecycle
- durable delivery failures

Does not own:

- ephemeral "thinking"
- live tool execution traces
- transient UI activity states

### Inbox Domain

Owns per-user private state:

- read cursors
- mute state
- pin state
- future notification preferences

This replaces the current pattern where read state is mixed into conversation membership rows.

### Task Domain

Tasks remain relational state.

They do not become part of the canonical stream model. If realtime UI updates are needed, tasks can emit thin outbox/changefeed events, but tasks should not be forced into the append-first core.

## Canonical Streams

### `conversation:<id>`

This is the canonical stream for channels, DMs, and team rooms.

Threads do not get their own canonical streams. A thread reply is a normal conversation event with `parent_message_id` set.

Canonical events:

- `conversation.created`
- `member.joined`
- `member.left`
- `message.posted`
- `message.edited` if edits are added later
- `message.redacted`

### `team:<id>`

Canonical events:

- `team.created`
- `team.member_added`
- `team.member_removed`
- `team.model_set`
- `team.delegation_requested`
- `team.quorum_signaled`
- `team.quorum_reached`

The `team:<id>` stream is for coordination semantics only. It should not duplicate team room messages.

### `agent:<name>`

Canonical events:

- `agent.created`
- `agent.runtime_configured`
- `agent.session_started`
- `agent.session_resumed`
- `agent.session_stopped`
- `agent.delivery_marked_failed`

### `inbox:<user>`

Canonical events:

- `conversation.read_cursor_set`
- `conversation.muted`
- `conversation.unmuted`
- `conversation.pinned`
- `conversation.unpinned`

## What Is Not Event-Sourced

These should remain outside the canonical event model:

- `tasks`
- attachment binary blobs
- agent env vars and secrets
- typing
- presence
- agent "thinking" and raw execution activity

These either have simpler relational state needs or are too transient to justify durable canonical streams.

## Event Envelope

Every canonical event should use a stable envelope:

```json
{
  "global_seq": 18442,
  "event_id": "evt_123",
  "stream_id": "conversation:ch_123",
  "stream_pos": 42,
  "event_type": "message.posted",
  "actor": {
    "name": "alice",
    "type": "human"
  },
  "correlation_id": "req_abc",
  "causation_id": "cmd_send_abc",
  "payload": {},
  "created_at": "2026-03-28T12:00:00Z"
}
```

Rules:

- `stream_id + stream_pos` is the canonical ordering identity
- `global_seq` is only for projector progress, debugging, and broad observability
- `payload` is additive-only
- events are immutable
- events are idempotent by `event_id`

## Storage Model

### Stream Metadata

```sql
CREATE TABLE streams (
  stream_id TEXT PRIMARY KEY,
  stream_kind TEXT NOT NULL,
  aggregate_id TEXT NOT NULL,
  current_pos INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL
);
```

### Canonical Event Store

```sql
CREATE TABLE stream_events (
  global_seq INTEGER PRIMARY KEY AUTOINCREMENT,
  event_id TEXT NOT NULL UNIQUE,
  stream_id TEXT NOT NULL REFERENCES streams(stream_id),
  stream_pos INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  actor_name TEXT,
  actor_type TEXT,
  correlation_id TEXT,
  causation_id TEXT,
  payload TEXT NOT NULL,
  created_at TEXT NOT NULL,
  UNIQUE(stream_id, stream_pos)
);
```

### Projector Checkpoints

```sql
CREATE TABLE projection_checkpoints (
  projector_name TEXT PRIMARY KEY,
  last_global_seq INTEGER NOT NULL
);
```

## Projections

The following become read models, not write-time truth.

### Conversation Views

```sql
CREATE TABLE conversations_view (
  conversation_id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  conversation_type TEXT NOT NULL,
  archived INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL
);

CREATE TABLE conversation_members_view (
  conversation_id TEXT NOT NULL,
  member_name TEXT NOT NULL,
  member_type TEXT NOT NULL,
  PRIMARY KEY (conversation_id, member_name)
);

CREATE TABLE conversation_messages_view (
  message_id TEXT PRIMARY KEY,
  conversation_id TEXT NOT NULL,
  conversation_stream_pos INTEGER NOT NULL,
  parent_message_id TEXT,
  sender_name TEXT NOT NULL,
  sender_type TEXT NOT NULL,
  body TEXT NOT NULL,
  redacted INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL
);
```

### Thread Summary View

```sql
CREATE TABLE thread_summaries_view (
  conversation_id TEXT NOT NULL,
  parent_message_id TEXT NOT NULL,
  reply_count INTEGER NOT NULL DEFAULT 0,
  last_reply_message_id TEXT,
  last_reply_at TEXT,
  PRIMARY KEY (conversation_id, parent_message_id)
);
```

Threads remain projections over conversation events.

### Inbox View

```sql
CREATE TABLE inbox_conversation_state_view (
  user_name TEXT NOT NULL,
  conversation_id TEXT NOT NULL,
  last_read_stream_pos INTEGER NOT NULL DEFAULT 0,
  last_read_message_id TEXT,
  unread_count INTEGER NOT NULL DEFAULT 0,
  muted INTEGER NOT NULL DEFAULT 0,
  pinned INTEGER NOT NULL DEFAULT 0,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (user_name, conversation_id)
);
```

This replaces `channel_members.last_read_seq` as the long-term source of truth for read state.

### Team View

```sql
CREATE TABLE team_view (
  team_id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  display_name TEXT NOT NULL,
  collaboration_model TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE team_members_view (
  team_id TEXT NOT NULL,
  member_name TEXT NOT NULL,
  member_type TEXT NOT NULL,
  role TEXT NOT NULL,
  PRIMARY KEY (team_id, member_name)
);
```

### Agent Runtime View

```sql
CREATE TABLE agent_runtime_view (
  agent_name TEXT PRIMARY KEY,
  runtime TEXT,
  model TEXT,
  session_id TEXT,
  status TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
```

## Task Model

Tasks should remain relational:

```sql
CREATE TABLE tasks (
  id TEXT PRIMARY KEY,
  channel_id TEXT NOT NULL,
  task_number INTEGER NOT NULL,
  title TEXT NOT NULL,
  status TEXT NOT NULL,
  claimed_by TEXT,
  created_by TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
```

If the UI needs push updates, use a thin outbox or task changefeed:

```sql
CREATE TABLE task_outbox (
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  event_type TEXT NOT NULL,
  channel_id TEXT NOT NULL,
  task_id TEXT NOT NULL,
  payload TEXT NOT NULL,
  created_at TEXT NOT NULL
);
```

Tasks remain current-state-first and query-friendly. They do not need to become replay-driven canonical streams.

## Command Model

Write rules:

- send a chat message -> append to `conversation:<id>`
- mark a conversation read -> append to `inbox:<user>`
- change team coordination state -> append to `team:<id>`
- update durable agent control-plane state -> append to `agent:<name>`
- update a task -> write the `tasks` table directly, optionally emit a thin task outbox event

Cross-domain behavior should use causation ids rather than synchronous dual canonical writes.

Example:

- human message mentioning `@team-design`
- append `message.posted` to `conversation:<id>`
- consumer detects the mention and appends `team.delegation_requested` to `team:<id>` with `causation_id = source event id`

## Read Model Bootstrapping

Bootstrap must be stream-local and atomic.

### Open Conversation

1. Read `conversation_messages_view` for the requested window.
2. Read `streams.current_pos` for `conversation:<id>`.
3. Return both from the same DB read transaction.
4. Client subscribes from that `stream_pos`.

This avoids the current race shape where history is read first and the cursor is fetched later.

### Open Inbox / Sidebar

1. Read `inbox_conversation_state_view`.
2. Read `streams.current_pos` for `inbox:<user>`.
3. Return both from the same DB read transaction.
4. Client subscribes from that `stream_pos`.

## Realtime Transport

Subscriptions should be by domain scope:

- `conversation:<id>`
- `thread:<parent_message_id>` as a filtered projection scope
- `team:<id>`
- `agent:<name>`
- `inbox:<user>`

Important distinction:

- `thread:<parent_message_id>` is a subscription/view scope
- it is not a canonical write stream

The server may still track a global internal sequence for projector progress and catch-up control, but replay for product correctness should be stream-local wherever possible.

## Why This Is Better Than A Single Global Stream

1. Ordering matches product reality.
   Conversation order matters. Workspace-global total order usually does not.

2. Replay is cheaper.
   Reopening one channel should not require scanning all workspace traffic.

3. Authorization is cleaner.
   Access checks become local to the domain stream or projection scope.

4. Schemas evolve independently.
   Team coordination, inbox state, and chat history can change without sharing one event vocabulary.

5. Backpressure is isolated.
   Hot chat traffic should not distort agent lifecycle or inbox processing.

## Why This Is Better Than The Current Branch Direction

The current branch shape is useful as a stepping stone, but not as the final architecture.

What it gets right:

- durable event persistence
- websocket transport
- replay as a first-class concern

What it still gets wrong long-term:

- a global event spine is overemphasized
- event payloads duplicate message truth
- chat bootstrap and live updates do not share a single clean stream-local model

## Migration Strategy

### Phase 1: Stop Treating Event Payload As Canonical Message Truth

- keep `messages` canonical for now
- make websocket events thinner
- fix history bootstrap so messages and cursor come from the same read transaction

This stabilizes the current system before any larger architectural flip.

### Phase 2: Introduce Stream Identity

- add `stream_id` and `stream_pos` beside the current global event sequence
- map chat writes to `conversation:<id>`
- keep existing global `event_id` only for projector checkpoints and observability

### Phase 3: Convert Chat To Append-First

- `conversation:<id>` becomes canonical for messaging
- `conversation_messages_view` becomes the projected history table
- thread counters and summaries move fully to projections

### Phase 4: Move Read State Into Inbox

- introduce `inbox:<user>` stream
- replace `channel_members.last_read_seq` with inbox projection state

### Phase 5: Split Team Coordination Out Of Chat

- move quorum/delegation semantics into `team:<id>`
- keep team room chat in `conversation:<id>`

### Phase 6: Leave Tasks Relational

- keep current task model relational
- optionally add a task outbox if needed for push updates

## Immediate Recommendation From The Current Branch

If starting from the current realtime branch, the next practical steps should be:

1. Fix the history bootstrap race by returning stream-local cursor and history from one read transaction.
2. Stop using denormalized event payloads as the long-term message truth model.
3. Add `stream_id` and `stream_pos` to the event store.
4. Reframe replay around `conversation:<id>` instead of global-scan-and-filter.
5. Keep tasks out of the canonical event-sourced core.

## Non-Goals

This design does not attempt to:

- event-source tasks
- event-source binary attachments
- persist typing/presence as durable domain state
- preserve a single global total order as a product invariant

## Final Recommendation

The long-term architecture for Chorus should be:

- multi-stream by domain
- conversation-local ordering for messaging
- projections for history, sidebar, thread summaries, and read state
- relational tasks
- global sequence only as infrastructure, not as the main product model

That is a cleaner end-state than either:

- one giant workspace event stream
- or the current "messages plus duplicated event payload" dual-truth design
