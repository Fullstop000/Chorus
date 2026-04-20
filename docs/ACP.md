# ACP Driver SOP

Standard operating procedures for adding new ACP runtimes to Chorus and diagnosing failures.

> **Why this doc exists**: The kimi integration took multiple sessions. The mistakes were not hard to avoid — they came from skipping verification before writing code. This doc makes those steps mandatory gates, not optional reading.

---

## Part 1 — Adding a New ACP Driver

The rule: **understand before implementing**. Every field name, every response format, and every CLI flag must be verified against the agent's source or a captured wire trace before any Rust is written. Guessing costs sessions. Verifying costs minutes.

---

### Gate 1 (pre-code): Capture a reference wire trace

Run the agent against its own official client (e.g., Zed) and capture the raw stdio exchange. This is the ground truth for every message format.

```bash
# Wrap the agent binary with a stdio tee
#!/bin/bash
exec <agent-binary> "$@" \
  > >(tee /tmp/acp-agent-out.log) \
  < <(tee /tmp/acp-host-in.log)
```

Or enable the agent's own wire logging:

```bash
KIMI_LOG_LEVEL=debug kimi acp ...
OPENCODE_LOG=debug opencode acp ...
```

**What to record:**
- `initialize` request/response pair
- `session/new` request/response pair
- At least one `session/prompt` exchange with tool calls
- Any `session/request_permission` request and the client's response

If you cannot get a wire trace, read the agent source directly (Gate 2). **Do not proceed without one or the other.**

---

### Gate 2 (pre-code): Read the agent's ACP source for every field you'll produce

Field names in ACP are camelCase in JSON (Pydantic `alias`): `optionId` not `option_id`, `sessionId` not `session_id`, `stopReason` not `stop_reason`. **Never guess — read the model.**

The three files that determine your implementation:

| Agent file | What to extract |
|---|---|
| `acp/server.py` or `session/new` handler | Required fields beyond `cwd` + `mcpServers` |
| `cli/__init__.py` or main entry | Whether global flags before a subcommand are silently dropped |
| `soul/approval.py` or permission handler | Whether `approve_for_session` persists; what action key is used |

**CLI flag trap — this cost us `--yolo` with kimi:**

```python
# kimi cli/__init__.py
@app.command()
def kimi(ctx: Context, yolo: bool = False, ...):
    if ctx.invoked_subcommand is not None:
        return   # ALL flags discarded when subcommand is invoked
```

If the runtime uses subcommands (`kimi acp`, `opencode acp`), any flag before the subcommand is dropped. The only reliable way to configure ACP-mode behavior is via `session/new` params or the agent's config file.

**Before writing `spawn_args()`**, answer: does each flag I plan to add actually reach the ACP code path? Prove it from the source.

---

### Gate 3 (pre-code): Validate your message formats against the schema

The ACP Python SDK ships canonical Pydantic models. Use them to validate what you plan to send before writing it in Rust.

```bash
# Find the schema from an installed ACP agent
find ~/.local/share/uv/tools -name "schema.py" -path "*/acp/*" 2>/dev/null

# Validate session/new params
python3 -c "
from acp.schema import NewSessionRequest
payload = {'cwd': '/tmp', 'mcpServers': [{'name': 'chat', 'command': '/bin/echo', 'args': [], 'env': []}]}
print(NewSessionRequest.model_validate(payload))
"

# Validate permission response
python3 -c "
from acp.schema import RequestPermissionResponse
payload = {'outcome': {'outcome': 'selected', 'optionId': 'approve_for_session'}}
print(RequestPermissionResponse.model_validate(payload))
"

# See exact serialized field names for any model
python3 -c "
from acp.schema import PermissionOption
print(PermissionOption(option_id='approve_for_session', name='Approve', kind='allow_always').model_dump(by_alias=True))
"
```

**Known correct formats** (verified against `agent-client-protocol==0.8.0`):

| Message | Our role | Correct `result`/`params` format |
|---|---|---|
| `session/request_permission` response | Client → Agent | `{"outcome": {"outcome": "selected", "optionId": "<id>"}}` |
| `session/request_permission` rejection | Client → Agent | `{"outcome": {"outcome": "cancelled"}}` |
| `session/new` params | Client → Agent | `{"cwd": "<abs_path>", "mcpServers": [{...}]}` |
| `session/prompt` params | Client → Agent | `{"sessionId": "<id>", "prompt": [{"type": "text", "text": "..."}]}` |

