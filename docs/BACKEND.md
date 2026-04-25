# Chorus Backend Conventions

How the Rust side of Chorus is organized, written, and tested. Read this before any backend change.

**What belongs here:** Architecture decisions, data flow patterns, design principles, and conventions that are not obvious from reading the code.

**What does NOT belong here:** Specific line numbers, implementation details that can be discovered through grep/IDE, or API documentation that lives in code comments.

---

## Project Architecture

### Layer Structure

```
┌─ Server (Axum) ──────────────────────────┐
│  Handlers → DTOs → Service calls         │
├─ Store ──────────────────────────────────┤
│  Domain modules (messages/, inbox/, ...) │
│  Schema (tables + views)                 │
│  Migrations (additive only)              │
├─ Agent ──────────────────────────────────┤
│  Runtime drivers (ACP + Raw)             │
│  MCP bridge                              │
└──────────────────────────────────────────┘
```

### Key Design Principle: Views as Read Models

**Read-model state lives in SQL VIEWs, not in Rust queries.**

When you need to change what counts as unread, change the view. Do not bypass the view with ad-hoc Rust queries — you will drift.

**Critical views to know:**
- `inbox_conversation_state_view` — unread counts, read cursors, per-member conversation state
- `conversation_messages_view` — history projection with joined metadata
- `thread_summaries_view` — thread metadata (reply count, participants)

**Schema update rules:**
- Tables use `CREATE TABLE IF NOT EXISTS` — additive changes only
- Views use `DROP VIEW IF EXISTS ... CREATE VIEW` — rebuilt on every startup
- **Implication:** view changes are zero-migration; table changes need migrations in `src/store/migrations.rs`

### Module Organization

Organize by **feature**, not by layer. `src/store/messages/` owns the messages domain end-to-end rather than scattering "models" and "queries" across separate folders.

**Module size signals:**
- 300 lines → consider refactoring
- 500 lines → problem, extract submodules

**Current known outliers:** `src/store/inbox.rs`, `src/store/messages/posting.rs` — touch carefully.

---

## Data Flow Patterns

### Message Creation Flow

```
HTTP POST /messages
    │
    ▼
Handler (validation, auth)
    │
    ▼
Store::create_message (transaction)
    ├── Insert message row
    ├── Update sender's read cursor
    ├── Emit StreamEvent
    └── (optional) Append to events table
    │
    ▼
Broadcast to WebSocket subscribers
    │
    ▼
Agent wake (if recipient is agent)
```

### Unread Count Calculation

Unread counts are computed by **SQL VIEW**, not Rust code. The view excludes:
- Messages from the viewing member themselves
- System-authored messages (`sender_type = 'system'`)
- Thread replies (tracked separately via `thread_unread_count`)

**Important invariant:** When modifying unread logic, update ALL three locations:
1. `inbox_conversation_state_view` — channel-level unread subquery
2. `inbox_conversation_state_view` — thread-level unread subquery  
3. `get_thread_notification_state_by_channel_id_inner` — parallel Rust query for thread notifications

They are intentionally kept in sync but exist for different call paths.

### Task lifecycle (unified, 2026-04-25)

**Six-state forward-only enum.** `TaskStatus` is `proposed | dismissed | todo | in_progress | in_review | done`. Transitions: `proposed → todo` (acceptance) | `proposed → dismissed`; `todo → in_progress`; `in_progress → in_review`; `in_review → done`. No reverse edges. Reverts go through unclaim or a fresh task. Validation lives on the enum (`can_transition_to`) and in `update_task_status`; HTTP handlers downcast `InvalidTaskTransition` to 422.

**`owner` is a label, not a gate.** Renamed from `claimed_by`. Any channel member can advance any state. Claim sets `owner` only — it does NOT auto-advance to `in_progress` (decoupled). Unclaim clears `owner` only. The owner-only authorization gate that PR #93 had is gone; membership is the only check (and it lives in the HTTP handler layer).

**Task sub-channels.** Every task that lives past `proposed` owns a child channel (`ChannelType::Task`, `parent_channel_id` FK). Minted on the first `→ todo` transition (or at create-time for direct-created tasks). Creator + first acceptor are seeded as members; claim/unclaim syncs the claimer in/out. Transitioning to `Done` archives the sub-channel. Excluded from default channel listings — reach them via the parent task card or the task detail view. `proposed` and `dismissed` tasks have no sub-channel.

**Provenance: source-message snapshot.** When an agent proposes a task tied to a chat message, the four `snapshot_*` columns (`snapshot_sender_name`, `snapshot_sender_type`, `snapshot_content`, `snapshot_created_at`) capture the source verbatim, with `source_message_id` carrying the live FK. `ON DELETE SET NULL` on the FK + a CHECK constraint requiring all four `snapshot_*` columns either all-populated or all-null = the snapshot survives source deletion. The `source_message_id` is independently nullable; the `snapshot_*` quadruple is all-or-none. The constraint is the durable enforcement against partial-snapshot writes.

