# Task Cases

## Smoke

### TSK-001 Create And Advance A Task

- Suite: smoke
- Goal: verify task workflow transitions match visible UI state
- Script: [`playwright/TSK-001.spec.ts`](./playwright/TSK-001.spec.ts)
- Steps:
  1. Open `Tasks`.
  2. Create a new task with an unambiguous title.
  3. Verify it appears in `To Do`.
  4. Advance it once.
  5. Verify it moves to the correct next state.
  6. Watch console and network responses during the transition.
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
