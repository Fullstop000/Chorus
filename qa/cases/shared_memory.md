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

---

## Behavioral Cases — Autonomous Tool Usage

These cases verify that agents call `remember` and `recall` **without being explicitly
told to** in the prompt. They test the mandatory-trigger guidance added to the system
prompt. Pass criteria are always observable facts (DB rows, breadcrumb count, absence
of re-explanation in chat), not agent self-reports.

### MEM-007 Recall On Task Assignment (Trigger: assigned a task)

- Tier: 0
- Release-sensitive: yes when touching system prompt or agent lifecycle
- Goal:
  - verify an agent calls `recall` unprompted when assigned a task via the task board,
    and uses the retrieved context in its reply — without the human mentioning recall
- Preconditions:
  - at least one active agent exists (use `qa-test-agent` or any claude agent)
  - the `shared_knowledge` table is pre-seeded with a fact the agent will need
    (use `POST /internal/agent/{id}/remember` directly or run MEM-001 first)
- Steps:
  1. Pre-seed a fact into shared knowledge via the API:
     ```
     POST /internal/agent/qa-test-agent/remember
     { "key": "design-decision", "value": "use-sqlite-fts5-not-elastic", "tags": "task-beh-001" }
     ```
  2. Assign a task to the agent that requires that knowledge:
     In `#general`, send:
     `@qa-test-agent — I've assigned you task #BEH-001: write a one-line summary of
     our search implementation decision. Check shared memory for prior research on
     tag 'task-beh-001' before answering.`
     (Note: mention tag but do NOT say "call recall")
  3. Wait for the agent's turn to complete.
  4. Check `shared_knowledge` is unchanged (agent should read, not re-write).
  5. Check `#shared-memory` breadcrumb count has NOT increased (recall is read-only).
  6. Verify the agent's reply in `#general` references `sqlite-fts5` — proving it
     retrieved and used the stored fact rather than guessing.
  7. Check agent activity log or message content for evidence of a `recall` tool call
     (agent will typically mention "I recalled..." or the reply will contain exact
     stored wording).
- Expected:
  - agent calls `recall` with `tags="task-beh-001"` before composing reply
  - reply contains the stored value (`use-sqlite-fts5-not-elastic`) or derived content
  - no new rows added to `shared_knowledge` by this agent for this task
  - agent does not ask "what search implementation?" — it found the answer itself
- Common failure signals:
  - agent replies without mentioning the stored fact (did not call recall)
  - agent asks the human to clarify what was decided (forgot to recall)
  - agent calls `remember` instead of `recall` (confused the direction)
  - recall is called but ignored — reply contradicts stored value

### MEM-008 Remember Before Handoff (Trigger: research complete, about to assign)

- Tier: 0
- Release-sensitive: yes when touching system prompt or agent lifecycle
- Goal:
  - verify an agent calls `remember` to persist research findings **before** or
    **during** handing off work, without the human saying "use remember"
- Preconditions:
  - at least one active agent exists
  - `shared_knowledge` row count is known before the test (record the baseline)
- Steps:
  1. Record baseline: `SELECT COUNT(*) FROM shared_knowledge;`
  2. In `#general`, ask an agent to research a concrete answerable question and
     then assign the follow-up to another agent. Do NOT mention `remember`:
     `@qa-test-agent — please find out what port Chorus runs the API on, then
     assign the task of writing a curl example to @test-agent.`
  3. Wait for `qa-test-agent`'s turn to complete (it replies and/or assigns the task).
  4. Check `shared_knowledge` row count — it must be > baseline.
  5. Verify the new row(s) contain relevant content (port number, API info).
  6. Check `#shared-memory` for a new breadcrumb from `qa-test-agent`.
  7. Navigate to `#shared-memory` in the UI and verify the breadcrumb is visible
     with correct key/value and agent name.
