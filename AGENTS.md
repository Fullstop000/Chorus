# Chorus

Local AI agent collaboration platform. Agents run as real OS processes (Claude Code CLI, Codex CLI) and communicate through a Slack-like chat interface backed by SQLite.

## Engineering Principles

- Add comments for key structs and non-trivial functions so the intent is clear on first read.
- Handle errors explicitly. Do not ignore `Result`s or rely on hidden failure paths.
- Prefer clear, sufficient design over speculative architecture.
- Read this file before making changes, and keep it aligned with shipped behavior.

## Branch Workflow For Feature Work

When the user explicitly asks to implement a new feature or do a refactor:

1. Check whether the worktree is dirty before switching branches.
2. If local changes exist, stop and ask the user whether to commit, stash, or move them aside.
3. Start the work from an up-to-date `main` based on `origin/main`.
4. Create a new branch with the `{agent}/` prefix (agent: codex/claude/gemini/...).
5. Do not carry unrelated residual changes into the new branch without explicit user approval.

## Commit Conventions

- Use conventional-style commit messages with a scope when practical.
- Preferred patterns: `feat(settings): ...`, `fix(command): ...`, `refactor(config): ...`, `docs(agent): ...`, `ci: ...`.

## Verification Policy

- Do not claim a task is complete without running verification that matches the risk of the change.
- For backend or data-path changes, run the relevant Rust tests first.
- For any change that affects a core user process, verify the real flow with end-to-end testing in a headless browser against the running app.
- Core process verification is mandatory for user-facing critical paths such as channel messaging, DM flows, thread replies, task board actions, and agent interaction loops.
- Backend integration tests alone are not sufficient when the user-visible flow changed.
- If the required headless-browser e2e verification cannot be run, say so clearly and do not present the work as fully verified.

## QA Standard Operating Procedure

The `qa/` directory is the authoritative execution layer for regression testing. Every QA activity — planning runs, executing cases, writing reports, fixing bugs, maintaining the catalog — follows this SOP.

#### Source of Truth Files

| File | Role |
| ---- | ---- |
| `qa/README.md` | Execution rules, run modes, evidence naming, maintenance rules |
| `qa/QA_CASES.md` | Case catalog index; links to per-domain modules under `qa/cases/` |
| `qa/cases/*.md` | Executable case specs (agents, channels, messaging, tasks, shared_memory) |
| `qa/QA_PRESETS.md` | Agent/runtime presets to use for each run type |
| `qa/QA_PLAN_TEMPLATE.md` | Fill-in template for a QA plan (before execution) |
| `qa/QA_REPORT_TEMPLATE.md` | Fill-in template for every QA run report (after execution) |
| `qa/BUG_FIX_REPORT_TEMPLATE.md` | Fill-in template for the fix pass that follows a failing run |
| `qa/runs/{datetime}/plan.md` | Pre-run plan for that session |
| `qa/runs/{datetime}/report.md` | Execution results for that session |
| `qa/runs/{datetime}/fix_report.md` | Fix pass record when code changes result |
| `qa/runs/{datetime}/evidence/` | Screenshots, console logs, network captures for that run |

#### When to Run QA Cases

Run QA proactively at these trigger points — do not wait to be asked:

| Trigger | Run mode |
| ------- | -------- |
| Before merging a medium or large PR | PR Smoke |
| After touching messaging, lifecycle, tasks, uploads, or workspace | Core Regression |
| After touching startup, persistence, session restore, or runtime integration | Recovery / Reliability |
| After touching runtime support, model options, driver registration, or create-agent modal | Agent Matrix |
| After fixing a High or Medium severity bug found in QA | Post-fix verification using the cases that originally failed |
| Before any release | Core Regression (minimum) |

For small UI-only changes (CSS tweaks, copy changes, icon adjustments), a full QA run is not required — verify visually and note the rationale.

#### When to Write or Update a QA Case

Write or update a case when any of the following are true:

- **New user-visible feature ships** — add a case before or alongside the PR; do not ship without coverage.
- **Bug escaped QA** — either add a case covering the exact failure path, or tighten an existing case so the failure would be caught next time. Do not leave an escaped bug without a coverage answer.
- **Case is too coarse** — if a case passed but a related failure slipped through, tighten the expected results or add a failure signal.
- **New domain or workflow area** — add a new module file under `qa/cases/` and register it in `qa/QA_CASES.md`.
- **Product control ships for a `blocked-until-shipped` case** — convert the case to `browser` or `hybrid` and remove the blocked marker.

#### Selecting a Run Mode

Choose based on what changed. Do not invent an ad hoc checklist.

| What changed | Run mode | Required cases |
| ------------ | -------- | -------------- |
| UI, non-critical path | **PR Smoke** | Base smoke set: ENV-001, AGT-001, LFC-001, CHN-001, MSG-001–003, TSK-001, TSK-002, PRF-001, ACT-001 |
| Messaging, lifecycle, tasks, uploads, workspace, or any channel-like collaboration flow | **Core Regression** | Entire PR Smoke set, plus every touched Tier 0 case and every release-sensitive Tier 1 case in the changed domain. For team work, include TMT-001–004 at minimum and add TMT-005–008 when membership, settings, delete, or multi-team behavior changed. |
| Startup, persistence, session restore, restart, or runtime recovery | **Recovery / Reliability** | LFC-002, REC-001, REC-002, MSG-004, WRK-001, ACT-001, ACT-002, PRF-001, plus any touched Tier 0 cases |
| Runtime support, model options, driver registration, reasoning-effort controls, or create-agent modal | **Agent Matrix** | AGT-002, plus ENV-001 and AGT-001 as a sanity check when the visible create flow changed |

Treat the table above as the minimum required set. If a changed feature has its own release-sensitive case, run it even when it is not named explicitly in the table.

#### Selecting an Agent Preset

Pick from `qa/QA_PRESETS.md`. Record the preset name in the plan and report. Never silently substitute a different runtime/model.

| Preset | When to use |
| ------ | ----------- |
| `claude-trio` | UI-only smoke runs not touching runtime-specific code |
| `mixed-runtime-trio` | Core regression after driver, bridge, lifecycle, or message fan-out changes |
| `codex-lifecycle-pair` | Restart, resume, idle-loop, workspace focused on the Codex driver |
| `agent-matrix` | Any run verifying runtime matrix, model list, or create-agent defaults |

Default to `mixed-runtime-trio` for any Tier 0 regression when driver code changed.

#### Running a QA Session

**Phase 1 — Plan (before executing any cases)**

1. Create the run directory: `qa/runs/YYYY-MM-DDTHHMMSS/` and `evidence/` subdirectory.
2. Copy `qa/QA_PLAN_TEMPLATE.md` → `qa/runs/{datetime}/plan.md`. Fill in: trigger, run mode + rationale, preset + rationale, planned case list, excluded cases + reasons, environment setup, and known risks.
3. **Present the completed plan to the human. Wait for explicit approval before executing any cases.** The human may narrow scope, change mode, or add risk areas.

**Phase 2 — Execute**

4. Start the server from the branch under test. Use a fresh temp data dir unless the case requires an existing state.
5. Create agents through the browser UI per the selected preset — never by mutating SQLite directly.
6. Copy `qa/QA_REPORT_TEMPLATE.md` → `qa/runs/{datetime}/report.md`. Fill in all metadata fields.
7. Execute each case according to the authoritative workflow in `qa/README.md`.
8. During execution, output per-case progress in the form `Executing {CASE_ID} ({finished_cases}/{total_cases})`, and note when a scripted case falls back to manual execution.
9. Record per-case results, capture required evidence, and complete the report fields for findings, release gate, and regression follow-up.

**Phase 3 — Human handoff (after report is complete)**

13. **Present the findings summary to the human.** Include: overall result, list of all failures with severity, release gate decision, and the Regression Follow-Up table showing what new/tightened coverage is needed.
14. **Explicitly ask the human which follow-up actions to take.** Offer these options, and wait for a clear choice before acting:
    - Fix the failing issues now (starts the fix pass below)
    - Improve or add QA cases to the catalog
    - Defer findings to a follow-up issue
    - No action — accept the current state
15. Do not begin any follow-up action without human approval. Do not mark a workflow regression-tested unless the report names the exact case IDs that were run end to end.

#### Fixing Bugs Found in QA

Only begin this phase after the human explicitly approves a fix pass.

