# Chorus QA Plan Template

Copy this file before starting a QA run. Fill it out completely and show it to the human for approval before executing any cases.

Recommended filename:
- `qa/runs/YYYY-MM-DDTHHMMSS/plan.md`

Companion report when execution is complete:
- `qa/runs/YYYY-MM-DDTHHMMSS/report.md`

---

## Plan Metadata

- Date:
- Branch:
- Commit:
- Tester:
- Trigger:
  - `PR review` — PR #:
  - `Bug escape` — bug title:
  - `Release prep` — target version:
  - `Post-fix verification` — related run:
  - `Routine regression`
  - other:

## Run Mode

- Selected mode:
  - `PR Smoke`
  - `Core Regression`
  - `Recovery / Reliability`
  - `Agent Matrix`
  - custom:
- Rationale for this mode:

## Agent Preset

- Selected preset:
  - `claude-trio`
  - `mixed-runtime-trio`
  - `codex-lifecycle-pair`
  - `agent-matrix`
  - custom:
- Rationale for this preset:
- Any deviations from the preset (document explicitly — never silently substitute):

## Scope

### Cases Planned

List each case ID to be run in this session.

| Case ID | Module | Reason included |
| ------- | ------ | --------------- |
|         |        |                 |

### Cases Explicitly Excluded

| Case ID | Reason excluded |
| ------- | --------------- |
|         |                 |

## Environment Setup

- Server start command:
- Data dir mode:
  - `fresh temp dir` (default)
  - `existing dir`:
- Browser:
- Server URL:
- Attachment file for upload testing:
- Any special setup steps before run:

## Risk Areas

List areas that deserve extra attention or exploratory probing during this run, beyond the case steps.

-
-

## Known Gaps or Concerns Before Running

Document anything you already know may block, flake, or mislead the run.

-
-

## Human Approval

**Show this plan to the human and wait for explicit approval before executing any cases.**

- Approved by:
- Approval notes / scope changes:
