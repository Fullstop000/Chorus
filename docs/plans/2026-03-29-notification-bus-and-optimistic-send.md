# Notification Bus And Optimistic Send Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace message-content websocket delivery with an absolute-state notification bus, add durable read/unread handling for conversations and threads, and add optimistic human send UX.

**Architecture:** The websocket becomes a notification-only transport carrying absolute conversation and thread state plus structural events. Active rooms fetch incremental message history by sequence when notifications arrive, inactive rooms update badges only, and human sends use optimistic pending rows that reconcile on durable server ack.

**Tech Stack:** Rust, Axum, SQLite, React, Vite, Vitest, Playwright

---

### Task 1: Freeze The Notification Contract In Tests

**Files:**
- Modify: `tests/realtime_tests.rs`
- Modify: `tests/server_tests.rs`
- Modify: `ui/src/transport/realtime.test.ts`
- Test: `qa/cases/playwright/MSG-005.spec.ts`

**Step 1: Write the failing tests**

Add failing assertions for:

- websocket frames for message arrival do not include message `content`
- websocket frames include absolute `conversation.state` fields:
  - `latestSeq`
  - `lastReadSeq`
  - `unreadCount`
- active-room browser flow updates through notification plus fetch, not message-body bus payload

Example UI-side assertion:

```ts
expect(frame.type).toBe('event')
expect(frame.event.eventType).toBe('conversation.state')
expect(frame.event.payload).not.toHaveProperty('content')
expect(frame.event.payload).toMatchObject({
  latestSeq: 12,
  unreadCount: 3,
})
```

**Step 2: Run tests to verify they fail**

Run:

```bash
cargo test --test realtime_tests --test server_tests
cd qa/cases/playwright && CHORUS_BASE_URL=http://localhost:3101 CHORUS_E2E_LLM=0 npx playwright test MSG-005.spec.ts --reporter=list
cd ui && npm test -- --run src/transport/realtime.test.ts
```

Expected: failures because the current websocket still uses message-shaped events.

**Step 3: Write minimal implementation**

Update the Rust event serialization and UI transport test fixtures so the new event family is expected:

- `src/server/transport/realtime.rs`
- `src/store/events.rs`
- `ui/src/types.ts`

Do not remove old behavior yet beyond what is needed to satisfy the new tests.

**Step 4: Run tests to verify they pass**

Run the same commands from Step 2.

Expected: all targeted tests pass.

**Step 5: Commit**

```bash
git add tests/realtime_tests.rs tests/server_tests.rs ui/src/transport/realtime.test.ts ui/src/types.ts src/server/transport/realtime.rs src/store/events.rs
git commit -m "test(im): freeze notification bus contract"
```

### Task 2: Emit Absolute Conversation And Thread State

**Files:**
- Modify: `src/store/messages.rs`
- Modify: `src/store/inbox.rs`
- Modify: `src/store/events.rs`
- Modify: `src/server/transport/realtime.rs`
- Test: `tests/store_tests.rs`
- Test: `tests/realtime_tests.rs`

**Step 1: Write the failing tests**

Add failing backend tests for:

- new message in a conversation emits `conversation.state`
- new thread reply emits both `conversation.state` and `thread.state`
- payload uses absolute fields, not deltas

Example expectation:

```rust
assert_eq!(event.event_type, "conversation.state");
assert_eq!(payload_field(&event, "latestSeq").as_i64(), Some(42));
assert_eq!(payload_field(&event, "unreadCount").as_i64(), Some(5));
```

**Step 2: Run tests to verify they fail**

Run:

```bash
cargo test --test store_tests --test realtime_tests
```

Expected: failures because those events do not exist yet.

**Step 3: Write minimal implementation**

Implement notification event append helpers in the message write path.

Touch:

- `src/store/messages.rs`
  - append conversation notification events after durable message write
  - append thread notification events for replies
- `src/store/inbox.rs`
  - expose helpers to compute absolute unread state
- `src/store/events.rs`
  - map new event types into stored event records

Do not include `content`, `attachments`, or rendered body fields in these notification payloads.

**Step 4: Run tests to verify they pass**

Run:

```bash
cargo test --test store_tests --test realtime_tests
```

Expected: PASS.

**Step 5: Commit**

```bash
git add src/store/messages.rs src/store/inbox.rs src/store/events.rs src/server/transport/realtime.rs tests/store_tests.rs tests/realtime_tests.rs
git commit -m "feat(im): emit absolute conversation notifications"
```

### Task 3: Fetch Active Rooms Incrementally On Notification

