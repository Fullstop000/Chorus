# Chorus QA Plan — Teams Feature Full Pass

## Plan Metadata

- Date: 2026-03-26T14:26:23
- Branch: `claude/team-concept`
- Commit: `34b9aa86a6adeb68bb05528f685564735727308f`
- Tester: (fill in)
- Trigger: post-implementation verification — team feature fully shipped (Tasks 6–12 completed by follow-on agent after previous partial run at `2026-03-25T150843`)

## Run Mode

- Selected mode: **Core Regression** with teams focus
- Rationale: The team feature touches message routing, agent lifecycle (stop/start on membership change), system prompt wiring, and channel state. Full regression is warranted. Several TMT cases were deferred or partial in the prior run — this run must complete all of them cleanly.

## Agent Preset

- Selected preset: **`mixed-runtime-trio`**
- Rationale: Teams feature wires directly into agent system prompts, the message fan-out path, and agent stop/restart. Using two Claude variants plus one Codex agent verifies the collaboration models work across both runtimes.
- Agents to create through the UI:

| Name   | Runtime | Model        |
| ------ | ------- | ------------ |
| bot-a  | claude  | sonnet       |
| bot-b  | claude  | opus         |
| bot-c  | codex   | gpt-5.4-mini |

- Deviations from preset: none. If a runtime hits quota, document the substitution explicitly — do not silently swap models.

## Scope

### Cases Planned

Execution order matters. Follow this sequence — later cases depend on state from earlier ones.

| Order | Case ID | Module     | Reason included                                                      |
| ----- | ------- | ---------- | -------------------------------------------------------------------- |
| 1     | ENV-001 | agents     | Sanity: app shell, sidebar, identity — foundation for all UI cases   |
| 2     | AGT-001 | agents     | Create the preset agents — prerequisite for every TMT case           |
| 3     | CHN-001 | channels   | Verify channel create still works before testing team-specific paths |
| 4     | MSG-001 | messaging  | Baseline fan-out: confirm agents receive and reply in `#all`         |
| 5     | TMT-001 | teams      | **Team create, badge, sidebar** — prerequisite for TMT-002 onward   |
| 6     | TMT-002 | teams      | **@mention routing + forwarded_from** — includes forwardedFrom fix   |
| 7     | TMT-003 | teams      | **Leader+Operators behavior** — was Partial in prior run             |
| 8     | TMT-004 | teams      | **Swarm deliberation + quorum** — was Partial; needs `qa-swarm`      |
| 9     | TMT-005 | teams      | **Member add/remove** — was Partial (UI parity not verified)         |
| 10    | TMT-006 | teams      | **Settings update** — was Partial (collab model flip not run)        |
| 11    | TMT-008 | teams      | **Multi-team context isolation** — was Not Run                       |
| 12    | TMT-007 | teams      | **Team delete** — was Not Run; run last, destroys team state         |

### Cases Explicitly Excluded

| Case ID | Reason excluded                                                                  |
| ------- | -------------------------------------------------------------------------------- |
| AGT-002 | Agent matrix — out of scope for this feature pass                                |
| AGT-003 | Agent settings — unchanged by team feature                                       |
| AGT-004 | Agent delete — unchanged                                                         |
| LFC-001 | Lifecycle stop/start covered implicitly by TMT-005 (member add triggers restart) |
| LFC-002 | Resume — unchanged by team feature                                               |
| MSG-002 | DM — unchanged; covered in prior run                                             |
| MSG-003 | Thread — unchanged; not affected by team feature                                 |
| MSG-004 | Unchanged                                                                        |
| HIS-001 | History — covered implicitly in TMT-002 (forwarded message history check)        |
| ATT-001 | Attachment — unchanged                                                           |
| ERR-001 | Unchanged                                                                        |
| TSK-001 | Tasks — unchanged                                                                |
| TSK-002 | Tasks — unchanged                                                                |
| PRF-001 | Profile — unchanged                                                              |
| ACT-001 | Activity — unchanged; covered in prior run                                       |
| ACT-002 | Activity — unchanged                                                             |
| NAV-001 | Unchanged                                                                        |
| WRK-001 | Workspace — team workspace covered by TMT-007 delete teardown                    |
| REC-001 | Resume — unchanged                                                               |
| REC-002 | Resume — unchanged                                                               |
| CHN-002 | Archive channel — tested implicitly in TMT-007                                   |
| CHN-003 | Channel settings — unchanged                                                     |
| CHN-004 | Channel delete guard — team channel delete guard covered in TMT-007 setup        |
| MEM-001–010 | Shared memory — unchanged by team feature                                   |

