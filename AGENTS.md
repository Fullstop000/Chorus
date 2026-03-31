# Chorus

AI agent collaboration platform. Agents run as OS processes and communicate through a Slack-like chat interface.

This file is the working contract for agents. Read it before making changes.

---

## Code Style

| # | Rule | Example |
|---|------|---------|
| 1 | Names are documentation | `isLoading` not `loading`; `hasPermission()` not `checkPermission()` |
| 2 | One word per concept | Don't alternate fetch/get/retrieve/load |
| 3 | Booleans read as questions | `isLoading`, `hasError`, `canSubmit` |
| 4 | No cryptic shortcuts | `id`, `url`, `err` ok in narrow scopes only |
| 5 | Functions do one thing | Ideal: 5-15 lines. Nested conditionals > 2 levels = extract |
| 6 | Max 3 arguments | Use options object: `createUser({ name, age, role })` |
| 7 | Return early | Guard clauses > deep nesting |
| 8 | Pure functions preferred | Isolate I/O, mutations, randomness at edges |

**Structure:**
- Organize by feature: `src/auth/`, `src/billing/` — not `src/models/`, `src/controllers/`
- Files over ~300 lines = refactor signal. Over 500 lines = problem.
- UI contains no SQL. Business logic contains no display strings.

**State:**
- Minimize mutable state. Prefer `const` over `let`.
- Colocate state with consumers. No global state unless truly shared.
- Make invalid states unrepresentable: `type Status = 'idle' | 'loading' | 'error'` not `{ isLoading: true, hasError: true }`

**Error Handling:**
- Fail fast and loudly. Never swallow exceptions.
- Add context when re-throwing: `throw new Error(\`Failed to load user ${userId}: ${e.message}\`)`
- Use typed errors, not `null` for failures.

**Testing:**
- Follow Arrange–Act–Assert (AAA).
- Test behavior, not implementation.
- One assertion per test (ideally).

**Comments:**
- Explain *why*, not *what*.
- Delete dead code; don't comment it out.
- Outdated comments are worse than no comments.

> **Meta-Principle:** Code is written once, read hundreds of times. Optimize for the next reader (often you, six months from now).

---

## Architecture

```
ui/ (React + Vite)  <->  src/ (Rust/Axum)  <->  ~/.chorus/
                              |
                    +---------+---------+
                    |                   |
                    agent processes      bridge processes
                 (claude/codex/kimi CLI) (chorus bridge --agent-id)
```

**Message Flow (Agent receives):**
1. Human sends via UI → `POST /api/channels/{name}/messages`
2. Server writes to SQLite, notifies via broadcast
3. `AgentManager` wakes agent, writes to agent stdin
4. Agent calls `mcp__chat__receive_message` via bridge MCP
5. Bridge POSTs to `GET /internal/agent/{id}/receive` (30s long-poll)
6. Agent replies via `mcp__chat__send_message` → Bridge POSTs to `/internal/agent/{id}/send`

**Message Flow (Human views):**
1. Select channel/DM/thread → Bootstrap history via `GET /internal/agent/{id}/history`
2. Open session-wide websocket at `/api/events/ws`
3. `message.created` frames carry (channel id, latest seq); UI refreshes badges via HTTP (`/api/inbox`, `/inbox-notification`)
4. Target switches replace subscriptions on same socket
5. New `latestSeq` → UI fetches incremental history with `after=<last_loaded_seq>`
6. Inactive targets refresh badges only; don't fetch message bodies
7. Human messages render optimistically until server returns `message_id` and `seq`
8. Read cursors advance on viewport visibility → `POST /internal/agent/{id}/read-cursor`

**Agent Sessions:**
- One process per agent across all channels/DMs. One session = one process.
- Session ID persisted on `SessionInit` and `TurnEnd`
- Auto-restart on server restart with `--resume <session_id>` (Claude), `codex exec resume <thread_id>` (Codex), `kimi --session <session_id>` (Kimi)
- Context isolation via `MEMORY.md` in agent workspace, not separate processes

---

## Code Organization

**Backend:**
| Path | Purpose |
|------|---------|
| `src/main.rs` | CLI entrypoint, `serve` bootstrap |
| `src/lib.rs` | Crate-level module exports |
| `src/agent/` | Agent lifecycle, process management, activity log, collaboration, workspace. Runtime drivers: `src/agent/drivers/` |
| `src/bridge/` | MCP bridge, request/response formatting |
| `src/server/` | Axum router assembly in `mod.rs`. Handlers grouped by domain under `src/server/handlers/` |
| `src/store/` | SQLite persistence. Modules: `agents`, `channels`, `messages`, `tasks`, `teams` |

