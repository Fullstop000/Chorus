# Team Cases

## Smoke

### TMT-001 Team Create, Badge, And Sidebar

- Suite: smoke
- Goal: Verify team creation via UI produces a `[team]` badge in the channel list and persists across refresh.
- Script: [`playwright/TMT-001.spec.ts`](./playwright/TMT-001.spec.ts)
- Preconditions: at least 2 agents exist
- Steps:
  1. Click `+ New Channel` and verify the modal has a Channel / Team toggle.
  2. Switch the toggle to `Team`.
  3. Fill in name `qa-eng`, display name `QA Engineering`, and add at least two initial members.
  4. Submit the form.
  5. Verify `#qa-eng` appears in the Channels list with a `[team]` badge, not a `[sys]` badge.
  6. Verify it does NOT appear in any separate Teams section.
  7. Refresh the page and verify `#qa-eng` still appears with the `[team]` badge.
- Expected:
  - Team channel appears with `[team]` badge; badge and membership persist after refresh.

---

### TMT-002 @mention Routing To Team Channel

- Suite: smoke
- Goal: Verify `@<team-name>` in any channel forwards a copy to the team channel with `forwarded_from` metadata.
- Script: [`playwright/TMT-002.spec.ts`](./playwright/TMT-002.spec.ts)
- Preconditions:
  - team `qa-eng` exists (from TMT-001) with at least one active agent member
- Steps:
  1. Open `#all` (or `#general`).
  2. Post: `@qa-eng please build a landing page`.
  3. Verify the message appears in `#all` as sent.
  4. Open `#qa-eng`.
  5. Verify a copy of the message appears in `#qa-eng`.
  6. Verify the copy shows forwarded-from attribution (origin channel and sender).
  7. Verify the agent member in `#qa-eng` responds or shows activity.
- Expected:
  - Exactly one forwarded copy with attribution; agent wakes and receives it.

---

### TMT-005 Team Member Management

- Suite: smoke
- Goal: Verify members can be added/removed via team settings and changes reflect in the channel and agent behavior.
- Script: [`playwright/TMT-005.spec.ts`](./playwright/TMT-005.spec.ts)
- Preconditions:
  - team `qa-eng` exists with one agent member (`bot-a`)
  - a second agent `bot-b` exists but is not a member of `qa-eng`
- Steps:
  1. Open `#qa-eng` team settings.
  2. Add `bot-b` as an operator.
  3. Verify `bot-b` appears in the members list in the team settings panel.
  4. Verify `bot-b` is listed in the channel members rail.
  5. Post a message in `#qa-eng` and verify `bot-b` receives and responds.
  6. Remove `bot-b` from the team via team settings.
  7. Verify `bot-b` disappears from the team settings member list.
  8. Verify `bot-b` is removed from the channel members rail.
  9. Post another message in `#qa-eng` and verify `bot-b` does NOT respond.
  10. Refresh and verify member state is consistent.
- Expected:
  - Added member appears in settings and rail, receives messages; removed member disappears and stops receiving.

---

## Regression

### TMT-007 Team Delete

- Suite: regression
- Goal: Verify deleting a team archives the channel, cleans up the workspace, and removes the team from agent system prompts.
- Script: [`playwright/TMT-007.spec.ts`](./playwright/TMT-007.spec.ts) (**disposable** `qa-del-*` team; step 9 not automated)
- Preconditions:
  - team `qa-eng` exists with at least one agent member (`bot-a`)
  - some messages exist in `#qa-eng`
- Steps:
  1. Open `#qa-eng` team settings.
  2. Click the delete team button.
  3. Confirm the deletion in any confirmation dialog.
  4. Verify `#qa-eng` disappears from the channel list.
  5. Verify the UI does not crash and falls back to a default channel.
  6. Refresh the page and verify `#qa-eng` does not reappear.
  7. Verify `bot-a` is still active and handling messages in other channels.
  8. Post in `#all` addressing `bot-a` and ask it to list its teams; verify it does not mention `qa-eng`.
  9. Attempt to create a new channel named `qa-eng` (non-team); verify this succeeds without conflict.
- Expected:
  - Channel gone from nav after delete; agents still function; deletion permanent after refresh.
- Common failure signals:
  - Deleted team channel reappears after refresh
  - `bot-a` stops responding in `#all` after team deletion
  - Agent still mentions deleted team in its self-description

---

### TMT-009 Agent Team Thread Wake

- Suite: regression
- Execution mode: hybrid
- Goal: Verify a stopped agent wakes and replies inside a team thread without flattening into the top-level timeline.
- Script: [`playwright/TMT-009.spec.ts`](./playwright/TMT-009.spec.ts) (runtime matrix; hybrid setup + browser thread flow + API polling)
- Preconditions:
  - mixed-runtime trio with at least one Claude agent and one Codex agent
  - per-runtime team channels with the agent as member and human as observer
- Steps:
  1. For each runtime under test, seed a top-level parent message from that agent in its team channel.
  2. Stop that agent so the next thread message must wake it.
  3. Open the team channel.
  4. Open a thread from the agent-authored parent message.
  5. In the thread, send a message asking the stopped agent to reply with an exact token.
  6. Wait for the agent to wake and reply.
  7. Verify the exact-token reply appears in the thread.
  8. Verify the top-level timeline does not show the thread-only reply as a channel message.
  9. Poll agent state and verify it returned to `active`.
- Expected:
  - Agent wakes across runtimes, replies in-thread only, lifecycle status agrees.
- Common failure signals:
  - One runtime wakes correctly while another does not
  - Agent posts top-level instead of in-thread
  - Agent never wakes on a thread it owns