**Files:**
- Modify: `ui/src/hooks/useHistory.ts`
- Modify: `ui/src/transport/realtime.ts`
- Modify: `ui/src/api.ts`
- Modify: `ui/src/types.ts`
- Test: `ui/src/transport/realtime.test.ts`
- Test: `qa/cases/playwright/MSG-005.spec.ts`

**Step 1: Write the failing tests**

Add failing tests for:

- when notification says `latestSeq` advanced for the active room, the client fetches `history?after=<maxSeq>`
- inactive rooms update notification state only and do not fetch history

Example helper expectation:

```ts
expect(fetchSpy).toHaveBeenCalledWith(
  expect.stringContaining('/history?'),
  expect.anything()
)
expect(fetchSpy.mock.calls[0][0]).toContain('after=42')
```

**Step 2: Run tests to verify they fail**

Run:

```bash
cd ui && npm test -- --run src/transport/realtime.test.ts
cd qa/cases/playwright && CHORUS_BASE_URL=http://localhost:3101 CHORUS_E2E_LLM=0 npx playwright test MSG-005.spec.ts --reporter=list
```

Expected: FAIL because the UI currently applies direct message-body events.

**Step 3: Write minimal implementation**

Implement an active-room incremental fetch path:

- keep initial room bootstrap in `useHistory`
- on `conversation.state` / `thread.state` for the active target:
  - compare incoming `latestSeq` to local max sequence
  - fetch only `after=<localMaxSeq>`
- for inactive targets:
  - do not fetch bodies

Update `ui/src/transport/realtime.ts` so the reducer handles notification events rather than content-bearing events.

**Step 4: Run tests to verify they pass**

Run the same commands from Step 2.

Expected: PASS.

**Step 5: Commit**

```bash
git add ui/src/hooks/useHistory.ts ui/src/transport/realtime.ts ui/src/api.ts ui/src/types.ts ui/src/transport/realtime.test.ts qa/cases/playwright/MSG-005.spec.ts
git commit -m "feat(ui): fetch active rooms from notifications"
```

### Task 4: Add Explicit Viewport-Based Read Cursors

**Files:**
- Modify: `src/server/handlers/messages.rs`
- Modify: `src/store/inbox.rs`
- Modify: `src/store/messages.rs`
- Modify: `ui/src/hooks/useHistory.ts`
- Modify: `ui/src/components/ChatPanel.tsx`
- Modify: `ui/src/components/ThreadPanel.tsx`
- Modify: `ui/src/api.ts`
- Test: `tests/store_tests.rs`
- Test: `tests/server_tests.rs`
- Test: `qa/cases/playwright/NAV-002.spec.ts`
- Create: `qa/cases/playwright/MSG-006.spec.ts`

**Step 1: Write the failing tests**

Add failing tests for:

- read cursor update endpoint or command persists conversation and thread read positions
- unread count drops only after messages become visible
- opening a room without visibility does not clear unread

Example payload expectation:

```json
{
  "target": "#all",
  "lastReadSeq": 82
}
```

**Step 2: Run tests to verify they fail**

Run:

```bash
cargo test --test store_tests --test server_tests
cd qa/cases/playwright && CHORUS_BASE_URL=http://localhost:3101 CHORUS_E2E_LLM=0 npx playwright test MSG-006.spec.ts --reporter=list
```

Expected: FAIL because explicit viewport-driven marking does not exist yet.

**Step 3: Write minimal implementation**

Add a read-cursor mutation path:

- backend:
  - accept read-cursor updates for conversations and threads
  - persist them through inbox-domain state
- frontend:
  - compute highest visible sequence in the active viewport
  - debounce updates
  - report only when focused and loaded

Re-use the unread-aware scroll logic already on the branch as the visibility basis where helpful.

**Step 4: Run tests to verify they pass**

Run the same commands from Step 2.

Expected: PASS.

**Step 5: Commit**

```bash
git add src/server/handlers/messages.rs src/store/inbox.rs src/store/messages.rs ui/src/hooks/useHistory.ts ui/src/components/ChatPanel.tsx ui/src/components/ThreadPanel.tsx ui/src/api.ts tests/store_tests.rs tests/server_tests.rs qa/cases/playwright/MSG-006.spec.ts
git commit -m "feat(im): add viewport-based read cursors"
```

### Task 5: Add Optimistic Human Send State

**Files:**
- Modify: `ui/src/components/MessageInput.tsx`
- Modify: `ui/src/components/ThreadPanel.tsx`
- Modify: `ui/src/hooks/useHistory.ts`
- Modify: `ui/src/types.ts`
- Modify: `ui/src/api.ts`
- Create: `ui/src/components/ToastRegion.tsx`
- Test: `qa/cases/playwright/MSG-007.spec.ts`

**Step 1: Write the failing tests**

Add failing browser tests for:

