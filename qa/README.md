# Chorus QA SOP

This directory contains the reusable QA operating docs for Chorus.

`qa/README.md` is the authoritative QA standard operating procedure for this repository. It defines how QA runs are executed, how failures are classified, how evidence is stored, and how the QA system is maintained as the product evolves.

## QA Assets

### Core Files

- `QA_CASES.md`
  - static and versioned browser QA case catalog index
- `cases/*.md`
  - executable case definitions grouped by product area
- `cases/playwright/*.spec.ts`
  - Playwright specs, one file per case ID
- `QA_PRESETS.md`
  - reusable agent and runtime setup presets
- `QA_PLAN_TEMPLATE.md`
  - pre-run plan template
- `QA_REPORT_TEMPLATE.md`
  - run report template
- `BUG_FIX_REPORT_TEMPLATE.md`
  - fix-pass report template

### Run Artifacts

Each QA run should live in its own timestamped directory:

- `runs/{datetime}/plan.md`
  - approved pre-run plan
- `runs/{datetime}/report.md`
  - execution results
- `runs/{datetime}/fix_report.md`
  - implementation/fix record when code changes follow a run
- `runs/{datetime}/evidence/`
  - evidence bundle for that run only

Recommended `{datetime}` format:

- `YYYY-MM-DDTHHMMSS`

Reusable fixtures that are not tied to one run should not live in a run evidence directory. Keep them in a separate location such as `qa/fixtures/`.

## Run QA

This section is the operator path for executing a QA run.

### Standard Environment

Use these defaults unless the run needs a targeted variation:

- browser: Chrome or headless Chrome via Playwright
- backend: local server started from the current branch
- data dir: fresh temp directory
- seed users: current user only
- seed agents: create 1-3 test agents

Use presets in [`QA_PRESETS.md`](./QA_PRESETS.md) and record which preset was used.

### Execution Rules

These rules apply to every QA run:

1. Use the real website in Chrome or headless Chrome
2. Prefer a fresh temp data dir for repeatable runs
3. Run script-backed cases through their linked Playwright spec first
4. If a script fails, rerun the case through the browser manually before deciding whether the failure is product behavior or automation drift
5. Execute cases through the browser, not API-only shortcuts
6. Capture evidence for every failure:
   - screenshot
   - console error
   - network error or API payload when relevant
7. Any escaped bug must be added to the static case catalog or mapped to an existing case with stronger coverage
8. If a case depends on a control that is not yet exposed in the shipped product, do not fake the workflow by mutating SQLite directly:
   - run the allowed hybrid path when the case explicitly allows CLI or API assistance
   - otherwise mark the case `Blocked` and record the product gap
9. If creating or starting a QA agent fails because the selected runtime has hit quota exhaustion, try another available runtime before blocking the run:
   - document the quota failure and the exact runtime or model substitution in the plan and report
   - prefer a fallback that preserves the intent of the preset as closely as possible
   - never silently substitute a different runtime
10. While executing a run, emit progress updates that name the current case and the overall run progress:
   - format updates as current case ID plus completed count, for example `Executing AGT-001 (3/12)`
   - send one update when starting each case
   - if a scripted case falls back to manual execution, mention that in the update for the same case rather than silently switching modes

### Execution Modes

Execution mode describes how a specific case is allowed to run:

- `browser`
  - execute entirely through the normal web product
- `hybrid`
  - browser-first, but the case may use a documented CLI or API step when the product still exposes part of the flow outside the main UI
- `blocked-until-shipped`
  - the product does not yet expose the control needed for this case; record the gap instead of inventing hidden setup

### Run Modes

Run mode describes the scope of the overall QA run.

- `PR Smoke`
  - fast confidence pass for a small or targeted change
  - run the smallest set of critical cases needed to cover the changed surface
- `Core Regression`
  - standard product regression pass
  - run the core cases for messaging, channels, tasks, agent lifecycle, and any changed user-facing flows
- `Recovery / Reliability`
  - targeted stability pass for retry behavior, resume flows, reconnect paths, and failure handling
  - use this when a change affects robustness more than steady-state UI behavior
- `Agent Matrix`
  - compatibility pass across multiple agent runtimes or preset combinations
  - use this when a change touches driver, lifecycle, prompt wiring, bridge behavior, or runtime-specific behavior

