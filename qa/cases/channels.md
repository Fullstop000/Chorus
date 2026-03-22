# Channel Cases

### CHN-001 Channel Create And Shared Availability

- Tier: 0
- Release-sensitive: yes
- Execution mode: browser
- Goal:
  - verify a new channel can be created from the product and becomes usable immediately
- Preconditions:
  - at least 3 test agents exist
- Steps:
  1. Create a new disposable channel such as `#qa-ops`.
  2. Verify it appears in the sidebar immediately.
  3. Open the new channel and verify the empty state is sane.
  4. Send one human message asking all agents to reply.
  5. Verify replies land in the new channel rather than `#general`.
  6. Navigate away and back, then verify the new channel history persists.
- Expected:
  - channel create succeeds
  - sidebar updates immediately
  - new channel is actually routable for chat traffic
  - agents and human can all use the new channel
- Common failure signals:
  - channel create succeeds but sidebar does not update
  - messages land in the wrong channel
  - new channel is visible but not usable

### CHN-002 Channel Name Validation, Normalization, And Duplicate Rejection

- Tier: 1
- Release-sensitive: yes when touching channel create, channel routing, or channel validation
- Execution mode: browser
- Goal:
  - verify channel names are normalized and duplicate channels are rejected cleanly
- Preconditions:
  - channel create modal is available
- Steps:
  1. Create a channel using mixed case or a leading `#`, for example `#Engineering`.
  2. Verify the stored/displayed name is normalized consistently, such as `engineering`.
  3. Attempt to create the same logical channel again using a different casing or with/without `#`.
  4. Attempt to create an invalid or empty channel name.
  5. Verify the UI shows a clear failure and does not create a partial sidebar entry.
- Expected:
  - normalization is consistent
  - duplicate rejection is based on the logical channel name, not raw input formatting
  - invalid names are rejected without corrupting navigation
- Common failure signals:
  - `#Engineering` and `engineering` create separate channels
  - duplicate create looks successful but produces partial state
  - invalid name is silently accepted

### CHN-003 Channel Member Add And Remove Operations

- Tier: 1
- Release-sensitive: yes when explicit membership controls exist or channel delivery logic changes
- Execution mode: hybrid
- Current product note:
  - the current build auto-joins agents to created channels and does not expose explicit add-member or remove-member controls in the normal UI
  - if that remains true for the build under test, mark this case `Blocked` and record the product gap
- Goal:
  - verify explicit membership controls change who can receive and participate in a channel
- Preconditions:
  - disposable channel exists
  - member add/remove controls exist in the current product build
- Steps:
  1. Create a disposable channel.
  2. Add `bot-a` and `bot-b`, but not `bot-c`.
  3. Send a message asking every present agent to reply.
  4. Verify only `bot-a` and `bot-b` reply.
  5. Remove `bot-b`.
  6. Send another message.
  7. Verify `bot-a` can still reply and `bot-b` no longer receives the message.
  8. Verify membership changes remain correct after page reload.
- Expected:
  - member add/remove changes actual delivery and participation
  - removed members stop receiving messages
  - non-members do not appear as active channel participants
- Common failure signals:
  - removed member still replies
  - non-member receives channel traffic
  - membership UI updates but delivery behavior does not

### CHN-004 Channel Delete And Selection Recovery

- Tier: 1
- Release-sensitive: yes when a delete flow exists or channel persistence changes
- Execution mode: hybrid
- Current product note:
  - the current build does not expose a normal delete-channel flow
  - if that remains true for the build under test, mark this case `Blocked` and record the product gap
- Goal:
  - verify deleting a channel removes it cleanly and leaves the UI in a sane selection state
- Preconditions:
  - disposable channel exists
  - delete control exists in the current product build
- Steps:
  1. Create a disposable channel and open it.
  2. Put some message history into it.
  3. Delete the channel through the shipped control.
  4. Verify the channel disappears from the sidebar.
  5. Verify the main panel falls back to a sane target instead of rendering stale channel state.
  6. Refresh and confirm the deleted channel does not reappear.
- Expected:
  - deleted channel is removed from navigation and selection state
  - no stale chat view remains attached to the deleted channel
  - refresh preserves the deleted state
- Common failure signals:
  - deleted channel remains selected
  - refresh resurrects deleted channel
  - old history appears under the wrong target
