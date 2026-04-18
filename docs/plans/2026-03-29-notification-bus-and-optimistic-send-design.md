# Notification Bus And Optimistic Send Design

Date: 2026-03-29
Status: Approved design draft

## Goal

Refactor Chorus messaging so the websocket becomes a single long-lived notification bus instead of a message-content transport, while preserving fast chat UX through active-room fetches and optimistic send state.

The target behavior is:

- maintain durable read and unread state for channels, DMs, and threads
- keep one long-lived event bus for notification and structural events
- remove message bodies and attachments from that bus
- fetch messages on demand when notifications say a room changed
- show pending, acknowledged, and failed send state for human messages

## Approved Decisions

### Absolute notification state

Notification events carry absolute room state, not deltas and not invalidation-only payloads.

Example:

- `conversationId`
- `target`
- `latestSeq`
- `lastReadSeq`
- `unreadCount`
- `lastMessageId`
- `lastMessageAt`

This is safer than `+1` or `+2` style deltas because reconnect, dedupe, and read-state convergence become straightforward.

### Active-room fetch strategy

When the currently open channel, DM, or thread receives a notification and `latestSeq` advanced, the client fetches incremental messages since its loaded max sequence.

Inactive rooms do not fetch message bodies. They update only unread badges and latest pointers.

### Read cursor semantics

Read state advances by viewport visibility, not by room-open.

That means Chorus should only mark messages as read when the room is focused and the messages are actually visible in the chat or thread viewport.

### Thread unread semantics

A new thread reply increments both:

- the parent conversation unread state
- the thread unread state

Threads therefore have their own unread boundary, while parent conversations still reflect reply activity.

### Human send acknowledgement

Server acknowledgement means:

- the message is durably stored
- it has a real `message_id`
- it has a real `seq`

Acknowledgement does not wait for websocket fanout or client render.

## Approaches Considered

### 1. Trim the current bus but keep the current client model

Keep the existing websocket and history shape, but remove only the message body payloads.

Pros:

- smallest migration
- lower code churn

Cons:

- unread and read state remain secondary concerns
- active-room fetch behavior stays implicit
- optimistic send still feels bolted on

### 2. Notification bus plus active-room fetch

Use one long-lived websocket carrying notification and structural events only. The active room fetches incremental messages on notification. Inactive rooms update badges only.

Pros:

- matches the desired UX
- removes message-content duplication from the bus
- keeps history as the only message-content truth
- fits the existing stream-aware backend direction

Cons:

- requires frontend state reshaping
- requires explicit read-cursor updates and fetch coordination

### 3. Immediate full-shell unification

Move every shell surface, message surface, read-state surface, and structural surface to the new notification model in one pass.

Pros:

- cleanest theoretical end-state

Cons:

- too much migration at once
- higher regression risk than necessary

## Recommendation

Choose approach 2.

It keeps the good parts of the current work:

- one websocket
- durable stream identity
- explicit inbox-style read state

while removing the part you do not want:

- message content on the bus

## Target Architecture

## 1. One Long-Lived Notification Bus

The browser opens one websocket for the workspace session.

That socket carries:

- conversation notification state
- thread notification state
- structural conversation events
- structural team and agent events

It does not carry message bodies.

### Notification event families

#### `conversation.state`

Absolute state for a channel or DM.

Example payload:

```json
{
  "conversationId": "ch_123",
  "target": "#all",
  "latestSeq": 82,
  "lastReadSeq": 77,
  "unreadCount": 5,
  "lastMessageId": "msg_82",
  "lastMessageAt": "2026-03-29T09:00:00Z"
}
```

#### `thread.state`

Absolute state for one thread.

Example payload:

```json
{
  "conversationId": "ch_123",
  "threadParentId": "msg_55",
  "latestSeq": 12,
  "lastReadSeq": 8,
  "unreadCount": 4,
  "lastReplyMessageId": "msg_67",
  "lastReplyAt": "2026-03-29T09:00:00Z"
}
```

#### Structural events

Keep these on the bus:

- `conversation.membership_changed`
- `conversation.archived`
- `conversation.deleted`
- `team.updated`
- `agent.updated`

