# Team Cases

### TMT-001 Team Create, Channel Badge, and Sidebar Appearance

- Tier: 0
- Release-sensitive: yes
- Execution mode: browser
- Goal:
  - verify a team can be created through the UI, gets a `[team]` badge in the channel list, and does not appear as a plain user channel
- Script:
  - [`playwright/TMT-001.spec.ts`](./playwright/TMT-001.spec.ts)
- Preconditions:
  - at least 2 agents exist
- Steps:
  1. Click `+ New Channel` and verify the modal has a Channel / Team toggle at the top.
  2. Switch the toggle to `Team`.
  3. Fill in name `qa-eng`, display name `QA Engineering`, collaboration model `Leader+Operators`, assign one agent as leader and one as operator.
  4. Submit the form.
  5. Verify `#qa-eng` appears in the Channels list with a `[team]` badge, not a `[sys]` badge.
  6. Verify it does NOT appear in any separate Teams section (there should be none).
  7. Click `+ New Team` shortcut (next to `+ New Channel`) and verify the same modal opens with the Team tab pre-selected.
  8. Cancel without creating.
  9. Open `#qa-eng` and verify the channel header shows team-specific settings (collab model, member roles, delete button).
  10. Refresh the page and verify `#qa-eng` still appears with the `[team]` badge.
- Expected:
  - team channel appears in Channels list with `[team]` badge
  - both creation paths (`+ New Channel → Team` and `+ New Team`) work
  - team settings panel is distinct from the regular channel edit modal
  - badge and membership persist after refresh
- Common failure signals:
  - `[team]` badge is absent; channel renders as a plain user channel
  - `+ New Team` shortcut is missing or opens the wrong modal
  - team settings show only generic channel fields
  - refresh causes the badge or channel to disappear

---

### TMT-002 @mention Routing Forwards Message to Team Channel

- Tier: 0
- Release-sensitive: yes
- Execution mode: browser
- Goal:
  - verify that posting a message containing `@<team-name>` in any channel causes a copy to appear in the team's channel, with `forwarded_from` metadata visible to agents
- Script:
  - [`playwright/TMT-002.spec.ts`](./playwright/TMT-002.spec.ts) (Steps 5–6/9 hybrid `history` as `bot-a`; annotates human `forwardedFrom` gap)
- Preconditions:
  - team `qa-eng` exists (from TMT-001) with at least one agent member
  - agent member is active
- Steps:
  1. Open `#all` (or `#general`).
  2. Post the message: `@qa-eng please build a landing page`.
  3. Verify the message appears in `#all` as sent.
  4. Open `#qa-eng`.
  5. Verify a copy of the message appears in `#qa-eng`.
  6. Verify the copied message indicates it was forwarded (origin channel and sender visible in the UI, or agent receives the `forwarded_from` metadata in their context).
  7. Verify the agent member in `#qa-eng` responds or shows activity (it received the wakeup).
  8. Post a message in `#all` that mentions two teams, e.g. `@qa-eng and @qa-algo both review this`.
  9. Verify the message is forwarded into both team channels (create `qa-algo` team first if needed).
- Expected:
  - @mention triggers exactly one forwarded copy per team mentioned
  - copy appears in team channel with forwarded-from attribution
  - agent members wake up and receive the forwarded message
  - multiple @team mentions in one message forward to all mentioned teams
- Common failure signals:
  - message not copied to team channel
  - duplicate copies appear in team channel
  - forwarded_from attribution missing
  - agent does not wake up after forwarded message arrives

---

### TMT-005 Team Member Management (Add, Remove, Role)

- Tier: 1
- Release-sensitive: yes when team membership or channel membership logic changes
- Execution mode: browser
- Goal:
  - verify members can be added and removed through the team settings panel, and that membership changes are reflected in the channel and in the agent's system prompt
- Script:
  - [`playwright/TMT-005.spec.ts`](./playwright/TMT-005.spec.ts) (Steps 5/9 LLM-dependent; membership UI + rail covered)
- Preconditions:
  - team `qa-eng` exists with one agent member (`bot-a`)
  - a second agent `bot-b` exists but is not a member of `qa-eng`
