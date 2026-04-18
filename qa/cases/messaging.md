# Messaging Cases

## Smoke

### MSG-001 Multi-Agent Channel Fan-Out

- Suite: smoke
- Goal: verify a human message in a shared channel wakes multiple agents with correct attribution
- Script: [`playwright/MSG-001.spec.ts`](./playwright/MSG-001.spec.ts)
- Preconditions:
  - `bot-a`, `bot-b`, and `bot-c` exist
- Steps:
  1. Send one prompt in the shared channel asking all agents to reply.
  2. Wait for all agents to process.
  3. Verify the human message appears once.
  4. Verify replies from all 3 agents appear in the same channel.
  5. Verify each reply shows the correct sender identity.
  6. Verify no messages are duplicated or misattributed.
- Expected: one human message, three distinct correctly-attributed agent replies, no cross-channel leak

### MSG-002 DM Round-Trip [runtime matrix]

- Suite: smoke
- Supersedes: MSG-002, DM-002
- Execution mode: hybrid
- Goal: verify a DM send → agent reply round-trip works for every supported runtime
- Script: [`playwright/MSG-002.spec.ts`](./playwright/MSG-002.spec.ts)
- Preconditions:
  - runtime and model selected via `CHORUS_RUNTIME` / `CHORUS_MODEL` env vars
  - agent `dm-e2e-<runtime>` seeded by spec `beforeAll`
- Runtime matrix:

  | `CHORUS_RUNTIME` | `CHORUS_MODEL` | Agent slug |
  | --- | --- | --- |
  | `codex` | `codex` | `dm-e2e-codex` |
  | `claude` | `sonnet` | `dm-e2e-claude` |
  | `kimi` | `kimi` | `dm-e2e-kimi` |

- Steps:
  1. Seed or confirm agent `dm-e2e-<runtime>` exists with the chosen runtime/model.
  2. Open a DM with the agent.
  3. Send a message containing a unique token (e.g. `DM2-<timestamp>`).
  4. Verify the human message appears immediately in the DM timeline.
  5. Poll the history API for an agent reply containing the token (up to 2 min).
  6. Verify the reply appears in the DM timeline, not in any channel.
  7. Verify the activity log shows a `tool_start` entry for `send_message`.
  8. Refresh and confirm both messages persist.
- Expected: agent reply in same DM with token; activity log confirms `send_message` tool call; history survives refresh

### MSG-003 Thread Reply In Busy Channel

- Suite: smoke
- Goal: verify thread replies stay scoped to the thread and do not leak into the main timeline
- Script: [`playwright/MSG-003.spec.ts`](./playwright/MSG-003.spec.ts)
- Preconditions:
  - channel with at least one agent reply exists
- Steps:
  1. In the shared channel, open a thread from one agent reply.
  2. Send a thread reply from the human.
  3. Wait for the addressed agent to reply in the thread.
  4. Return to the main channel view.
  5. Verify thread messages stay attached to the thread and do not pollute the main timeline.
- Expected: thread replies remain in thread; channel timeline does not contain thread-only messages

### MSG-004 DM Wake From Inactive State

- Suite: smoke
- Goal: verify a sleeping agent wakes from a DM and replies into the same DM timeline
- Script: [`playwright/MSG-004.spec.ts`](./playwright/MSG-004.spec.ts)
- Preconditions:
  - `bot-a` is currently inactive (stopped)
- Steps:
  1. Open the agent list sidebar.
  2. Open a DM to `bot-a`.
  3. Send a message requesting a unique token (e.g., `wake-dm-2`).
  4. Wait for the agent to start and reply.
  5. Confirm the reply appears in the DM timeline.
- Expected: agent auto-starts; reply rendered in the DM, not in a channel

### MSG-005 Attachment Upload And Download

- Suite: smoke
- Supersedes: MSG-006, ATT-001
- Goal: verify file upload, inline rendering, and download integrity end-to-end
- Script: [`playwright/MSG-005.spec.ts`](./playwright/MSG-005.spec.ts)
- Steps:
  1. Open the composer and attach a prepared test file.
  2. Send the message.
  3. Verify the attachment name appears inline in the sent message.
  4. Click the attachment link and download the file.
  5. Verify downloaded content matches the original file.
- Expected: attachment visible inline; download returns identical content

### MSG-006 Clickable Mention Opens Agent Profile

- Suite: smoke
- Supersedes: MSG-012
- Goal: verify clicking an @agent mention opens the correct agent profile panel
- Script: [`playwright/MSG-006.spec.ts`](./playwright/MSG-006.spec.ts)
- Steps:
  1. Open a channel or DM containing an @mention of an existing agent.
  2. Hover over the @mention and verify pointer cursor.
  3. Click the @mention.
  4. Verify the Profile panel opens showing the correct agent.
- Expected: @mention styled as pill; click opens correct agent profile

## Regression

### MSG-007 Optimistic Send And Rollback

