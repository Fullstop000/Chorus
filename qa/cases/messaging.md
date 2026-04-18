# Messaging Cases

### MSG-001 Multi-Agent Channel Fan-Out

- Suite: smoke
- Release-sensitive: yes
- Goal:
  - verify a human message in a shared channel can wake multiple agents and preserve message correctness
- Script:
  - [`playwright/MSG-001.spec.ts`](./playwright/MSG-001.spec.ts) (Step 1 UI; Steps 3–6 hybrid `history` API)
- Preconditions:
  - `bot-a`, `bot-b`, and `bot-c` exist
  - active test channel is open
- Steps:
  1. Send one clear prompt in the shared channel asking all agents to reply.
  2. Wait long enough for all agents to process.
  3. Verify the original human message appears once.
  4. Verify replies from all 3 agents appear in the same channel.
  5. Verify each reply shows the correct sender identity.
  6. Verify reply order is chronologically reasonable and no messages are duplicated.
- Expected:
  - one human message
  - three distinct agent replies
  - no reply is misattributed to another agent
  - no message is rendered in the wrong channel
- Common failure signals:
  - only one agent wakes
  - duplicate replies
  - missing sender labels
  - replies appear under the wrong conversation target

### MSG-002 Direct Message Round-Trip

- Suite: smoke
- Release-sensitive: yes
- Goal:
  - verify DM send and agent reply rendering work independently of channel traffic
- Script:
  - [`playwright/MSG-002.spec.ts`](./playwright/MSG-002.spec.ts) (Steps 4–6 hybrid poll `history` for agent row)
- Preconditions:
  - at least one test agent exists
  - the selected agent is reachable and not already mid-turn
- Steps:
  1. Open a DM with `bot-a`.
  2. Send a human DM that asks for an exact short token such as `dm-check-1`.
  3. Verify the human DM appears once in the DM timeline immediately after send.
  4. Wait for the agent reply.
  5. Verify the reply appears in the same DM timeline, not in `#general` or another target.
  6. Verify the reply text matches the requested token closely enough to prove it is the response to this DM.
  7. Refresh the page.
  8. Re-open the same DM and verify both the human message and the agent reply are still visible.
  9. Switch to another target and return to the DM once to confirm the reply remains attached to the DM.
- Expected:
  - DM target is clearly identified
  - human send is visible immediately in the DM
  - reply arrives in the DM, not in a channel
  - refresh does not lose DM history
  - target switching does not hide or relocate the reply
- Common failure signals:
  - human DM sends successfully but no agent reply row ever appears
  - DM routes to wrong target
  - agent processes the DM but the reply renders in a channel or disappears after refresh
  - a failed composer state leaks from another target

### MSG-003 Thread Reply In Busy Channel

- Suite: smoke
- Release-sensitive: yes
- Goal:
  - prove thread behavior still works when the parent channel has multiple agents participating
- Script:
  - [`playwright/MSG-003.spec.ts`](./playwright/MSG-003.spec.ts) (precondition: API seed to `#all` when LLM enabled)
- Preconditions:
  - `MSG-001` completed
- Steps:
  1. In the shared channel, open a thread from one agent reply.
  2. Send a thread reply from the human.
  3. Wait for the addressed agent to reply in the thread.
  4. Return to the main channel view.
  5. Verify thread messages stay attached to the thread and do not pollute the main timeline.
- Expected:
  - thread panel opens correctly
  - thread reply persists in history
  - thread activity remains associated with the thread
  - channel timeline does not flatten thread-only content into the main flow incorrectly
- Common failure signals:
  - thread reply appears as a top-level message
  - thread history disappears after reload
  - wrong agent responds

### MSG-004 Direct Message Wake And Reply Visibility

- Suite: regression
- Release-sensitive: yes when touching lifecycle, runtime integration, DM routing, or restart/wake behavior
- Goal:
  - verify a sleeping or inactive agent can wake from a DM and render its reply back into the same DM timeline
- Script:
  - [`playwright/MSG-004.spec.ts`](./playwright/MSG-004.spec.ts) (inactive DM wake + reply visibility)
- Preconditions:
  - `bot-a` is currently inactive (stopped)
