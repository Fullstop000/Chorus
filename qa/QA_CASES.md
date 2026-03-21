# Chorus Static QA Case Catalog

This is the reusable browser QA case list for Chorus.

It is intentionally:
- detailed enough to execute without guesswork
- stable enough to reuse every iteration
- strict enough to catch state-consistency failures, not just obvious crashes

All cases below are browser-first cases unless explicitly marked otherwise.
The explicit `Tier:` field on each case is authoritative. Nearby placement in the file is for related-domain readability.

## How To Use This Catalog

For each run:

1. pick the run mode from [`README.md`](./README.md)
2. create a fresh run report from [`QA_REPORT_TEMPLATE.md`](./QA_REPORT_TEMPLATE.md)
3. execute cases in the browser
4. mark each case `Pass`, `Fail`, `Blocked`, or `Not Run`
5. attach evidence for every failure

## Shared Preconditions

Apply these unless a case overrides them:

- server started from the branch under test
- browser opened to the real app shell
- data dir is fresh
- current human user confirmed by `whoami`
- 3 agents created:
  - `bot-a`
  - `bot-b`
  - `bot-c`
- one text file prepared for attachment upload testing
- default working channel is `#general` or a dedicated `#qa-multi-agent`

## Result Definitions

- `Pass`
  - all expected results observed
- `Fail`
  - at least one expected result is violated
- `Blocked`
  - cannot finish because an earlier failure or environment issue prevents execution
- `Not Run`
  - intentionally skipped in this run mode

## Notes On Product Gaps

- Some QA cases intentionally cover product controls that are not fully shipped yet, such as delete flows or explicit channel member management.
- When a case is marked `hybrid` or `blocked-until-shipped`, follow the case instructions exactly.
- Do not simulate missing user-facing flows by editing SQLite directly during QA.

## Tier 0 Cases

### ENV-001 App Startup And Identity

- Tier: 0
- Release-sensitive: yes
- Goal:
  - prove the product shell boots and identifies the current user correctly
- Preconditions:
  - fresh server start
- Steps:
  1. Open the app root URL in Chrome.
  2. Verify the main shell loads without a blank page or crash state.
  3. Verify the sidebar renders channels, agents, and humans sections.
  4. Verify the current user is shown in the footer.
  5. Verify the `whoami` value matches the visible current user.
- Expected:
  - app loads without fatal UI error
  - current user is stable across shell and API
  - no immediate console exception
- Common failure signals:
  - white screen
  - hydration/render error
  - mismatched current user names

### AGT-001 Create Three Agents And Verify Sidebar Presence

- Tier: 0
- Release-sensitive: yes
- Goal:
  - verify agent creation works repeatedly, not just once
- Preconditions:
  - no existing test agents in the fresh data dir
- Steps:
  1. Create `bot-a`.
  2. Create `bot-b`.
  3. Create `bot-c`.
  4. Verify each agent appears in the sidebar.
  5. Click each agent once and verify its tabs load without crashing.
- Expected:
  - all three agents are created successfully
  - sidebar updates after each creation
  - each agent is selectable
- Common failure signals:
  - only first creation works
  - stale sidebar list
  - duplicate-name handling is broken
  - profile tabs crash for newly created agents

### AGT-002 Agent Create Matrix Across Every Driver And Model

- Tier: 1
- Release-sensitive: yes when touching agent creation, runtime defaults, model defaults, driver registration, or the create-agent modal
- Execution mode: browser
- Goal:
  - verify every runtime and model pair currently exposed in the UI can be created and is stored correctly
  - verify duplicate agent names are rejected consistently
- Preconditions:
  - fresh data dir
  - current runtime/model matrix captured from the create-agent modal before execution
- Required matrix:
  - Claude:
    - `claude-sonnet-4-6`
    - `claude-opus-4-6`
    - `claude-haiku-4-5`
  - Codex:
    - `gpt-5.4`
    - `gpt-5.4-mini`
    - `gpt-5.3-codex`
    - `gpt-5.2-codex`
    - `gpt-5.2`
    - `gpt-5.1-codex-max`
    - `gpt-5.1-codex-mini`
- Steps:
  1. Open the create-agent modal and record the runtime and model options actually shown in the build under test.
  2. Create one disposable agent for every runtime/model pair using a stable naming scheme such as `matrix-<runtime>-<model>`.
  3. After each creation, verify the new agent appears in the sidebar.
  4. Open the new agent profile and verify the runtime badge and model badge match the selected pair exactly.
  5. Verify creation does not silently fall back to a different runtime or model.
  6. Attempt to create one duplicate name using the exact same config.
  7. Attempt to create the same duplicate name again with a different runtime or model.
  8. Verify both duplicate-name attempts fail with a clear error and do not create extra records.
