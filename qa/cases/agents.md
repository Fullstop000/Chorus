# Agent Cases

### ENV-001 App Startup And Identity

- Tier: 0
- Release-sensitive: yes
- Goal:
  - prove the product shell boots and identifies the current user correctly
- Script:
  - [`playwright/ENV-001.spec.ts`](./playwright/ENV-001.spec.ts)
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
- Script:
  - [`playwright/AGT-001.spec.ts`](./playwright/AGT-001.spec.ts)
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
  - verify runtime-specific controls appear only when applicable, especially Codex reasoning effort
- Script:
  - [`playwright/AGT-002.spec.ts`](./playwright/AGT-002.spec.ts) (browser-driven matrix create + duplicate-name checks)
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
  - Kimi:
    - `kimi-for-coding`
- Steps:
  1. Open the create-agent modal and record the runtime and model options actually shown in the build under test.
  2. Switch between Claude, Codex, and Kimi in the create-agent modal.
  3. Verify the reasoning-effort control is hidden for Claude and Kimi, and visible for Codex.
  4. For at least one Codex model, choose a non-default reasoning effort and create the agent.
  5. Create one disposable agent for every runtime/model pair using a stable naming scheme such as `matrix-<runtime>-<model>`.
  6. After each creation, verify the new agent appears in the sidebar.
  7. Open the new agent profile and verify the runtime badge and model badge match the selected pair exactly.
  8. For Codex agents created with a non-default reasoning effort, verify the profile reflects the selected reasoning effort exactly.
  9. Verify creation does not silently fall back to a different runtime, model, or Codex reasoning effort.
  10. Attempt to create one duplicate name using the exact same config.
  11. Attempt to create the same duplicate name again with a different runtime or model.
  12. Verify both duplicate-name attempts fail with a clear error and do not create extra records.
- Expected:
  - every visible runtime/model pair is creatable
  - stored runtime and model match the selected values exactly
  - Codex shows a reasoning-effort control and persists the chosen value exactly
  - Claude and Kimi do not show a meaningless reasoning-effort control
  - duplicate names are rejected regardless of runtime or model
  - failures are attributable to a specific pair, not hidden behind a generic fallback
- Common failure signals:
  - one runtime/model pair cannot be created
  - created agent shows a different model than requested
  - reasoning-effort control is missing for Codex
  - reasoning-effort control appears for Claude or Kimi
  - created Codex agent ignores the selected reasoning effort
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
- Script:
  - [`playwright/AGT-003.spec.ts`](./playwright/AGT-003.spec.ts) (hybrid delete + recreate-name regression)
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

### AGT-004 Agent Control Center Edit, Restart, Delete, And Deleted History

- Tier: 1
- Release-sensitive: yes when touching profile controls, agent edit flows, restart modes, delete flows, environment variables, or deleted-agent history rendering
- Execution mode: browser
- Goal:
  - verify the profile control center can edit config, run each restart mode coherently, delete an agent with explicit workspace handling, and preserve deleted-history attribution
- Script:
  - [`playwright/AGT-004.spec.ts`](./playwright/AGT-004.spec.ts) (browser edit flow + hybrid restart/delete verification)
- Preconditions:
  - at least one test agent exists
  - a shared channel or DM contains at least one visible historical message from that agent before delete
- Steps:
  1. Open a test agent profile and record its runtime, model, and current role text.
  2. If the test agent runtime is Codex, verify the edit control shows a reasoning-effort selector; if the runtime is Claude, verify it does not.
  3. Use the edit control to change the role text and add one environment variable.
  4. If the runtime is Codex, also change the reasoning effort.
  5. Save the change and verify the profile reflects the new role, environment state, and Codex reasoning effort when applicable.
  6. Use the restart control in plain `Restart` mode and verify the agent returns to a usable state without losing workspace files.
  7. Use the restart control in `Reset Session & Restart` mode and verify the agent still has workspace files but behaves like a fresh conversation session.
  8. Send at least one visible message from the agent in a channel or DM so there is history to inspect after delete.
  9. Delete the agent using the `keep workspace` option.
  10. Verify the agent disappears from navigation, the workspace files still exist, and historical messages remain readable with deleted styling.
  11. Recreate an agent with the same name if the product still allows clean name reuse after delete.
  12. Verify the recreated agent does not silently remove the deleted styling from the old history rows.
