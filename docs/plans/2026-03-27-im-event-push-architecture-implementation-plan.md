# IM Event Push Architecture Implementation Plan

Date: 2026-03-27
Status: Draft implementation plan
Related design: `docs/plans/2026-03-27-im-event-push-architecture-design.md`

## Objective

Implement the approved production-grade IM event push architecture in phases without breaking the current MCP bridge behavior or the existing browser messaging surface.

## Constraints

- Keep the MCP bridge contract unchanged.
- Preserve the current `send` and `history` APIs during migration.
- Do not regress message ordering, DM behavior, thread behavior, team forwarding, or agent wake semantics.
- Verification must include focused Rust tests, backend integration coverage, and browser QA for user-visible critical flows.

## Phase 0: Baseline and guardrails

### Goals

- Lock in current behavior before refactor.
- Expand test coverage where the new architecture is most likely to regress existing IM semantics.

### Tasks

- Audit current tests covering:
  - channel messaging
  - DM messaging
  - thread replies
  - team forwarding
  - agent wake and notify behavior
  - history pagination
- Add targeted tests for current behavior gaps if needed:
  - reply count correctness after thread replies
  - forwarded provenance visibility in history
  - membership edge cases for DM and team channels
- Document migration invariants directly in tests where practical.

### Exit criteria

- Existing IM semantics are captured by tests strongly enough to refactor safely.

## Phase 1: Durable event storage

### Goals

- Introduce a durable event log without changing browser behavior yet.

### Backend tasks

- Add an `events` table with:
  - global monotonic `event_id`
  - event type
  - scope kind
  - scope id
  - conversation and thread identifiers as needed
  - actor metadata
  - payload JSON
  - created timestamp
- Add store APIs to append events transactionally with domain writes.
- Update message-writing paths to append initial events:
  - `message.created`
  - `thread.reply_count_changed`
  - `thread.activity_bumped`
  - `team.message_forwarded`
  - `system.notice_posted`
  - `agent.message_delivery_requested` if used as durable observability
- Keep existing broadcast wake behavior intact.

### Verification

- Store tests for event append ordering and atomicity.
- Handler tests to verify message commands create the expected event sequences.

### Exit criteria

- Every relevant message mutation writes durable events in the same transaction.

## Phase 2: Realtime gateway

### Goals

- Add browser-facing push transport and replay semantics without yet removing polling.

### Backend tasks

- Add a websocket endpoint for authenticated browser realtime sessions.
- Support:
  - subscribe
  - unsubscribe
  - resume from event cursor
  - heartbeat
  - resync required control messages
- Build a subscription registry keyed by scope.
- Add replay queries:
  - by global event cursor
  - by scope
- Implement bounded outbound queues and drop-to-resync behavior for slow clients.

### Verification

- Integration tests for:
  - subscribe and receive
  - disconnect and resume
  - replay after missed events
  - unauthorized subscription rejection
  - slow-consumer fallback

### Exit criteria

- A browser client can receive durable event replay and live fanout from one socket.

## Phase 3: Browser workspace subscription

### Goals

- Replace workspace polling first, before replacing message history polling.

### UI tasks

- Add a realtime client module with:
  - socket lifecycle
  - resume cursor persistence
  - subscription management
  - event dispatch
- Refactor `ui/src/store.tsx` to own the workspace subscription.
- Move these surfaces to event-driven updates:
  - channels list
  - teams list
  - agent activity and status
  - presence dots
  - conversation metadata updates

### Migration strategy

- Keep the five-second polling path as a fallback behind a feature flag or guarded retry path.
- Start with push-preferred, poll-fallback behavior.

### Verification

- Browser QA for:
  - sidebar updates after create or rename
  - agent status changes
  - no duplicate rows after reconnect

### Exit criteria

- Workspace sidebar state updates from push in steady state.

## Phase 4: Channel and DM message push

### Goals

- Replace two-second main chat polling with snapshot plus incremental events.

### UI tasks

- Replace `useHistory` polling with:
  - initial history bootstrap
  - normalized message store
  - incremental apply of `message.created`
  - incremental apply of metadata events like tombstones and reply counts
- Reconcile optimistic sends using `clientNonce`.
- Preserve current scroll behavior with incremental append semantics.

### Backend tasks

- Add history bootstrap plus event cursor handshake for selected conversation.
- Emit user-scoped unread events for humans as needed by the sidebar.

