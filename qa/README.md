# QA Docs

This directory contains the reusable QA operating docs for Chorus.

## Files

- `QA_CASES.md`
  Static and versioned browser QA case catalog.
- `QA_PRESETS.md`
  Reusable agent/runtime setup presets for runs that need consistent coverage across Claude and Codex.
- `QA_REPORT_TEMPLATE.md`
  Fill-in template for each QA run.
- `BUG_FIX_REPORT_TEMPLATE.md`
  Fill-in template for the implementation/fix pass that follows a QA run.
- `runs/{datetime}/report.md`
  One report per QA run.
- `runs/{datetime}/fix_report.md`
  One fix report per QA run when code changes are made in response to findings.
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

## Recommended Run Modes

### PR Smoke

Run before merging medium or large product changes.

Recommended preset:
- `claude-trio` for generic UI changes
- `mixed-runtime-trio` when agent, driver, lifecycle, prompt, or bridge code changed

Required cases:
- `ENV-001`
- `AGT-001`
- `LFC-001`
- `CHN-001`
- `MSG-001`
- `MSG-002`
- `MSG-003`
- `TSK-001`
- `PRF-001`
- `ACT-001`

### Core Regression

Run before release or after touching messaging, lifecycle, tasks, upload, or workspace logic.

Recommended preset:
- `mixed-runtime-trio`

Required cases:
- all Tier 0 cases
- all Tier 1 cases marked `release-sensitive`
- `MSG-004` when DM routing, DM reply rendering, or agent wake behavior changed
- `ACT-002` when lifecycle or activity aggregation changed
- `AGT-002` when the agent create flow, runtime list, model list, defaults, or driver registration changed
- `CHN-002`, `CHN-003`, and `CHN-004` when channel management or channel membership behavior changed

### Recovery / Reliability

Run after changes to startup, persistence, session restore, or runtime integration.

Recommended preset:
- `codex-lifecycle-pair`
- plus `mixed-runtime-trio` when the bug could depend on multi-agent channel fan-out

Required cases:
- `LFC-002`
- `REC-001`
- `REC-002`
- `MSG-004`
- `WRK-001`
- `ACT-001`
- `ACT-002`
- `PRF-001`

### Agent Matrix

Run before releases that touch runtime support, model options, driver registration, or the create-agent modal.

Recommended preset:
- `agent-matrix`

Required cases:
- `AGT-002`

## Standard Environment

Use these defaults unless the run needs a targeted variation:

- browser: Chrome or headless Chrome via Playwright
- backend: local server started from current branch
- data dir: fresh temp directory
- seed users: current user only
- seed agents: create 3 test agents
- seed channel: use `#general` or create a dedicated `#qa-multi-agent`
- test attachment: one small text file
- matrix runs: create one disposable agent per runtime and model pair in the current UI

## Standard Agent Set

Unless a case says otherwise, create these agents for the run:

- `bot-a`
- `bot-b`
- `bot-c`

This is intentional. Single-agent testing hides fan-out, ordering, activity, and stale-state bugs.
The main messaging suite should prove that the app behaves correctly when multiple agents share the
same channel and respond in the same window.

When runtime-specific behavior matters, do not assume this trio is all-Claude. Use one of the
documented presets in [`QA_PRESETS.md`](./QA_PRESETS.md) and record which preset was used.

## Evidence Naming

Each run should live in its own directory:

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

1. the tier inventory in [`testing-plan.md`](../.design/testing-plan.md)
2. the executable case list in [`QA_CASES.md`](./QA_CASES.md)

If a bug escapes, either:
- add a new case, or
- tighten an existing case so the failure would be caught next time.