- Steps:
  1. Open the agent list sidebar.
  2. Open a DM to `bot-a`.
  3. Send a message requesting a unique token (e.g., `wake-dm-2`).
  4. Wait for the agent to start and reply.
  5. Confirm the reply appears in the DM timeline.
- Expected:
  - agent starts automatically
  - reply is rendered in the DM, not in a channel
- Common failure signals:
  - agent does not start from DM
  - reply appears in wrong target

### MSG-005 Websocket-Driven Chat After Initial History Bootstrap

- Suite: regression
- Release-sensitive: yes when touching composer, message list, or send pipeline
- Goal:
  - verify the chat view boots from one history fetch, then stays websocket-driven for local and remote messages
- Script:
  - [`playwright/MSG-005.spec.ts`](./playwright/MSG-005.spec.ts) (history bootstrap + websocket append contract)
- Preconditions:
  - any channel or DM is open
- Steps:
  1. Open a channel and wait for the initial history bootstrap window to settle.
  2. Send one local message and verify it appears without triggering another history fetch.
  3. Inject one remote message and verify it appears without triggering another history fetch.
  4. Confirm realtime traffic is still visible and no background polling resumes.
- Expected:
  - initial history bootstrap may include immediate gap-fill requests, but later message delivery stays websocket-driven
  - later message delivery stays websocket-driven
  - no duplicate history polling resumes after bootstrap
- Common failure signals:
  - additional history GETs after local or remote sends
  - message appears only after a refetch
  - background polling resumes after the conversation is open

### MSG-006 Inline Attachment Rendering

- Suite: regression
- Release-sensitive: yes when touching attachment upload, storage, or rendering
- Goal:
  - verify attachments are visible and downloadable after upload
- Script:
  - [`playwright/MSG-006.spec.ts`](./playwright/MSG-006.spec.ts) (upload + inline attachment link)
- Preconditions:
  - a text file is prepared on disk for upload
- Steps:
  1. Open the composer in any channel or DM.
  2. Attach the prepared file.
  3. Send the message.
  4. Verify the attachment name appears in the sent message.
  5. Click the attachment link and verify it downloads or opens correctly.
- Expected:
  - attachment is listed inline
  - link is functional
- Common failure signals:
  - attachment missing from message
  - broken link
  - file content mismatch

### MSG-007 Optimistic Send Success And Failure States

- Suite: regression
- Release-sensitive: yes when touching composer, message list, or send pipeline
- Goal:
  - verify the shipped ack-first composer surfaces success and failure coherently
- Script:
  - [`playwright/MSG-007.spec.ts`](./playwright/MSG-007.spec.ts) (network delay/failure simulation for main chat and threads)
- Preconditions:
  - any channel or DM is open
- Steps:
  1. Type a message and send.
  2. While the request is in flight, verify the composer exposes a pending state.
  3. If send succeeds: verify the confirmed message appears once and the composer returns to idle.
  4. If send fails: verify a visible error toast appears and the unsent text remains in the composer.
  5. Repeat the same checks inside a thread composer.
- Expected:
  - success path shows one confirmed message and no phantom failure state
  - failure path is visible and recoverable from the composer
- Common failure signals:
  - successful send duplicates a message row
  - failed send disappears with no visible error
  - failed send clears the draft unexpectedly

### MSG-008 Conversation Read Cursor Advances On Visibility

- Suite: regression
- Release-sensitive: yes when touching unread state, viewport detection, or cursor APIs
- Goal:
  - verify read cursor advances when messages become visible in viewport
- Script:
  - [`playwright/MSG-008.spec.ts`](./playwright/MSG-008.spec.ts) (read cursor + viewport visibility)
- Preconditions:
  - a channel with existing unread messages
- Steps:
  1. Open a channel with unread messages.
  2. Scroll to make unread messages visible.
  3. Verify unread badge updates as messages come into view.
  4. Switch to another channel and return.
  5. Verify previously viewed messages remain marked as read.
- Expected:
  - read cursor advances based on viewport visibility
  - unread badge reflects actual unread count
  - state persists across navigation
- Common failure signals:
  - unread badge does not update
  - cursor advances without visibility
  - state lost on navigation

