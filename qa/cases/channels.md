# Channel Cases

## Smoke

### CHN-001 Channel Create And Default Membership

- Suite: smoke
- Goal: verify channel create, default single-member state, invite flow, and `#all` invite guardrail
- Script: [`playwright/CHN-001.spec.ts`](./playwright/CHN-001.spec.ts)
- Preconditions: at least 3 test agents exist
- Steps:
  1. Create a new disposable channel such as `#qa-ops`.
  2. Verify it appears in the sidebar immediately.
  3. Open the new channel and verify the empty state is sane.
  4. Open the members rail and verify the count starts at `1`, showing only the current human user.
  5. Invite one agent into the channel through the shipped member control.
  6. Send one human message asking the invited agent to reply.
  7. Open `#all` members rail and verify no invite button exists.
  8. Navigate away and back, then verify the new channel history and membership count persist.
- Expected:
  - Channel creates and appears in sidebar immediately
  - New channel starts with only the creator as a member
  - Invited agent becomes active participant
  - `#all` exposes no invite affordance
  - Membership persists after navigation

### CHN-002 Channel Name Validation, Normalization, And Duplicate Rejection

- Suite: smoke
- Goal: verify channel names are normalized and duplicates are rejected cleanly
- Script: [`playwright/CHN-002.spec.ts`](./playwright/CHN-002.spec.ts)
- Steps:
  1. Create a channel using mixed case or a leading `#`, for example `#Engineering`.
  2. Verify the stored/displayed name is normalized consistently, such as `engineering`.
  3. Attempt to create the same logical channel again using a different casing or with/without `#`.
  4. Attempt to create an invalid or empty channel name.
  5. Verify the UI shows a clear failure and does not create a partial sidebar entry.
- Expected:
  - Normalization is consistent across casing and `#` prefix
  - Duplicate rejection uses the logical name, not raw input
  - Invalid names are rejected without corrupting navigation

### CHN-003 Channel Rename Updates Sidebar Immediately

- Suite: smoke
- Supersedes: CHN-005
- Goal: verify renaming a channel updates sidebar and chat header immediately without reload
- Script: [`playwright/CHN-003.spec.ts`](./playwright/CHN-003.spec.ts)
- Preconditions: channel edit control exists in the current build
- Steps:
  1. Create a disposable channel and open it.
  2. Trigger the shipped edit-channel flow from the sidebar or channel controls.
  3. Rename the channel to a distinct value and save.
  4. Verify the modal closes cleanly.
  5. Verify the sidebar shows only the new name.
  6. Verify the open chat header also updates to the new name without reload.
- Expected:
  - Rename persists; sidebar and header update immediately
  - Old name does not linger in navigation

## Regression

### CHN-004 Channel Delete And Selection Recovery

- Suite: regression
- Execution mode: hybrid
- Blocked until shipped: the current build does not expose a delete-channel flow; if still true, mark `Blocked` and record the product gap
- Goal: verify deleting a channel removes it cleanly and leaves the UI in a sane selection state
- Script: [`playwright/CHN-004.spec.ts`](./playwright/CHN-004.spec.ts)
- Preconditions: delete control exists in the current product build
- Steps:
  1. Create a disposable channel and open it.
  2. Put some message history into it.
  3. Delete the channel through the shipped control.
  4. Verify the channel disappears from the sidebar.
  5. Verify the main panel falls back to a sane target instead of rendering stale channel state.
  6. Refresh and confirm the deleted channel does not reappear.
- Expected:
  - Deleted channel is removed from navigation and selection state
  - No stale chat view remains attached to the deleted channel
  - Refresh preserves the deleted state
- Common failure signals:
  - Deleted channel remains selected after delete
  - Refresh resurrects deleted channel
  - Old history appears under the wrong target
