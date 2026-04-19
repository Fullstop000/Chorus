# Agent Cases

## Smoke

### ENV-001 App Startup And Identity

- Suite: smoke
- Goal: prove the product shell boots and identifies the current user correctly
- Script: [`playwright/ENV-001.spec.ts`](./playwright/ENV-001.spec.ts)
- Preconditions: fresh server start
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

### AGT-001 Create Agent And Verify Sidebar Presence

- Suite: smoke
- Goal: verify agent creation works with a non-claude runtime and appears in the sidebar
- Script: [`playwright/AGT-001.spec.ts`](./playwright/AGT-001.spec.ts)
- Preconditions: fresh data dir
- Steps:
  1. Create `smoke-bot` using the Codex runtime.
  2. Verify the agent appears in the sidebar.
  3. Click the agent and verify its tabs load without crashing.
- Expected:
  - agent is created successfully
  - sidebar updates after creation
  - agent is selectable and tabs render

### AGT-002 Agent Edit Persists Correctly

- Suite: smoke
- Goal: verify the edit agent flow saves role, model, and reasoning effort and the profile reflects changes immediately
- Script: [`playwright/AGT-002.spec.ts`](./playwright/AGT-002.spec.ts)
- Preconditions: `smoke-bot` from AGT-001 exists (or any Codex agent)
- Steps:
  1. Open the agent profile and click Edit.
  2. Verify Codex runtime shows a reasoning-effort selector in the edit dialog; Claude runtime does not.
  3. Change the role text to a distinct value.
  4. Change the reasoning effort to `high` (Codex only).
  5. Save and verify the profile shows the updated role text and reasoning effort.
  6. Verify the API returns the updated values.
- Expected:
  - edit dialog opens and accepts changes
  - Codex shows reasoning-effort selector; Claude does not
  - saved values persist in profile and API

### AGT-003 Agent Delete And Name-Reuse Contract

- Suite: smoke
- Goal: verify deleting an agent removes it from navigation, preserves history with deleted styling, and allows name reuse
- Script: [`playwright/AGT-003.spec.ts`](./playwright/AGT-003.spec.ts)
- Preconditions: a test agent exists with at least one visible message in a channel or DM
- Steps:
  1. Delete the agent through the shipped product control using the "keep workspace" option.
  2. Verify the agent disappears from the sidebar.
  3. Verify historical messages from the agent remain visible with deleted styling.
  4. Create a new agent with the same name.
  5. Verify the new agent appears cleanly without inheriting old profile or status.
  6. Verify old deleted-history rows do not lose their deleted styling.
- Expected:
  - deleted agent removed from navigation
  - historical messages preserved with deleted attribution
  - name reuse works without stale state bleed

### NAV-001 Sidebar Navigation And Selection Persistence

- Suite: smoke
- Goal: verify sidebar navigation selects correctly and persists across refresh
- Script: [`playwright/NAV-001.spec.ts`](./playwright/NAV-001.spec.ts)
- Steps:
  1. Select a channel, then select an agent.
  2. Switch between Chat and Profile tabs.
  3. Refresh and verify the selected state persists.
- Expected:
  - selection highlights match rendered content
  - tab content matches target
  - refresh preserves selection

### LFC-001 Agent Lifecycle Start, Idle, Stop, And Manual Restart

- Suite: smoke
- Goal: verify the visible lifecycle path from startup to idle to stop to manual restart is coherent across sidebar, profile, and activity
- Script: [`playwright/LFC-001.spec.ts`](./playwright/LFC-001.spec.ts)
- Preconditions: test agent is not currently mid-turn
- Steps:
  1. Create or select a test agent.
  2. Verify the agent enters a startup state such as `working` or `starting`.
  3. Wait until the agent settles into idle state such as `online` or `ready`.
  4. Verify profile status and sidebar status agree at each state.
  5. Stop the agent from the shipped UI control.
  6. Verify sidebar, profile, and activity all move to stopped state.
  7. Start the agent again from the shipped UI control.
  8. Verify it returns to startup and then back to idle.
- Expected:
  - startup, idle, stop, and restart are all visible and stable
  - profile status and sidebar status agree at every transition
  - manual restart restores the agent cleanly

## Regression

### LFC-002 System Restart Routine And Post-Restart Recovery

- Suite: regression
- Execution mode: hybrid
- Goal: verify the system restart routine preserves lifecycle state and returns agents to a usable post-restart state
- Script: [`playwright/LFC-002.spec.ts`](./playwright/LFC-002.spec.ts)
- Preconditions:
  - at least one active agent has completed a real reply cycle before restart
  - run once with default data dir and once with a custom temp data dir during release-level QA
- Steps:
  1. Record pre-restart agent statuses, recent conversation, and selected target.
  2. Restart the server process against the same data dir.
  3. Reload the browser.
  4. Verify agents reappear with sensible lifecycle states rather than phantom UI state.
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