For each run, the plan should state:

- chosen run mode
- why that mode is sufficient for the change
- selected preset, if applicable
- any intentionally skipped cases and why

### Playwright Workflow

Playwright specs are the executable form of the catalog cases under `cases/*.md`.

Each catalog case must include a `Script:` link to `cases/playwright/<CASE>.spec.ts`.

Run script-backed cases with this workflow:

1. Build UI and server from the repo root:
   - `cd ui && npm run build && cd .. && cargo build`
   - `cargo build` builds **`chorus`** and **`chorus-stub-agent`** (required for `CHORUS_E2E_LLM=stub`).
2. **Server process:** By default, Playwright **does not** use a manually started server — each worker’s fixture spawns `chorus` on `http://localhost:3200` + worker index with an isolated temp data dir (`qa/cases/playwright/helpers/fixtures.ts`). To drive a server you started yourself, set **`CHORUS_BASE_URL`** (for example `http://localhost:3101`) and run:
   - `./target/debug/chorus serve --port 3101 --data-dir /tmp/chorus-qa-playwright`
3. Install dependencies and browsers, then run tests:

```bash
cd qa/cases/playwright
npm install
npx playwright install chromium
npx playwright test
```

Useful environment and runner options:

- `CHORUS_BASE_URL`
  - when unset, tests use the per-worker spawned server (see step 2); when set, all workers hit this URL instead
- `CHORUS_E2E_LLM=0`
  - skips tests that wait on real agent replies
- `CHORUS_E2E_LLM=stub`
  - uses the **`stub-trio`** preset (`stub-a` / `stub-b` / `stub-c`) and the stub driver for fast, deterministic agent traffic; see [`QA_PRESETS.md`](./QA_PRESETS.md)
- `CHORUS_WORKERS`
  - parallel worker count (default `4` in `playwright.config.ts`); use `1` for serial runs or easier logs
- recommended live reporter:
  - `npx playwright test --reporter=list`
- recommended interactive repro modes:
  - `npx playwright test <CASE>.spec.ts --headed --reporter=line`
  - `npx playwright test --ui`

Execution policy for script-backed cases:

1. Run the case's linked spec first
2. If the script passes, use that as the primary execution record
3. If the script fails, rerun the case through the original browser-driven flow before finalizing the case result
4. Record both outcomes in the run report when they differ:
   - script failure plus manual pass means automation drift or stale script coverage
   - script failure plus manual fail means product failure

When a case is not fully automated yet:

- keep the per-case script file anyway
- mark the spec as a placeholder with an explicit `test.fixme(...)` reason

If a case changes, update the markdown case and its Playwright spec in the same behavior change. The case steps, expected results, spec header, and `test.step(...)` labels should continue to describe the same flow.

## Debug Failures

When a scripted QA case fails, do not write a vague report entry like "Playwright failed." Record the exact failing step, exact repro command, and concrete artifacts that prove the failure.

### Identify The Failed Step

Use Playwright output first.

- every case spec should wrap major assertions in `test.step(...)`
- failing output names the case and the specific `test.step(...)` label
- the stack trace includes the spec file and line number

Example shape:

```text
MSG-003.spec.ts › MSG-003 › Thread Reply In Busy Channel
› Step 1: Open thread from an agent reply
```

That `test.step(...)` label is the step to cite in the QA report.

### Reproduce The Failure

Always rerun the exact spec against a fresh temp data dir before deciding whether the problem is a product regression or automation drift.

Baseline repro:

```bash
cd ui && npm run build && cd ..
cargo build
./target/debug/chorus serve --port 3101 --data-dir /tmp/chorus-qa-repro

cd qa/cases/playwright
npx playwright test <CASE>.spec.ts --reporter=list
```

For live observation:

```bash
npx playwright test <CASE>.spec.ts --headed --reporter=line
```

For interactive debugging:

```bash
npx playwright test --ui
```

Classification rule:

- rerun still fails: treat as reproduced product failure unless evidence shows automation drift
- rerun passes: treat as likely automation drift or nondeterminism and say so explicitly

### Playwright Artifacts

The current Playwright configuration already produces useful debugging output:

- terminal output with failing case and step name
- `test-results/.../error-context.md` for failure context
- `playwright-report/` for the HTML report
- trace artifacts on first retry because `trace: 'on-first-retry'` is enabled in `playwright.config.ts`