- Expected:
  - every visible runtime/model pair is creatable
  - stored runtime and model match the selected values exactly
  - duplicate names are rejected regardless of runtime or model
  - failures are attributable to a specific pair, not hidden behind a generic fallback
- Common failure signals:
  - one runtime/model pair cannot be created
  - created agent shows a different model than requested
  - runtime picker and stored runtime disagree
  - duplicate name is accepted
  - duplicate name creates partial sidebar or DB state

### AGT-003 Agent Delete And Name-Reuse Contract

- Tier: 1
- Release-sensitive: yes when a delete flow exists or agent cleanup logic changes
- Execution mode: hybrid
- Current product note:
  - the current build does not expose a normal delete-agent control in the browser or server API
  - if that remains true for the build under test, mark this case `Blocked` and record the product gap
- Goal:
  - verify deleting an agent fully removes the user-facing record and cleans up enough state to safely reuse the name
- Preconditions:
  - disposable agent exists
  - delete entrypoint is exposed in the current build
- Steps:
  1. Create a disposable agent.
  2. Open its DM, profile, activity, and workspace surfaces at least once so references exist in the UI.
  3. Delete the agent through the shipped product control for the current build.
  4. Verify the agent disappears from the sidebar.
  5. Verify the old DM/profile route no longer loads stale agent state.
  6. Verify sending to the deleted agent is no longer possible through the normal UI path.
  7. Recreate a new agent with the exact same name.
  8. Verify the recreated agent appears cleanly and does not inherit stale UI selection, wrong status, or old profile metadata.
- Expected:
  - delete removes the agent from user-visible navigation
  - name reuse works cleanly after delete
  - old agent state does not bleed into the recreated agent
- Common failure signals:
  - deleted agent remains in sidebar
  - recreated agent inherits stale profile or channel state
  - name cannot be reused after successful delete

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

### TSK-001 Create And Advance A Task

- Tier: 0
- Release-sensitive: yes
- Goal:
  - verify task workflow transitions match visible UI state
- Preconditions:
  - tasks tab available
- Steps:
  1. Open `Tasks`.
  2. Create a new task with an unambiguous title.
  3. Verify it appears in `To Do`.
  4. Advance it once.
  5. Verify it moves to the correct next state.
  6. Watch console and network responses during the transition.
- Expected:
  - state change succeeds without server error
  - card moves exactly once
  - visual state matches backend state
- Common failure signals:
  - UI moves card but backend returns a 4xx or 5xx
  - impossible state transition
  - duplicate task cards

### TSK-002 Create Message-As-Task From Composer

- Tier: 0
- Release-sensitive: yes
- Goal:
  - verify the chat composer can create a task while sending a message
- Preconditions:
  - channel composer visible
- Steps:
  1. Enable `Also create as a task`.
  2. Send a message that should also create a task.
  3. Verify the message lands in chat.
  4. Open `Tasks` and verify a matching task was created.
- Expected:
  - one chat message
  - one related task
  - no duplicate send
- Common failure signals:
  - message sends but task missing
  - task created with wrong text
  - duplicate task creation

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

### PRF-001 Agent Profile Accuracy During Lifecycle Changes

- Tier: 0
- Release-sensitive: yes
- Goal:
  - verify profile state matches actual runtime state
- Preconditions:
  - at least one active agent exists
- Steps:
  1. Open `bot-a` profile.
  2. Record visible status and activity.
  3. Stop the agent from the UI.
  4. Verify the profile updates to inactive or stopped state.
  5. Start or wake the agent again if supported.
  6. Verify the profile updates back to active state.
- Expected:
  - profile status changes promptly and correctly
  - action buttons match the actual lifecycle state
  - no stale active label after stop
- Common failure signals:
  - backend stops but UI still shows running
  - stop/start buttons remain wrong for the new state
  - activity text is stale

### LFC-001 Agent Lifecycle Start, Idle, Stop, And Manual Restart

- Tier: 0
- Release-sensitive: yes
- Execution mode: browser
- Goal:
  - verify the visible lifecycle path from startup to idle to stop to manual restart is coherent across sidebar, profile, and activity
- Preconditions:
  - at least one test agent exists
  - the agent is not currently mid-turn when the case starts
- Steps:
  1. Create or select a test agent.
  2. Verify the agent enters a startup state such as `working`, `starting`, or similar transitional status.
  3. Wait until the agent settles into its idle state such as `online`, `ready`, or `waiting for messages`.
  4. Verify sidebar status, profile status, and activity log all tell the same lifecycle story.
  5. Stop the agent from the shipped UI control.
  6. Verify sidebar, profile, and activity all move to an inactive or stopped state.
  7. Start the agent again from the shipped UI control if one exists.
  8. Verify it returns to startup and then back to idle.
- Expected:
  - startup is visible
  - idle is visible and stable
  - stop is visible and stable
  - manual restart restores the agent cleanly
- Common failure signals:
  - profile and sidebar disagree about status
  - activity never records a lifecycle transition
  - stop works in backend but not in UI
  - restart leaves the agent stuck in an impossible state

## Tier 1 Cases

### ACT-001 Activity Timeline Completeness And Readability

- Tier: 1
- Release-sensitive: yes
- Goal:
  - verify activity is readable and complete for real mixed flows
- Preconditions:
  - run `MSG-001`, `MSG-003`, and `MSG-002` first
- Steps:
  1. Open `bot-a` activity tab.
  2. Verify there are clear entries for:
     - status changes
     - received messages
     - sent messages
     - tool/thinking/output events when available
  3. Verify entries are visually distinguishable at a glance.
  4. Verify there is no flood of duplicate unchanged status transitions.
- Expected:
  - activity tells a coherent story of what happened
  - message send and receive are visible
  - state changes are meaningful and not noisy
- Common failure signals:
  - missing message events
  - unreadable grouping
  - duplicate status spam

### CHN-001 Channel Create And Shared Availability

- Tier: 0
- Release-sensitive: yes
- Execution mode: browser
- Goal:
  - verify a new channel can be created from the product and becomes usable immediately
- Preconditions:
  - at least 3 test agents exist
- Steps:
  1. Create a new disposable channel such as `#qa-ops`.
  2. Verify it appears in the sidebar immediately.
  3. Open the new channel and verify the empty state is sane.
  4. Send one human message asking all agents to reply.
  5. Verify replies land in the new channel rather than `#general`.
  6. Navigate away and back, then verify the new channel history persists.
- Expected:
  - channel create succeeds
  - sidebar updates immediately
  - new channel is actually routable for chat traffic
  - agents and human can all use the new channel
- Common failure signals:
  - channel create succeeds but sidebar does not update
  - messages land in the wrong channel
  - new channel is visible but not usable

### CHN-002 Channel Name Validation, Normalization, And Duplicate Rejection

- Tier: 1
- Release-sensitive: yes when touching channel create, channel routing, or channel validation
- Execution mode: browser
- Goal:
  - verify channel names are normalized and duplicate channels are rejected cleanly
- Preconditions:
  - channel create modal is available
- Steps:
  1. Create a channel using mixed case or a leading `#`, for example `#Engineering`.
  2. Verify the stored/displayed name is normalized consistently, such as `engineering`.
  3. Attempt to create the same logical channel again using a different casing or with/without `#`.
  4. Attempt to create an invalid or empty channel name.
  5. Verify the UI shows a clear failure and does not create a partial sidebar entry.
- Expected:
  - normalization is consistent
  - duplicate rejection is based on the logical channel name, not raw input formatting
  - invalid names are rejected without corrupting navigation
- Common failure signals:
  - `#Engineering` and `engineering` create separate channels
  - duplicate create looks successful but produces partial state
  - invalid name is silently accepted

### CHN-003 Channel Member Add And Remove Operations

- Tier: 1
- Release-sensitive: yes when explicit membership controls exist or channel delivery logic changes
- Execution mode: hybrid
- Current product note:
  - the current build auto-joins agents to created channels and does not expose explicit add-member or remove-member controls in the normal UI
  - if that remains true for the build under test, mark this case `Blocked` and record the product gap
- Goal:
  - verify explicit membership controls change who can receive and participate in a channel
- Preconditions:
  - disposable channel exists
  - member add/remove controls exist in the current product build
- Steps:
  1. Create a disposable channel.
  2. Add `bot-a` and `bot-b`, but not `bot-c`.
  3. Send a message asking every present agent to reply.
  4. Verify only `bot-a` and `bot-b` reply.
  5. Remove `bot-b`.
  6. Send another message.
  7. Verify `bot-a` can still reply and `bot-b` no longer receives the message.
  8. Verify membership changes remain correct after page reload.
- Expected:
  - member add/remove changes actual delivery and participation
  - removed members stop receiving messages
  - non-members do not appear as active channel participants
- Common failure signals:
  - removed member still replies
  - non-member receives channel traffic
  - membership UI updates but delivery behavior does not

### CHN-004 Channel Delete And Selection Recovery

- Tier: 1
- Release-sensitive: yes when a delete flow exists or channel persistence changes
- Execution mode: hybrid
- Current product note:
  - the current build does not expose a normal delete-channel flow
  - if that remains true for the build under test, mark this case `Blocked` and record the product gap
- Goal:
  - verify deleting a channel removes it cleanly and leaves the UI in a sane selection state
- Preconditions:
  - disposable channel exists
  - delete control exists in the current product build
- Steps:
  1. Create a disposable channel and open it.
  2. Put some message history into it.
  3. Delete the channel through the shipped control.
  4. Verify the channel disappears from the sidebar.
  5. Verify the main panel falls back to a sane target instead of rendering stale channel state.
  6. Refresh and confirm the deleted channel does not reappear.