**Frontend:**
| Path | Purpose |
|------|---------|
| `ui/src/App.tsx` | Top-level shell |
| `ui/src/api.ts` | Browser-to-server API calls |
| `ui/src/store.tsx` | Client app state and selection logic |
| `ui/src/hooks/` | Reusable data-loading and interaction hooks |
| `ui/src/components/` | UI grouped by panel, modal, component responsibility |
| `ui/src/channelList.ts`, `types.ts` | Shared UI-side derivation and types |

**Rules:**
- HTTP handlers go in `src/server/handlers/`, not `src/server/mod.rs`
- Persistence logic goes in `src/store/*.rs`, not handlers
- Agent runtime goes in `src/agent/`, not HTTP or store modules
- Bridge formatting goes in `src/bridge/`
- Frontend state changes only in `ui/src/store.tsx`
- Component styles co-located: `Component.tsx` + `Component.css`
- `qa/` is its own execution layer; specs/plans/reports stay under `qa/`

---

## Core Conventions

**Engineering:**
- Add comments for key structs and non-trivial functions
- Handle errors explicitly; never ignore `Result`s
- Prefer clear, sufficient design over speculative architecture
- Read this file before making changes; keep it aligned with shipped behavior

**Store:**
- DB operations: `self.conn.lock().unwrap()` (connection is `Mutex<Connection>`)
- IDs: `uuid::Uuid::new_v4().to_string()`
- Timestamps: ISO 8601 text, parsed via `chrono`

**UI:**
- Component styles in co-located `.css` files
- Design tokens in CSS variables in `App.css`
- Icons: `lucide-react` (13px inline, 16px panel)
- No global state mutations outside `ui/src/store.tsx`
- API calls through `ui/src/api.ts`
- Shell bootstraps `/api/server-info`, `/api/channels`, `/api/agents`, `/api/teams` once; no polling while idle

**Logging:**
- `RUST_LOG=chorus=debug` for verbose output
- Use `tracing`; never `eprintln!` or `println!` in library code

---

## Development Workflow

**Branch Workflow:**
1. Check worktree dirty before switching branches
2. If local changes exist, ask user to commit/stash/move
3. Start from up-to-date `main` based on `origin/main`
4. Create branch with `{agent}/` prefix (`codex/`, `claude/`, `gemini/`, etc.)
5. Don't carry unrelated changes into new branch

**Commits:**
- Use conventional style with scope: `feat(settings):`, `fix(command):`, `refactor(config):`, `docs(agent):`, `ci:`

**Commands:**
```bash
# Full dev environment (backend + UI hot-reload)
./dev.sh

# Backend only
cargo build && ./target/debug/chorus serve

# UI only (needs backend running)
cd ui && npm run dev
```

---

## Verification Policy

Do not claim complete without matching verification.

**Minimum:**
1. Run focused Rust tests for affected modules
2. Run `cargo test --test e2e_tests` when backend message/task/DM/thread/agent flow affected
3. For user-facing changes, run browser QA pass in `qa/README.md`

**Escalation:**
- Backend/data-path changes: Rust tests first
- Core user process changes: headless-browser e2e testing mandatory
- Core paths: channel messaging, DM flows, thread replies, task board, agent loops
- Backend tests alone insufficient for user-visible changes
- If headless-browser verification cannot run, state it clearly; don't claim fully verified

---

## QA Workflow

Authoritative workflow in `qa/README.md`. Case catalog and templates under `qa/`.

---

## Extension Points

**Adding A New Driver:**
1. Create `src/agent/drivers/myruntimename.rs`
2. Implement `Driver` trait
3. Register in `src/agent/drivers/mod.rs`
4. Add to driver selection in `src/agent/manager.rs`
5. Follow [`docs/DRIVER_GUIDE.md`](./docs/DRIVER_GUIDE.md)

---

## Completion Checklist

Before stopping, confirm:
- [ ] Change lives in correct subsystem and file
- [ ] Verification matches risk of change
- [ ] Required e2e/browser QA run for user-facing critical paths, or gap called out
- [ ] `AGENTS.md` or related docs updated if shipped behavior/workflow changed