- Suite: regression
- Goal: verify the ack-first composer surfaces success and failure states coherently
- Script: [`playwright/MSG-007.spec.ts`](./playwright/MSG-007.spec.ts)
- Steps:
  1. Type a message and send.
  2. While in flight, verify the composer exposes a pending state.
  3. On success: verify the confirmed message appears once and the composer returns to idle.
  4. On failure: verify a visible error toast appears and unsent text remains in the composer.
  5. Repeat the same checks inside a thread composer.
- Expected: success shows one message, no phantom failure; failure is visible and recoverable
- Common failure signals:
  - successful send duplicates a message row
  - failed send disappears with no visible error
  - failed send clears the draft

### MSG-008 Read Cursor Advances On Visibility

- Suite: regression
- Goal: verify read cursor advances when messages become visible in viewport
- Script: [`playwright/MSG-008.spec.ts`](./playwright/MSG-008.spec.ts)
- Steps:
  1. Open a channel with unread messages.
  2. Scroll to make unread messages visible.
  3. Verify unread badge updates as messages come into view.
  4. Switch to another channel and return.
  5. Verify previously viewed messages remain marked as read.
- Expected: cursor advances on visibility; badge reflects actual count; state persists across navigation
- Common failure signals:
  - unread badge does not update
  - cursor advances without visibility
  - state lost on navigation

### MSG-009 Single Websocket Tunnel Across Target Switches

- Suite: regression
- Goal: verify one WebSocket is reused across channel/DM switches without reconnecting
- Script: [`playwright/MSG-009.spec.ts`](./playwright/MSG-009.spec.ts)
- Preconditions:
  - at least 2 channels and 1 DM available
- Steps:
  1. Open DevTools Network tab.
  2. Open channel A and verify WebSocket connection established.
  3. Switch to channel B and verify same WebSocket still active.
  4. Switch to a DM and verify same WebSocket still active.
  5. Send a message in the DM and verify it arrives via the existing WebSocket.
- Expected: single WebSocket connection across all targets; subscriptions change without reconnect
- Common failure signals:
  - new WebSocket created per target
  - connection drops on switch
  - missed realtime events

### MSG-010 Inactive Room Unread Badge Lifecycle

- Suite: regression
- Goal: verify unread badges appear for inactive rooms and clear when the room is viewed
- Script: [`playwright/MSG-010.spec.ts`](./playwright/MSG-010.spec.ts)
- Preconditions:
  - at least 2 channels available; `bot-a` exists
- Steps:
  1. Open channel A and send a message mentioning `bot-a`.
  2. Wait for agent reply.
  3. Switch to channel B without viewing the reply.
  4. Verify channel A shows unread badge.
  5. Switch back to channel A.
  6. Verify unread badge clears after viewing.
- Expected: badge appears for unread messages in inactive rooms; clears on view; count accurate
- Common failure signals:
  - badge missing for new messages
  - badge does not clear on view
  - incorrect count

### MSG-011 Thread Unread Lifecycle And Reply Count

- Suite: regression
- Goal: verify thread reply counts and unread indicators update correctly through the lifecycle
- Script: [`playwright/MSG-011.spec.ts`](./playwright/MSG-011.spec.ts)
- Preconditions:
  - a message with an existing thread exists
- Steps:
  1. Open a channel and locate a message with replies.
  2. Verify reply count badge is visible.
  3. Open the thread panel and verify replies load.
  4. Add a new reply in the thread.
  5. Close the thread panel.
  6. Verify reply count incremented.
  7. From another session or agent, add another reply.
  8. Verify unread indicator appears on the thread.
  9. Re-open the thread and verify unread clears.
- Expected: reply count accurate; unread tracks new replies since last view; badge clears on open
- Common failure signals:
  - reply count wrong
  - unread badge missing
  - state doesn't clear

### HIS-001 History Reload And Selection Stability

- Suite: regression
- Goal: verify scrolling loads older messages correctly without duplication
- Script: [`playwright/HIS-001.spec.ts`](./playwright/HIS-001.spec.ts)
- Preconditions:
  - a channel with many messages (more than one page)
- Steps:
  1. Open a channel with substantial history.
  2. Scroll up to trigger pagination.
  3. Verify older messages load and append correctly.
  4. Verify no duplicate messages appear at pagination boundaries.
- Expected: smooth pagination; no duplicates; correct ordering
- Common failure signals:
  - pagination doesn't trigger
  - duplicate messages
  - wrong order

### ERR-001 Error Handling And Recovery

- Suite: regression
- Goal: verify errors surface visibly and the UI recovers without restart
- Script: [`playwright/ERR-001.spec.ts`](./playwright/ERR-001.spec.ts)
- Steps:
  1. Trigger a failure path (e.g. invalid upload or broken transition).
  2. Verify the UI surfaces the failure visibly.
  3. Verify the console or network log contains actionable details.
  4. Clear the failed state.
  5. Verify unrelated flows proceed afterward.
- Expected: failure is not silent; recovery possible without app restart; stale error state does not poison later actions
- Common failure signals:
  - silent failure
  - sticky broken composer state
  - later sends fail due to retained error state