## Environment Setup

- Server start command:
  ```bash
  cargo build && RUST_LOG=chorus=debug ./target/debug/chorus serve --port 3101 --data-dir /tmp/qa-teams-$(date +%s)
  ```
- Data dir mode: **fresh temp dir** — wipe any prior `/tmp/qa-teams-*` before starting
- Browser: headless Chromium via Playwright, or Chrome
- Server URL: `http://localhost:3101`
- UI: use embedded `ui/dist` (run `cd ui && npm run build` first if not current)
- Attachment file: not required for this run
- Special setup: none — all teams and state created through the UI during the run

## Team State Map (build in order during run)

This table is the canonical reference for which teams exist and what they contain at each stage of the run.

| Team name  | Collab model      | Leader | Members            | Created at case |
| ---------- | ----------------- | ------ | ------------------ | --------------- |
| `qa-eng`   | leader_operators  | bot-a  | bot-a (leader), bot-b (operator) | TMT-001 |
| `qa-swarm` | swarm             | —      | bot-a, bot-b       | TMT-004 setup   |
| `qa-algo`  | swarm             | —      | bot-a, bot-c       | TMT-008 setup   |

`bot-a` intentionally appears in **all three teams** — this is required for TMT-008 multi-team isolation.

`qa-eng` is preserved across the full run. `qa-algo` is created just before TMT-008. TMT-007 runs on a **disposable** `qa-del-test` team, not on `qa-eng`, so the other tests' state survives.

## Case-by-Case Execution Notes

### ENV-001 — App startup
Follow spec. Confirm sidebar shows Channels section (no separate Teams section). Team channels will appear in the Channels list once created.

### AGT-001 — Create agents
Create all three preset agents (bot-a, bot-b, bot-c) through the UI. Do not proceed until all three show `active` status. **This is a hard gate.**

### CHN-001 — Channel create + membership
Quick smoke: create `#qa-test`, invite bot-a, verify reply. This confirms the channel flow is intact before team-specific paths run.

### MSG-001 — Fan-out baseline
Post `ping-baseline` in `#all`, wait for all three agents to reply. **This is a hard gate** — if agents do not reply, the collaboration model tests are meaningless.

### TMT-001 — Team create, badge, sidebar

**Extended from prior run (steps 9–10 were shortened last time):**

- Step 9 (team settings header): Open `#qa-eng` and explicitly verify the channel header shows team-specific controls: collaboration model display, member roles list, and a delete button. A generic channel edit form is a failure.
- Step 10 (refresh): Do a full page reload and verify `[team]` badge persists.

**Additional check not in original case:**
- After creating `qa-eng`, verify that bot-a and bot-b appear in the `#qa-eng` channel members rail (they were added as initial members).

### TMT-002 — @mention routing + forwarded_from

**Known finding from prior run:** `forwardedFrom` field was absent or null in sampled `HistoryMessage` JSON, and human `joined: false` on team channels meant the human couldn't read team history via the agent history endpoint.

**Extra verification steps for this run:**
1. After posting `@qa-eng please build a landing page` in `#all`, open `#qa-eng` **as the human** in the browser UI — verify the forwarded message is visible (not an empty channel for non-member humans).
2. Check that the forwarded message visually indicates it came from `#all` (origin attribution in the UI, or at minimum the raw message content contains the origin info).
3. If the human cannot see `#qa-eng` messages (because `joined: false`), record this as a **Medium** bug with exact repro — this was the open finding from the prior run.

For the dual @mention test (`@qa-eng and @qa-algo`): create `qa-algo` (Swarm, bot-a + bot-c) first, then post the dual mention and verify both channels receive a forwarded copy.

