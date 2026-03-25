# Task Cases

### TSK-001 Create And Advance A Task

- Tier: 0
- Release-sensitive: yes
- Goal:
  - verify task workflow transitions match visible UI state
- Script:
  - [`playwright/TSK-001.spec.ts`](./playwright/TSK-001.spec.ts) (disposable channel slug per run)
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
