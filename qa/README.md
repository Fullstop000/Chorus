# QA Docs

This directory contains the reusable QA operating docs for Chorus.

## Files

- `cases/playwright/` — Playwright specs (one file per automated case ID, e.g. `ENV-001.spec.ts`). Each spec’s header and `test.step` labels track the matching **Preconditions**, **Steps**, and **Expected** blocks in `cases/*.md`. Run from that directory after `npm install` (see below).
- `QA_CASES.md` — static and versioned browser QA case catalog index. Actual cases live in `cases/`.
- `QA_PRESETS.md` — reusable agent/runtime setup presets for runs that need consistent coverage across Claude and Codex.
- `QA_PLAN_TEMPLATE.md` — fill-in template for the pre-run plan. Created before execution; shown to human for approval before any cases are run.
- `QA_REPORT_TEMPLATE.md` — fill-in template for each QA run. Created during execution; completed before presenting findings to the human.
- `BUG_FIX_REPORT_TEMPLATE.md` — fill-in template for the implementation/fix pass that follows a QA run. Created only after human approves a fix pass.
- `runs/{datetime}/plan.md` — pre-run plan for that session.
- `runs/{datetime}/report.md` — execution results for that session.
- `runs/{datetime}/fix_report.md` — fix pass record when code changes are made in response to findings.
- `runs/{datetime}/evidence/` — evidence bundle for that specific run only.

## Why This Directory Exists

The testing plan in `.design/` explains strategy and release philosophy.
This `qa/` directory is the execution layer:

- what to test every time
- how to run it in the real web product
- how to record pass/fail status consistently
- how to scale the process as the product surface grows

## Playwright automation (`cases/playwright/`)

1. Build UI and server from repo root:
   - `cd ui && npm run build && cd .. && cargo build`
2. Start Chorus with a **fresh** temp data dir (recommended):
   - `./target/debug/chorus serve --port 3101 --data-dir /tmp/chorus-qa-playwright`
3. Install browsers and run tests:

```bash
cd qa/cases/playwright
npm install
npx playwright install chromium
npx playwright test
```

- **`CHORUS_BASE_URL`** — default `http://localhost:3101`
- **`CHORUS_E2E_LLM=0`** — skip tests that wait on real agent replies (MSG/TMT parts, etc.)

Each catalog case that is automated lists **Script:** with a relative link to `playwright/<CASE>.spec.ts`. Specs may use **hybrid** checks (UI + `history` / internal API) where the case allows or where the UI alone cannot observe the assertion; notes live in the case’s `Script:` line and in the spec header.

Treat a Playwright spec as the preferred executable form of a case when one exists:

1. If the case’s `Script:` line points at a spec, run that script first.
2. If the script passes, use that result as the primary execution record for the case.
3. If the script fails, rerun the case through the original browser-driven flow, using headed or headless browser control as appropriate, before finalizing the case result.
4. Record both outcomes in the run report when they differ:
   - script failure + manual pass means automation drift or stale script coverage
   - script failure + manual fail means product failure

If a case changes, update the markdown case and its Playwright spec in the same behavior change. The case steps, expected results, spec header, and `test.step` labels should continue to describe the same flow.

Not every case needs a Playwright spec. `_None — manual only_` is acceptable only when at least one of these is true:

- the case is `blocked-until-shipped`
- the case depends on subjective visual judgment or exploratory observation that is not yet stable enough to automate
- the case depends on external runtime behavior, quota, or live-model output that makes deterministic automation impractical today
- the automation cost would require hidden setup or assertions that no longer match the user-visible product flow

When a case intentionally has no script, say why on the `Script:` line in the case file. Do not leave the omission unexplained.

## Execution Rules

1. Use the real website in Chrome or headless Chrome.
2. Prefer a fresh temp data dir for repeatable runs.
3. Run script-backed cases through their Playwright spec first. If the script fails, rerun the case through the browser manually before deciding whether the failure is product behavior or automation drift.
4. Run the cases through the browser, not API-only shortcuts.
5. Capture evidence for every failure:
   - screenshot
   - console error
   - network error or API payload when relevant
6. Any escaped bug should be added to the static case catalog or mapped to an existing case.
7. If a case depends on a control that is not yet exposed in the shipped product, do not fake the workflow by mutating SQLite directly. Follow the case's documented execution mode:
   - run the allowed hybrid path when the case explicitly allows CLI or API assistance
   - otherwise mark the case `Blocked` and record the product gap
8. If creating or starting a QA agent fails because the selected runtime has hit quota exhaustion, try another available runtime for that agent before blocking the run:
   - document the quota failure and the exact runtime/model substitution in the plan and report
   - prefer a fallback that preserves the intent of the preset as closely as possible
   - never silently substitute a different runtime
9. While executing a run, emit progress updates that name the current case and the overall run progress:
   - format the update as current case ID plus completed count, for example `Executing AGT-001 (3/12)`
   - send one update when starting each case
   - if a scripted case falls back to manual execution, mention that in the update for the same case rather than silently switching modes

## Execution Modes

- `browser`
  - execute entirely through the normal web product
- `hybrid`
  - browser-first, but the case may use a documented CLI or API step when the current product still exposes part of the flow outside the main UI
- `blocked-until-shipped`
  - the product does not yet expose the control needed for this QA case; record the gap instead of inventing a hidden setup

## Run Modes

Four modes exist: **PR Smoke**, **Core Regression**, **Recovery / Reliability**, and **Agent Matrix**.

For the authoritative definition of each mode — which trigger conditions apply, which cases are required, and which preset to use — see the **QA Standard Operating Procedure** section in `AGENTS.md`. That is the single source of truth. Do not duplicate case lists here; they diverge.

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
