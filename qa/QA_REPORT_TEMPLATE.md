# Chorus QA Report Template

Copy this file for each QA run.

Recommended filename:
- `qa/runs/YYYY-MM-DDTHHMMSS/report.md`

Companion fix report when bugs are addressed:
- `qa/runs/YYYY-MM-DDTHHMMSS/fix_report.md`

## Run Metadata

- Date:
- Branch:
- Commit:
- Tester:
- Run mode:
  - `PR Smoke`
  - `Core Regression`
  - `Recovery / Reliability`
  - custom:
- QA preset:
  - `claude-trio`
  - `mixed-runtime-trio`
  - `codex-lifecycle-pair`
  - `agent-matrix`
  - custom:
- Browser:
- Server URL:
- Data dir mode:
  - `default`
  - `custom temp dir`
- Run directory:
- Evidence directory:
  - `qa/runs/YYYY-MM-DDTHHMMSS/evidence/`
- Related change / PR:

## Scope

- Planned cases:
- Additional exploratory coverage:
- Intentionally skipped areas:

## Environment Setup

- Current user:
- Test channel:
- Test agents:
  - name:
  - runtime:
  - model:
  - name:
  - runtime:
  - model:
  - name:
  - runtime:
  - model:
- Attachment file used:
- Notes about environment:

## Feature QA Result Table

| Feature | Tier | Cases Covered | Result | Notes |
| --- | --- | --- | --- | --- |
| App startup and identity | Tier 0 | `ENV-001` |  |  |
| Agent creation and selection | Tier 0 | `AGT-001` |  |  |
| Agent runtime/model matrix and uniqueness | Tier 1 | `AGT-002`, `AGT-003` |  |  |
| Agent lifecycle | Tier 0/1 | `LFC-001`, `LFC-002`, `PRF-001` |  |  |
| Channel CRUD and membership | Tier 0/1 | `CHN-001`, `CHN-002`, `CHN-003`, `CHN-004` |  |  |
| Channel messaging | Tier 0 | `MSG-001` |  |  |
| DM messaging | Tier 0/1 | `MSG-002`, `MSG-004` |  |  |
| Thread messaging | Tier 0 | `MSG-003` |  |  |
| History reload and selection stability | Tier 0 | `HIS-001` |  |  |
| Tasks | Tier 0 | `TSK-001`, `TSK-002` |  |  |
| Attachments | Tier 0 | `ATT-001` |  |  |
| Profile and lifecycle accuracy | Tier 0 | `PRF-001` |  |  |
| Activity timeline | Tier 1 | `ACT-001`, `ACT-002` |  |  |
| Workspace browsing | Tier 1 | `WRK-001` |  |  |
| Navigation and target selection | Tier 1 | `NAV-001` |  |  |
| Error handling and recovery | Tier 1 | `ERR-001` |  |  |
| Restart and reliability | Tier 1 | `LFC-002`, `REC-001`, `REC-002` |  |  |

## Case Execution Table

| Case ID | Result | Evidence | Notes |
| --- | --- | --- | --- |
| `ENV-001` |  |  |  |
| `AGT-001` |  |  |  |
| `AGT-002` |  |  |  |
| `AGT-003` |  |  |  |
| `LFC-001` |  |  |  |
| `LFC-002` |  |  |  |
| `CHN-001` |  |  |  |
| `CHN-002` |  |  |  |
| `CHN-003` |  |  |  |
| `CHN-004` |  |  |  |
| `MSG-001` |  |  |  |
| `MSG-002` |  |  |  |
| `MSG-003` |  |  |  |
| `MSG-004` |  |  |  |
| `HIS-001` |  |  |  |
| `TSK-001` |  |  |  |
| `TSK-002` |  |  |  |
| `ATT-001` |  |  |  |
| `PRF-001` |  |  |  |
| `ACT-001` |  |  |  |
| `ACT-002` |  |  |  |
| `WRK-001` |  |  |  |
| `NAV-001` |  |  |  |
| `ERR-001` |  |  |  |
| `REC-001` |  |  |  |
| `REC-002` |  |  |  |

## Findings

List findings in severity order.

### High

1. Title:
   - Cases:
   - Repro:
   - Observed:
   - Expected:
   - User impact:
   - Evidence:

### Medium

1. Title:
   - Cases:
   - Repro:
   - Observed:
   - Expected:
   - User impact:
   - Evidence:

### Low

1. Title:
   - Cases:
   - Repro:
   - Observed:
   - Expected:
   - User impact:
   - Evidence:

## Release Gate Decision

- Overall result:
  - `DONE`
  - `DONE_WITH_CONCERNS`
  - `BLOCKED`
- Release blockers:
- Non-blocking concerns:
- Blocked cases:

## Regression Follow-Up

For every significant bug, decide how it should be prevented next time.

| Bug / Finding | Existing Case Covered It? | New Or Tightened Case Needed? | Automation Layer Target |
| --- | --- | --- | --- |
|  |  |  |  |

Automation layer target examples:
- Rust unit test
- Rust integration test
- contract test
- browser E2E
- manual exploratory only

## Evidence Index

| Evidence File | Related Case / Finding | Notes |
| --- | --- | --- |
|  |  |  |

## Notes For Next Iteration

- What should be added to the static case catalog?
- What should move from manual QA into automated coverage?
- Which areas were noisy, flaky, or too expensive to verify?
