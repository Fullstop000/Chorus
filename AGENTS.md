# Chorus

Local AI agent collaboration platform. Agents run as real OS processes (Claude Code CLI, Codex CLI) and communicate through a Slack-like chat interface backed by SQLite.

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

| File                    | Purpose                                                     |
| ----------------------- | ----------------------------------------------------------- |
| `src/main.rs`           | CLI entry point, `serve()` bootstrap                        |
| `src/server.rs`         | Axum HTTP handlers + `AgentLifecycle` trait                 |
| `src/agent_manager.rs`  | Process lifecycle, activity log ring buffer                 |
| `src/store.rs`          | All SQLite I/O (channels, messages, agents, tasks)          |
| `src/bridge.rs`         | MCP server spawned as subprocess by agents                  |
| `src/drivers/mod.rs`    | `Driver` trait + `ParsedEvent` enum                         |
| `src/drivers/claude.rs` | Claude Code CLI driver                                      |
| `src/drivers/codex.rs`  | OpenAI Codex CLI driver                                     |
| `src/drivers/prompt.rs` | System prompt builder (shared by both drivers)              |
| `src/models.rs`         | All shared types (`Channel`, `Message`, `Agent`, `Task`, …) |
| `ui/src/App.tsx`        | Root layout + WebSocket polling                             |
| `ui/src/components/`    | React components (one per panel/modal)                      |
| `ui/src/store.tsx`      | Client-side state (zustand)                                 |

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

When constructing message targets, always look up the peer name from `channel_members` — never use the raw internal channel name as the target. See `store.rs:get_messages_for_agent()`.

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

1. Write an `async fn handle_*` in `src/server.rs`
2. Add the route in `build_router_with_lifecycle()`
3. If the handler needs lifecycle access (start/stop/activity), add a method to the `AgentLifecycle` trait — then implement it on both `AgentManager` and `NoopAgentLifecycle` (and `MockLifecycle` in tests)

## Store Conventions

- Every DB operation uses `self.conn.lock().unwrap()` — the connection is `Mutex<Connection>`
- IDs are `uuid::Uuid::new_v4().to_string()`
- Timestamps are stored as ISO 8601 text; parsed via `chrono`
- Broadcast notifications: `self.notify.send((channel_id, message_id))` after any new message

## Testing

```bash
cargo test
```

Tests live in `tests/`. Integration tests use `:memory:` SQLite databases.
When adding methods to `AgentLifecycle`, add stub implementations to `MockLifecycle` in `tests/server_tests.rs`.

## UI Conventions

- Component styles in co-located `.css` files (e.g., `ActivityPanel.tsx` + `ActivityPanel.css`)
- Design tokens via CSS variables defined in `App.css`
- Icons: Lucide React (`lucide-react`) — keep sizes consistent (13px for inline tool icons, 16px for panel icons)
- No global state mutations outside `ui/src/store.tsx`
- API calls go through `ui/src/api.ts`

## Common Pitfalls

- **Bridge logs are invisible**: The bridge process runs with `Stdio::piped()` stderr (never read). Log agent communication in `server.rs` HTTP handlers, not `bridge.rs`.
- **DM target mismatch**: Always resolve `dm:@peer` from `channel_members`, never from the channel's internal name.
- **`AgentLifecycle` trait changes**: Must update `AgentManager`, `NoopAgentLifecycle`, and `MockLifecycle` together.
- **Vite cache**: `ui/.vite/` and `ui/tsconfig.tsbuildinfo` are gitignored — do not commit them.
