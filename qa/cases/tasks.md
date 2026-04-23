# Task Cases

## Smoke

### TSK-001 Create And Advance A Task

- Suite: smoke
- Goal: verify task workflow transitions match visible UI state
- Script: [`playwright/TSK-001.spec.ts`](./playwright/TSK-001.spec.ts)
- Steps:
  1. Open `Tasks` on a fresh channel.
  2. Create a new task with an unambiguous title.
  3. Verify it appears in `To Do`.
  4. Click the card to open the TaskDetail view.
  5. Click `Start` in TaskDetail to claim + advance the task.
  6. Return to the board via the back button.
  7. Verify the card has moved to `In Progress`.
- Expected: state change succeeds, card moves exactly once, visual state matches backend

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
- Goal: verify task sub-channels stay hidden from the sidebar, carry the embedded chat, and survive archival on Done
- Script: [`playwright/TSK-003.spec.ts`](./playwright/TSK-003.spec.ts)
- Steps:
  1. Create a channel.
  2. Create a task with a unique title.
  3. Verify the task sub-channel does not appear in the sidebar.
  4. Click the task card and verify the TaskDetail view renders.
  5. Post a message in the embedded chat and verify it appears.
  6. Advance the task through `Start` → `Submit for review` → `Mark done`.
  7. Verify the task lands in `Done` on the board.
  8. Reopen the task detail after Done and verify the posted message is still visible.
- Expected: sub-channel is never shown in the sidebar; status transitions succeed; archival preserves message history
