# Messaging Cases

### MSG-001 Multi-Agent Channel Fan-Out

- Tier: 0
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

- Tier: 0
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
  - refresh loses conversation
  - a failed composer state leaks from another target

### MSG-003 Thread Reply In Busy Channel

- Tier: 0
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

- Tier: 1
- Release-sensitive: yes when touching lifecycle, runtime integration, DM routing, or restart/wake behavior
- Goal:
  - verify a sleeping or inactive agent can wake from a DM and render its reply back into the same DM timeline
- Script:
  - [`playwright/MSG-004.spec.ts`](./playwright/MSG-004.spec.ts) (inactive DM wake + reply visibility)
- Preconditions:
  - at least one test agent exists
  - the selected agent can be moved into a sleeping or inactive state through the shipped product flow
- Steps:
  1. Open a DM with `bot-a`.
  2. Move `bot-a` into a non-active state such as `sleeping` or `inactive`.
  3. Confirm the profile or sidebar reflects the non-active state before sending.
  4. Return to the DM and send a message asking for an exact short token such as `dm-wake-1`.
  5. Wait for the agent to wake and reply.
  6. Verify the reply appears in the same DM timeline as the triggering message.
  7. Verify the UI does not jump to another target while the agent wakes.
  8. Open the agent profile or activity view and confirm the wake/reply lifecycle is coherent.
  9. Return to the DM and verify the visible reply still matches the DM that triggered the wake-up.
- Expected:
  - a non-active agent can wake from a DM
  - the reply renders in the correct DM without manual page repair
  - lifecycle surfaces and DM history tell the same story
- Common failure signals:
  - DM to sleeping agent never produces a visible reply
  - agent wakes according to profile or activity but the DM timeline never updates
  - reply appears under the wrong DM or in `#general`
  - UI target changes unexpectedly during the wake-up flow

### MSG-005 Notification-Driven Incremental History Fetch

- Tier: 0
- Release-sensitive: yes
- Goal:
  - verify chat stays websocket-driven after bootstrap and only fetches incremental history windows when notification events advance the active room
- Script:
  - [`playwright/MSG-005.spec.ts`](./playwright/MSG-005.spec.ts) (websocket console + `history?after=` assertions)
- Preconditions:
  - shared channel is selected
  - websocket console logging is enabled in the current build
- Steps:
  1. Open the app and wait for the initial chat bootstrap to settle.
  2. Record the initial `/internal/agent/{id}/history` requests and verify they do not include `after`.
  3. Send one local message in the active room.
  4. Verify the optimistic local row appears immediately; if the client performs any follow-up history fetch, it must use an `after` cursor rather than a full reload.
  5. Inject one new message into the same room from another actor.
  6. Verify the UI logs a `conversation.state` websocket event and performs one incremental `history` fetch with `after`.
  7. Wait several more seconds and verify no periodic history polling resumes.
- Expected:
  - bootstrap uses full history fetch once
  - local human send is visible immediately via optimistic UI plus durable ack
  - any local reconciliation fetch is incremental `history?after=...`, not a full history reload
  - remote active-room updates use incremental `history?after=...`
  - websocket notification payload contains metadata such as `latestSeq` and `unreadCount`, not message bodies
  - no background history polling continues after the room is idle again
- Common failure signals:
  - repeated full-history fetches while idle
  - local optimistic send is not visible until a background history reload completes
  - websocket receives full message content instead of notification metadata
  - remote message appears only after a timer-driven poll

### MSG-006 Thread Read Cursor Advances On Visibility

- Tier: 0
- Release-sensitive: yes
- Goal:
  - verify a thread read cursor is only advanced after thread replies become visible in the thread panel
- Script:
  - [`playwright/MSG-006.spec.ts`](./playwright/MSG-006.spec.ts) (thread open + `read-cursor` request assertions)
- Preconditions:
  - at least one agent exists, or the test can create a disposable one
  - one parent channel message and one thread reply can be seeded before opening the UI
- Steps:
  1. Seed a parent message and a thread reply under it.
  2. Open the app with the parent conversation selected.
  3. Verify no `POST /internal/agent/{id}/read-cursor` has yet been sent for the thread target.
  4. Open the thread panel from the parent message.
  5. Wait until the reply is visibly rendered in the thread panel.
  6. Verify a thread-targeted `read-cursor` POST is then sent with a concrete `lastReadSeq`.
- Expected:
  - conversation selection alone does not mark the thread read
  - thread read state advances after the reply is actually visible
  - read-cursor payload identifies the thread target correctly
- Common failure signals:
  - thread is marked read before it is opened
  - no read-cursor update occurs after the reply becomes visible
  - conversation read state is updated when only the thread should be

### MSG-007 Optimistic Send Success And Failure States

- Tier: 0
- Release-sensitive: yes
- Goal:
  - verify human-sent messages remain visible with local sending state until durable ack, and stay visible as failed rows with a toast when send fails
- Script:
  - [`playwright/MSG-007.spec.ts`](./playwright/MSG-007.spec.ts) (main chat and thread optimistic-send interception)
- Preconditions:
  - request interception is available in the browser harness
  - a thread can be opened for the thread-composer portion of the case
