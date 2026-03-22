# Chorus QA Report

## Run Metadata

- Date: 2026-03-22
- Branch: `codex/update-agents-testing`
- Commit: `a74f637`
- Tester: Codex
- Run mode:
  - custom: targeted lifecycle and runtime regression
- QA preset:
  - `mixed-runtime-trio`
- Browser: Playwright headless Chromium
- Server URL: `http://localhost:3102`
- Data dir mode:
  - `custom temp dir`
- Run directory:
  - `qa/runs/2026-03-22T122805/`
- Evidence directory:
  - `qa/runs/2026-03-22T122805/evidence/`
- Related change / PR:
  - wake-message restart path for sleeping and inactive agents

## Scope

- Planned cases:
  - `ENV-001`
  - `AGT-001`
  - `MSG-001`
  - `MSG-002`
  - `LFC-001`
  - `LFC-002`
  - `PRF-001`
  - `ACT-001`
  - `WRK-001`
  - `REC-001`
- Additional exploratory coverage:
  - verified an inactive Codex-backed agent (`bot-c`) wakes from a new DM and replies after a browser-driven stop
  - verified the same Codex-backed agent remains recoverable after full server restart against the same data dir
- Intentionally skipped areas:
  - `AGT-002` full runtime/model matrix
  - thread-specific cases
  - attachment and task flows

## Environment Setup

- Current user: `bytedance`
- Test channel: `#general`
- Test agents:
  - name: `bot-a`
  runtime: `claude`
  model: `sonnet`
  - name: `bot-b`
  runtime: `claude`
  model: `opus`
  - name: `bot-c`
  runtime: `codex`
  model: `gpt-5.4-mini`
- Attachment file used:
  - none
- Notes about environment:
  - fresh server started with `./target/debug/chorus serve --port 3102 --data-dir /tmp/chorus-qa-LQMlfu`
  - browser console remained clean for the executed flow

## Feature QA Result Table


| Feature                        | Tier     | Cases Covered                   | Result | Notes                                                                                                |
| ------------------------------ | -------- | ------------------------------- | ------ | ---------------------------------------------------------------------------------------------------- |
| App startup and identity       | Tier 0   | `ENV-001`                       | Pass   | Shell loaded cleanly on fresh data dir.                                                              |
| Agent creation and selection   | Tier 0   | `AGT-001`                       | Pass   | Created mixed-runtime trio through the browser UI and opened agent surfaces successfully.            |
| Agent lifecycle                | Tier 0/1 | `LFC-001`, `LFC-002`, `PRF-001` | Pass   | Codex-backed `bot-c` showed coherent sleeping, active, inactive, and post-restart recovery states.   |
| Channel messaging              | Tier 0   | `MSG-001`                       | Pass   | Shared-channel prompt woke `bot-a`, `bot-b`, and real Codex `bot-c`.                                 |
| DM messaging                   | Tier 0   | `MSG-002`                       | Pass   | DM to `bot-c` persisted and the reply remained in the DM after reload/restart.                       |
| Profile and lifecycle accuracy | Tier 0   | `PRF-001`                       | Pass   | Profile status matched stop/start and sleeping states for `bot-c`.                                   |
| Activity timeline              | Tier 1   | `ACT-001`                       | Pass   | Activity showed state changes, received messages, tool calls, and sent replies for the restart path. |
| Workspace browsing             | Tier 1   | `WRK-001`                       | Pass   | Workspace path reflected `/tmp/chorus-qa-LQMlfu/agents/bot-c` and listed real files.                 |
| Restart and reliability        | Tier 1   | `LFC-002`, `REC-001`            | Pass   | After server restart, `bot-c` surfaced as sleeping and woke correctly on the next DM.                |


## Case Execution Table


| Case ID   | Result | Evidence | Notes                                                                               |
| --------- | ------ | -------- | ----------------------------------------------------------------------------------- |
| `ENV-001` | Pass   | none     | App shell loaded without console errors.                                            |
| `AGT-001` | Pass   | none     | Created `bot-a`, `bot-b`, `bot-c` via create-agent modal.                           |
| `MSG-001` | Pass   | none     | All three agents replied in `#general`, including Codex-backed `bot-c`.             |
| `MSG-002` | Pass   | none     | DM round-trip with `bot-c` returned `codex` and later restart-recovery replies.     |
| `LFC-001` | Pass   | none     | `bot-c` moved through sleeping -> active/waiting -> inactive via browser controls.  |
| `LFC-002` | Pass   | none     | After full server restart, persisted agent state remained coherent and recoverable. |
| `PRF-001` | Pass   | none     | Profile reflected `active`, `sleeping`, and `inactive` accurately.                  |
| `ACT-001` | Pass   | none     | Activity log showed received DM, tool calls, send, waiting, idle, and process stop. |
| `WRK-001` | Pass   | none     | Workspace tab showed correct custom data-dir path and contents.                     |
| `REC-001` | Pass   | none     | Post-restart DM to sleeping `bot-c` produced `recovered after restart`.             |


## Findings

No findings in the executed scope.

## Release Gate Decision

- Overall result:
  - `DONE`
- Release blockers:
  - none in the executed scope
- Non-blocking concerns:
  - full `AGT-002` runtime/model matrix was not part of this targeted pass
- Blocked cases:
  - none

## Regression Follow-Up


| Bug / Finding                                                     | Existing Case Covered It? | New Or Tightened Case Needed?                                                                                                  | Automation Layer Target |
| ----------------------------------------------------------------- | ------------------------- | ------------------------------------------------------------------------------------------------------------------------------ | ----------------------- |
| Claude-only fixture hid Codex driver regressions in prior QA runs | No                        | Added reusable runtime presets in `qa/QA_PRESETS.md` and updated QA docs to require preset selection for driver/lifecycle work | browser E2E             |


## Evidence Index


| Evidence File | Related Case / Finding | Notes                                   |
| ------------- | ---------------------- | --------------------------------------- |
| none          | n/a                    | No failures; browser console was clean. |


## Notes For Next Iteration

- Run `AGT-002` with the new `agent-matrix` preset when runtime registration or model lists change again.
- Add a reusable browser case that explicitly checks sleeping/inactive wake-up for a Codex-backed agent, so this path is not only covered by targeted exploratory verification.

