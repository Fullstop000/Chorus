# Chorus QA Report Template

> Copy this file for each QA run and fill it out as the run progresses. Keep the report scoped to the cases and exploratory coverage that actually happened in that run.

Recommended filename:

- `qa/runs/YYYY-MM-DDTHHMMSS/report.md`

Companion fix report when bugs are addressed:

- `qa/runs/YYYY-MM-DDTHHMMSS/fix_report.md`

## Run Metadata

> Instruction: Record the exact run identity and environment so another person can trace the run back to a branch, commit, preset, and evidence bundle.

- Date:
- Branch:
- Commit:
- Tester:
- Related change / PR:
- Run mode:
  - `PR Smoke`
  - `Core Regression`
  - `Recovery / Reliability`
  - `Agent Matrix`
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

## Release Gate Decision

> Instruction: State the final QA outcome early so a reader can understand the run status before reading the details. Update this section as the run progresses and finalize it before sharing the report.

- Overall result:
  - `✅ DONE`
  - `⚠️ DONE_WITH_CONCERNS`
  - `⛔ BLOCKED`
- Release blockers:
- Non-blocking concerns:
- Blocked cases:
- Recommended next action:

## Scope Summary

Instruction: Summarize what this run covered, what it intentionally did not cover, and why the chosen scope was appropriate.

- Planned cases:
- Cases actually executed:
- Additional exploratory coverage:
- Intentionally skipped areas:
- Scope notes:

## Environment Setup

> Instruction: Capture the concrete test setup used for this run, including human identity, channel context, test agents, attachment fixtures, and any non-default setup.

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

## Coverage Summary

> Instruction: Summarize executed coverage by product area or workflow. Use this section for a human-readable overview, not as a second source of truth for the catalog.

| Area / Workflow | Cases Run | Result | Notes |
| --- | --- | --- | --- |
|  |  |  |  |

## Case Execution Table

> Instruction: List only the cases that were actually part of this run. Add one row per executed, blocked, or intentionally not-run case that mattered to the final decision.

| Case ID | Module | Result | Execution Type | Evidence | Notes |
| --- | --- | --- | --- | --- | --- |
|  |  | `✅ Pass` / `❌ Fail` / `⛔ Blocked` / `⏭️ Not Run` | `script` / `manual` / `hybrid` / `blocked` / `not run` |  |  |

## Findings

> Instruction: List findings in severity order. Include concrete repro information and evidence pointers. Omit empty severity sections if there were no findings at that level.

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

## Regression Follow-Up

> Instruction: For every meaningful bug or notable miss, decide how this class of issue should be prevented in future runs.

| Bug / Finding | Existing Case Covered It? | New Or Tightened Case Needed? | Automation Layer Target | Follow-Up Notes |
| --- | --- | --- | --- | --- |
|  |  |  |  |  |

Automation layer target examples:

- Rust unit test
- Rust integration test
- contract test
- browser E2E
- manual exploratory only

## Evidence Index

> Instruction: Index every evidence artifact referenced elsewhere in the report so another reviewer can find the supporting files quickly.

| Evidence File | Related Case / Finding | Notes |
| --- | --- | --- |
|  |  |  |

## Notes For Next Iteration

- What should be added to the static case catalog?
- What should move from manual QA into automated coverage?
- Which areas were noisy, flaky, or too expensive to verify?