- pending row appears immediately after send
- pending row shows loading icon before ack
- success clears loading state without removing the row
- failed send leaves row visible as failed and shows toast

Example UI state shape:

```ts
{
  id: 'client:123',
  status: 'sending',
  content: 'hello',
}
```

**Step 2: Run tests to verify they fail**

Run:

```bash
cd qa/cases/playwright && CHORUS_BASE_URL=http://localhost:3101 CHORUS_E2E_LLM=0 npx playwright test MSG-007.spec.ts --reporter=list
```

Expected: FAIL because current human sends do not keep pending rows or failed state.

**Step 3: Write minimal implementation**

Implement optimistic rows:

- create a local client nonce before send
- insert a pending message into the current room
- reconcile from send ack using real `message_id` and `seq`
- on failure:
  - set row status to `failed`
  - show toast

Do this for both:

- main chat composer
- thread composer

**Step 4: Run tests to verify they pass**

Run the same command from Step 2.

Expected: PASS.

**Step 5: Commit**

```bash
git add ui/src/components/MessageInput.tsx ui/src/components/ThreadPanel.tsx ui/src/hooks/useHistory.ts ui/src/types.ts ui/src/api.ts ui/src/components/ToastRegion.tsx qa/cases/playwright/MSG-007.spec.ts
git commit -m "feat(ui): add optimistic human send state"
```

### Task 6: Remove Message Content From The Bus End-To-End

**Files:**
- Modify: `src/server/transport/realtime.rs`
- Modify: `src/store/messages.rs`
- Modify: `src/store/events.rs`
- Modify: `ui/src/transport/realtime.ts`
- Test: `tests/realtime_transport_tests.rs`
- Test: `qa/cases/playwright/MSG-005.spec.ts`

**Step 1: Write the failing tests**

Add failing assertions for:

- no `message.created` content-bearing transport frames remain in the browser flow
- active-room incremental fetch still renders new content correctly
- websocket console logs still show notification receipt

**Step 2: Run tests to verify they fail**

Run:

```bash
cargo test --test realtime_transport_tests
cd qa/cases/playwright && CHORUS_BASE_URL=http://localhost:3101 CHORUS_E2E_LLM=0 npx playwright test MSG-005.spec.ts --reporter=list
```

Expected: FAIL because the transport still knows about message-content events.

**Step 3: Write minimal implementation**

Delete the remaining direct content path:

- remove message materialization from bus payloads
- keep only notification and structural events on the websocket
- ensure active-room history fetch is now the only way new message bodies appear

**Step 4: Run tests to verify they pass**

Run the same commands from Step 2.

Expected: PASS.

**Step 5: Commit**

```bash
git add src/server/transport/realtime.rs src/store/messages.rs src/store/events.rs ui/src/transport/realtime.ts tests/realtime_transport_tests.rs qa/cases/playwright/MSG-005.spec.ts
git commit -m "refactor(im): remove message bodies from notification bus"
```

### Task 7: Final Integration QA And Doc Sync

**Files:**
- Modify: `AGENTS.md` if workflow expectations changed
- Modify: `qa/README.md` if new required cases were added
- Test: `qa/cases/playwright/NAV-002.spec.ts`
- Test: `qa/cases/playwright/CHN-005.spec.ts`
- Test: `qa/cases/playwright/MSG-005.spec.ts`
- Test: `qa/cases/playwright/MSG-006.spec.ts`
- Test: `qa/cases/playwright/MSG-007.spec.ts`

**Step 1: Write any missing failing QA assertions**

Make sure the browser suite explicitly covers:

- no shell polling
- notification-only active-room fetch
- viewport-driven unread clearing
- optimistic send success
- optimistic send failure

**Step 2: Run the full focused verification set**

Run:

```bash
cargo test --test store_tests --test server_tests --test realtime_tests --test realtime_transport_tests
cd ui && npm test && npm run build
cd qa/cases/playwright && CHORUS_BASE_URL=http://localhost:3101 CHORUS_E2E_LLM=0 npx playwright test NAV-002.spec.ts CHN-005.spec.ts MSG-005.spec.ts MSG-006.spec.ts MSG-007.spec.ts --reporter=list
```

Expected: PASS.

**Step 3: Update docs if behavior changed**

If shipped behavior or QA workflow changed, update:

- `AGENTS.md`
- `qa/README.md`

**Step 4: Commit**

```bash
git add AGENTS.md qa/README.md qa/cases/playwright/NAV-002.spec.ts qa/cases/playwright/CHN-005.spec.ts qa/cases/playwright/MSG-005.spec.ts qa/cases/playwright/MSG-006.spec.ts qa/cases/playwright/MSG-007.spec.ts
git commit -m "docs(qa): cover notification bus and optimistic send"
```