### MSG-009 Single Websocket Tunnel Across Target Switches

- Suite: regression
- Release-sensitive: yes when touching realtime transport, subscription management, or socket lifecycle
- Goal:
  - verify one WebSocket is reused across channel/DM switches instead of creating new connections
- Script:
  - [`playwright/MSG-009.spec.ts`](./playwright/MSG-009.spec.ts) (single WS across target switches)
- Preconditions:
  - at least 2 channels and 1 DM available
- Steps:
  1. Open DevTools Network tab.
  2. Open channel A and verify WebSocket connection established.
  3. Switch to channel B and verify same WebSocket still active (no new connection).
  4. Switch to a DM and verify same WebSocket still active.
  5. Send a message in the DM and verify it arrives via the existing WebSocket.
- Expected:
  - single WebSocket connection for all targets
  - subscriptions change without reconnecting
- Common failure signals:
  - new WebSocket created per target
  - connection drops on switch
  - missed realtime events

### MSG-010 Inactive Room Unread Badge Lifecycle

- Suite: regression
- Release-sensitive: yes when touching unread badges, realtime events, or inbox state
- Goal:
  - verify unread badges appear correctly for inactive rooms when new messages arrive
- Script:
  - [`playwright/MSG-010.spec.ts`](./playwright/MSG-010.spec.ts) (unread badges + realtime lifecycle)
- Preconditions:
  - at least 2 channels available
  - `bot-a` exists and can reply
- Steps:
  1. Open channel A.
  2. Send a message mentioning `bot-a`.
  3. Wait for agent reply.
  4. Without viewing channel A, switch to channel B.
  5. Verify channel A shows unread badge.
  6. Switch back to channel A.
  7. Verify unread badge clears after viewing.
- Expected:
  - unread badge appears for new messages in inactive rooms
  - badge clears when room becomes active
  - count is accurate
- Common failure signals:
  - badge missing for new messages
  - badge does not clear on view
  - incorrect count

### MSG-011 Thread Unread Lifecycle And Reply Count

- Suite: regression
- Release-sensitive: yes when touching thread inbox, reply counts, or badge aggregation
- Goal:
  - verify thread reply counts and unread states update correctly through the lifecycle
- Script:
  - [`playwright/MSG-011.spec.ts`](./playwright/MSG-011.spec.ts) (placeholder: thread unread + reply count)
- Preconditions:
  - a message with an existing thread exists
- Steps:
  1. Open a channel and locate a message with replies.
  2. Verify reply count badge is visible on the message.
  3. Open the thread panel and verify replies load.
  4. Add a new reply in the thread.
  5. Close the thread panel.
  6. Verify reply count incremented.
  7. From another session or agent, add another reply.
  8. Verify unread indicator appears on the thread.
  9. Re-open the thread and verify unread clears.
- Expected:
  - reply count accurate and visible
  - unread state tracks new replies since last view
  - badge clears on open
- Common failure signals:
  - reply count wrong
  - unread badge missing
  - state doesn't clear

### MSG-012 Clickable Mention Opens Agent Profile

- Suite: regression
- Release-sensitive: yes when touching message rendering, mention styling, or profile navigation
- Goal:
  - verify clicking an @agent mention in a message opens the agent's profile panel
- Script:
  - [`playwright/MSG-012.spec.ts`](./playwright/MSG-012.spec.ts)
- Preconditions:
  - `bot-a` exists and has sent at least one message containing `@bot-b` or any mention
  - OR: send a message as human containing `@bot-a` mention
- Steps:
  1. Open any channel or DM with existing messages containing @mentions.
  2. Locate a message containing `@bot-a` (or any existing agent name).
  3. Hover over the @mention and verify cursor changes to pointer.
  4. Click the @mention.
  5. Verify the Profile panel opens.
  6. Verify the profile shows the correct agent (matching the clicked mention).
  7. Verify the agent name and details are displayed correctly.
- Expected:
  - @mention renders with distinct styling (pill/badge)
  - clickable @mentions show pointer cursor on hover
  - clicking opens Profile tab
  - correct agent is selected and displayed
  - non-existent agent mentions are not clickable