1. Copy `qa/BUG_FIX_REPORT_TEMPLATE.md` → `qa/runs/{datetime}/fix_report.md`.
2. Link it to the corresponding `report.md` via the Run Linkage section.
3. For each finding approved for fixing, document: root cause, fix status, verification method, and result.
4. Run the Verification Matrix: Rust tests → UI build → browser E2E covering the fixed path.
5. Complete the Regression Coverage Follow-Up table — every fix must identify whether new or tightened QA coverage is needed.
6. After the fix pass, present a summary to the human and confirm whether a follow-up QA run is required before the release gate can be cleared.

#### Case Design Rules

When writing a new QA case or tightening an existing one:

- **Assign a stable ID** using the domain prefix (e.g., `MSG-005`). Never reuse a retired ID.
- **Set the Tier explicitly**: Tier 0 = critical path required every run; Tier 1 = reliability / secondary flows.
- **Set `Release-sensitive`** to `yes` when the case must be run before any release touching that domain.
- **Set `Execution mode`**: `browser`, `hybrid`, or `blocked-until-shipped`.
- **Write atomic, deterministic steps** — specific enough to execute without guesswork across multiple runs.
- **State expected results and common failure signals** — make it clear what a pass looks like and what specific failures look like.
- **Do not fake missing UI flows**: if a product control is not yet shipped, mark the case `blocked-until-shipped` and note the gap.
- Follow the script authoring and maintenance rules in `qa/README.md` when a case has, or should have, Playwright coverage.
- Place the case in the appropriate module file under `qa/cases/` and add it to the index table in `qa/QA_CASES.md`.

#### Catalog Maintenance Rules

Keeping the case catalog current is a first-class engineering responsibility.

- **New user-visible feature** → add a case before or alongside the feature PR.
- **Escaped bug** → add or tighten a case; do not leave the gap open.
- **Retired feature or flow** → mark the case `Not Run` with a note; do not silently delete it.
- **Runtime/model list changes** → update the agent matrix in `qa/QA_PRESETS.md` to match the current UI.
- **Case ID stability** → reports are compared across iterations by ID. Never renumber or reorder without updating historical reports that reference the changed IDs.
- After any catalog update, verify the index table in `qa/QA_CASES.md` stays in sync with the module files under `qa/cases/`.

## Architecture

```
ui/ (React + Vite)  ←→  src/ (Rust/Axum)  ←→  ~/.chorus/
                              │
                    ┌─────────┴─────────┐
                    │                   │
              agent processes      bridge processes
              (claude/codex CLI)   (chorus bridge --agent-id)
```

**Data flow for an agent receiving a message:**

1. Human sends via UI → `POST /api/channels/{name}/messages`
2. Server writes to SQLite, notifies via broadcast channel
3. `AgentManager` wakes the agent's notification task → writes to agent's stdin
4. Agent (Claude/Codex) calls `mcp__chat__receive_message` via bridge MCP server
5. Bridge POSTs to `GET /internal/agent/{id}/receive` (long-poll, 30s timeout)
6. Agent replies via `mcp__chat__send_message` → bridge POSTs to `/internal/agent/{id}/send`

## Key Files

| File                      | Purpose                                                       |
| ------------------------- | ------------------------------------------------------------- |
| `src/main.rs`             | CLI entry point, `serve()` bootstrap                          |
| `src/server/mod.rs`       | Router construction, `AgentLifecycle` trait, shared server glue |
| `src/server/handlers.rs`  | Axum HTTP handlers for bridge and UI endpoints                |
| `src/agent_manager.rs`    | Process lifecycle, parsed event handling, agent wakeups       |
| `src/activity_log.rs`     | Agent activity log data structures and sequencing             |
| `src/store/mod.rs`        | SQLite store bootstrap, schema, shared helpers                |
| `src/store/messages.rs`   | Message history, unread, DM/thread delivery logic             |
| `src/store/tasks.rs`      | Task board persistence and transitions                        |
| `src/store/agents.rs`     | Agent records and session persistence                         |
| `src/store/channels.rs`   | Channel membership and channel creation logic                 |
| `src/bridge.rs`           | MCP server spawned as subprocess by agents                    |
| `src/drivers/mod.rs`      | `Driver` trait + `ParsedEvent` enum                           |
| `src/drivers/claude.rs`   | Claude Code CLI driver                                        |
| `src/drivers/codex.rs`    | OpenAI Codex CLI driver                                       |
| `src/drivers/prompt.rs`   | System prompt builder (shared by both drivers)                |
| `src/models.rs`           | All shared types (`Channel`, `Message`, `Agent`, `Task`, …)   |
| `ui/src/App.tsx`          | Root layout + WebSocket polling                               |
| `ui/src/components/`      | React components (one per panel/modal)                        |
| `ui/src/store.tsx`        | Client-side state (zustand)                                   |

