# Messaging Cases

### MSG-001 Multi-Agent Channel Fan-Out

- Tier: 0
- Release-sensitive: yes
- Goal:
  - verify a human message in a shared channel can wake multiple agents and preserve message correctness
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

### HIS-001 History Reload And Selection Stability

- Tier: 0
- Release-sensitive: yes
- Goal:
  - verify history and current selection survive refresh
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