These are metadata and structure events, not message-content events.

## 2. Message Content Comes Only From History APIs

The source of truth for chat bodies stays in history reads.

The client fetches message bodies through:

- initial history bootstrap on room open
- incremental `after=<loadedMaxSeq>` fetches when notification state says the active room advanced

The bus becomes:

- stable
- smaller
- easier to reason about

The history API becomes the only place that shapes message content.

## 3. Read And Unread State

Chorus should maintain separate durable read state for:

- conversation unread
- thread unread

The client must send read-cursor updates based on viewport visibility.

### Conversation rule

If the active channel or DM viewport visibly contains messages through sequence `N`, the client may report `lastReadSeq = N`.

### Thread rule

If the active thread viewport visibly contains replies through sequence `N`, the client may report the thread read cursor through `N`.

### Parent-thread interaction

When a thread reply arrives:

- parent conversation unread increases
- thread unread increases

When the thread is read:

- thread unread decreases to zero or the correct remaining count
- parent conversation unread also reflects that the reply is no longer unread for that user

The projection layer must keep those two counters consistent.

## 4. Active Versus Inactive Room Behavior

### Active room

When a `conversation.state` or `thread.state` event arrives for the currently open target:

1. compare `latestSeq` against the local max sequence
2. if higher, fetch incremental messages
3. merge returned messages into local history
4. update local read/unread state

### Inactive room

When the event is for another room:

- update unread badge
- update latest pointer
- do not fetch message bodies

This preserves good UX without letting notification traffic explode into background content fetches.

## 5. Optimistic Human Send

When a human sends a message:

1. insert a temporary local message row immediately
2. mark it `sending`
3. show a loading icon beside it
4. submit the send request

### Success

On durable ack:

- replace temp id with real `message_id`
- store real `seq`
- clear the loading indicator

### Failure

On failure:

- keep the message row in place
- mark it `failed`
- show a fail toast
- allow retry

This model gives fast UX while preserving backend truth.

## Data Flow

### Incoming message to active room

1. backend stores the message
2. backend emits `conversation.state` and maybe `thread.state`
3. active client sees `latestSeq` advanced
4. client fetches incremental messages
5. client renders new messages
6. if visible, client advances read cursor

### Incoming message to inactive room

1. backend stores the message
2. backend emits notification state
3. client updates unread badge only
4. no body fetch until the user opens the room

### Human send

1. UI inserts pending row
2. UI posts send command
3. server durably stores the message and returns real id and seq
4. UI reconciles pending row immediately from ack
5. bus notifies other clients and inactive surfaces

## API And Transport Changes

### Keep

- one websocket endpoint
- existing history APIs, extended where needed for incremental fetch and read metadata

### Change

- websocket payloads stop carrying message content
- send response should include enough information to reconcile optimistic rows cleanly
- read-cursor update command or endpoint becomes explicit

## Frontend State Changes

The frontend should split:

- room notification state
- loaded message history per active room
- pending local sends

That means:

- sidebar state can stay light and notification-driven
- chat panels can fetch bodies only when active
- optimistic rows can exist independently from durable history until ack

## Risks

### Read cursor complexity

Viewport-based read state is correct but more complex than mark-on-open. It needs debounce, focus-awareness, and precise visible-range calculation.

### Thread and parent unread consistency

Dual unread increments for thread replies require careful projection logic and tests.

### Optimistic reconciliation

Pending rows need strong correlation between client intent and server ack. A stable client nonce is the simplest way to achieve that.

## Rollout Strategy

1. introduce notification event types alongside current bus payloads
2. add frontend notification state reducer
3. make active rooms fetch incrementally on notification
4. add optimistic send state for human messages
5. add explicit viewport-based read reporting
6. stop sending message content on the bus
7. remove legacy content-driven bus handling

## Verification Requirements

At minimum, the implementation must prove:

- unread state is durable for channels, DMs, and threads
- inactive rooms update badges without fetching bodies
- active rooms fetch incrementally on notification
- optimistic sends reconcile on success
- failed sends remain visible and retryable
- thread replies update both parent and thread unread state