### TMT-003 — Leader+Operators behavior

**Was Partial in prior run. This time must be complete.**

Strict verification sequence:
1. Confirm no deliberation system message appears after `@qa-eng do X` is forwarded to `#qa-eng`. The absence of a "discuss your approach" system message is the key signal.
2. Observe bot-a (leader) posts first in `#qa-eng`.
3. Observe bot-a directs a subtask at bot-b (via DM or a message naming bot-b explicitly). This is the delegation signal.
4. Observe bot-b responds to the delegation.
5. Observe bot-a posts a synthesis/summary.

If LLM responses are slow, give each step a 3-minute window before marking it partial. Document timing in the report.

**Failure patterns to watch:**
- bot-b replies before bot-a posts anything (bypassed leader)
- a system message containing "discuss" or "READY" appears (Swarm bleed-over from wrong collab model)
- neither agent responds within 5 minutes (wakeup failed)

### TMT-004 — Swarm deliberation + quorum

**Was Partial in prior run. `qa-swarm` was not created; dedicated team required.**

Create `qa-swarm` (Swarm model, bot-a + bot-b) specifically for this case. Do not reuse `qa-eng`.

Required verification:
1. Deliberation system message appears in `#qa-swarm` immediately after forward (within ~10 seconds of sending).
2. bot-a posts a `READY:` message.
3. bot-b posts a `READY:` message.
4. A system message "[System] All members ready — execution begins." (or equivalent) appears after both READY: messages.
5. Agents begin their declared subtasks after GO.

**Queued task test (step 8–9 from case):** Post a second `@qa-swarm` task while the first quorum is open. Verify the second task does not corrupt the first quorum (no premature GO).

**Non-quorum signal test:** This is new for this run — have bot-c (not a member of `qa-swarm`) post a message in `#all` that contains `READY: something`. Verify this does NOT trigger the quorum in `#qa-swarm`. (Tests the `record_swarm_signal` non-quorum guard added in Task 2.)

### TMT-005 — Member add/remove

**Was Partial in prior run. UI parity check was skipped.**

Required this time:
- After adding bot-c to `qa-eng` via the team settings panel:
  1. Verify bot-c appears in the **team settings member list** (in the panel)
  2. Verify bot-c appears in the **channel members rail** (the sidebar or header member list for `#qa-eng`)
  3. Post a message in `#qa-eng` and verify bot-c receives and responds (it woke up as a member)
- After removing bot-c from `qa-eng`:
  1. Verify bot-c disappears from the team settings member list
  2. Verify bot-c disappears from the channel members rail
  3. Post a message in `#qa-eng` and verify bot-c does NOT respond
  4. Full page refresh — verify member state is consistent after reload

The refresh check was not done in the prior run. **It is mandatory this time.**

### TMT-006 — Settings update

**Was Partial in prior run. Collaboration model flip was not executed.**

Required this time:
1. Change `qa-eng` display name → verify in header + after reopen
2. Change collaboration model `leader_operators` → `swarm` → save
3. Post `@qa-eng test the new model` in `#all` → verify a Swarm deliberation prompt appears in `#qa-eng`
4. Change model back to `leader_operators` + change leader to bot-b → save
5. Post another `@qa-eng task` → verify no deliberation prompt, bot-b leads
6. Full page refresh → verify all settings persist

### TMT-008 — Multi-team context isolation

**Was Not Run in prior run. Run before TMT-007 so teams still exist.**

Preconditions: bot-a must be in `qa-eng` (leader_operators, leader), `qa-swarm` (swarm, member), and `qa-algo` (swarm, member) simultaneously.

Required:
1. Ask bot-a in `#all`: "What teams are you a member of and what is your role in each?" — verify it names all three teams with correct roles.
2. Post `@qa-eng verify isolation` → open `#qa-eng` → confirm no deliberation prompt (L+O model).
3. Post `@qa-swarm verify isolation` → open `#qa-swarm` → confirm deliberation prompt appears (Swarm model).
4. Verify bot-a behaves as **leader** (decomposes/delegates) in `#qa-eng` — NOT posting `READY:`.
5. Verify bot-a behaves as **swarm member** (discusses, posts `READY:`) in `#qa-swarm` — NOT delegating.

