# Chorus Static QA Case Catalog

This is the reusable browser QA case list for Chorus.

It is intentionally:
- detailed enough to execute without guesswork
- stable enough to reuse every iteration
- strict enough to catch state-consistency failures, not just obvious crashes

All cases below are browser-first cases unless explicitly marked otherwise.
The `Suite:` field on each case is authoritative (`smoke` or `regression`). Nearby placement in the file is for related-domain readability.

**Non-browser case modules:** [`cases/bridge.md`](./cases/bridge.md) covers subprocess and live-runtime tests (IDs `BRG-NNN`, `LRT-NNN`, `INT-NNN`). Those run through `cargo test`, not Playwright. See [`README.md`](./README.md) → "Subprocess and External Runtime Tests" for the evidence rules that apply to them.

## Suite Definitions

- `smoke` — runs on every PR. Must pass before merge. Covers all data-model CRUD, base message flows, and critical UX paths.
- `regression` — runs in Core Regression and deeper passes. Includes all smoke cases plus edge cases, stress tests, recovery flows, and niche UI assertions. Regression ⊃ smoke.

## Case Modules

Cases live in dedicated `.md` files. Each file is the source of truth for its domain.

| Domain | Module | Covers |
|--------|--------|--------|
| Agents & Lifecycle | [`cases/agents.md`](./cases/agents.md) | Agent CRUD, lifecycle, activity, workspace, navigation, recovery |
| Channels | [`cases/channels.md`](./cases/channels.md) | Channel CRUD, membership, archive |
| Teams | [`cases/teams.md`](./cases/teams.md) | Team CRUD, members, delete, thread wake |
| Tasks | [`cases/tasks.md`](./cases/tasks.md) | Task CRUD, message-as-task |
| Messaging | [`cases/messaging.md`](./cases/messaging.md) | DM, channels, threads, mentions, unread, history, errors |
| Bridge & Runtime | [`cases/bridge.md`](./cases/bridge.md) | Bridge subprocess, live runtime, integration |

## Case record template (authoring)

Each executable case under `cases/*.md` should use this shape (omit `Execution mode` when default browser-first):

- `### ABC-NNN Short Title`
- `Suite:` (`smoke` or `regression`)
- `Execution mode:` (when not default browser-first)
- `Goal:` (one line)
- **`Script:`** — required link to Playwright spec under [`cases/playwright/`](./cases/playwright/); if not yet automated, the spec must exist with `test.fixme(...)`.
- `Preconditions:` (only when non-default)
- `Steps:` (numbered, ≤ 8 for smoke cases)
- `Expected:` (terse assertions, one per line)

When you add or change automation, update the case's `Script:` line and keep the spec in sync.


## How To Use This Catalog

For each run:

1. pick the run mode from [`README.md`](./README.md)
2. pick the agent/runtime preset from [`QA_PRESETS.md`](./QA_PRESETS.md) when the run touches driver or lifecycle behavior
3. create a fresh run report from [`QA_REPORT_TEMPLATE.md`](./QA_REPORT_TEMPLATE.md)
4. execute cases in the browser
5. mark each case `Pass`, `Fail`, `Blocked`, or `Not Run`
6. attach evidence for every failure

## Shared Preconditions

Apply these unless a case overrides them:

- server started from the branch under test
- browser opened to the real app shell
- data dir is fresh
- current human user confirmed by `whoami`
- 3 agents created according to the selected preset from [`QA_PRESETS.md`](./QA_PRESETS.md)
- one text file prepared for attachment upload testing
- default working channel is `#general` or a dedicated `#qa-multi-agent`

## Result Definitions

- `Pass` — all expected results observed
- `Fail` — at least one expected result is violated
- `Blocked` — cannot finish because an earlier failure or environment issue prevents execution
- `Not Run` — intentionally skipped in this run mode

## Notes On Product Gaps

- Some QA cases intentionally cover product controls that are not fully shipped yet, such as delete flows or explicit channel member management.
- When a case is marked `hybrid` or `blocked-until-shipped`, follow the case instructions exactly.
- Do not simulate missing user-facing flows by editing SQLite directly during QA.

## Maintenance Notes

When the product changes:

- add a new case if a new user-visible flow appears
- tighten an existing case if a bug slipped through
- keep case IDs stable so reports remain comparable across iterations
- do not silently delete old cases without updating release expectations