- Suite: regression
- Goal: verify activity is readable and complete for real mixed flows
- Script: [`playwright/ACT-001.spec.ts`](./playwright/ACT-001.spec.ts)
- Preconditions: run `MSG-001`, `MSG-003`, and `MSG-002` first
- Steps:
  1. Open `bot-a` activity tab.
  2. Verify the most recent entries include status changes, received messages, sent messages, and tool work.
  3. Pick one received-message row and verify sender, target, and content preview are present.
  4. Pick one sent-message row and verify the target label matches where the message was sent.
  5. Pick one tool row and verify the label distinguishes the work type.
  6. Verify entries are visually distinguishable and free of duplicate status spam.
  7. Refresh the page and verify the activity log still loads with the same recent story.
- Expected:
  - activity tells a coherent story of what happened
  - message send and receive are visible with sender and target
  - refresh does not blank or scramble recent activity
- Common failure signals:
  - missing message events or empty previews
  - duplicate status spam
  - refresh loses recent activity

### ACT-002 Activity Timeline Ordering During Wake And Recovery

- Suite: regression
- Goal: verify the activity timeline preserves the order and meaning of a wake-up flow, especially for DM-triggered recovery
- Script: [`playwright/ACT-002.spec.ts`](./playwright/ACT-002.spec.ts)
- Preconditions:
  - run `MSG-004` first
  - if restart behavior changed, run `LFC-002` or `REC-001` first
- Steps:
  1. Open the activity tab for the agent used in `MSG-004`.
  2. Locate the portion of the timeline covering the DM-triggered wake-up.
  3. Verify the sequence is coherent: offline → startup → received message → work → sent reply → idle.
  4. Verify the triggering DM content preview appears before the follow-up send.
  5. Verify the sent-reply row content matches the DM reply rendered in chat.
  6. If the case includes a server restart, verify no phantom active periods appear after restart.
  7. Refresh the page and verify the same wake-up segment remains visible and ordered.
- Expected:
  - wake-up activity appears in a defensible order
  - received and sent DM rows match the visible chat
  - restart transitions do not produce contradictory states
- Common failure signals:
  - sent reply appears before the triggering received message
  - timeline shows agent as active while actually offline
  - refresh changes the order or hides the wake-up segment

### NAV-002 Idle Shell Does Not Poll Sidebar Resources

- Suite: regression
- Goal: verify the app shell does one bootstrap fetch for sidebar resources and then remains idle
- Script: [`playwright/NAV-002.spec.ts`](./playwright/NAV-002.spec.ts)
- Preconditions: fresh app load, no user action after initial shell render
- Steps:
  1. Open the app root.
  2. Wait for the shell to render and remain idle for at least 6 seconds.
  3. Count requests to `/api/humans`, `/api/channels`, `/api/agents`, and `/api/teams`.
  4. Verify each endpoint is fetched exactly once during bootstrap.
- Expected:
  - shell bootstraps correctly
  - sidebar data is fetched once
  - no background polling while idle
- Common failure signals:
  - repeated humans-list polling
  - sidebar lists refetch without user action
  - idle shell continuously wakes the network panel

### WRK-001 Workspace Tab Path And File Visibility

- Suite: regression
- Goal: verify the workspace tab reflects the actual agent workspace and the split-view explorer can select and preview real files
- Script: [`playwright/WRK-001.spec.ts`](./playwright/WRK-001.spec.ts)
- Preconditions:
  - agent has started and produced workspace files including at least one markdown file under `notes/`
  - run once with default storage path and once with a custom data dir
- Steps:
  1. Open an agent workspace tab.
  2. Verify the top path row shows the real workspace path and the copy-path control is present.
  3. Verify expected files and directories are listed, including `notes/` and `MEMORY.md` when they exist.
  4. Expand `notes/` and select a markdown file; verify the row is highlighted.
  5. Verify the right pane header shows relative file path, file size, and modified timestamp.
  6. Toggle `Raw` and `Preview`; verify `Raw` shows literal contents and `Preview` renders markdown.
  7. Trigger workspace refresh and confirm the tree and preview remain usable.
  8. Repeat in a run that uses `--data-dir <temp dir>`.
- Expected:
  - displayed path matches actual storage path
  - file browsing works in both default and custom path modes
  - markdown preview renders and raw mode remains literal
- Common failure signals:
  - path ignores configured data dir
  - files exist on disk but UI shows empty state
  - selecting a file does not load preview content

### REC-001 Restart Server And Verify Agent Recovery

- Suite: regression
- Goal: verify server restart does not destroy active product state
- Script: [`playwright/REC-001.spec.ts`](./playwright/REC-001.spec.ts)
- Preconditions: at least one active agent has an established session
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
  - agents disappear after restart
  - session resume breaks replies
  - profile or activity contradict real state

### REC-002 Concurrent Agent Activity Under One Channel

- Suite: regression
- Goal: verify the system remains usable when at least one agent responds while the UI is active
- Script: [`playwright/REC-002.spec.ts`](./playwright/REC-002.spec.ts)
- Preconditions: `bot-a`, `bot-b`, and `bot-c` all available
- Steps:
  1. Send a channel prompt that agents will answer.
  2. While replies are arriving, switch to activity and back.
  3. Confirm the UI remains stable and messages are not lost.
- Expected:
  - at least one agent reply appears; no dropped or duplicated messages
  - activity and chat stay in sync
- Note: only kimi (bot-b) reliably responds; the case validates UI stability and message integrity during live updates, not that all three agents reply concurrently
- Common failure signals:
  - disappearing messages
  - activity entries missing for one agent
  - stale tab content during live updates
