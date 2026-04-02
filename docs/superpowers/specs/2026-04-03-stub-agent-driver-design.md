# Stub Agent Driver for QA Acceleration

## Problem

QA runs take a long time because 30+ cases that don't need real LLM reasoning still wait on real LLM latency, cost API credits, and flake on network issues. `CHORUS_E2E_LLM=0` skips these cases entirely — losing coverage of the full message pipeline, lifecycle, and UI rendering.

## Solution

A lightweight stub agent binary that implements the MCP bridge protocol with deterministic echo-based responses. It plugs into the existing driver architecture as `AgentRuntime::Stub`, registered server-side but hidden from the production UI. QA tests select it via `CHORUS_E2E_LLM=stub`.

## Decisions

| Question | Decision | Rationale |
|----------|----------|-----------|
| UI visibility | API-only (option C) | Playwright tests already create agents via `createAgentApi()`. No UI changes needed, no production confusion. |
| Response behavior | Token extraction + echo (option B) | Covers both "echo specific token" cases and "just need any reply" cases without external config. |
| Implementation | Separate Rust crate (option B) | `crates/stub-agent/` — clean boundary, doesn't add deps to the main binary. |
| Message delivery | Full MCP bridge | `send_message` is the only path that stores messages in the DB. No shortcut possible. |
| Lifecycle | Full loop (option A) | `wait_for_message` -> `send_message` loop keeps the agent alive. Needed for lifecycle, wake, and multi-message cases. |
| Test selection | Extend `CHORUS_E2E_LLM` to three values (option B) | Single knob: `0` (skip), `1` (real LLM), `stub` (stub driver). |

## Architecture

### Components

#### 1. `crates/stub-agent/` — Standalone Rust binary

This introduces a Cargo workspace for the first time. The existing `Cargo.toml` at the repo root must be converted to a workspace with two members: the main `chorus` crate (`.`) and the stub crate (`crates/stub-agent`). `cargo build` and `cargo test` from the repo root must build and test both crates.

A small MCP client process that:

- Reads the MCP tool list from the bridge on startup
- Runs a `wait_for_message` -> `send_message` loop
- Extracts echo tokens from incoming messages:
  - Pattern: `reply with "X"`, `token: X`, or quoted strings after keywords like `echo`, `say`
  - Fallback: `stub-reply-{seq}` when no token is found
- Emits JSON lines on stdout matching the format the manager expects (the specific format depends on what `StubDriver::parse_line` is written to handle — the stub binary and driver are co-designed)
- Configurable response delay via `STUB_DELAY_MS` env var (default 200ms)
- Exits cleanly on stdin close or SIGTERM

The binary speaks MCP JSON-RPC over stdio — same transport as real drivers talking to the bridge.

#### 2. `src/agent/drivers/stub.rs` — Driver trait implementation

```
StubDriver implements Driver:
  runtime()                     -> AgentRuntime::Stub
  supports_stdin_notification() -> true
  mcp_tool_prefix()             -> matches the stub binary's tool call prefix
  spawn()                       -> launches chorus-stub-agent with bridge config
  parse_line()                  -> parses the stub's stdout JSON format
  encode_stdin_message()        -> stdin notification for wake
  build_system_prompt()         -> minimal prompt (agent name + role)
  detect_runtime_status()       -> always Available (no external dependency)
  list_models()                 -> vec!["echo"]
```

#### 3. `AgentRuntime::Stub` enum variant

Added to `src/store/agents.rs`. Wired into:

- `AgentRuntime::parse()` and `AgentRuntime::as_str()`
- `get_driver()` in `src/agent/manager.rs`
- `resumable_session_id` match in `start_agent` (`manager.rs:L88-97`) — the stub has no sessions to resume, so add `AgentRuntime::Stub => None`
- `all_runtime_drivers()` in `src/agent/drivers/mod.rs` — **include** `StubDriver` here so `list_models("stub")` works
- `handle_list_runtime_statuses` in `src/server/handlers/mod.rs` — **filter out** `stub` from the response so the UI never shows it. The `/runtimes` endpoint currently returns everything from `all_runtime_drivers()`. Add a `.filter(|s| s.runtime != "stub")` before returning.

