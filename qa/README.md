# QA Docs

This directory contains the reusable QA operating docs for Chorus.

## Files

- `QA_CASES.md`
Static and versioned browser QA case catalog index. Actual cases live in `cases/`.
- `QA_PRESETS.md`
Reusable agent/runtime setup presets for runs that need consistent coverage across Claude and Codex.
- `QA_PLAN_TEMPLATE.md`
Fill-in template for the pre-run plan. Created before execution; shown to human for approval before any cases are run.
- `QA_REPORT_TEMPLATE.md`
Fill-in template for each QA run. Created during execution; completed before presenting findings to the human.
- `BUG_FIX_REPORT_TEMPLATE.md`
Fill-in template for the implementation/fix pass that follows a QA run. Created only after human approves a fix pass.
- `runs/{datetime}/plan.md`
Pre-run plan for that session.
- `runs/{datetime}/report.md`
Execution results for that session.
- `runs/{datetime}/fix_report.md`
Fix pass record when code changes are made in response to findings.
- `runs/{datetime}/evidence/`
Evidence bundle for that specific run only.

## Why This Directory Exists

The testing plan in `.design/` explains strategy and release philosophy.
This `qa/` directory is the execution layer:

- what to test every time
- how to run it in the real web product
- how to record pass/fail status consistently
- how to scale the process as the product surface grows

## Execution Rules

1. Use the real website in Chrome or headless Chrome.
2. Prefer a fresh temp data dir for repeatable runs.
3. Run the cases through the browser, not API-only shortcuts.
4. Capture evidence for every failure:
  - screenshot
  - console error
  - network error or API payload when relevant
5. Any escaped bug should be added to the static case catalog or mapped to an existing case.
6. If a case depends on a control that is not yet exposed in the shipped product, do not fake the workflow by mutating SQLite directly. Follow the case's documented execution mode:
  - run the allowed hybrid path when the case explicitly allows CLI or API assistance
  - otherwise mark the case `Blocked` and record the product gap

## Execution Modes

- `browser`
  - execute entirely through the normal web product
- `hybrid`
  - browser-first, but the case may use a documented CLI or API step when the current product still exposes part of the flow outside the main UI
- `blocked-until-shipped`
  - the product does not yet expose the control needed for this QA case; record the gap instead of inventing a hidden setup

## Run Modes

Four modes exist: **PR Smoke**, **Core Regression**, **Recovery / Reliability**, and **Agent Matrix**.

For the authoritative definition of each mode — which trigger conditions apply, which cases are required, and which preset to use — see the **QA Standard Operating Procedure** section in `CLAUDE.md`. That is the single source of truth. Do not duplicate case lists here; they diverge.

## Standard Environment

Use these defaults unless the run needs a targeted variation:

- browser: Chrome or headless Chrome via Playwright
- backend: local server started from current branch
- data dir: fresh temp directory
- seed users: current user only
- seed agents: create 1-3 test agents

## Standard Agent Set

Use presets in `[QA_PRESETS.md](./QA_PRESETS.md)` and record which preset was used.

## Evidence Naming

Each run should live in its own directory:

- `qa/runs/{datetime}/plan.md`
- `qa/runs/{datetime}/report.md`
- `qa/runs/{datetime}/fix_report.md`
- `qa/runs/{datetime}/evidence/`

Recommended `{datetime}` format:

- `YYYY-MM-DDTHHMMSS`

Within the run-local `evidence/` directory, use stable names:

- `YYYY-MM-DD-case-id-short-title.png`
- `YYYY-MM-DD-case-id-console.txt`
- `YYYY-MM-DD-case-id-network.txt`

Reusable fixtures that are not tied to one run should not live in a run evidence folder.
Keep them in a separate location such as `qa/fixtures/`.

## Maintenance Rule

When a new product feature becomes user-visible, update both:

1. the tier inventory in `[testing-plan.md](../.design/testing-plan.md)`
2. the executable case list in `[QA_CASES.md](./QA_CASES.md)`

If a bug escapes, either:

- add a new case, or
- tighten an existing case so the failure would be caught next time.