These artifacts are useful, but they are not the authoritative QA evidence location for a run. The authoritative evidence location is still the run-local `qa/runs/{datetime}/evidence/` directory.

## Record Evidence

For every failing scripted case, copy or export the relevant Playwright artifacts into the run-local evidence directory and reference those filenames in `report.md`.

### Minimum Evidence Bundle

For a failing scripted case, capture:

1. screenshot or visible failure capture
2. Playwright failure context (`error-context.md` or equivalent text extract)
3. trace file or HTML report pointer when available
4. console or network detail when relevant to the failure mode

### Evidence Naming

Recommended naming inside `qa/runs/{datetime}/evidence/`:

- `YYYY-MM-DD-<CASE>-failure.png`
- `YYYY-MM-DD-<CASE>-error-context.md`
- `YYYY-MM-DD-<CASE>-trace.zip`
- `YYYY-MM-DD-<CASE>-console.txt`
- `YYYY-MM-DD-<CASE>-network.txt`

More general stable naming patterns are also acceptable:

- `YYYY-MM-DD-case-id-short-title.png`
- `YYYY-MM-DD-case-id-console.txt`
- `YYYY-MM-DD-case-id-network.txt`

### Report Requirements

For a failed scripted case, the report entry should include all of the following:

- case ID
- exact failed `test.step(...)` label
- exact repro command used
- whether the failure reproduced on rerun
- evidence filenames copied into `qa/runs/{datetime}/evidence/`

Good example:

```text
MSG-003 failed at "Step 1: Open thread from an agent reply".
Reproduced with: npx playwright test MSG-003.spec.ts --headed --reporter=line
Evidence: 2026-03-26-MSG-003-failure.png, 2026-03-26-MSG-003-error-context.md
```

Bad example:

```text
MSG-003 failed in Playwright.
```

## Maintain The QA System

This section is the maintainer path. Use it whenever the product, QA process, or automation coverage changes.

### Maintenance Rule

When a new user-visible feature, workflow, or state transition ships:

- update [`QA_CASES.md`](./QA_CASES.md) if the catalog surface changed
- add or update the relevant executable case file under `cases/*.md`
- add or update the linked Playwright spec under `cases/playwright/`
- keep the case `Script:` link, spec header, case steps, expected results, and `test.step(...)` labels aligned

When a bug escapes:

- add a new case, or tighten an existing case so the failure would be caught next time
- update the linked Playwright spec if automation coverage contributed to the miss
- record whether the gap was product coverage, automation drift, or missing execution discipline

When the QA process changes:

- update this README if operator behavior, evidence rules, run modes, or failure-classification rules changed
- update [`QA_PRESETS.md`](./QA_PRESETS.md) if runtime or agent setup guidance changed
- update templates if the required plan, report, or fix-pass output changed

### Invariants

These rules should continue to hold as the QA system grows:

- keep case IDs stable so reports remain comparable across iterations
- do not silently delete old cases without updating release expectations
- every executable case should have a discoverable Playwright script, even if it is currently `fixme`
- do not let `QA_CASES.md`, `cases/*.md`, and `cases/playwright/*.spec.ts` drift out of sync
- do not simulate missing user-facing flows by editing SQLite directly during QA

## Reference

### Shared Preconditions

Apply these unless a case overrides them:

- server started from the branch under test
- browser opened to the real app shell
- data dir is fresh
- current human user confirmed by `whoami`
- 3 agents created according to the selected preset from [`QA_PRESETS.md`](./QA_PRESETS.md)
- one text file prepared for attachment upload testing
- default working channel is `#all` or a dedicated `#qa-multi-agent`

### Result Definitions

- `Pass`
  - all expected results observed
- `Fail`
  - at least one expected result is violated
- `Blocked`
  - cannot finish because an earlier failure or environment issue prevents execution
- `Not Run`
  - intentionally skipped in this run mode

### Notes On Product Gaps

- Some QA cases intentionally cover product controls that are not fully shipped yet, such as delete flows or explicit channel member management
- When a case is marked `hybrid` or `blocked-until-shipped`, follow the case instructions exactly
- Do not simulate missing user-facing flows by editing SQLite directly during QA
