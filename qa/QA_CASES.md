# Chorus Static QA Case Catalog

This is the reusable browser QA case list for Chorus.

It is intentionally:
- detailed enough to execute without guesswork
- stable enough to reuse every iteration
- strict enough to catch state-consistency failures, not just obvious crashes

All cases below are browser-first cases unless explicitly marked otherwise.
The explicit `Tier:` field on each case is authoritative. Nearby placement in the file is for related-domain readability.

## How To Use This Catalog

For each run:

1. pick the run mode from [`README.md`](./README.md)
2. create a fresh run report from [`QA_REPORT_TEMPLATE.md`](./QA_REPORT_TEMPLATE.md)
3. execute cases in the browser
4. mark each case `Pass`, `Fail`, `Blocked`, or `Not Run`
5. attach evidence for every failure

## Shared Preconditions

Apply these unless a case overrides them:

- server started from the branch under test
- browser opened to the real app shell
- data dir is fresh
- current human user confirmed by `whoami`
- 3 agents created:
  - `bot-a`
  - `bot-b`
  - `bot-c`
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
| [`cases/agents.md`](./cases/agents.md) | ENV-001, AGT-001, AGT-002, AGT-003, PRF-001, LFC-001, LFC-002, ACT-001, NAV-001, WRK-001, REC-001, REC-002 |
| [`cases/channels.md`](./cases/channels.md) | CHN-001, CHN-002, CHN-003, CHN-004 |
| [`cases/messaging.md`](./cases/messaging.md) | MSG-001, MSG-002, MSG-003, HIS-001, ATT-001, ERR-001 |
| [`cases/tasks.md`](./cases/tasks.md) | TSK-001, TSK-002 |

## Maintenance Notes

When the product changes:

- add a new case if a new user-visible flow appears
- tighten an existing case if a bug slipped through
- keep case IDs stable so reports remain comparable across iterations
- do not silently delete old cases without updating release expectations
