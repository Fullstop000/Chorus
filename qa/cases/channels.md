# Channel Cases

### CHN-001 Channel Create And Default Membership

- Tier: 0
- Release-sensitive: yes
- Execution mode: browser
- Goal:
  - verify a new channel can be created from the product, starts with only the human creator, and becomes usable after explicit invites
- Preconditions:
  - at least 3 test agents exist
- Steps:
  1. Create a new disposable channel such as `#qa-ops`.
  2. Verify it appears in the sidebar immediately.
  3. Open the new channel and verify the empty state is sane.
  4. Open the members rail and verify the count starts at `1`, showing only the current human user.
  5. Invite one agent into the channel through the shipped member control.
  6. Send one human message asking the invited agent to reply.
  7. Verify the invited agent replies in the new channel and uninvited agents do not.
  8. Navigate away and back, then verify the new channel history and membership count persist.
- Expected:
  - channel create succeeds
  - sidebar updates immediately
  - new channel starts with only the creator as a member
  - invited members become active participants in that channel
  - uninvited agents do not behave like implicit members
- Common failure signals:
  - channel create succeeds but sidebar does not update
  - member rail shows agents before any invite occurs
  - uninvited agents receive or reply in the new channel
  - invited replies land in the wrong channel

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

### CHN-003 Channel Invite Operations And `#all` Guardrails

- Tier: 1
- Release-sensitive: yes when explicit membership controls exist or channel delivery logic changes
- Execution mode: browser
- Goal:
  - verify invite controls add the intended members to user channels, and verify the built-in `#all` channel behaves as a complete-membership system room with no invite affordance
- Preconditions:
  - disposable user channel exists
  - at least one extra human and one extra agent exist in the build under test
- Steps:
  1. Open `#all`.
  2. Open the members rail.
  3. Verify there is no invite button in `#all`.
  4. Verify the member list includes all visible humans and agents from the sidebar.
  5. Open the disposable user channel.
  6. Open the members rail and invite one extra human and one extra agent.
  7. Verify the member count increments after each invite and the newly invited entries appear in the list.
  8. Refresh the page and confirm the disposable channel still shows the invited members.
- Expected:
  - `#all` exposes no invite affordance
  - `#all` member list is the full set of humans and agents
  - user-channel invites update the member list immediately
  - invited membership persists after refresh
- Common failure signals:
  - `#all` shows an invite button or incomplete membership
  - invited human or agent does not appear until a manual refresh
  - refreshed page drops invited members from the disposable channel

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