Gates 1–3 are complete when you can answer without guessing:
- What exact JSON does `session/new` need?
- Do any spawn flags I want actually work in ACP mode?
- What does a valid permission response look like for this runtime?

---

### Step 1: Implement `RuntimeDriver` + `Session`

Create `src/agent/drivers/<runtime>.rs`. Every decision here should come from Gates 1–3.

The new driver API uses two traits (see `docs/DRIVERS.md`):

- `RuntimeDriver::open_session(key, spec, intent)` — allocates the session handle, requests a bridge pairing token, optionally sends the ACP `session/new` wire message. Must not emit `DriverEvent`s.
- `Session::run(init_prompt)` — spawns the child process, sends the ACP `initialize` + `session/new` sequence, emits `SessionAttached` and `Lifecycle::Active`. All `DriverEvent`s flow from `run` and `prompt`.

```rust
// Sketch — adapt field names from Gates 1–3 for your runtime.
impl RuntimeDriver for MyDriver {
    async fn open_session(
        &self,
        key: AgentKey,
        spec: AgentSpec,
        intent: SessionIntent,
    ) -> anyhow::Result<SessionAttachment> {
        let (events, event_tx) = EventFanOut::new();
        let handle = MyHandle { key, spec, intent, event_tx };
        Ok(SessionAttachment { session: Box::new(handle), events })
    }
    // ...
}

impl Session for MyHandle {
    async fn run(&mut self, init_prompt: Option<PromptReq>) -> anyhow::Result<()> {
        // spawn binary, send initialize + session/new with field names from Gates 1–3
        // emit Lifecycle::Active
        // if init_prompt is Some, issue it as first prompt
        Ok(())
    }
    // ...
}
```

**Spawn args:** only include flags verified to work in ACP subcommand mode (Gate 2).

**`session/new` params:** use field names from agent source or reference trace (Gates 1/2), validated against schema (Gate 3):

```json
{
  "cwd": "<working_directory>",
  "mcpServers": [{ "name": "chat", "command": "<mcp_command>", "args": [], "env": [] }]
}
```

---

### Step 2: Run DM-002

```bash
cargo build

nohup ./target/debug/chorus serve --port 3102 --data-dir /tmp/chorus-test-3102 \
  > /tmp/chorus-3102.log 2>&1 &

cd qa/cases/playwright
CHORUS_BASE_URL=http://127.0.0.1:3102 \
CHORUS_RUNTIME=<runtime> \
CHORUS_MODEL=<model> \
npx playwright test DM-002.spec.ts --reporter=list --timeout=180000
```

Expected: passes in under 60s. If it times out, go to Part 2.

---

### Step 3: Verify permission persistence

If the runtime uses `session/request_permission`: send a **second** message after the first succeeds. It must **not** trigger permission requests for the same tools.

If it does, `approve_for_session` was never stored — your approval response was silently rejected. Re-check format against Gate 3.

---

### Step 4: Commit

```
feat(<runtime>): add ACP driver for <runtime>

- spawn_args: <what flags and why>
- session_new_params: <what fields the agent requires>
- requires_session_id_in_prompt: <true/false and why>

Verified: DM-002 passes with <runtime>/<model> in <N>s

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>
```

---

## Part 2 — Diagnosing ACP Driver Failures

### Step 1: Identify which phase failed

ACP has four distinct phases. Failures in each phase look different.

| Phase | Method | Failure Symptom |
|-------|--------|-----------------|
| Initialization | `initialize` | Agent process starts but Chorus never sends prompts; session_id is empty |
| Session creation | `session/new` | Agent starts, `initialize` succeeds, but DM/chat never activates |
| Prompt turn | `session/prompt` | Agent receives message but never replies; activity log shows "working" forever |
| Permission | `session/request_permission` | Agent loops: same tool requested repeatedly; turn ends and restarts |

**How to identify the phase:**

