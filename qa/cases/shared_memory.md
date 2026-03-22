# Shared Memory Cases

### MEM-001 Remember And Breadcrumb Visibility

- Tier: 0
- Release-sensitive: yes
- Goal:
  - verify an agent can store a knowledge entry and the breadcrumb appears in `#shared-memory`
- Preconditions:
  - at least one active agent exists
  - `#shared-memory` channel is visible in the sidebar
- Steps:
  1. Open a channel chat with an active agent (e.g. `#general`).
  2. Ask the agent to remember a specific fact using an exact token such as `mem-check-1`.
     Example prompt: "Please remember the fact: key='test-finding', value='mem-check-1', tags='qa'."
  3. Wait for the agent to call `mcp_chat_remember`.
  4. Navigate to `#shared-memory` in the sidebar.
  5. Verify a breadcrumb message appears from the agent containing the key `test-finding` and value `mem-check-1`.
  6. Verify the breadcrumb includes the agent's name as sender.
  7. Refresh the page.
  8. Re-open `#shared-memory` and verify the breadcrumb is still present.
- Expected:
  - agent calls `remember` without error
  - breadcrumb message visible in `#shared-memory` immediately after agent turn
  - breadcrumb includes key, value, and agent name
  - breadcrumb survives page refresh
- Common failure signals:
  - agent errors on `remember` call
  - breadcrumb never appears in `#shared-memory`
  - `#shared-memory` channel is not visible in the sidebar
  - breadcrumb disappears after refresh

### MEM-002 Recall Returns Previously Stored Fact

- Tier: 0
- Release-sensitive: yes
- Goal:
  - verify an agent can retrieve a fact stored in an earlier turn via `mcp_chat_recall`
- Preconditions:
  - `MEM-001` completed (a fact is stored in the knowledge store)
  - the same or a different agent is active
- Steps:
  1. In `#general`, send a prompt asking the agent to recall facts tagged `qa`.
     Example: "Use recall to find any facts with tag 'qa' and tell me what you find."
  2. Wait for the agent to call `mcp_chat_recall`.
  3. Verify the agent's reply includes the value `mem-check-1` stored in `MEM-001`.
  4. Verify the agent correctly attributes the fact to the original author agent.
- Expected:
  - recall returns at least one entry
  - returned value matches what was stored
  - author attribution is correct
- Common failure signals:
  - recall returns no results despite the fact existing
  - agent sees an error from the recall tool
  - returned content does not match stored value

### MEM-003 System Channel Write Guard

- Tier: 0
- Release-sensitive: yes
- Goal:
  - verify that `#shared-memory` is read-only from the human message composer and agents cannot
    bypass the guard via `send_message`
- Preconditions:
  - `#shared-memory` channel is visible in the sidebar
- Steps:
  1. Navigate to `#shared-memory` in the sidebar.
  2. Attempt to type and send a message directly from the human composer in `#shared-memory`.
  3. Verify the message is rejected or the composer is disabled/read-only.
  4. Ask an agent: "Try to send the message 'direct-post-attempt' directly to #shared-memory using send_message."
  5. Verify the agent receives an error response from the tool.
  6. Verify no message with content `direct-post-attempt` appears in `#shared-memory`.
- Expected:
  - human composer in `#shared-memory` is disabled or read-only
  - agent `send_message` to `#shared-memory` returns an error mentioning `mcp_chat_remember`
  - no unauthorized messages appear in `#shared-memory`
- Common failure signals:
  - human can post to `#shared-memory` directly
  - agent successfully bypasses the guard via `send_message`
  - unstructured messages appear in the channel

### MEM-004 Two-Agent Handoff Via Shared Memory

- Tier: 1
- Release-sensitive: yes when touching knowledge store, task board, agent wakeup, or prompt collaboration section
- Goal:
  - verify the full research-to-implementation handoff: agent A stores findings, agent B recalls them
    and completes the task without human mediation
- Preconditions:
  - two agents exist (`researcher` and `implementer`, or any two available agents)
  - both agents are active or can wake from a message
- Steps:
  1. In `#general`, send: "Researcher: find the best approach to X and remember your findings tagged 'handoff-test'.
     Then assign the implementation to Implementer."
  2. Wait for the researcher agent to call `remember` and assign the task.
  3. Navigate to `#shared-memory` and verify the researcher's breadcrumb is visible.
  4. Wait for the implementer agent to wake and call `recall` with tag `handoff-test`.
  5. Verify the implementer's activity log shows a `recall` tool call.
  6. Verify the implementer completes the task and posts a result in `#general`.
  7. Verify the result references the finding stored by the researcher (the content should reflect it).
- Expected:
  - researcher stores at least one knowledge entry before handing off
  - `#shared-memory` shows the breadcrumb from the researcher
  - implementer calls recall and receives the stored finding
  - implementer posts a meaningful result without needing a re-explanation from the human
  - neither agent errors on remember or recall
- Common failure signals:
  - researcher does not call `remember` (falls back to sending a message instead)
  - implementer does not call `recall` on wakeup
  - handoff produces no visible result in `#general`
  - breadcrumb appears but implementer ignores it

### MEM-005 Shared Memory Survives Server Restart

- Tier: 1
- Release-sensitive: yes when touching knowledge store persistence, startup, or schema init
- Goal:
  - verify knowledge entries stored before a restart are still retrievable after restart
- Preconditions:
  - `MEM-001` completed (at least one fact is in the store)
- Steps:
  1. Record the breadcrumb content visible in `#shared-memory` before restart.
  2. Restart the server process against the same data dir.
  3. Reload the browser.
  4. Navigate to `#shared-memory` and verify the previously stored breadcrumbs are still visible.
  5. Ask an agent to recall the previously stored fact (e.g. `recall(tags="qa")`).
  6. Verify the agent's reply includes the value stored before restart.
- Expected:
  - knowledge entries persist across server restart
  - `#shared-memory` breadcrumbs are still visible after reload
  - recall returns correct pre-restart entries
- Common failure signals:
  - `#shared-memory` is empty after restart
  - recall returns no results despite entries existing before restart
  - server startup fails due to FTS5 schema conflict

### MEM-006 Shared Memory Channel Not Listed As Regular Channel

- Tier: 1
- Release-sensitive: yes when touching channel listing, UI sidebar, or channel_type handling
- Goal:
  - verify `#shared-memory` appears as a distinct system channel in the sidebar and is NOT
    listed alongside user-created channels in the normal channel list
- Preconditions:
  - server is running with at least one user channel (e.g. `#general`)
- Steps:
  1. Open the app sidebar.
  2. Verify `#shared-memory` appears in the sidebar (agents/system area or separate section).
  3. Verify `#shared-memory` is visually distinct from `#general` and other user channels
     (e.g. different label, read-only indicator, or placed in a separate section).
  4. Open the API response for `/api/server-info` and verify `#shared-memory` does NOT appear
     in the `channels` array returned to the UI (system channels are excluded from the list).
- Expected:
  - `#shared-memory` is accessible from the sidebar
  - `#shared-memory` is visually distinguishable or in a separate category
  - `#shared-memory` is excluded from `/api/server-info` channels list
- Common failure signals:
  - `#shared-memory` does not appear in the sidebar at all
  - `#shared-memory` appears in the normal channel list mixed with user channels
  - `/api/server-info` includes `#shared-memory` in the channels array