- Steps:
  1. Intercept `/internal/agent/{id}/send` so the first send succeeds after a short delay and the second send fails.
  2. Send one top-level message.
  3. Verify the message appears immediately with a sending indicator, then clears that indicator after the delayed success response.
  4. Send one more top-level message through the forced-failure path.
  5. Verify the failed message remains visible with failed styling and a visible failure toast.
  6. Repeat the same success-then-failure sequence inside an open thread.
- Expected:
  - optimistic rows appear immediately in both main chat and thread chat
  - success reconciles the optimistic row without removing it
  - failure keeps the row visible, marks it failed, and surfaces a toast
- Common failure signals:
  - sent message does not appear until the network response returns
  - failed message disappears entirely
  - no visible distinction between sending and failed states
  - thread composer behaves differently from the main composer without reason

### MSG-008 Conversation Read Cursor Advances On Visibility

- Tier: 1
- Release-sensitive: yes when touching read/unread state, viewport reporting, or conversation history bootstrapping
- Goal:
  - verify an unread top-level conversation message is marked read only after it becomes visible in the active chat viewport
- Script:
  - [`playwright/MSG-008.spec.ts`](./playwright/MSG-008.spec.ts) (seed unread top-level message + conversation `read-cursor` assertion)
- Preconditions:
  - at least one agent exists, or the test can create a disposable one
  - the default conversation target can be opened in the browser
- Steps:
  1. Seed one unread top-level message into the active conversation before opening the UI.
  2. Start capturing `POST /internal/agent/{id}/read-cursor` requests.
  3. Open the app with that conversation selected.
  4. Wait until the seeded message is visibly rendered in the main chat viewport.
  5. Verify a conversation-targeted `read-cursor` POST is sent with a `lastReadSeq` at or beyond the seeded message sequence.
- Expected:
  - visible top-level messages advance the conversation read cursor
  - the payload identifies the conversation target, not a thread target
  - read advancement happens from viewport visibility without any manual refresh
- Common failure signals:
  - conversation read cursor never advances for visible messages
  - only thread targets emit read-cursor updates
  - read cursor advances to the wrong target

### MSG-009 Single Websocket Tunnel Across Target Switches

- Tier: 0
- Release-sensitive: yes
- Goal:
  - verify the frontend keeps one session-wide realtime websocket while switching among channel, DM, and back again
- Script:
  - [`playwright/MSG-009.spec.ts`](./playwright/MSG-009.spec.ts) (websocket-count assertion during channel/DM switches)
- Preconditions:
  - at least one agent exists, or the test can create a disposable one
  - one disposable user channel can be created before loading the UI
- Steps:
  1. Seed a disposable user channel and ensure one agent exists for DM navigation.
  2. Start counting browser websocket connections targeting `/api/events/ws`.
  3. Open the app.
  4. Switch from the default room to the disposable channel.
  5. Switch from that channel to a DM with the agent.
  6. Switch back to the disposable channel.
  7. Verify the browser created only one realtime websocket for the whole sequence.
- Expected:
  - one websocket tunnel survives target switches
  - channel/DM switches update subscriptions in place
  - target changes do not create websocket fan-out in the browser
- Common failure signals:
  - one new websocket per clicked target
  - DM open creates an additional websocket next to the channel websocket
  - target switching only works by tearing down and rebuilding the realtime transport

### HIS-001 History Reload And Selection Stability

- Tier: 0
- Release-sensitive: yes
- Goal:
  - verify history and current selection survive refresh
- Script:
  - [`playwright/HIS-001.spec.ts`](./playwright/HIS-001.spec.ts) (channel, DM, and thread history reload)
- Preconditions:
  - at least one populated channel, one DM, and one thread exist
- Steps:
  1. Refresh while a channel is selected.
  2. Verify the selected target or a sensible default is shown.
  3. Open the DM and verify earlier messages are present.
  4. Open the thread and verify earlier thread messages are present.
  5. Navigate between these views multiple times.
- Expected:
  - no blank history panes
  - no target confusion between channel, DM, and thread
  - no duplicated message rendering after reload
- Common failure signals:
  - stale selection points to missing data
  - history truncates unexpectedly
  - messages duplicate after navigation

### ATT-001 Attachment Upload And Render

- Tier: 0
- Release-sensitive: yes
- Goal:
  - verify a human can upload an attachment from the browser and send it in chat
- Script:
  - [`playwright/ATT-001.spec.ts`](./playwright/ATT-001.spec.ts) (attachment upload + render)
- Preconditions:
  - small text file prepared locally
- Steps:
  1. Open a channel chat.
  2. Attach the file.
  3. Verify the pending attachment appears in the composer.
  4. Send the message with the attachment.
  5. Verify the sent message appears.
  6. Verify the attachment renders with a usable download target.
  7. Switch to another target and confirm failed state does not leak.
- Expected:
  - upload request succeeds
  - message with attachment is sent
  - attachment is downloadable
  - composer is cleared after success
- Common failure signals:
  - upload returns 4xx or 5xx
  - stale failed attachment remains in composer
  - attachment appears but download fails

### ERR-001 Error Surfacing And Recovery

- Tier: 1
- Release-sensitive: yes
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