- Expected:
  - profile edit persists correctly
  - Codex reasoning effort is editable and persisted correctly
  - Claude does not expose Codex-only reasoning controls
  - restart modes behave differently but coherently
  - delete removes the live record while preserving readable history
  - deleted messages remain attributed but visibly historical
  - name reuse does not reattach old messages to the recreated live identity
- Common failure signals:
  - edit saves but does not actually apply
  - Codex reasoning effort is missing, ignored, or shown for Claude
  - restart mode effects are indistinguishable
  - workspace keep/delete choice is ignored
  - deleted history rows lose attribution or look live
  - recreated agent makes old deleted history look active again

### PRF-001 Agent Profile Accuracy During Lifecycle Changes

- Tier: 0
- Release-sensitive: yes
- Goal:
  - verify profile state matches actual runtime state
- Script:
  - [`playwright/PRF-001.spec.ts`](./playwright/PRF-001.spec.ts)
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
- Script:
  - [`playwright/LFC-001.spec.ts`](./playwright/LFC-001.spec.ts)
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
- Script:
  - [`playwright/LFC-002.spec.ts`](./playwright/LFC-002.spec.ts) (placeholder `fixme` until restart/recovery automation is implemented)
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
- Script:
  - [`playwright/ACT-001.spec.ts`](./playwright/ACT-001.spec.ts) (hybrid pre-seed when `CHORUS_E2E_LLM` is not `0`; see spec header)
- Preconditions:
  - run `MSG-001`, `MSG-003`, and `MSG-002` first
- Steps:
  1. Open `bot-a` activity tab.
  2. Verify the most recent entries include all of these row types when they occurred in the run:
    - status changes
    - received messages
    - sent messages
    - tool or tool-like work
    - thinking or output text when available
  3. Pick one received-message row and verify:
    - sender name is shown
    - target label is shown
    - content preview is recognizable and not empty
  4. Pick one sent-message row and verify the target label matches where the message was actually sent.
  5. Pick one tool row and verify the label is specific enough to distinguish waiting, checking, sending, or file/tool work.
  6. Verify entries are visually distinguishable at a glance.
  7. Verify there is no flood of duplicate unchanged status transitions.
  8. Refresh the page and verify the activity log still loads with the same recent story.
- Expected:
  - activity tells a coherent story of what happened
  - message send and receive are visible
  - state changes are meaningful and not noisy
  - refresh does not blank or scramble recent activity
- Common failure signals:
  - missing message events
  - rows exist but omit sender, target, or meaningful preview text
  - unreadable grouping
  - duplicate status spam
  - refresh loses recent activity or changes entry order unexpectedly

### ACT-002 Activity Timeline Ordering During Wake And Recovery

- Tier: 1
- Release-sensitive: yes when touching lifecycle, restart, driver wake behavior, or activity aggregation
- Goal:
  - verify the activity timeline preserves the order and meaning of a wake-up flow, especially for DM-triggered recovery
- Script:
  - [`playwright/ACT-002.spec.ts`](./playwright/ACT-002.spec.ts) (DM wake + activity ordering assertions)
- Preconditions:
  - run `MSG-004` first
  - if restart behavior changed, run `LFC-002` or `REC-001` first
- Steps:
  1. Open the activity tab for the agent used in `MSG-004`.
  2. Locate the portion of the timeline covering the DM-triggered wake-up.
  3. Verify the sequence is coherent, for example:
    - non-active or offline state
    - startup or working state
    - received message
    - tool/check/wait work
    - sent reply
    - idle or waiting state
  4. Verify the triggering DM content preview appears before or alongside the follow-up send, not after unrelated work.
  5. Verify the sent-reply row content matches the DM reply that was actually rendered in chat.
  6. If the case includes a server restart, verify the timeline does not fabricate phantom active periods after the restart.
  7. Refresh the page and verify the same wake-up segment remains visible and ordered.
- Expected:
  - wake-up activity appears in a defensible order
  - received and sent DM rows can be matched back to the visible chat
  - restart and wake transitions do not produce contradictory states
