# Chorus

Chorus is an AI agent collaboration platform. Agents run as real OS processes and communicate through a Slack-like chat interface.

This file is the working contract for agents in this repository. Read it before making changes, and keep it aligned with shipped behavior.

## Architecture

```text
ui/ (React + Vite)  <->  src/ (Rust/Axum)  <->  ~/.chorus/
                              |
                    +---------+---------+
                    |                   |
                    agent processes      bridge processes
                 (claude/codex/kimi CLI) (chorus bridge --agent-id)
```

### Message Flow

Data flow for an agent receiving a message:

1. Human sends via UI -> `POST /api/channels/{name}/messages`
2. Server writes to SQLite and notifies via broadcast channel
3. `AgentManager` wakes the agent notification task and writes to the agent stdin
4. Agent (Claude/Codex) calls `mcp__chat__receive_message` via the bridge MCP server
5. Bridge POSTs to `GET /internal/agent/{id}/receive` (long-poll, 30s timeout)
6. Agent replies via `mcp__chat__send_message`, and the bridge POSTs to `/internal/agent/{id}/send`

Data flow for a human viewing chat in the browser:

1. Selecting a channel, DM, or thread bootstraps a history snapshot through `GET /internal/agent/{id}/history`
2. After bootstrap, the UI opens one session-wide websocket at `/api/events/ws`
3. The websocket carries notification events such as `conversation.state` and `thread.state`, with absolute unread and latest-seq state but no message bodies
4. Channel, DM, and thread switches replace active subscriptions on that same socket rather than opening per-target sockets
5. When the active target receives a newer `latestSeq`, the UI fetches incremental history with `after=<last_loaded_seq>` and merges those rows into the visible timeline
6. Inactive targets use the same notifications only to refresh badges and unread state; they do not eagerly fetch message bodies
7. Human-composed messages render optimistically with a local sending state until `/internal/agent/{id}/send` returns the durable `message_id` and `seq`
8. Read cursors advance only when messages become visible in the viewport, and the browser reports that through `POST /internal/agent/{id}/read-cursor`

### Agent Sessions

Each agent runs as a single process across all channels and DMs. One session equals one process and retains full conversation history in the agent memory.

- Session ID is persisted to SQLite on `SessionInit` and `TurnEnd`
- On server restart, active agents are auto-restarted with `--resume <session_id>` (Claude), `codex exec resume <thread_id>` (Codex), or `kimi --session <session_id>` (Kimi)
- Context isolation between channels is provided through `MEMORY.md` in the agent workspace, not through separate processes

## Code Organization

Organize code by subsystem, not by request or one-off feature patches.

### Backend Layout

- `src/main.rs`
  - CLI entrypoint and `serve` bootstrap only
- `src/lib.rs`
  - crate-level module exports
- `src/agent/`
  - agent lifecycle, process management, activity log, collaboration logic, workspace handling
  - runtime-specific subprocess drivers live under `src/agent/drivers/`
- `src/bridge/`
  - MCP bridge implementation, request and response formatting, bridge-local types
- `src/server/`
  - Axum router assembly in `mod.rs`
  - HTTP handlers grouped by domain under `src/server/handlers/`
- `src/store/`
  - SQLite persistence and domain store modules (`agents`, `channels`, `messages`, `tasks`, `teams`, `knowledge`)

### Frontend Layout

- `ui/src/App.tsx`
  - top-level shell composition only
- `ui/src/api.ts`
  - browser-to-server API calls only
- `ui/src/store.tsx`
  - client app state and selection logic
- `ui/src/hooks/`
  - reusable data-loading and interaction hooks
- `ui/src/components/`
  - UI grouped by panel, modal, and component responsibility
- `ui/src/channelList.ts` and `ui/src/types.ts`
  - shared UI-side derivation and types

### Organization Rules

- Put new HTTP handlers in the matching file under `src/server/handlers/`; do not grow `src/server/mod.rs` into a handler dump
- Put persistence logic in the matching `src/store/*.rs` module; do not hide DB writes in handlers
- Put agent runtime and subprocess behavior in `src/agent/`; do not mix it into HTTP or store modules
- Put bridge-only formatting and protocol glue in `src/bridge/`
- Keep frontend state changes in `ui/src/store.tsx`; components should call APIs and store actions, not invent parallel state systems
- Co-locate component styles with the component in `ui/src/components/`
- Treat `qa/` as its own execution layer; specs, plans, reports, and evidence stay under `qa/`, not mixed into app code

