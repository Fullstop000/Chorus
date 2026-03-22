# Chorus Bug Fix Report Template

Copy this file when a QA run leads to code changes.

Recommended filename:
- `qa/runs/YYYY-MM-DDTHHMMSS/fix_report.md`

Use this as the implementation companion to:
- `qa/runs/YYYY-MM-DDTHHMMSS/report.md`

## Run Linkage

- QA run directory:
- QA report:
- Bug fix report:
- Branch:
- Commit range under fix:
- Author:
- Date:

## Summary

- Overall fix status:
  - `FIXED`
  - `PARTIALLY_FIXED`
  - `NOT_FIXED`
- Scope of this fix pass:
- Explicitly deferred items:

## Fix Status Table

| Finding / Bug ID | Severity | Root Cause Summary | Fix Status | Verification Status | Notes |
| --- | --- | --- | --- | --- | --- |
|  |  |  |  |  |  |

## Verification Matrix

| Verification Layer | Command / Method | Scope | Result | Notes |
| --- | --- | --- | --- | --- |
| Rust tests |  |  |  |  |
| UI build |  |  |  |  |
| Browser E2E |  |  |  |  |
| Manual exploratory |  |  |  |  |

## Remaining Gaps

- Bugs still open after the fix pass:
- Behaviors changed intentionally:
- Known tradeoffs:
- Cases that should be rerun in the next regression:

## Regression Coverage Follow-Up

| Bug / Fix | Existing Coverage | Coverage Gap | Required Follow-Up |
| --- | --- | --- | --- |
|  |  |  |  |

## Notes For Next Iteration

- What should be codified into the static QA case catalog?
- What should gain an automated regression test?
- What should be split into a separate refactor instead of staying inside bug-fix work?