- Steps:
  1. Open `#qa-eng` team settings.
  2. Add `bot-b` as an operator.
  3. Verify `bot-b` appears in the members list in the team settings panel.
  4. Verify `bot-b` is now listed as a member in the channel members rail.
  5. Post a message in `#qa-eng` and verify `bot-b` receives and responds to it (confirming it woke up as a member).
  6. Remove `bot-b` from the team via the team settings panel.
  7. Verify `bot-b` disappears from the team settings member list.
  8. Verify `bot-b` is removed from the channel members rail.
  9. Post another message in `#qa-eng` and verify `bot-b` does NOT respond (it is no longer a member).
  10. Refresh and verify member state is consistent.
- Expected:
  - added member appears in both team settings and channel members rail
  - added member receives messages in the team channel
  - removed member disappears from both lists
  - removed member no longer receives team channel messages
- Common failure signals:
  - added member not visible in channel members rail
  - added member does not wake up on new messages
  - removed member remains in channel membership
  - refresh resurrects removed member

---

### TMT-007 Team Delete — Channel Archive and Workspace Cleanup

- Tier: 1
- Release-sensitive: yes when team deletion or channel archive logic changes
- Execution mode: browser
- Goal:
  - verify that deleting a team archives the channel (preserving message history), removes the workspace on disk, and rebuilds agent system prompts so former members no longer reference the team
- Script:
  - [`playwright/TMT-007.spec.ts`](./playwright/TMT-007.spec.ts) (**disposable** `qa-del-*` team — catalog uses `qa-eng`; see spec header. Step 9 not automated.)
- Preconditions:
  - team `qa-eng` exists with at least one agent member (`bot-a`)
  - some messages exist in `#qa-eng`
- Steps:
  1. Open `#qa-eng` team settings.
  2. Click the delete team button.
  3. Confirm the deletion in any confirmation dialog.
  4. Verify `#qa-eng` disappears from the channel list.
  5. Verify the UI does not crash and falls back to a sane default channel.
  6. Refresh the page and verify `#qa-eng` does not reappear.
  7. Verify `bot-a` is still active and handling messages in other channels (it was not destroyed, just removed from the team).
  8. Post a message in `#all` addressing `bot-a` and ask it to list its teams; verify it does not mention `qa-eng`.
  9. Attempt to create a new channel named `qa-eng` (non-team); verify this succeeds without conflict (archived channel should not block the name — note: if name uniqueness prevents this, mark as a known limitation).
- Expected:
  - team channel disappears from navigation after delete
  - former agent members continue to function in other channels
  - agent no longer reports `qa-eng` in its team list
  - deletion is permanent after refresh
- Common failure signals:
  - deleted team channel reappears after refresh
  - `bot-a` stops responding in `#all` after team deletion
  - agent still mentions deleted team in its self-description
  - UI crashes or leaves stale state after delete

---

### TMT-009 Agent Team Thread Wake And In-Thread Reply

- Tier: 0
- Release-sensitive: yes when touching agent lifecycle, restart prompts, bridge formatting, thread routing, or team-channel messaging
- Execution mode: hybrid
- Goal:
  - verify a stopped agent can wake and reply in a team thread it already belongs to
  - verify the reply remains in the thread and does not flatten into the top-level team timeline
- Script:
  - [`playwright/TMT-009.spec.ts`](./playwright/TMT-009.spec.ts) (runtime matrix across supported agents; hybrid setup for agent-authored parent; browser thread flow; hybrid API polling for exact thread history and agent status)
- Preconditions:
  - mixed-runtime trio exists with at least one Claude agent and one Codex agent
  - per-runtime team channels exist with the selected agent as a member and the current human user as an observer
- Steps:
  1. For each supported runtime under test, seed a unique top-level parent message from that agent in its team channel.
  2. Stop that agent so the next thread message must wake it.
  3. Open the team channel.
  4. Open a thread from the agent-authored parent message.
  5. In the thread, send a message asking the stopped agent to reply with an exact token.
  6. Wait for the agent to wake and reply.
  7. Verify the exact-token reply appears in the thread.
  8. Verify the top-level team timeline does not show the thread-only reply as a normal channel message.
  9. Open the agent profile or poll agent state and verify the wake path returned it to `active`.
- Expected:
  - stopped agents across supported runtimes wake from a thread where they are already the parent author
  - reply appears under the same thread target
  - top-level team timeline still contains only the parent message
  - lifecycle status and visible thread history agree
- Common failure signals:
  - one runtime wakes correctly while another does not
  - stopped agent never wakes on a thread it owns
  - agent wakes but posts in `#qa-codex-thread` top-level instead of the thread
  - agent replies in the wrong target or drops the exact token request
  - profile shows active but thread history never updates