```bash
RUST_LOG=chorus=debug ./target/debug/chorus serve --port 3102 --data-dir /tmp/dbg
# Tail: grep for the phase keywords
tail -f /tmp/chorus-3102.log | grep -E "initialize|session/new|session/prompt|request_permission|TurnEnd|PermissionRequested"
```

---

### Step 2: Read the raw wire before reading Chorus code

Before looking at `acp.rs`, capture what the agent actually sends and what we actually send back.

Add a temporary raw-log shim — wrap the agent's stdin/stdout with a log file:

```bash
# Replace the agent binary with a wrapper that tees stdio
#!/bin/bash
tee /tmp/acp-agent-stdout.log | kimi acp 2>/tmp/acp-agent-stderr.log | tee /tmp/acp-host-stdin.log
```

Or in `acp.rs`, temporarily add `tracing::debug!("→ agent stdin: {}", data)` on `WriteStdin` and `← agent stdout: {}" on each parsed line.

This gives you the exact JSON on the wire. **Read the wire before guessing.**

---

### Step 3: Validate each JSON-RPC message against the ACP schema

The ACP Python SDK (`agent-client-protocol`) ships the canonical Pydantic models. Use them as ground truth.

```bash
# Find the installed schema from a kimi installation
find ~/.local/share/uv/tools -name "schema.py" -path "*/acp/*" 2>/dev/null

# Validate a message interactively
python3 -c "
import json, sys
from acp.schema import RequestPermissionResponse
payload = json.loads(sys.stdin.read())
print(RequestPermissionResponse.model_validate(payload))
" <<< '{"outcome": {"outcome": "selected", "optionId": "approve_for_session"}}'
```

Key schema facts (verified against `agent-client-protocol==0.8.0`):

| Message | Field | Correct format |
|---------|-------|----------------|
| `session/request_permission` response | `result` | `{"outcome": {"outcome": "selected", "optionId": "<id>"}}` |
| `session/request_permission` response (denied) | `result` | `{"outcome": {"outcome": "cancelled"}}` |
| `session/new` params | `mcpServers` | Array of `{name, command, args, env}` objects |
| `session/prompt` params | `prompt` | Array of `ContentBlock` objects (`{type: "text", text: "..."}`) |

**Never trust field name guesses.** `optionId` not `option_id`, `sessionId` not `session_id`, `stopReason` not `stop_reason`. Use `model_dump(by_alias=True)` to see what the SDK actually serializes.

---

### Step 4: Check for silent failure paths in the agent runtime

Most ACP agents have a broad `except Exception` in their permission/event handlers that silently swallows errors. The symptom is: the agent keeps running but the operation is rejected without any log.

**Known silent failure paths in kimi:**

```python
# session.py — _handle_approval_request
try:
    response = await self._conn.request_permission(...)
    request.resolve(response.outcome.option_id)
except Exception:
    request.resolve("reject")   # ← silent rejection, logs nothing to our side
```

If you see a permission response round-trip complete in ~150ms (far faster than human latency), the agent is hitting the exception path. The real error is a Pydantic validation failure on your response format.

**How to expose these errors:** Enable the agent's own debug logging:

```bash
# kimi
KIMI_LOG_LEVEL=debug kimi acp ...

# general pattern — look for the agent's log env var
<runtime> --help | grep -i log
```

---

### Step 5: Verify state persistence across turns

Some failures only manifest on the second turn. Common causes:

- **Permission state not persisted**: `approve_for_session` should suppress future requests for the same action. If the second turn still requests permission, your first approval was rejected.
- **Session ID drift**: If you send a retry `session/prompt` while the original is still running, you create two concurrent turns. The agent may handle them incorrectly.
- **Turn ID change**: kimi prefixes tool call IDs with a per-turn UUID (`{turn_id}/{tool_call_id}`). A new `session/prompt` means a new turn_id, making all prior tool call IDs invalid.

**Rule**: Permission approval is **inline** in ACP. The agent awaits the `session/request_permission` response within the same `session/prompt` call (up to the agent's timeout, typically 300s). Do **not** send a follow-up `session/prompt` after approving — the current turn continues automatically.

---

### Step 6: Check CLI flag behavior for ACP subcommands

Some runtimes (`kimi acp`, `opencode acp`) use a subcommand. CLI flags placed before the subcommand are often silently dropped.

**kimi example:**
```python
# cli/__init__.py
@app.command()
def kimi(ctx: Context, yolo: bool = False, ...):
    if ctx.invoked_subcommand is not None:
        return   # ← ALL flags discarded; --yolo does nothing
