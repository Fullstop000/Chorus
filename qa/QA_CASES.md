# Chorus Static QA Case Catalog

This is the reusable browser QA case list for Chorus.

It is intentionally:
- detailed enough to execute without guesswork
- stable enough to reuse every iteration
- strict enough to catch state-consistency failures, not just obvious crashes

All cases below are browser-first cases unless explicitly marked otherwise.
The explicit `Tier:` field on each case is authoritative. Nearby placement in the file is for related-domain readability.

## Case record template (authoring)

Each executable case under `cases/*.md` should use this shape (fields may omit `Execution mode` when default is browser-first):

- `### ABC-NNN Short Title`
- `Tier:` / `Release-sensitive:` / `Execution mode:` (when not default)
- `Goal:` (bullet list)
- **`Script:`** — required link to Playwright spec under [`cases/playwright/`](./cases/playwright/) (e.g. [`playwright/ENV-001.spec.ts`](./cases/playwright/ENV-001.spec.ts)); if the case is not yet automated, the linked spec must exist and state the gap with `test.fixme(...)`.
- `Preconditions:`
- `Steps:` (numbered)
- `Expected:`
- `Common failure signals:`

When you add or change automation, update the case’s `Script:` line and keep the spec’s file header in sync with Preconditions / Steps / Expected (see `qa/README.md` → Playwright).

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

- `Pass`
  - all expected results observed
- `Fail`
  - at least one expected result is violated
- `Blocked`
  - cannot finish because an earlier failure or environment issue prevents execution
- `Not Run`
  - intentionally skipped in this run mode

## Notes On Product Gaps

- Some QA cases intentionally cover product controls that are not fully shipped yet, such as delete flows or explicit channel member management.
- When a case is marked `hybrid` or `blocked-until-shipped`, follow the case instructions exactly.
- Do not simulate missing user-facing flows by editing SQLite directly during QA.

## Case Modules

Cases are split into focused modules under [`cases/`](./cases/):

| Module | Cases |
| ------ | ----- |
| [`cases/agents.md`](./cases/agents.md) | ENV-001, AGT-001, AGT-002, AGT-003, AGT-004, PRF-001, LFC-001, LFC-002, ACT-001, ACT-002, NAV-001, NAV-002, WRK-001, REC-001, REC-002 |
| [`cases/channels.md`](./cases/channels.md) | CHN-001, CHN-002, CHN-003, CHN-004, CHN-005 |
| [`cases/messaging.md`](./cases/messaging.md) | MSG-001, MSG-002, MSG-003, MSG-004, MSG-005, MSG-006, MSG-007, MSG-008, MSG-009, MSG-010, MSG-011, HIS-001, ATT-001, ERR-001 |
| [`cases/tasks.md`](./cases/tasks.md) | TSK-001, TSK-002 |
| [`cases/teams.md`](./cases/teams.md) | TMT-001, TMT-002, TMT-003, TMT-004, TMT-005, TMT-006, TMT-007, TMT-008, TMT-009 |
| [`cases/shared_memory.md`](./cases/shared_memory.md) | MEM-001, MEM-002, MEM-003, MEM-004, MEM-005, MEM-006, MEM-007, MEM-008, MEM-009, MEM-010 |

## Maintenance Notes

When the product changes:

- add a new case if a new user-visible flow appears
- tighten an existing case if a bug slipped through
- keep case IDs stable so reports remain comparable across iterations
- do not silently delete old cases without updating release expectations