**Server-side access control:** The `POST /api/agents` handler accepts any runtime string that passes `AgentRuntime::parse()`. Adding `Stub` to the enum means any API client can create stub agents — not just Playwright. This is acceptable: the stub runtime is harmless (it's a local echo process), and gating it behind an env var adds complexity for no security benefit. If this changes, add `CHORUS_STUB_ENABLED=1` gating in the create handler.

Playwright tests create stub agents via `createAgentApi({ runtime: 'stub', model: 'echo' })`.

#### 4. Frontend: no changes

The stub runtime is API-only. No UI presence.

### MCP Bridge Interaction

The stub agent follows the same protocol as real drivers:

```
Bridge spawns alongside stub process
  |
  v
Stub reads MCP tool list (JSON-RPC initialize)
  |
  v
Loop:
  call wait_for_message() -> receive incoming message
  extract token from message content
  sleep STUB_DELAY_MS
  call send_message(target, content) -> deliver reply
  emit JSON status line on stdout
```

### Protocol Coupling

The stub binary and MCP bridge share the repo. When the bridge protocol changes:

- The stub binary must be updated in the same PR
- A basic integration test (`cargo test`) should verify the stub can complete one send/receive cycle against the bridge

## QA Integration

### `CHORUS_E2E_LLM` Three-Way Switch

| Value | Behavior |
|-------|----------|
| `1` (default) | Real LLM drivers. Full test suite. |
| `0` | Skip all agent-reply cases (existing behavior). |
| `stub` | Create agents with `runtime: stub`. Run all stub-eligible cases. Skip real-LLM-only cases. |

### Playwright Helpers

New helper: `ensureStubTrio(request)` — creates `stub-a`, `stub-b`, `stub-c` with `runtime: 'stub', model: 'echo'`.

Stub agents use distinct names (`stub-a/b/c`) rather than `bot-a/b/c` to avoid collisions with `ensureMixedRuntimeTrio`. This means specs that reference `bot-a` by name must parameterize the agent name based on the mode:

```ts
const agentA = useStub ? 'stub-a' : 'bot-a'
```

Spec-level wiring:

```ts
const mode = process.env.CHORUS_E2E_LLM ?? '1'
const skipLLM = mode === '0'           // existing — skip entirely
const useStub = mode === 'stub'        // new — use stub agents
const skipRealLLM = skipLLM || useStub // for cases needing real reasoning
```

Cases needing real LLM reasoning use `test.skip(skipRealLLM, ...)`.

### `stub-trio` QA Preset

Added to `QA_PRESETS.md`:

```
Use for:
  - fast QA runs that test the full UI + message pipeline without LLM latency
  - CI smoke tests

Agents:
  stub-a — runtime stub, model echo
  stub-b — runtime stub, model echo
  stub-c — runtime stub, model echo
```

## Case Coverage Matrix

| Category | Count | Stub-eligible | Examples |
|----------|-------|--------------|----------|
| No agent reply needed | 6 | N/A (already pass) | ENV-001, NAV-001/002, MSG-005, TSK-001/002 |
| Any reply sufficient | 10 | Yes | MSG-001, MSG-003, MSG-006, MSG-010, CHN-001, HIS-001, REC-002, ATT-001, CHN-003, MSG-012 |
| Echo token required | 3 | Yes | MSG-002, MSG-004, MSG-008 |
| Lifecycle transitions | 7 | Yes | PRF-001, LFC-001/002, REC-001, ACT-001/002, MSG-009 |
| Creation/config only | 7 | Yes (no reply needed) | AGT-001/002/003/004, CHN-002/004/005 |
| Real LLM required | 5 | No | TMT-003/004/006/008/009 |
| Error/edge cases | 4 | Partial | MSG-007, MSG-011, ERR-001, WRK-001 |

Approximately 30 cases accelerated from minutes-per-case to sub-second.

## Out of Scope

The stub does NOT cover:

- **Team collaboration models** — leader delegation, swarm `READY:` protocol
- **Multi-team context isolation** — agent reasoning about its roles
- **Content-dependent cases** — any case where response content must demonstrate understanding
- **Workspace file creation** — stub doesn't write files (WRK-001 needs a real or mock workspace)

These stay on `CHORUS_E2E_LLM=1` runs.