## Development

```bash
# Full dev environment (backend + UI hot-reload)
./dev.sh

# Backend only
cargo build && ./target/debug/chorus serve

# UI only (needs backend running)
cd ui && npm run dev

# Run tests
cargo test

# Production build
make release
```

**Ports:** API on `:3001`, UI dev server on `:5173`.

**Logging:** Use `RUST_LOG=chorus=debug` for verbose output.
All logging uses `tracing` — never use `eprintln!` or `println!` in library code.

## Agent Sessions

Each agent runs as a **single process** across all channels and DMs. One session = one process = full conversation history in the agent's memory.

- Session ID persisted to SQLite on `SessionInit` / `TurnEnd`
- On server restart, active agents are auto-restarted with `--resume <session_id>` (Claude) or `codex exec resume <thread_id>` (Codex)
- Context isolation between channels is provided via `MEMORY.md` in the agent's workspace, not via separate processes

## Agent Chat & MCP Integration

### How the Bridge Works

Each agent process communicates with the Chorus server through a **bridge subprocess** (`chorus bridge --agent-id <name>`). The bridge is spawned by the agent driver (via `--mcp-server` flag for Claude, `--mcp` for Codex) and runs as a local MCP server on stdio. The agent CLI sees it as just another MCP tool provider.

```
Agent CLI process
    └─ bridge subprocess (chorus bridge --agent-id alice)
           │  MCP over stdio (JSON-RPC)
           │
           └─ HTTP → Chorus server (localhost:3001)
                  └─ SQLite + broadcast channel
```

## Adding a New Driver

1. Create `src/drivers/myruntimename.rs`
2. Implement the `Driver` trait (all methods required):
   - `spawn()` — launch the CLI subprocess
   - `parse_line()` — parse stdout JSON into `ParsedEvent`
   - `encode_stdin_message()` — format wakeup notifications
   - `build_system_prompt()` — driver-specific prompt formatting
   - `summarize_tool_input()` — short display string per tool call
3. Register in `src/drivers/mod.rs`
4. Add to the match in `agent_manager.rs` where drivers are instantiated

## Adding a New API Endpoint

1. Write an `async fn handle_*` in `src/server/handlers.rs`
2. Add the route in `src/server/mod.rs` inside `build_router_with_lifecycle()`
3. If the handler needs lifecycle access (start/stop/activity), add a method to the `AgentLifecycle` trait — then implement it on both `AgentManager` and `NoopAgentLifecycle` (and `MockLifecycle` in tests)

## Store Conventions

- Every DB operation uses `self.conn.lock().unwrap()` — the connection is `Mutex<Connection>`
- IDs are `uuid::Uuid::new_v4().to_string()`
- Timestamps are stored as ISO 8601 text; parsed via `chrono`

## Testing

```bash
cargo test
cargo test --test e2e_tests
```

Catalog-aligned browser automation: `qa/cases/playwright/` (see `qa/README.md`).

Tests live in `tests/`. Integration tests use `:memory:` SQLite databases.
When adding methods to `AgentLifecycle`, add stub implementations to `MockLifecycle` in `tests/server_tests.rs`.

**Minimum verification bar before claiming a task complete:**

1. Run focused Rust tests for the affected modules.
2. Run `cargo test --test e2e_tests` when the backend message flow, task flow, DM flow, thread flow, or agent lifecycle is affected.
3. For any core user-facing workflow change, run the browser QA pass per the [QA SOP](#qa-standard-operating-procedure) above — follow the run mode selection table to pick the right scope.

Rust/integration tests alone are not sufficient when the user-visible flow changed. If headless-browser e2e cannot be run, say so explicitly; do not present the work as fully verified.

## UI Conventions

- Component styles in co-located `.css` files (e.g., `ActivityPanel.tsx` + `ActivityPanel.css`)
- Design tokens via CSS variables defined in `App.css`
- Icons: Lucide React (`lucide-react`) — keep sizes consistent (13px for inline tool icons, 16px for panel icons)
- No global state mutations outside `ui/src/store.tsx`
- API calls go through `ui/src/api.ts`