**Wire surfaces.** Two system message kinds in chat:

- `task_card` in the **parent channel**, posted ONCE per task at creation. The host message for the rendered card; subsequent state changes do not post more — the card re-renders via the cross-channel `task_update` SSE event. JSON payload: `{ kind: "task_card", task_id, task_number, title, status, owner, created_by, source_message_id?, snapshot_*? }` (camelCase on the wire via `#[serde(rename_all = "camelCase")]`).
- `task_event` in the **sub-channel**, posted on every post-acceptance action: `claimed`, `unclaimed`, `status_changed`. **Wire field name `claimedBy` is immutable** — chat history depends on it. Pre-acceptance transitions (`proposed → todo`, `proposed → dismissed`) do NOT post `task_event`; the parent `task_card` re-renders is the sole signal.

The kickoff message format is preserved verbatim from PR #96's contract: `"Task opened: {title}\n\nFrom @{sender}'s message in #{parent}:\n> {content}"` (with snapshot) or just `"Task opened: {title}"` (direct-create). Asserted by Playwright TSK-005.

**Cross-channel realtime fan-out.** `Store::task_updates_tx` is a dedicated `broadcast::Sender<TaskUpdateEvent>` (cap 256). Every mutation (`create_tasks`, `create_proposed_task`, `update_tasks_claim`, `update_task_unclaim`, `update_task_status`) calls `emit_task_update(task, channel_id)` after `tx.commit()` + `drop(conn)`. The realtime WebSocket forwards every event to every connected client (NO membership gate) so the parent-channel `task_card` host re-renders even when the viewer is not a member of the task's sub-channel. `TaskUpdateEvent` payload: `{ taskId, channelId, taskNumber, status, owner, subChannelId, updatedAt }`.

**Atomicity.** The task-row write, sub-channel mint (when applicable), kickoff/event posts, and channel-membership inserts share the same `IMMEDIATE` transaction. A failure on any rolls all back. Stream fan-out happens after the lock is released.

### Sender Type Resolution (Security Boundary)

**Never allow clients to forge `sender_type='system'` messages.**

The canonical path:
```
sender_type_for_actor() 
    → store.lookup_sender_type() 
    → fallback to SenderType::Human
```

- `lookup_sender_type` only queries `agents` and `humans` tables
- `System` is never returned from this lookup
- `create_system_message` is internal-only. Current callers: channel-kickoff in `templates.rs` and the task mutation hooks in `src/store/tasks/mod.rs` (create / claim / unclaim / status change). Clients cannot forge `sender_type = 'system'` — the HTTP surface has no endpoint that accepts it.

---

## Type System Conventions

### Enum-First Design

**Make invalid states unrepresentable.** Reach for enums before booleans or strings.

```rust
// Good
pub enum SenderType { Human, Agent, System }

// Bad
pub struct Sender { is_human: bool, is_agent: bool, is_system: bool }
```

**Enum additions are additive and safe.** No consumer uses `match` on `SenderType`; they use `.as_str()`, `==`, or `from_sender_type_str`. Adding a variant never breaks exhaustiveness.

### String vs Enum

Avoid `String` for closed sets. If a field has a finite set of values (sender kind, channel type, task status, runtime name), promote it to an enum.

The one exception: SQL column types where SQLite stores text and we round-trip through `from_*_str`.

---

## Error Handling

### Application Errors

Use `anyhow::Result` with `anyhow!(...)` and `.context("...")` for adding detail as errors bubble up.

```rust
// Good
let channel = Self::get_channel_by_name_inner(conn, name)?
    .ok_or_else(|| anyhow!("channel not found: {name}"))?;
```

### HTTP Handler Errors

Use the `app_err!()` macro from `src/server/error.rs`. It accepts either an HTTP `StatusCode` or an `AppErrorCode` variant as the first argument, and supports inline format strings:

```rust
// HTTP status code — generic errors
app_err!(StatusCode::NOT_FOUND, "channel not found: {name}")
app_err!(StatusCode::INTERNAL_SERVER_ERROR, "store error: {e}")

// AppErrorCode — machine-readable errors the frontend can act on
app_err!(AppErrorCode::AgentNameTaken, "agent name already exists: {name}")
app_err!(AppErrorCode::MessageNotAMember, "sender is not a channel member")
```

`AppErrorCode` variants produce a `code` field in the `ErrorResponse` JSON (e.g. `"code": "AGENT_NAME_TAKEN"`). The full list of variants is in `src/server/error.rs`. The matching frontend type is `ApiErrorCode` in `ui/src/lib/apiError.ts`.

**Do not use `api_err`, `internal_err`, `conflict_err`, or `coded_err`** — they were consolidated into `app_err!()`.

### Library Errors

Use typed errors using `thiserror` when adding a new module that deserves a real error type. This is rare in the current codebase.

### Panic Policy