- Common failure signals:
  - @mention not styled distinctly
  - no hover effect or cursor change
  - click does nothing
  - wrong agent profile shown
  - profile panel does not open

### DM-002 Single-Agent DM E2E Reply

- Suite: smoke
- Release-sensitive: yes when touching DM routing, agent lifecycle, driver output parsing, or bridge wiring
- Execution mode: hybrid
- Goal:
  - verify that a single agent of any runtime can receive a DM, process it, and reply into the same DM timeline
  - parameterisable by runtime so the same test routine covers all supported runtimes
- Script:
  - [`playwright/DM-002.spec.ts`](./playwright/DM-002.spec.ts)
- Preconditions:
  - server running with the branch under test
  - runtime and model selected via env vars `CHORUS_RUNTIME` (default: `claude`) and `CHORUS_MODEL` (default: `sonnet`)
  - agent `dm-e2e-<runtime>` seeded automatically by the spec `beforeAll`
- Steps:
  1. Seed or confirm agent `dm-e2e-<runtime>` exists with the chosen runtime/model.
  2. Open a DM with the agent.
  3. Send a message containing a unique exact token (e.g. `DM2-<timestamp>`).
  4. Verify the human message appears immediately in the DM timeline.
  5. Poll the history API for an agent reply that contains the token, up to 2 minutes.
  6. Verify the reply appears in the DM timeline in the browser, not in a channel.
  7. Verify the agent activity log shows a `tool_start` entry for `send_message`.
- Expected:
  - human message visible immediately after send
  - agent reply appears in the same DM target with the token
  - reply does not appear in any channel timeline
  - activity log confirms the agent used `send_message` (not raw stdout)
- Common failure signals:
  - no agent reply arrives within the timeout (lifecycle or bridge failure)
  - reply routes to the wrong target (DM routing bug)
  - activity log shows only raw text output instead of a `send_message` tool call

### HIS-001 History Reload And Selection Stability

- Suite: regression
- Release-sensitive: no
- Goal:
  - verify scrolling loads older messages correctly without duplication
- Script:
  - [`playwright/HIS-001.spec.ts`](./playwright/HIS-001.spec.ts) (channel + DM + thread reload stability)
- Preconditions:
  - a channel with many messages (more than one page)
- Steps:
  1. Open a channel with substantial history.
  2. Scroll up to trigger pagination.
  3. Verify older messages load and append correctly.
  4. Verify no duplicate messages appear at pagination boundaries.
- Expected:
  - smooth pagination
  - no duplicates
  - correct ordering
- Common failure signals:
  - pagination doesn't trigger
  - duplicate messages
  - wrong order

### ATT-001 Attachment Upload And Download

- Suite: regression
- Release-sensitive: yes when touching upload pipeline or storage
- Goal:
  - verify file upload and download work end-to-end
- Script:
  - [`playwright/ATT-001.spec.ts`](./playwright/ATT-001.spec.ts)
- Preconditions:
  - a test file prepared
- Steps:
  1. Open composer.
  2. Upload a file.
  3. Send message.
  4. Download the file.
  5. Verify content matches original.
- Expected:
  - upload succeeds
  - download returns identical file
- Common failure signals:
  - upload error
  - corrupted file
  - broken link

### ERR-001 Error Handling And Recovery

- Suite: regression
- Release-sensitive: no
- Goal:
  - verify errors are visible and the UI can recover cleanly
- Script:
  - [`playwright/ERR-001.spec.ts`](./playwright/ERR-001.spec.ts) (forced upload error + recovery)
- Preconditions:
  - use at least one intentionally failing path discovered during testing or known from recent regressions
- Steps:
  1. Trigger a failure path such as an invalid upload or broken transition.
  2. Verify the UI surfaces the failure somewhere user-visible.
  3. Verify the console or network log contains actionable details.
  4. Clear the failed state.
  5. Verify unrelated flows can proceed afterward.
- Expected:
  - failure is not silent
  - user can recover without full app restart when appropriate
  - stale failed state does not poison later actions
- Common failure signals:
  - silent failure
  - sticky broken composer state
  - later unrelated sends fail because earlier error state was retained
