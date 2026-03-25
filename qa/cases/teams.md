# Team Cases

### TMT-001 Team Create, Channel Badge, and Sidebar Appearance

- Tier: 0
- Release-sensitive: yes
- Execution mode: browser
- Goal:
  - verify a team can be created through the UI, gets a `[team]` badge in the channel list, and does not appear as a plain user channel
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

### TMT-003 Leader+Operators Collaboration Model

- Tier: 0
- Release-sensitive: yes
- Execution mode: browser
- Goal:
  - verify that in a Leader+Operators team, a forwarded task goes to the leader, the leader delegates to operators, and the result is reported back
- Preconditions:
  - team `qa-eng` exists with collaboration model `leader_operators`, one leader agent (`bot-a`), one operator agent (`bot-b`), both active
- Steps:
  1. Open `#all` and post: `@qa-eng build a simple to-do list app`.
  2. Open `#qa-eng` and observe what happens.
  3. Verify the leader agent (`bot-a`) picks up the task first (posts decomposition or delegation message).
  4. Verify the leader delegates a subtask to the operator via DM or a message directed at `bot-b`.
  5. Verify the operator (`bot-b`) acknowledges and works on the subtask.
  6. Verify the leader eventually posts a summary/synthesis back to the channel.
  7. Verify there is NO deliberation phase prompt (no "discuss your approach" system message).
- Expected:
  - no deliberation prompt appears in the team channel
  - leader picks up the task and delegates
  - operator responds to delegation
  - leader synthesizes and reports back
- Common failure signals:
  - operator picks up the task directly, bypassing the leader
  - deliberation prompt appears (wrong collab model behavior)
  - leader takes no action after forwarded message
  - neither agent responds

---

### TMT-004 Swarm Collaboration Model with Deliberation Phase

- Tier: 0
- Release-sensitive: yes
- Execution mode: browser
- Goal:
  - verify the Swarm model's two-phase flow: deliberation prompt appears on task arrival, agents discuss, each posts `READY:`, system posts GO message, agents execute
- Preconditions:
  - team `qa-swarm` exists with collaboration model `swarm`, two agent members (`bot-a`, `bot-b`), both active
- Steps:
  1. Open `#all` and post: `@qa-swarm research the best frontend framework`.
  2. Open `#qa-swarm` immediately.
  3. Verify a system message appears containing "New task received" and instructs agents to post `READY:`.
  4. Observe both agents posting responses discussing their approach.
  5. Wait for both agents to post messages beginning with `READY:`.
  6. Verify a system message appears stating "All members ready — execution begins."
  7. Verify agents then begin executing their declared subtasks.
  8. Post a second task `@qa-swarm also summarize the results` before the first is resolved.
  9. Verify the second task queues correctly (agents handle the first task's quorum before the second).
- Expected:
  - deliberation prompt appears immediately after forwarded message
  - `READY:` signals from both agents are detected
  - GO system message appears only after all agents have posted `READY:`
  - agents execute their subtasks after GO
  - queued tasks handled in order
- Common failure signals:
  - no deliberation prompt appears
  - GO message fires after only one agent posts `READY:`
  - GO message never fires even after both agents post `READY:`
  - agents start working before GO message
  - second task corrupts the first task's quorum

---

### TMT-005 Team Member Management (Add, Remove, Role)

- Tier: 1
- Release-sensitive: yes when team membership or channel membership logic changes
- Execution mode: browser
- Goal:
  - verify members can be added and removed through the team settings panel, and that membership changes are reflected in the channel and in the agent's system prompt
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

### TMT-006 Team Settings Update (Display Name, Collaboration Model, Leader)

- Tier: 1
- Release-sensitive: yes when team settings or collaboration model changes
- Execution mode: browser
- Goal:
  - verify that display name, collaboration model, and leader changes are saved and take effect
- Preconditions:
  - team `qa-eng` exists with model `leader_operators` and leader `bot-a`
- Steps:
  1. Open `#qa-eng` team settings.
  2. Change display name to `QA Engineering v2` and save.
  3. Verify the updated display name appears in the settings panel.
  4. Change the collaboration model to `Swarm` and save.
  5. Close and reopen the settings panel; verify the model shows `Swarm`.
  6. Post a task `@qa-eng do something` and verify a deliberation prompt appears (confirming Swarm is now active).
  7. Change the model back to `Leader+Operators` and designate `bot-b` as leader.
  8. Post another task and verify no deliberation prompt appears and `bot-b` handles it as leader.
  9. Refresh; verify all settings changes persist.
- Expected:
  - display name, collab model, and leader changes save immediately
  - collaboration model changes take effect on the next forwarded task
  - settings persist after refresh
- Common failure signals:
  - settings form saves but does not reflect changes after reopen
  - collaboration model change does not affect task routing
  - leader change does not change which agent handles incoming tasks
  - refresh reverts settings

---

### TMT-007 Team Delete — Channel Archive and Workspace Cleanup

- Tier: 1
- Release-sensitive: yes when team deletion or channel archive logic changes
- Execution mode: browser
- Goal:
  - verify that deleting a team archives the channel (preserving message history), removes the workspace on disk, and rebuilds agent system prompts so former members no longer reference the team
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

### TMT-008 Multi-Team Agent Context Isolation

- Tier: 1
- Release-sensitive: yes when system prompt or team membership wiring changes
- Execution mode: browser
- Goal:
  - verify that an agent belonging to multiple teams correctly identifies its role in each team and does not cross-contaminate team contexts
- Preconditions:
  - two teams exist: `qa-eng` (Leader+Operators, `bot-a` as leader) and `qa-algo` (Swarm, `bot-a` as member)
  - `bot-a` is a member of both teams
- Steps:
  1. Ask `bot-a` in `#all`: "What teams are you a member of and what is your role in each?"
  2. Verify `bot-a` correctly names both `qa-eng` (role: leader) and `qa-algo` (role: member or operator).
  3. Post `@qa-eng design a new API` and observe `#qa-eng` — verify no deliberation prompt appears (Leader+Operators model).
  4. Post `@qa-algo analyze the results` and observe `#qa-algo` — verify deliberation prompt appears (Swarm model).
  5. Verify `bot-a` behaves as a leader in `#qa-eng` (decomposes and delegates) and as a swarm member in `#qa-algo` (posts `READY:` after discussing).
  6. Verify that messages in `#qa-eng` do not trigger `bot-a` to behave in Swarm mode, and messages in `#qa-algo` do not trigger leader behavior.
- Expected:
  - agent correctly reports its role in each team
  - collaboration model behavior matches each team's configuration, not the other team's
  - agent does not mix up roles across teams
- Common failure signals:
  - agent reports wrong role in one or both teams
  - `bot-a` posts `READY:` in `#qa-eng` (Swarm bleed-over)
  - `bot-a` tries to delegate in `#qa-algo` instead of deliberating
  - agent says it belongs to only one team despite being in two
