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
  - verify DM send/receive works independently of channel traffic
- Preconditions:
  - at least one test agent exists
- Steps:
  1. Open a DM with `bot-a`.
  2. Send a human DM.
  3. Wait for the reply.
  4. Refresh the page.
  5. Re-open the same DM and verify the history is preserved.
- Expected:
  - DM target is clearly identified
  - reply arrives in the DM, not in a channel
  - refresh does not lose DM history
- Common failure signals:
  - DM routes to wrong target
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
