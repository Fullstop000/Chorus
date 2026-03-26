# Chorus QA Report — Teams Feature Full Pass

## Run Metadata

- Date: 2026-03-26T14:26:23
- Branch: `claude/team-concept`
- Starting commit: `34b9aa86` — ending commit: `74b7cc9` (one bug fixed mid-run)
- Tester: Claude Code (automated Playwright + API verification)
- Run mode: **Core Regression** (teams focus)
- QA preset: **mixed-runtime-trio** (`bot-a` claude/sonnet, `bot-b` claude/opus, `bot-c` codex/gpt-5.4-mini)
- Browser: Playwright Chromium headless (v1.58.2)
- Server URL: `http://localhost:3101`
- Data dir: `/tmp/qa-teams-run` (fresh, wiped before run)
- Run directory: `qa/runs/2026-03-26T142623/`

## Scope

- Planned cases: ENV-001, AGT-001, CHN-001, MSG-001, TMT-001–008
- MSG-001 and TMT-003/004/008: `CHORUS_E2E_LLM=0` — LLM-dependent steps skipped per spec, structural/non-LLM steps verified via API
- TMT-008: LLM behavioral steps skipped; structural preconditions (team membership, collab model routing, forwarded_from + deliberation prompt placement) verified via API

## Environment

| Agent | Runtime | Model        | Status |
| ----- | ------- | ------------ | ------ |
| bot-a | claude  | sonnet       | active |
| bot-b | claude  | opus         | active |
| bot-c | codex   | gpt-5.4-mini | active |

Server started from project root; `ui/dist` built and embedded before run.

## Result Table

| Case ID | Result | Method | Notes |
| ------- | ------ | ------ | ----- |
| ENV-001 | **Pass** | Playwright | Shell loads, sidebar renders, no console errors |
| AGT-001 | **Pass** | Playwright | Mixed-runtime trio created; all three `active` |
| CHN-001 | **Pass** | Playwright | Channel create + member invite |
| MSG-001 | **Pass (structural)** | API | Agents active, `bot-a`/`bot-b`/`bot-c` confirmed active; LLM reply step skipped (`E2E_LLM=0`) |
| TMT-001 | **Pass** | Playwright | `qa-eng` created with `[team]` badge; sidebar badge verified; team settings header verified |
| TMT-002 | **Pass** | Playwright + API | @mention forward to team channel; `forwardedFrom` field in history response verified (bug found + fixed — see Findings) |
| TMT-003 | **Pass (structural)** | API | Leader+Operators: NO deliberation system message confirmed in `#qa-eng` after @mention; LLM behavioral chain skipped |
| TMT-004 | **Pass (structural)** | API | Swarm: deliberation system prompt posted to `#qa-swarm` after @mention; quorum created in DB; LLM READY/GO chain skipped |
| TMT-005 | **Pass** | Playwright | Member add + channel rail + wakeup + remove + channel rail + refresh all verified |
| TMT-006 | **Pass** | Playwright | Display name change; L+O → Swarm model flip; leader change; settings persist after refresh |
| TMT-007 | **Pass** | Playwright | Disposable `qa-del-test` team deleted; channel gone from sidebar; former agent still active |
| TMT-008 | **Pass (structural)** | API | bot-a confirmed in all 3 teams with correct roles; L+O channel has no deliberation prompt; Swarm channels have deliberation prompt; `forwardedFrom` field populated on forwarded messages |

## Bug Fixed During Run

### `forwardedFrom` absent from channel history API response

- **Case:** TMT-002 (prior run Medium finding)
- **Root cause:** `HistoryMessage` struct in `src/store/messages.rs` had no `forwarded_from` field. The `get_history` SQL selected only 7 columns — `forwarded_from` was never fetched or returned.
- **Fix:** Added `forwarded_from` field to `HistoryMessage` struct; updated `get_history` SQL to select `forwarded_from`; parse and populate the field using the same pattern as `get_messages_for_agent`.
- **Commit:** `74b7cc9 fix(history): include forwarded_from in HistoryMessage response`
- **Verified:** `GET /internal/agent/bot-a/history?channel=qa-eng` now returns `"forwardedFrom":{"channel_name":"all","sender_name":"bytedance"}` on forwarded messages.
- **DB was always correct** — the column was being written; only the read path was broken.

### Spec timing bug in TMT-006

- **Case:** TMT-006
- **Root cause:** The spec selected `leader_operators` from the collab model dropdown and immediately tried to interact with the leader `<select>`, which is conditionally rendered by React. React had not yet re-rendered.
- **Fix:** Added `await expect(leaderSelect).toBeVisible()` between the model select and leader select interactions.
- **Commit:** not committed separately (spec fix only, no product code change)

## Findings

### No remaining open bugs from prior run

The Medium finding from `2026-03-25T150843` (`forwardedFrom` absent from wire format) is resolved by `74b7cc9`.

### `suppressAgentDelivery: true` suppresses @mention forwarding

- **Observed:** During manual API testing, `suppressAgentDelivery: true` caused the @mention routing to not create the forwarded message in the team channel.
- **Severity:** Low / expected — this flag is intended to suppress delivery. However it may be surprising if callers want to forward without waking agents. No code change required for this QA pass; worth noting in docs.

### TMT-003/004/008 LLM behavioral steps not verified in this run

All three cases have LLM-dependent steps (leader delegation chain, READY: signal timing, bot-a self-describing its teams). These require a live LLM run (`CHORUS_E2E_LLM=1`). Structural preconditions all pass; behavioral verification deferred.

## Release Gate Decision

- Overall result: **PASS WITH LLM DEFERRED**
- Blocking issues: **none**
- The one pre-existing bug (`forwardedFrom` in history) was found and fixed in this run.
- LLM-dependent behavioral steps (delegation chain, READY:/GO quorum timing, multi-team role self-description) are deferred — these require a live LLM run and are not blocking release.

## Evidence Index

| Item | Notes |
| ---- | ----- |
| Playwright runs | `ENV-001`, `AGT-001`, `CHN-001`, `TMT-001`, `TMT-002`, `TMT-005`, `TMT-006`, `TMT-007` — all Pass |
| DB verification | `/tmp/qa-teams-run/chorus.db` — `forwarded_from` column populated; system deliberation messages present in Swarm channels |
| API verification | `GET /api/teams`, `GET /api/teams/{name}`, history endpoint — all responses correct |
| Bug fix commit | `74b7cc9` on `claude/team-concept` |
