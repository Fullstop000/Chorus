# Chorus

 AI agent collaboration platform. Agents run as real OS processes  and communicate through a Slack-like chat interface .

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

## QA

The authoritative QA execution workflow lives in `qa/README.md`, with the case catalog and templates under `qa/`.

- Use `qa/README.md` for run modes, scripted-case execution, failure debugging, evidence collection, and report-writing procedure.
- Use `qa/QA_CASES.md` and `qa/cases/*.md` for the executable case definitions.
- Use `qa/QA_PRESETS.md`, `qa/QA_PLAN_TEMPLATE.md`, `qa/QA_REPORT_TEMPLATE.md`, and `qa/BUG_FIX_REPORT_TEMPLATE.md` for run setup and reporting.
- Keep `qa/runs/{datetime}/` as the source of truth for plan, report, fix report, and evidence for each run.

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

## Code Organization

Organize code by subsystem, not by request or by one-off feature patch.

Backend layout:

- `src/main.rs`
  - CLI entrypoint and `serve` bootstrap only
- `src/lib.rs`
  - crate-level module exports
- `src/agent/`
  - agent lifecycle, process management, activity log, collaboration logic, workspace handling
  - runtime-specific subprocess drivers live under `src/agent/drivers/`
- `src/bridge/`
  - MCP bridge implementation, request/response formatting, bridge-local types
- `src/server/`
  - Axum router assembly in `mod.rs`
  - HTTP handlers grouped by domain under `src/server/handlers/`
- `src/store/`
  - SQLite persistence and domain store modules (`agents`, `channels`, `messages`, `tasks`, `teams`, `knowledge`)

Frontend layout:

- `ui/src/App.tsx`
  - top-level shell composition only
- `ui/src/api.ts`
  - browser-to-server API calls only
- `ui/src/store.tsx`
  - client app state and selection logic
- `ui/src/hooks/`
  - reusable data-loading and polling hooks
- `ui/src/components/`
  - UI grouped by panel/modal/component responsibility
- `ui/src/channelList.ts` and `ui/src/types.ts`
  - shared UI-side derivation and types

Organization rules:

- Put new HTTP handlers in the matching file under `src/server/handlers/`; do not grow `server/mod.rs` into a handler dump.
- Put persistence logic in the matching `src/store/*.rs` module; do not hide DB writes in handlers.
- Put agent runtime and subprocess behavior in `src/agent/`; do not mix it into HTTP or store modules.
- Put bridge-only formatting and protocol glue in `src/bridge/`.
- Keep frontend state changes in `ui/src/store.tsx`; components should call APIs and store actions, not invent parallel state systems.
- Co-locate component styles with the component in `ui/src/components/`.
- Treat `qa/` as its own execution layer: specs, plans, reports, and evidence should stay under `qa/`, not mixed into app code.

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

1. Create `src/agent/drivers/myruntimename.rs`
2. Implement the `Driver` trait (all methods required):
   - `spawn()` — launch the CLI subprocess
   - `parse_line()` — parse stdout JSON into `ParsedEvent`
   - `encode_stdin_message()` — format wakeup notifications
   - `build_system_prompt()` — driver-specific prompt formatting
   - `summarize_tool_input()` — short display string per tool call
3. Register in `src/agent/drivers/mod.rs`
4. Add to the driver selection match in `src/agent/manager.rs`

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

Minimum verification policy:

1. Run focused Rust tests for the affected modules.
2. Run `cargo test --test e2e_tests` when backend message flow, task flow, DM flow, thread flow, or agent lifecycle is affected.
3. For core user-facing workflow changes, run the browser QA pass defined in `qa/README.md`.

Rust and integration tests alone are not sufficient when the user-visible flow changed. If the required headless-browser verification cannot be run, say so explicitly and do not present the work as fully verified.

## UI Conventions

- Component styles in co-located `.css` files (e.g., `ActivityPanel.tsx` + `ActivityPanel.css`)
- Design tokens via CSS variables defined in `App.css`
- Icons: Lucide React (`lucide-react`) — keep sizes consistent (13px for inline tool icons, 16px for panel icons)
- No global state mutations outside `ui/src/store.tsx`
- API calls go through `ui/src/api.ts`