## Core Conventions

### Engineering Principles

- Add comments for key structs and non-trivial functions so intent is clear on first read
- Handle errors explicitly; do not ignore `Result`s or rely on hidden failure paths
- Prefer clear, sufficient design over speculative architecture
- Read this file before making changes, and keep it aligned with shipped behavior

### Store Conventions

- Every DB operation uses `self.conn.lock().unwrap()`; the connection is `Mutex<Connection>`
- IDs are `uuid::Uuid::new_v4().to_string()`
- Timestamps are stored as ISO 8601 text and parsed via `chrono`

### UI Conventions

- Component styles live in co-located `.css` files (for example `ActivityPanel.tsx` + `ActivityPanel.css`)
- Design tokens are CSS variables defined in `App.css`
- Icons use `lucide-react`; keep sizes consistent (13px for inline tool icons, 16px for panel icons)
- No global state mutations outside `ui/src/store.tsx`
- API calls go through `ui/src/api.ts`
- The shell bootstraps `/api/server-info`, `/api/channels`, `/api/agents`, and `/api/teams` once and should not poll them again while idle; sidebar lists refresh only after explicit create/edit flows or other real user-triggered invalidation

### Logging

Use `RUST_LOG=chorus=debug` for verbose output. All logging uses `tracing`; never use `eprintln!` or `println!` in library code.

## Development Workflow

### Branch Workflow For Feature Work

When the user explicitly asks to implement a new feature or do a refactor:

1. Check whether the worktree is dirty before switching branches
2. If local changes exist, stop and ask the user whether to commit, stash, or move them aside
3. Start work from an up-to-date `main` based on `origin/main`
4. Create a new branch with the `{agent}/` prefix (`codex/`, `claude/`, `gemini/`, and so on)
5. Do not carry unrelated residual changes into the new branch without explicit user approval

### Commit Conventions

- Use conventional-style commit messages with a scope when practical
- Preferred patterns: `feat(settings): ...`, `fix(command): ...`, `refactor(config): ...`, `docs(agent): ...`, `ci: ...`

### Development Commands

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

### Ports

- API: `:3001`
- UI dev server: `:5173`

## Verification Policy

Do not claim a task is complete without running verification that matches the risk of the change.

### Minimum Verification

1. Run focused Rust tests for the affected modules
2. Run `cargo test --test e2e_tests` when backend message flow, task flow, DM flow, thread flow, or agent lifecycle is affected
3. For core user-facing workflow changes, run the browser QA pass defined in `qa/README.md`

### Required Escalation

- For backend or data-path changes, run the relevant Rust tests first
- For any change that affects a core user process, verify the real flow with headless-browser end-to-end testing against the running app
- Core process verification is mandatory for user-facing critical paths such as channel messaging, DM flows, thread replies, task board actions, and agent interaction loops
- Backend integration tests alone are not sufficient when the user-visible flow changed
- If required headless-browser verification cannot be run, say so clearly and do not present the work as fully verified

## QA Workflow

The authoritative QA execution workflow lives in `qa/README.md`, with the case catalog and templates under `qa/`.

## Extension Points

### Adding A New Driver

1. Create `src/agent/drivers/myruntimename.rs`
2. Implement the `Driver` trait with all required methods:
3. Register the driver in `src/agent/drivers/mod.rs`
4. Add it to the driver selection match in `src/agent/manager.rs`
5. Follow [`docs/DRIVER_GUIDE.md`](./docs/DRIVER_GUIDE.md) for protocol discovery, live-runtime debugging, and required verification

## Completion Checklist

Before stopping, confirm all of the following:

- The change lives in the correct subsystem and file
- Verification matches the risk of the change
- Required e2e or browser QA was run for user-visible critical paths, or the gap was called out explicitly
- `AGENTS.md` or related docs were updated if shipped behavior or workflow changed