- Common failure signals:
  - sent reply appears before the triggering received message
  - wake-up shows tool activity but no received-message row
  - timeline shows the agent as active while the profile or process state is offline
  - refresh changes the order or hides the critical wake-up segment

### NAV-001 Sidebar Navigation And Selection Persistence

- Tier: 1
- Release-sensitive: yes
- Goal:
  - verify users can move between channels, agents, and tabs without stale selection bugs
- Script:
  - [`playwright/NAV-001.spec.ts`](./playwright/NAV-001.spec.ts) (sidebar and tab navigation persistence)
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

### NAV-002 Idle Shell Does Not Poll Sidebar Resources

- Tier: 1
- Release-sensitive: yes when touching shell bootstrap, sidebar refresh logic, or server-info fetching
- Goal:
  - verify the app shell does one bootstrap fetch for sidebar resources and then remains idle until a real user action requires refresh
- Script:
  - [`playwright/NAV-002.spec.ts`](./playwright/NAV-002.spec.ts) (request-count assertions for idle shell behavior)
- Preconditions:
  - fresh app load
  - no user action after initial shell render
- Steps:
  1. Open the app root.
  2. Wait for the shell to render and then remain idle for at least 6 seconds.
  3. Count requests to `/api/server-info`, `/api/channels`, `/api/agents`, and `/api/teams`.
  4. Verify each endpoint is fetched exactly once during bootstrap.
- Expected:
  - shell bootstraps correctly
  - sidebar data is fetched once
  - no background polling resumes while the shell is idle
- Common failure signals:
  - repeated `server-info` polling
  - channels, agents, or teams lists refetch without user action
  - idle shell continuously wakes the network panel

### WRK-001 Workspace Tab Path And File Visibility

- Tier: 1
- Release-sensitive: yes
- Goal:
  - verify the workspace tab reflects the actual agent workspace, including non-default data dirs
  - verify the split-view explorer can select and preview real files instead of showing a placeholder list
- Script:
  - [`playwright/WRK-001.spec.ts`](./playwright/WRK-001.spec.ts) (workspace tree, preview, and metadata coverage)
- Preconditions:
  - agent has started and produced workspace files
  - the workspace contains at least one markdown file under `notes/`, such as `notes/work-log.md`
  - run once with default storage path and once with a custom temp data dir
- Steps:
  1. Open an agent workspace tab.
  2. Verify the top path row shows the real workspace path for the current server configuration and that the copy-path control is present.
  3. Verify expected files and directories are listed in the left tree, including `notes/` and `MEMORY.md` when they exist on disk.
  4. Expand `notes/` and select a markdown file such as `notes/work-log.md`.
  5. Verify the selected row is visibly highlighted.
  6. Verify the right pane header shows:
    - the relative file path
    - file size
    - modified timestamp
  7. Toggle `Raw` and `Preview`.
  8. Verify `Raw` shows the literal file contents.
  9. Verify `Preview` renders markdown formatting for `.md` files.
  10. Trigger the workspace refresh action and confirm the tree and preview remain usable.
  11. Repeat in a run that uses `--data-dir <temp dir>`.
- Expected:
  - displayed path matches actual storage path
  - workspace is not falsely shown empty
  - file browsing works in both default and custom path modes
  - selected file metadata and content stay aligned with the file chosen in the tree
  - markdown preview is rendered for markdown files and raw mode remains literal
- Common failure signals:
  - path ignores configured data dir
  - files exist on disk but UI shows empty state
  - selecting a file does not load preview content
  - preview header shows stale path, wrong size, or missing timestamp
  - raw and preview modes show the same unrendered output for markdown
  - refresh clears selection or leaves the pane in a broken state
  - file open fails for valid files

### REC-001 Restart Server And Verify Agent Recovery

- Tier: 1
- Release-sensitive: yes when touching runtime/session logic
- Goal:
  - verify server restart does not destroy active product state
- Script:
  - [`playwright/REC-001.spec.ts`](./playwright/REC-001.spec.ts) (placeholder `fixme` until restart-session recovery automation is implemented)
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
- Script:
  - [`playwright/REC-002.spec.ts`](./playwright/REC-002.spec.ts) (concurrent multi-agent channel stability)
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