### TMT-007 — Team delete (run LAST)

**Was Not Run in prior run. Run last to avoid destroying shared test state.**

Use a **disposable team** `qa-del-test` (create it at the start of this case with bot-b as only member). Do NOT delete `qa-eng` or `qa-swarm`.

Required:
1. Create `qa-del-test` (leader_operators, bot-b as operator)
2. Post a few messages in `#qa-del-test`
3. Delete via team settings panel
4. Verify `#qa-del-test` disappears from channel list immediately
5. Refresh — verify it does not reappear
6. Verify bot-b is still active and replies in `#all` (agent was not destroyed)
7. Ask bot-b in `#all` to list its teams — verify it does NOT mention `qa-del-test`

**Disk cleanup check:** If the data dir is accessible, verify `~/.chorus/teams/qa-del-test/` no longer exists after deletion (or confirm the path via `ls` in a terminal alongside the browser test).

## Risk Areas

1. **forwardedFrom visibility for non-member humans** — Medium finding from prior run. TMT-002 must explicitly check human-visible history in `#qa-eng`. If still broken, file as a tracked bug with repro steps.

2. **Agent restart latency on membership changes** — TMT-005 add/remove triggers agent stop+restart. Agents may take 10–30 seconds to reconnect. Build in wait time before asserting wakeup behavior.

3. **Swarm quorum timing** — TMT-004 READY: signal detection is live — agents must actually post `READY:` text. If LLM output varies in phrasing, the quorum may not resolve. If this happens, check whether the message literally starts with `READY:` (no leading space permitted per `is_consensus_signal` implementation).

4. **Collaboration model swap latency** — TMT-006 changes the collab model and then immediately tests the new behavior. The model change takes effect on the next forwarded message. If the previous deliberation prompt still fires, the agent may have a cached model — check the `PATCH /api/teams/qa-eng` response confirms the update before sending the test task.

5. **bot-a in three teams** — TMT-008 has bot-a in qa-eng, qa-swarm, and qa-algo simultaneously. Its system prompt includes all three team memberships. Verify via step 1 (the "list your teams" question) before testing isolation behavior.

6. **Ordering dependency** — TMT-007 must run after TMT-008. If TMT-007 runs on `qa-eng` by mistake (not the disposable team), the rest of the suite is unrecoverable. Double-check the team name before confirming delete.

## Known Gaps or Concerns Before Running

1. **`forwardedFrom` in wire format** — the prior run found `forwardedFrom` absent or null in `HistoryMessage` JSON. This may be a serialization gap where the field is stored but not returned in the channel history API response. Check both the browser UI (does it show origin info?) and the raw API response (`GET /api/channels/qa-eng/messages`).

2. **Human membership on team channels** — prior run showed `joined: false` for the human on team channels. This may be intentional (teams are agent-only channels) or a bug (humans should be able to observe team channels). Clarify expected behavior per the spec before marking as a bug vs working-as-intended. The spec says "own dedicated `#<team-name>` channel" but does not explicitly state human visibility.

3. **Playwright specs exist for all TMT cases** — the specs were shipped in `33dc630`. Run them headlessly first (per QA README rule 3), then fall back to manual browser verification for any spec failures.

4. **`qa-swarm` naming** — TMT-004 requires a team named `qa-swarm`. This name must not already exist in the fresh data dir. Confirm the data dir is clean before starting.

## Pre-Run Checklist

- [ ] `cargo test` passes on `claude/team-concept` (or document any failures)
- [ ] `cd ui && npm run build` succeeds
- [ ] Fresh temp data dir confirmed (no leftover `/tmp/qa-teams-*`)
- [ ] Server started, UI accessible at `http://localhost:3101`
- [ ] Playwright installed (`cd qa/cases/playwright && npx playwright install --with-deps chromium`)
- [ ] All three agents created and confirmed `active` before starting TMT cases

## Human Approval

**Show this plan and wait for explicit approval before executing any cases.**

- Approved by:
- Approval notes / scope changes:
