# Agent Cases

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