### Verification

- Browser QA for:
  - human sends message and sees immediate reconciliation
  - agent reply appears without polling
  - DM message ordering remains correct
  - page refresh plus reconnect preserves state

### Exit criteria

- Main chat panel no longer polls for live updates.

## Phase 5: Thread push

### Goals

- Move thread panel from polling to incremental realtime updates.

### UI tasks

- Add thread-scoped subscriptions when the thread panel opens.
- Apply:
  - `message.created` for replies
  - `thread.reply_count_changed`
  - `thread.activity_bumped`
  - `thread.participant_added` if exposed
- Preserve current parent-message display model.

### Verification

- Browser QA for:
  - open thread and receive live replies
  - reply count updates in parent channel
  - refresh and reconnect restore thread state

### Exit criteria

- Thread panel no longer polls for live replies.

## Phase 6: Read and unread semantics

### Goals

- Replace implicit browser unread behavior with explicit human read-state updates.

### Backend tasks

- Add explicit read-state mutation endpoint or websocket command.
- Emit:
  - `conversation.read_state_updated`
  - `user.unread_changed`
  - `user.mention_unread_changed` when applicable
- Keep agent `/receive` semantics unchanged for now.

### UI tasks

- Mark read based on explicit visibility policy, not fetch timing.
- Update sidebar badges from user-scoped unread events.

### Verification

- Browser QA for:
  - unread badge increments while away from a room
  - unread clears when opening and reading
  - reconnect does not double-count unread

### Exit criteria

- Human unread state is explicit, event-driven, and cross-session consistent.

## Phase 7: Presence and typing

### Goals

- Add ephemeral collaboration signals after core message correctness is stable.

### Tasks

- Add websocket commands for typing and presence heartbeats.
- Emit and route:
  - `presence.updated`
  - `typing.started`
  - `typing.stopped`
- Define TTL and expiry cleanup rules.

### Verification

- Browser QA for typing visibility and cleanup after disconnect.
- Reliability QA for stale presence expiration.

### Exit criteria

- Presence and typing are live, bounded, and non-disruptive.

## Phase 8: Cleanup and deprecation

### Goals

- Remove now-obsolete polling paths and reduce duplicated state logic.

### Tasks

- Remove polling from:
  - `ui/src/hooks/useHistory.ts`
  - workspace refresh paths that are fully replaced by push
- Collapse duplicated history refresh logic in `MainPanel`.
- Keep bootstrap snapshot APIs for initial load and resync only.
- Update docs and QA coverage to reflect the new architecture.

### Exit criteria

- Push is the steady-state architecture.
- Snapshot APIs are used for bootstrap and recovery only.

## Data and API Work Summary

### New backend components

- event storage
- event append APIs
- websocket gateway
- subscription registry
- replay queries
- read-state command path

### Existing components to modify

- `src/store/messages.rs`
- `src/server/handlers/messages.rs`
- `src/store/mod.rs`
- UI app state in `ui/src/store.tsx`
- message and thread state hooks

## Verification Plan

### Rust and integration tests

- store tests for event ordering and replay
- handler tests for event emission sequences
- realtime gateway tests for subscribe, replay, auth, and resync

### Browser QA

Run real browser verification for:

- channel messaging
- DM flows
- thread replies
- unread updates
- reconnect recovery
- team forwarding visibility
- agent activity/sidebar updates

### Reliability QA

Run focused recovery tests for:

- tab sleep and reconnect
- backend restart during active session
- slow client queue overflow
- stale typing and stale presence cleanup

## Risks

- Splitting event truth from message truth if transactions are not disciplined
- Reconnect bugs creating duplicate or missing events in UI state
- Human unread semantics drifting from actual viewport behavior
- Backpressure mistakes causing memory growth or silent drops
- Scope routing bugs leaking private DM or thread events

## Recommended Order Of Execution

1. Phase 0
2. Phase 1
3. Phase 2
4. Phase 3
5. Phase 4
6. Phase 5
7. Phase 6
8. Phase 7
9. Phase 8

## Definition Of Done

The refactor is complete when:

- browser IM uses push as the steady-state transport
- reconnect resumes from durable event cursors
- channel, DM, and thread timelines update without polling
- human unread state is explicit and consistent
- agent bridge behavior remains compatible
- browser QA passes for core user-visible IM flows