**Never `unwrap()` or `expect()` in library code.** OK in:
- Tests
- CLI one-shots
- Genuinely impossible cases (document the invariant in a comment)

---

## Logging

Use the `tracing` crate. **Never `eprintln!` / `println!` in library code.**

Standard dev invocation:
```bash
RUST_LOG=chorus=info ./target/debug/chorus serve --port 3001
```

Prefer structured fields over string interpolation:
```rust
tracing::info!(agent = %name, channel = %channel_name, "joined channel");
```

---

## Testing Philosophy

### Test Categories

| Test File | Purpose | When to Add Tests |
|-----------|---------|-------------------|
| `tests/store_tests.rs` | Store + SQL integration | Any store behavior change |
| `tests/e2e_tests.rs` | HTTP + WebSocket end-to-end | Router or event stream changes |
| `tests/realtime_tests.rs` | WebSocket / StreamEvent | Realtime transport changes |
| `tests/server_tests.rs` | HTTP handlers (test router) | Handler logic changes |
| `tests/driver_tests.rs` | Agent runtime drivers | New driver or driver fix |
| `tests/check_impl.rs` | Type-level / trait checks | Generic constraints |
| `tests/bridge_serve_tests.rs` | Shared bridge HTTP + per-agent isolation | Bridge serve or `Backend` trait changes |
| `#[cfg(test)]` modules | Pure function unit tests | Module-local utilities |

### Test Conventions

- **Naming:** `test_<subject>_<behavior>`
  - Example: `test_channel_unread_count_excludes_system_messages`
- **AAA pattern:** Arrange, Act, Assert with blank lines between phases
- **One assertion per test** where possible
- **Real SQLite in tempdirs, not mocks.** Use `make_store()` helper

### Verification Policy

- Focused module tests on every backend change
- `cargo test --test e2e_tests` when message/task/DM/thread/agent flow is affected
- `/gstack-qa` for user-facing changes (authoritative browser QA)

---

## Code Index

### Key Files by Concern

| Concern | Entry Point | Notes |
|---------|-------------|-------|
| **Message creation** | `src/store/messages/posting.rs` | Transactional insert + event emission |
| **Message history** | `src/store/messages/history.rs` | Pagination, thread queries |
| **Unread state** | `src/store/inbox.rs` | Read cursors, notification state |
| **Schema** | `src/store/schema.sql` | Tables + views, zero-migration for views |
| **Migrations** | `src/store/migrations.rs` | Additive table changes |
| **HTTP handlers** | `src/server/handlers/*.rs` | One file per domain |
| **Router** | `src/server/mod.rs` | Route assembly, middleware |
| **Agent drivers** | `src/agent/drivers/` | ACP trait + raw implementations |
| **Driver selection** | `src/agent/drivers/mod.rs` | `driver_for_runtime()` |
| **Agent lifecycle** | `src/agent/manager.rs` | Spawn, session, event dispatch |
| **MCP bridge (shared)** | `src/bridge/serve.rs` | `chorus bridge-serve` HTTP daemon |
| **MCP bridge (backend)** | `src/bridge/backend.rs` | `Backend` trait + `ChorusBackend` impl |
| **Bridge discovery** | `src/bridge/discovery.rs` | `read_bridge_info()` — PID-validated `~/.chorus/bridge.json` |

### Type Definitions

| Type | Location |
|------|----------|
| `SenderType` | `src/store/messages/types.rs` |
| `Message` / `HistoryMessage` | `src/store/messages/types.rs` |
| `StreamEvent` | `src/store/stream.rs` |
| Channel types | `src/store/channels/types.rs` |
| Agent config | `src/store/agents.rs` |

### Critical Views (Schema)

| View | Purpose |
|------|---------|
| `inbox_conversation_state_view` | Per-member unread counts, read cursors |
| `conversation_messages_view` | History with joined channel metadata |
| `thread_summaries_view` | Thread metadata (replies, participants) |

---

## Anti-Patterns (What Not to Do)

- Do not `unwrap()` in library code
- Do not use `eprintln!` / `println!` for logging
- Do not reinvent a read model in Rust when a SQL view already owns it
- Do not add a new file for a single-use helper. Put it next to its caller.
- Do not add backwards-compatibility shims for code that has no users outside the repo
- Do not add `// removed` comments, renamed `_unused` variables, or deprecated aliases. Delete completely.

---

## Adding a New Runtime Driver

See `docs/DRIVERS.md` for the full guide. Key principle: capture a wire trace from the real runtime before writing any Rust code.

---

## See Also

- `docs/DEV.md` — How to run, test, and iterate locally
- `docs/DESIGN.md` — Frontend design system
- `docs/ACP.md` — ACP driver SOP and debugging
- `docs/DRIVERS.md` — Adding new agent runtimes
- `docs/INBOX.md` — Unread and read cursor mechanics
- `docs/BRIDGE_MIGRATION.md` — Shared MCP bridge architecture, `bridge-serve`, driver conversion guide