- Expected:
  - deleted channel is removed from navigation and selection state
  - no stale chat view remains attached to the deleted channel
  - refresh preserves the deleted state
- Common failure signals:
  - deleted channel remains selected
  - refresh resurrects deleted channel
  - old history appears under the wrong target

### WRK-001 Workspace Tab Path And File Visibility

- Tier: 1
- Release-sensitive: yes
- Goal:
  - verify the workspace tab reflects the actual agent workspace, including non-default data dirs
- Preconditions:
  - agent has started and produced workspace files
  - run once with default storage path and once with a custom temp data dir
- Steps:
  1. Open an agent workspace tab.
  2. Verify the displayed path is correct for the current server configuration.
  3. Verify expected files or directories are listed.
  4. Open at least one file.
  5. Repeat in a run that uses `--data-dir <temp dir>`.
- Expected:
  - displayed path matches actual storage path
  - workspace is not falsely shown empty
  - file browsing works in both default and custom path modes
- Common failure signals:
  - path ignores configured data dir
  - files exist on disk but UI shows empty state
  - file open fails for valid files

### NAV-001 Sidebar Navigation And Selection Persistence

- Tier: 1
- Release-sensitive: yes
- Goal:
  - verify users can move between channels, agents, and tabs without stale selection bugs
- Preconditions:
  - at least one channel and one agent populated
- Steps:
  1. Select a channel.
  2. Select an agent.
  3. Switch between `Chat`, `Tasks`, `Profile`, `Activity`, and `Workspace`.
  4. Return to the channel.
  5. Refresh and verify selected state behaves predictably.
- Expected:
  - selection highlights are correct
  - tab content matches the currently selected target
  - no cross-target data bleed
- Common failure signals:
  - selected target and rendered content disagree
  - switching tabs changes the underlying target unexpectedly

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

## Tier 1 Reliability Cases

### LFC-002 System Restart Routine And Post-Restart Recovery

- Tier: 1
- Release-sensitive: yes when touching startup, persistence, session restore, or lifecycle transitions
- Execution mode: hybrid
- Goal:
  - verify the system restart routine preserves lifecycle state and returns agents to a usable post-restart state
- Preconditions:
  - at least one active agent has already completed a real reply cycle before restart
  - run once with a default data dir and once with a custom temp data dir during release-level QA
- Steps:
  1. Record pre-restart agent statuses, recent conversation, and selected target.
  2. Restart the server process against the same data dir.
  3. Reload the browser.
  4. Verify agents reappear with sensible lifecycle states such as `active`, `sleeping`, or `inactive`, rather than phantom UI state.
  5. Send a fresh message that should wake at least one surviving agent.
  6. Verify the post-restart reply loop works again.
  7. Verify profile, sidebar, and activity all agree about the recovered state.
- Expected:
  - restart preserves durable state
  - post-restart lifecycle state is coherent
  - agents can still respond after restart
- Common failure signals:
  - restart leaves phantom active agents
  - agents appear but never reply
  - UI state after restart disagrees with backend state

### REC-001 Restart Server And Verify Agent Recovery

- Tier: 1
- Release-sensitive: yes when touching runtime/session logic
- Goal:
  - verify server restart does not destroy active product state
- Preconditions:
  - at least one active agent has an established session
- Steps:
  1. Record visible agent state and recent conversation.
  2. Restart the server.
  3. Re-open the app.
  4. Verify agents reappear with sensible status.
  5. Send a new message and verify reply behavior still works.
- Expected:
  - agents and history persist
  - session recovery is coherent
  - no phantom active agents without a backing process
- Common failure signals:
  - agents disappear
  - session resume breaks replies
  - profile or activity contradict real state

### REC-002 Concurrent Agent Activity Under One Channel

- Tier: 1
- Release-sensitive: yes when touching agent manager, runtime, or activity aggregation
- Goal:
  - verify the system remains usable when several agents respond in the same channel window
- Preconditions:
  - `bot-a`, `bot-b`, and `bot-c` all available
- Steps:
  1. Send a channel prompt that all three agents will answer.
  2. While replies are arriving, switch to activity and back.
  3. Open a thread from one of the arriving messages.
  4. Confirm the UI remains stable and messages are not lost.
- Expected:
  - no dropped or duplicated messages from concurrency
  - activity and chat stay in sync
  - thread open does not corrupt the channel timeline
- Common failure signals:
  - disappearing messages
  - activity entries missing for one agent
  - stale tab content during live updates

## Maintenance Notes

When the product changes:

- add a new case if a new user-visible flow appears
- tighten an existing case if a bug slipped through
- keep case IDs stable so reports remain comparable across iterations
- do not silently delete old cases without updating release expectations