- Expected:
  - `shared_knowledge` gains at least one new row before or alongside the handoff
  - new row contains the researched fact (e.g. port `3001`)
  - breadcrumb appears in `#shared-memory` from `qa-test-agent`
  - `qa-test-agent` does NOT just describe the port in a long message to `test-agent`
    and rely on `test-agent` reading chat history
- Common failure signals:
  - `shared_knowledge` row count unchanged after agent replies (did not remember)
  - agent only described findings in chat message, no `remember` call
  - breadcrumb missing from `#shared-memory`
  - `test-agent` receives the task but has no entry to recall — pure chat handoff

### MEM-009 Two-Agent Handoff Without Human Mediation (Tier 1)

- Tier: 1
- Release-sensitive: yes when touching system prompt, agent wakeup, task board, or knowledge store
- Goal:
  - verify the full autonomous cycle: agent A researches and stores findings,
    agent B wakes from a task assignment, calls recall unprompted, and completes
    the work — with zero human prompts to B about what was found
- Preconditions:
  - two active claude agents exist (e.g. `qa-test-agent` and `test-agent`)
  - task board is accessible in the UI
- Steps:
  1. Record baseline `shared_knowledge` row count.
  2. In `#general`, send a single prompt to agent A only — do NOT address agent B:
     `@qa-test-agent — research what HTTP method is used to send a message in Chorus
     (hint: check the bridge or API code), store your findings in shared memory
     tagged 'task-beh-009', then assign the curl-example task to @test-agent.`
  3. Wait for `qa-test-agent` to complete its turn (reply + task assignment).
  4. Verify `shared_knowledge` has new row(s) tagged `task-beh-009` from `qa-test-agent`.
  5. Verify `#shared-memory` shows breadcrumb from `qa-test-agent`.
  6. Wait for `test-agent` to wake and complete its turn (task board notification or message).
  7. Verify `test-agent`'s reply includes a `curl` example with the correct HTTP method.
  8. Verify `test-agent` did NOT ask the human "what method is used?" — it found the
     answer via `recall`, not via re-asking.
  9. (Optional) Check agent activity log for `test-agent` showing a `recall` tool call.
- Expected:
  - `qa-test-agent` stores at least one finding before assigning the task
  - `test-agent` wakes, calls recall with tag `task-beh-009` or the subject keyword
  - `test-agent` produces a correct curl example referencing the stored method (POST)
  - no human message addresses `test-agent` before it replies
  - the human needed to send exactly **one** message to drive the whole cycle
- Common failure signals:
  - `qa-test-agent` assigns the task but skips `remember` (pure chat handoff)
  - `test-agent` wakes and asks the human for clarification (missed recall)
  - `test-agent` produces a wrong answer (recalled nothing, guessed)
  - neither agent's activity shows a knowledge store tool call

### MEM-010 No Re-Explanation In Chat When Shared Memory Exists

- Tier: 1
- Release-sensitive: yes when touching system prompt
- Goal:
  - verify that when shared memory contains context an agent needs, the agent
    uses it rather than sending a verbose re-explanation to another agent via chat
- Preconditions:
  - MEM-009 completed (findings stored under `task-beh-009`)
  - both agents are active
- Steps:
  1. In `#general`, ask `test-agent` the same research question that `qa-test-agent`
     already answered in MEM-009 — without referencing the prior work:
     `@test-agent — what HTTP method does Chorus use to send a message? Brief answer only.`
  2. Wait for `test-agent`'s reply.
  3. Verify `test-agent` answers correctly and concisely.
  4. Verify `test-agent` did NOT re-investigate (no file reads, no code searches) —
     it should have used stored knowledge.
  5. Verify NO new `remember` call was made (row count unchanged from MEM-009 end state).
- Expected:
  - `test-agent` answers correctly from recalled memory
  - reply is concise — no wall-of-text re-explanation of how the codebase works
  - `shared_knowledge` row count unchanged (read-only turn)
- Common failure signals:
  - `test-agent` re-searches the codebase instead of recalling
  - answer is wrong (recall not used or returned wrong entry)
  - agent calls `remember` again for the same fact (redundant write)
