# IM Event Push Architecture Design

Date: 2026-03-27
Status: Approved design draft

## Goal

Refactor Chorus messaging from snapshot polling into a production-ready, Slack-like realtime IM architecture while keeping the MCP bridge contract unchanged.

The target is not a demo UI. The target is a durable IM system with ordered delivery, replay after reconnect, explicit unread semantics, scalable fanout, and browser-native push transport.

## Current State

### Browser UI

- The production app mounts `Sidebar` and `MainPanel` from `ui/src/App.tsx`.
- Global workspace state is refreshed on a fixed interval in `ui/src/store.tsx`.
- Channel and thread message history are polled every two seconds in `ui/src/hooks/useHistory.ts`.
- Browser sends use `/internal/agent/{username}/send` and history reads use `/internal/agent/{username}/history`.
- The browser does not use the bridge receive loop.

### Backend Messaging

- `handle_send` resolves the target, persists the message, then fans out to agents.
- `Store::send_message` writes the message row, advances the sender read position, and emits an in-memory broadcast wake signal.
- `handle_receive` uses the broadcast channel only as an agent wake mechanism for long-poll delivery.
- `handle_history` is a snapshot API backed by SQLite history queries.
- The existing broadcast channel is not durable and is not sufficient for browser realtime correctness.

## Approaches Considered

### 1. Transport-only push

Add browser push on top of the current polling-oriented data model.

Pros:

- Smallest code change
- Faster to prototype

Cons:

- Weak reconnect behavior
- No durable replay
- Keeps polling-era state assumptions
- Not a production-grade IM foundation

### 2. Durable event log plus browser push

Add a durable event stream while keeping `messages` as the canonical content store.

Pros:

- Stronger ordering and replay
- Better browser correctness
- Lower risk than a broader platform shift

Cons:

- Can split message truth and event truth if not designed carefully
- Often evolves into option 3 anyway

### 3. Slack-like realtime platform

Treat IM as an event-driven platform end to end: durable event log, realtime gateway, subscription model, replay cursors, explicit unread state, presence, typing, thread updates, and incremental client state.

Pros:

- Correct long-term architecture
- Supports browser push, reconnect, unread, presence, threads, and activity cleanly
- Removes snapshot polling as the steady-state model

Cons:

- Highest implementation complexity
- Requires disciplined event design and migration sequencing

## Recommendation

Choose approach 3 and ship it in phases.

The architecture should be designed once at the platform level, but rolled out incrementally:

1. durable event model
2. realtime gateway and browser subscriptions
3. message and thread event delivery
4. unread and sidebar state
5. presence, typing, and secondary realtime surfaces

The MCP bridge remains unchanged. It is infrastructure, not the product transport surface for browser IM.

## Architecture Vision

Chorus should behave like a realtime collaboration system rather than a page that replaces snapshots on a timer.

The browser opens a long-lived realtime session to the backend and subscribes to one or more scopes:

- workspace
- channel
- dm
- thread
- user
- agent

The backend persists domain state and durable events atomically, then fans those events out to live subscribers. Reconnecting clients resume from their last received event cursor and replay missed events in order. If replay is not possible, the server instructs the client to do a scoped resync.

This allows chat, threads, unread badges, sidebar counts, member changes, presence, typing, agent activity, and future inbox-like workflows to use one delivery model.

## Realtime Layers

### 1. Command handlers

Accept state-changing commands such as:

- send message
- send thread reply
- mark read
- join or leave channel
- open DM
- start or stop typing

### 2. Transactional write model

Each command writes domain rows and appends one or more durable events in the same transaction.

### 3. Fanout and subscription layer

Routes committed events to interested subscribers based on scope and authorization.

### 4. Realtime gateway

Prefer WebSocket as the primary transport. It must handle:

- authentication
- subscribe and unsubscribe
- resume from cursor
- heartbeats
- bounded outbound queues
- resync control frames

SSE can be added later as a fallback transport if needed.

### 5. Client state reducer

The browser should bootstrap from a snapshot once, then maintain normalized state by applying events incrementally.

## Event Model

### Durable domain events vs transport frames

Split realtime into two layers:

- domain events: durable, ordered, replayable facts about product state
- transport frames: websocket control messages for subscribe, ack, heartbeat, replay, and resync

Only domain events are persisted.

### Event envelope

Each durable event should use one stable envelope:

```json
{
  "eventId": 18442,
  "eventType": "message.created",
  "scopeKind": "channel",
  "scopeId": "channel:all",
  "channelId": "uuid",
  "channelName": "all",
  "threadParentId": null,
  "actor": {
    "name": "alice",
    "type": "human"
  },
  "causedBy": {
    "kind": "send_message"
  },
  "occurredAt": "2026-03-27T11:22:33Z",
  "payload": {}
}
```

### Event envelope rules

- `eventId` is globally monotonic within the workspace.
- `eventType` is explicit and stable.
- `scopeKind` and `scopeId` define subscription routing.
- `payload` is additive-only.
- Events are immutable after commit.
- Events are idempotent by `eventId`.
- A single command may emit multiple events.
- All events from one command commit atomically with the state change.

### Scope model

Use these scopes:

- `workspace`
- `channel`
- `dm`
- `thread`
- `user`
- `agent`

One command may fan events out to multiple scopes. Example: a thread reply can update the thread feed, the parent conversation reply badge, and one or more user unread counters.

### Conversation identity

Use stable ids, not parsed target strings:

- channels: `channel:<channel_id>`
- dms: `dm:<channel_id>`
- threads: `thread:<parent_message_id>`

The existing string targets remain for compatibility, but event routing should use stable ids.

## Event Catalog

### Message events

#### `message.created`

Emit when any top-level message, DM message, forwarded copy, system message, or thread reply is committed.

Used by:

- channel timeline
- DM timeline
- thread timeline
- optimistic-send reconciliation
- notifications
- unread derivation

Payload:

- `messageId`
- `conversationId`
- `conversationType`
- `threadParentId`
- `sender`
- `content`
- `attachments`
- `seq`
- `createdAt`
- `forwardedFrom`
- `clientNonce` when applicable

#### `message.edited`

Emit when persisted message content changes.

Used by:

- live message patching
- history consistency across devices

Payload:

- `messageId`
- `conversationId`
- `threadParentId`
- `content`
- `editedAt`
- `revision`

#### `message.deleted`

Emit when a message is hard-deleted or removed from normal rendering.

Used by:

- live timeline removal or replacement
- moderator and admin workflows

Payload:

- `messageId`
- `conversationId`
- `threadParentId`
- `deletedAt`
- `deleteMode`

#### `message.tombstone_changed`

Emit when the sender is soft-deleted but history remains visible.

Used by:

- live and historical rendering of deleted-agent messages

Payload:

- `messageId`
- `senderDeleted`
- replacement display metadata if needed

#### `message.delivery_failed`

Emit only to the sender scope when an optimistic browser send cannot be completed.

Used by:

- optimistic send rollback
- sender-side error banners

Payload:

- `clientNonce`
- `reasonCode`
- human-readable `message`

### Thread events

#### `thread.reply_count_changed`

Emit whenever reply count changes for a parent message.

Used by:

- top-level history cards
- channel message rows with reply badges

Payload:

- `parentMessageId`
- `conversationId`
- `replyCount`

#### `thread.participant_added`

Emit when a member becomes a thread participant for the first time.

In Chorus, this happens when the member authored the parent message or sends the first reply in that thread.

Used by:

- thread inbox membership
- participant indicators
- reply notification scoping

Payload:

- `parentMessageId`
- `participant`
- `reason`

#### `thread.activity_bumped`

Emit when the thread latest-activity pointer changes.

Used by:

- future thread inbox ordering
- thread recency indicators

Payload:

- `parentMessageId`
- `lastReplyAt`
- `lastReplyMessageId`

#### `thread.read_state_updated`

Emit when a thread-specific read cursor changes, if threads become independently unreadable objects.

Used by:

- thread inbox unread markers
- per-thread new-reply indicators

Payload:

- `parentMessageId`
- `member`
- `lastReadReplySeq` or `lastReadEventId`

### Conversation and membership events

#### `conversation.created`

Emit when a new channel, DM, or team-backed room is created.

Used by:

- sidebar insertion
- routing
- conversation discovery

Payload:

- `conversationId`
- `conversationType`
- `name`
- `displayName`
- `description`
- initial metadata

#### `conversation.updated`

Emit when mutable conversation metadata changes.

Used by:

- sidebar labels
- chat headers
- settings views

Payload:

- changed metadata fields only
- canonical identifiers

#### `conversation.archived`

Emit when a conversation becomes inactive but history remains.

Used by:

- sidebar removal or dimming
- read-only enforcement

Payload:

- `conversationId`
- `archivedAt`
- `visibilityPolicy`

#### `conversation.member_added`

Emit when a human or agent is added to a conversation.

Used by:

- member lists
- sidebar visibility
- delivery eligibility

Payload:

- `conversationId`
- `member`
- `role`
- `addedBy`

#### `conversation.member_removed`

Emit when a member leaves or is removed.

Used by:

- member list updates
- sidebar pruning
- authorization invalidation

Payload:

- `conversationId`
- `member`
- `removedBy`
- `reason`

#### `dm.opened`

Emit when a DM is first materialized.

Used by:

- recent DM lists
- sidebar insertion
- audit of DM creation

Payload:

- `conversationId`
- the two participants

### Read and unread events

#### `conversation.read_state_updated`

Emit when a member explicitly advances read position for a channel or DM.

Used by:

- unread badge updates
- new-message separators
- cross-device read consistency

Payload:

- `conversationId`
- `member`
- `lastReadSeq`
- `lastReadMessageId`
- `lastReadEventId`

#### `user.unread_changed`

Emit to user-scoped subscriptions when the server-derived unread count changes.

Used by:

- sidebar badges
- inbox-like filters

Payload:

- `conversationId`
- `unreadCount`
- `mentionCount`
- `threadUnreadCount` if supported

#### `user.mention_unread_changed`

Emit when mention unread count changes specifically.

Used by:

- high-priority badges
- mention filters

Payload:

- `conversationId`
- `mentionCount`

### Presence and typing events

#### `presence.updated`

Emit when a human or agent changes online state, active state, or last-seen freshness.

Used by:

- sidebar status dots
- profile panels
- operator awareness

Payload:

- `subject`
- `presence`
- `activity`
- `detail`
- `expiresAt` for ephemeral states

#### `typing.started`

Emit when a participant begins typing in a conversation or thread.

Used by:

- live typing indicators

Payload:

- `conversationId` or `threadParentId`
- `member`
- `expiresAt`

#### `typing.stopped`

Emit when typing ends or is canceled.

Used by:

- clearing typing indicators

Payload:

- same identity fields as `typing.started`

### Agent and system events

#### `agent.activity_updated`

Emit when an agent changes activity state such as `online`, `thinking`, `working`, or `offline`.

Used by:

- sidebar agent rows
- profile and activity panels

Payload:

- `agentName`
- `status`
- `activity`
- `detail`

#### `agent.session_resumed`

Emit when an agent process resumes from persisted session state.

Used by:

- runtime observability
- operator diagnostics

Payload:

- `agentName`
- `runtime`
- `sessionId`

#### `agent.message_delivery_requested`

Emit when a committed message causes Chorus to notify or wake agents.

Used by:

- observability
- delivery debugging

Payload:

- `messageId`
- `recipients`
- `deliveryMode`

#### `system.notice_posted`

Emit for server-authored notices such as swarm quorum lines or other system announcements.

Used by:

- live chat timelines
- audit tooling

Payload:

- `messageId`
- `noticeKind`
- `conversationId`

### Cross-channel and team events

#### `team.message_forwarded`

Emit when an `@team` mention mirrors a message into a team room.

Used by:

- provenance display
- forwarding observability

Payload:

- `sourceConversationId`
- `sourceMessageId`
- `targetConversationId`
- `forwardedMessageId`

#### `team.consensus_state_changed`

Emit when team coordination state changes, such as quorum reached or execution opened.

Used by:

- team room status
- coordination views
- audit history

Payload:

- `teamId`
- `state`
- `triggerMessageId`
- `resolvedAt`

## Expected Multi-Event Sequences

### Human sends a normal channel message

Emit:

- `message.created`
- `user.unread_changed` for affected humans
- `agent.message_delivery_requested` if any agents are recipients

### Agent replies in a thread

Emit:

- `message.created`
- `thread.reply_count_changed`
- `thread.activity_bumped`
- `thread.participant_added` if first reply by that member
- unread-change events for affected participants
- `agent.activity_updated` if runtime activity changed

### Channel rename

Emit:

- `conversation.updated`

Emit `user.unread_changed` only if badge labels or visibility logic depends on that metadata.

### `@team` forwarding

Emit:

- source `message.created`
- `team.message_forwarded`
- target-room `message.created`
- any delivery and unread derived events for the target room

## Events Not Worth Persisting In V1

Do not persist:

- draft text changes
- cursor position changes
- hover and focus state
- local panel open or close state

These are UI-local concerns, not durable product facts.

## Subscription And Delivery Direction

The next design step should define:

- which scopes receive each event
- which events are workspace-scoped vs conversation-scoped vs user-scoped
- replay and resume behavior
- backpressure and resync rules
- how browser read semantics and agent queue semantics coexist
