# Chorus

Local AI agent collaboration platform. Agents run as real OS processes (Claude Code CLI, Codex CLI) and communicate through a Slack-like chat interface backed by SQLite.

## Agent Working Rules

### Engineering Principles

- Add comments for key structs and non-trivial functions so the intent is clear on first read.
- Handle errors explicitly. Do not ignore `Result`s or rely on hidden failure paths.
- Prefer clear, sufficient design over speculative architecture.
- Read this file before making changes, and keep it aligned with shipped behavior.

### Branch Workflow For Feature Work

When the user explicitly asks to implement a new feature or do a refactor:

1. Check whether the worktree is dirty before switching branches.
2. If local changes exist, stop and ask the user whether to commit, stash, or move them aside.
3. Start the work from an up-to-date `main` based on `origin/main`.
4. Create a new branch with the `codex/` prefix.
5. Do not carry unrelated residual changes into the new branch without explicit user approval.

### Commit Conventions

- Use conventional-style commit messages with a scope when practical.
- Preferred patterns: `feat(settings): ...`, `fix(command): ...`, `refactor(config): ...`, `docs(agent): ...`, `ci: ...`.

### Verification Policy

- Do not claim a task is complete without running verification that matches the risk of the change.
- For backend or data-path changes, run the relevant Rust tests first.
- For any change that affects a core user process, verify the real flow with end-to-end testing in a headless browser against the running app.
- Core process verification is mandatory for user-facing critical paths such as channel messaging, DM flows, thread replies, task board actions, and agent interaction loops.
- Backend integration tests alone are not sufficient when the user-visible flow changed.
- If the required headless-browser e2e verification cannot be run, say so clearly and do not present the work as fully verified.

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

## DM Channel Naming

Internal DB name: `dm-{sorted_a}-{sorted_b}` (e.g., `dm-alice-richard`)
Target string used in messages: `dm:@{peer_name}` (e.g., `dm:@richard`)

When constructing message targets, always look up the peer name from `channel_members` — never use the raw internal channel name as the target. See `src/store/messages.rs:get_messages_for_agent()`.

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
- Broadcast notifications: `self.msg_tx.send((channel_id, message_id))` after any new message

## Testing

```bash
cargo test
cargo test --test e2e_tests
```

Tests live in `tests/`. Integration tests use `:memory:` SQLite databases.
When adding methods to `AgentLifecycle`, add stub implementations to `MockLifecycle` in `tests/server_tests.rs`.

Use this minimum verification bar:

1. Run focused Rust tests for the affected modules.
2. Run `cargo test --test e2e_tests` when the backend message flow, task flow, DM flow, thread flow, or agent lifecycle is affected.
3. For any core user-facing workflow change, start the app and verify the real behavior with a headless browser e2e pass.

Headless-browser e2e must exercise the critical path end to end, not just load the page. At minimum, verify the primary happy-path flow the user relies on, and include the exact flow you checked in the final report.

## UI Conventions

- Component styles in co-located `.css` files (e.g., `ActivityPanel.tsx` + `ActivityPanel.css`)
- Design tokens via CSS variables defined in `App.css`
- Icons: Lucide React (`lucide-react`) — keep sizes consistent (13px for inline tool icons, 16px for panel icons)
- No global state mutations outside `ui/src/store.tsx`
- API calls go through `ui/src/api.ts`

## Common Pitfalls

- **Bridge logs are invisible**: The bridge process runs with `Stdio::piped()` stderr (never read). Log agent communication in `src/server/handlers.rs`, not `src/bridge.rs`.
- **DM target mismatch**: Always resolve `dm:@peer` from `channel_members`, never from the channel's internal name.
- **`AgentLifecycle` trait changes**: Must update `AgentManager`, `NoopAgentLifecycle`, and `MockLifecycle` together.
- **Vite cache**: `ui/.vite/` and `ui/tsconfig.tsbuildinfo` are gitignored — do not commit them.
