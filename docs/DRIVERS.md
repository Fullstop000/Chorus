# Driver Architecture

Runtime driver abstraction for Chorus. Each agent runtime (Claude, Codex, Gemini, Kimi, OpenCode) is backed by a pair of traits defined in `src/agent/drivers/mod.rs`.

---

## Core Traits

### `RuntimeDriver` — factory

One instance per runtime. Registered in `AgentManager::driver_registry`.

```rust
#[async_trait]
pub trait RuntimeDriver: Send + Sync + 'static {
    fn runtime(&self) -> AgentRuntime;
    async fn probe(&self) -> anyhow::Result<RuntimeProbe>;
    async fn login(&self) -> anyhow::Result<LoginOutcome>;
    async fn list_sessions(&self) -> anyhow::Result<Vec<StoredSessionMeta>>;
    async fn list_models(&self) -> anyhow::Result<Vec<ModelInfo>>;
    async fn list_commands(&self) -> anyhow::Result<Vec<SlashCommand>>;

    // Primary entry point — unified replacement for the old attach/new_session/resume_session trio.
    async fn open_session(
        &self,
        key: AgentKey,
        spec: AgentSpec,
        intent: SessionIntent,   // New | Resume(session_id)
    ) -> anyhow::Result<SessionAttachment>;
}
```

### `Session` — per-session handle

One instance per live session, returned inside `SessionAttachment`.

```rust
#[async_trait]
pub trait Session: Send {
    fn key(&self) -> &AgentKey;
    fn session_id(&self) -> Option<&str>;
    fn state(&self) -> AgentState;

    async fn run(&mut self, init_prompt: Option<PromptReq>) -> anyhow::Result<()>;
    async fn prompt(&mut self, req: PromptReq) -> anyhow::Result<RunId>;
    async fn cancel(&mut self, run: RunId) -> anyhow::Result<CancelOutcome>;
    async fn close(&mut self) -> anyhow::Result<()>;
}
```

### `SessionAttachment` — return value of `open_session`

```rust
pub struct SessionAttachment {
    pub session: Box<dyn Session>,
    pub events: EventStreamHandle,
}
```

---

## Contract

- `open_session` (the factory) may do wire I/O to set up process/session state, but **must not emit driver events**. It is safe to call before any subscriber has called `events.subscribe()`.
- `run` is the **sole emitter** of `DriverEvent`s. All `SessionAttached`, `Lifecycle`, `Output`, `Completed`, and `Failed` events flow from `run` and `prompt`.
- `open_session(SessionIntent::New)` starts a fresh session; `open_session(SessionIntent::Resume(id))` resumes a stored session.
- After `open_session` returns, the session is in `AgentState::Idle`. Call `run(init_prompt)` to bring it online.

---

## Shared Types

| Type | Purpose |
|------|---------|
| `AgentKey` | Agent's persisted UUID as a string. HashMap key + event routing. |
| `SessionId` | Runtime-assigned session identifier. Never synthesized by Chorus. |
| `RunId` | Per-prompt-in-flight identifier. Correlates `prompt()` call to `Completed`/`Failed` event. |
| `SessionIntent` | `New` or `Resume(SessionId)`. Passed to `open_session`. |
| `CapabilitySet` | Bitflags: `LOGIN`, `SESSION_LIST`, `RESUME_SESSION`, `CANCEL`, `SLASH_COMMANDS`, `MODEL_LIST`. |
| `AgentSpec` | Config handed to `open_session`: model, system prompt, env vars, working dir, bridge endpoint. |

---

## Where Driver Support Lives

New driver work usually touches these files:

- `src/agent/drivers/<runtime>.rs`
  - implements `RuntimeDriver` and the runtime-specific `Session` handle
- `src/agent/drivers/mod.rs`
  - module registration, shared types, `EventFanOut`, `AgentRegistry`
- `src/agent/manager.rs`
  - driver selection, session lifecycle, event forwarder wiring
- `src/store/agents.rs`
  - persisted runtime/model support when needed
- `src/server/handlers/dto.rs`
  - API runtime enum validation when needed
- `ui/src/components/AgentConfigForm.tsx`
  - runtime exposed in the create/edit UI
- `tests/live_runtime_tests.rs`
  - per-runtime `open_session` + `run` + `prompt` integration tests
- `tests/live_multi_session_tests.rs`
  - multi-session concurrency tests

See `docs/DRIVER_GUIDE.md` for the step-by-step guide to adding a new driver.

---

## Shared Bridge

Every driver gets an `AgentSpec` when it is spawned. `AgentSpec.bridge_endpoint` is a
required `String` pointing at the shared HTTP bridge (for example
`"http://127.0.0.1:4321"`); populated by `AgentManager::start_agent` from
`~/.chorus/bridge.json`. If the bridge is not running the manager fails loudly —
there is no stdio fallback.

In `open_session` or `run`, request a per-agent pairing token:

```rust
let token = super::request_pairing_token(&spec.bridge_endpoint, &key).await?;
```

Then point the runtime's MCP config at `{bridge_endpoint}/token/{token}/mcp` using the
runtime-specific config shape (see `docs/BRIDGE_MIGRATION.md` for a per-runtime table).

---

## Shared Scaffolding

### `EventFanOut` + `EventStreamHandle`

Fan-out dispatcher: transport tasks write `DriverEvent`s into a single inbound `mpsc::Sender`; each observer gets its own bounded 256-deep receiver. A full queue drops events with a `chorus_driver_events_dropped` metrics bump — never back-pressures the emitter.

### `AgentRegistry<P>`

Process-global per-driver agent registry (`static`). Replaces the near-identical `OnceLock<Mutex<HashMap>>` copies that each driver used to carry. Evicts stale entries on re-attach. Drivers use `get_or_init` (build-if-missing) or `get_or_evict_stale` + `insert` (kimi pattern).

### `emit_driver_event`

Helper that wraps `try_send` with uniform warn-on-drop logging. All drivers call this instead of raw `try_send` so back-pressure behavior is consistent across runtimes.

---

## Driver Implementations

| Driver | File | Transport |
|--------|------|-----------|
| Claude | `src/agent/drivers/claude.rs` | `StreamJson` — bespoke streaming JSON |
| Codex | `src/agent/drivers/codex.rs` | `CodexAppServer` — JSONL over stdio |
| Gemini | `src/agent/drivers/gemini.rs` | `AcpNative` — `gemini --acp` subprocess |
| Kimi | `src/agent/drivers/kimi.rs` | `AcpNative` — `kimi acp` subprocess |
| OpenCode | `src/agent/drivers/opencode.rs` | `HttpAppServer` — OpenCode daemon |
| Fake | `src/agent/drivers/fake.rs` | In-memory test double |
