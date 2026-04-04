# Chorus QA Presets

This file defines reusable agent/runtime presets for browser QA runs.

Use a preset whenever the run touches agent lifecycle, runtime support, driver behavior,
startup/restart, or the create-agent modal. The goal is to stop ad hoc agent setup from
accidentally covering only one runtime.

## How To Use

1. Pick the preset that matches the change risk.
2. Create the listed agents through the shipped browser UI unless the referenced case says otherwise.
3. Record the preset name and any deviations in the QA run report.
4. If the product build under test does not expose one of the documented runtime/model pairs, note that explicitly in the run report instead of silently substituting a different pair.

## Presets

### `claude-trio`

Use for:
- UI smoke runs that do not touch runtime-specific code
- broad multi-agent messaging sanity checks

Agents:
- `bot-a` — runtime `claude`, model `sonnet`
- `bot-b` — runtime `claude`, model `sonnet`
- `bot-c` — runtime `claude`, model `sonnet`

Notes:
- This is the legacy default.
- Do not use this preset alone for driver, lifecycle, resume, or runtime-matrix changes.

### `mixed-runtime-trio`

Use for:
- core regression after touching driver code, bridge code, lifecycle state, prompt wiring, or message fan-out
- any run where Codex behavior must be verified in the real product

Agents:
- `bot-a` — runtime `claude`, model `sonnet`
- `bot-b` — runtime `claude`, model `opus`
- `bot-c` — runtime `codex`, model `gpt-5.4-mini`

Notes:
- This keeps the normal three-agent concurrency pressure while guaranteeing one real Codex-backed agent in the browser flow.
- Prefer this preset over `claude-trio` for Tier 0 messaging and lifecycle regressions when driver code changed.

### `codex-lifecycle-pair`

Use for:
- restart, resume, idle-loop, wake-up, and workspace verification focused on the Codex driver
- validating that a sleeping or restarted Codex agent wakes on new messages

Agents:
- `codex-a` — runtime `codex`, model `gpt-5.4`
- `codex-b` — runtime `codex`, model `gpt-5.4-mini`

Recommended cases:
- `LFC-001`
- `LFC-002`
- `REC-001`
- `REC-002`
- `WRK-001`
- `PRF-001`

Notes:
- Run this preset in addition to, not instead of, the main messaging trio when the bug could depend on multi-agent fan-out.

### `agent-matrix`

Use for:
- `AGT-002`
- releases that change runtime registration, model lists, or create-agent defaults

Agents:
- one disposable agent for every runtime/model pair currently visible in the create-agent modal

Current UI matrix:
- Claude:
  - `sonnet` (`claude-sonnet-4-6`)
  - `opus` (`claude-opus-4-6`)
  - `haiku` (`claude-haiku-4-5`)
- Codex:
  - `gpt-5.4`
  - `gpt-5.4-mini`
  - `gpt-5.3-codex`
  - `gpt-5.2-codex`
  - `gpt-5.2`
  - `gpt-5.1-codex-max`
  - `gpt-5.1-codex-mini`
- Kimi:
  - `kimi-code/kimi-for-coding`

Notes:
- Use stable names such as `matrix-claude-sonnet` and `matrix-codex-gpt-5-4-mini`.
- Verify the runtime and model badges after creation for every pair.

### `stub-trio`

Use for:
- fast QA runs that test the full UI + message pipeline without LLM latency
- CI smoke tests
- core regression runs where real LLM reasoning is not required

Agents:
- `stub-a` — runtime `stub`, model `echo`
- `stub-b` — runtime `stub`, model `echo`
- `stub-c` — runtime `stub`, model `echo`

Notes:
- Select with `CHORUS_E2E_LLM=stub`.
- Use `agentNames()` from the test helpers to get mode-aware agent names.
- From the repo root, `cargo build` also builds `chorus-stub-agent` beside `chorus` (workspace default members); the server needs that binary to spawn stub runtimes.
- **Fully skipped** in stub mode (entire spec `test.skip` for real LLM): **TMT-004, TMT-008, TMT-009**.
- **Still run** in stub mode but **omit or shorten** LLM-only steps (e.g. team chat wakeups, swarm narrative checks): **TMT-003, TMT-005, TMT-006, TMT-007** — see each spec’s `skipRealLLM` / `useStub` branches.
- Playwright uses a **600s** default test timeout when `CHORUS_E2E_LLM=stub` (`playwright.config.ts`); use `CHORUS_WORKERS=1` for easier debugging.
- The stub runtime is not visible in the create-agent modal — agents are created via API only.