```

**Check:** Read the runtime's CLI source before adding global flags. The only reliable way to configure ACP-mode behavior is through `session/new` params or the agent's own config file.

---

### Step 7: Verify Chorus correctly parses the runtime's wire format

Wire looks correct but the activity log is still wrong? The bug may be in Chorus's own `parse_line`, not in the agent.

**Symptom:** Raw wire shows the agent calling the right tool (e.g. `chat_send_message`), but DM-002 Step 7 fails ("activity log shows send_message tool call").

**Root cause pattern:** `acp.rs` extracts the event `kind` from the update object. Different runtimes use different field names for the same concept, and a runtime may emit *multiple* fields where only one is the event discriminator.

```
opencode session/update:
  {
    "sessionUpdate": "tool_call",   ← event type (what we want)
    "kind":          "other",       ← tool category (opencode-internal, NOT event type)
    "title":         "chat_send_message"
  }
```

If Chorus checks `kind` before `sessionUpdate`, it reads `"other"` instead of `"tool_call"` and the event falls through unhandled.

**Check:** In `acp.rs`, find the `kind` extraction block and confirm field priority matches what the runtime actually uses as its event discriminator:

```bash
grep -n "get(\"kind\")\|get(\"type\")\|get(\"sessionUpdate\")" src/agent/drivers/acp.rs
```

The correct order is: `sessionUpdate` → `kind` → `type` (kimi and opencode use `sessionUpdate`; claude ACP uses `kind`).

**Check tool name stripping:** Even if `ParsedEvent::ToolCall` fires, the display name lookup may fail. `strip_mcp_prefix` normalizes `mcp__chat__send_message`, `mcp_chat_send_message`, `chat_send_message` → `send_message`. If the runtime uses a different prefix scheme, add it to `strip_mcp_prefix` in `acp.rs`.

---

### Failure Diagnosis Checklist

```
[ ] Identified which phase failed (init / session / prompt / permission)
[ ] Captured raw wire JSON (not just Chorus logs)
[ ] Validated our JSON-RPC messages against the ACP Pydantic schema
[ ] Checked round-trip timing — <200ms response = silent exception in agent
[ ] Verified CLI flags are not silently dropped by subcommand structure
[ ] Confirmed no concurrent session/prompt calls were sent
[ ] Checked agent's own debug log for the actual exception
[ ] Wire looks correct but activity log wrong → check acp.rs kind-field priority and strip_mcp_prefix
```

---

## Quick Reference

### ACP Protocol Flow (Chorus as Client)

```
Chorus                              Agent (stdio)
  │                                     │
  ├─── initialize ──────────────────────►│
  │◄── initialize result ───────────────┤
  │                                     │
  ├─── session/new ─────────────────────►│
  │◄── {sessionId} ─────────────────────┤
  │                                     │
  ├─── session/prompt ──────────────────►│  (agent awaits; turn is open)
  │◄── session/update (tool_call) ──────┤  notification
  │◄── session/request_permission ──────┤  request (agent blocks)
  ├─── permission result ───────────────►│  response (agent unblocks)
  │◄── session/update (tool_call_update)┤  notification
  │◄── session/update (agent_message)───┤  notification
  │◄── session/prompt result ───────────┤  {stopReason: "end_turn"}
```

### Permission Response (required format)

```json
{
  "jsonrpc": "2.0",
  "id": <same id as request>,
  "result": {
    "outcome": {
      "outcome": "selected",
      "optionId": "approve_for_session"
    }
  }
}
```

### Key Rust files

| File | Responsibility |
|---|---|
| `src/agent/drivers/acp.rs` | Shared ACP protocol: JSON-RPC parsing, permission handling, tool display |
| `src/agent/drivers/<runtime>.rs` | Runtime-specific: spawn args, `session/new` params, model list |
| `src/agent/drivers/mod.rs` | Driver selection: `driver_for_runtime()` |
| `src/agent/manager.rs` | Agent lifecycle: event dispatch, session tracking |
| `qa/cases/playwright/DM-002.spec.ts` | E2E verification test |
