# Task Cases

## Smoke

### TSK-001 Create And Advance A Task

- Suite: smoke
- Goal: verify claim and start are two separate user actions on the parent-channel TaskCard, and the same card updates in place
- Script: [`playwright/TSK-001.spec.ts`](./playwright/TSK-001.spec.ts)
- Steps:
  1. Open `Tasks` on a fresh channel.
  2. Create a new task with an unambiguous title; verify it appears in `To Do`.
  3. Switch to `Chat`; verify the parent-channel TaskCard renders in `todo`.
  4. Click `[claim]` on the card; verify the owner badge appears.
  5. Click `[start]` on the card; verify the card flips to `in_progress`.
  6. Re-open `Tasks`; verify the task is in the `In Progress` column.
- Expected: claim and start are decoupled, each updates the same card in place, and UI matches backend

### TSK-002 Create Message-As-Task From Composer

- Suite: smoke
- Goal: verify the chat composer can create a task while sending a message
- Script: [`playwright/TSK-002.spec.ts`](./playwright/TSK-002.spec.ts)
- Steps:
  1. Enable `Also create as a task`.
  2. Send a message that should also create a task.
  3. Verify the message lands in chat.
  4. Open `Tasks` and verify a matching task was created.
- Expected: one chat message, one matching task, no duplicate send

### TSK-003 Task Sub-Channel Lifecycle

- Suite: smoke
- Goal: verify task sub-channels stay hidden from the sidebar, carry the embedded chat, advance through the unified state machine driven by the TaskCard, and survive archival on Done
- Script: [`playwright/TSK-003.spec.ts`](./playwright/TSK-003.spec.ts)
- Steps:
  1. Create a channel.
  2. Create a task with a unique title.
  3. Verify the task sub-channel does not appear in the sidebar.
  4. Switch to `Chat`; verify the parent-channel TaskCard renders in `todo`.
  5. Click `[claim]` then `[start]` on the card; verify status flips to `in_progress`.
  6. Click the card deep-link; in `TaskDetail` post a message in the embedded chat and verify it appears.
  7. Back in chat, click `[send for review]` on the card; verify `data-status="in_review"`.
  8. Click `[mark done]` on the card; verify it collapses to the `task-card-done-pill`.
  9. Reopen the task detail from the kanban board and verify the posted sub-channel message is still visible.
- Expected: sub-channel is never shown in the sidebar; status transitions succeed; archival preserves message history

### TSK-004 Channel Task-Event Feed

- Suite: smoke
- Goal: verify task lifecycle transitions surface as a single living `TaskCard` host message in the parent channel chat and collapse to the done pill when terminal
- Script: [`playwright/TSK-004.spec.ts`](./playwright/TSK-004.spec.ts)
- Steps:
  1. Create a channel.
  2. Create a task via the kanban tab.
  3. Open the parent channel chat and verify the task card renders in the `todo` state.
  4. Click `[claim]` then `[start]` on the card; verify `data-status="in_progress"`.
  5. Click `[send for review]`; verify `data-status` flips to `in_review` in place.
  6. Click `[mark done]`; verify the card collapses to the done pill (`task-card-done-pill` visible, `data-status="done"`).
- Expected: one task → one card; state transitions update the same card without duplication; terminal done swaps to the compact pill view

### TSK-005 Agent Proposal And Snapshot Kickoff

- Suite: smoke
- Goal: verify the agent-scoped propose endpoint snapshots the source message into the proposal, the parent-channel TaskCard renders the snapshot, accepting flips it to `todo`, and the sub-channel kickoff carries the three snapshot sections
- Script: [`playwright/TSK-005.spec.ts`](./playwright/TSK-005.spec.ts)
- Steps:
  1. Create a parent channel.
  2. Seed a source chat message via the API and capture its id.
  3. Agent calls `POST /internal/agent/{agent}/tasks/propose` with `{ channel, title, source_message_id }`.
  4. Reload the UI; verify the parent-channel TaskCard renders in `proposed` with the snapshot blockquote (sender + content).
  5. Click `[create]`; verify the card flips to `todo` and shows the `[claim]` CTA.
  6. Advance the card to `in_progress` (claim → start) so the sub-channel deep-link surfaces.
  7. Click the deep-link; in `TaskDetail` verify the kickoff system message contains `Task opened: {title}`, `From @{sender}'s message in #{parent}:`, and `> {content}`.
- Expected: proposal carries the snapshot end-to-end; acceptance mints the sub-channel; kickoff matches the unified-lifecycle contract

### TSK-006 Full Lifecycle Smoke

- Suite: smoke
- Goal: drive a single direct-created task through every CTA branch on the parent-channel TaskCard, end to end, and verify the done pill links into the sub-channel
- Script: [`playwright/TSK-006.spec.ts`](./playwright/TSK-006.spec.ts)
- Steps:
  1. Create a channel and a task with a unique title.
  2. Switch to `Chat`; verify the card renders with `data-status="todo"`, `data-claimed="false"`, and the `[claim]` CTA.
  3. Click `[claim]`; verify the owner badge and `[start]` CTA appear (status stays `todo`).
  4. Click `[start]`; verify `data-status="in_progress"` and `[send for review]` CTA.
  5. Click `[send for review]`; verify `data-status="in_review"` and `[mark done]` CTA.
  6. Click `[mark done]`; verify the card collapses to `task-card-done-pill`.
  7. Click the pill; verify the sub-channel opens via `TaskDetail`.
- Expected: every CTA mutates the same card in place; the forward-only state machine never goes backwards; the done pill is a working sub-channel link
