# ACP Driver SOP

Standard operating procedures for diagnosing ACP driver failures and integrating new ACP-based runtimes in Chorus.

---

## Part 1 — Investigating ACP Driver Failures

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

### Failure Diagnosis Checklist

```
[ ] Identified which phase failed (init / session / prompt / permission)
[ ] Captured raw wire JSON (not just Chorus logs)
[ ] Validated our JSON-RPC messages against the ACP Pydantic schema
[ ] Checked round-trip timing — <200ms response = silent exception in agent
[ ] Verified CLI flags are not silently dropped by subcommand structure
[ ] Confirmed no concurrent session/prompt calls were sent
[ ] Checked agent's own debug log for the actual exception
```

---

## Part 2 — Adding a New ACP Driver

### Pre-work: Read the agent's ACP source

Before writing any Rust, read the agent's ACP implementation. The three files that matter:

| File pattern | What to find |
|---|---|
| `acp/server.py` or equivalent | `session/new` params — what fields are required? |
| `acp/session.py` or equivalent | Permission flow — does it use `session/request_permission`? What options? |
| `soul/approval.py` or equivalent | How is `approve_for_session` stored? Does it persist across turns? |

If source is not available, capture a reference trace from the official client (e.g., Zed) using a proxy.

---

### Step 1: Implement `AcpRuntime` (~50–80 lines)

Create `src/agent/drivers/<runtime>.rs`:

```rust
impl AcpRuntime for MyRuntime {
    fn runtime(&self) -> AgentRuntime { AgentRuntime::MyRuntime }
    fn binary_name(&self) -> &str { "myruntime" }

    fn spawn_args(&self, ctx: &SpawnContext) -> Vec<String> {
        // IMPORTANT: read the agent's CLI source first.
        // Flags before a subcommand may be silently dropped.
        vec!["acp".to_string()]
    }

    fn session_new_params(&self, ctx: &SpawnContext) -> serde_json::Value {
        // Match the agent's expected session/new schema exactly.
        // Validate against the agent source or a reference trace.
        json!({
            "cwd": ctx.working_directory,
            "mcpServers": [{ "name": "chat", "command": ctx.mcp_command, "args": ctx.mcp_args }]
        })
    }

    fn requires_session_id_in_prompt(&self) -> bool {
        // Set true if agent expects sessionId in session/prompt params.
        // Check the agent's session/prompt handler.
        true
    }
}
```

---

### Step 2: Verify `session/new` params against the agent schema

The agent must receive exactly the fields it expects. Common mismatches:

| Runtime | Gotcha |
|---|---|
| kimi | Requires `mcpServers` (array), not `mcp_servers`. Env must be `[]` not omitted. |
| opencode | May require different MCP transport type field names |
| Generic ACP | `cwd` must be an absolute path per spec |

**Verify by looking at the agent's `session/new` handler**, not by guessing from the ACP spec. Agents frequently add required fields beyond the spec minimum.

---

### Step 3: Confirm `session/request_permission` behavior

Determine if the agent uses permissions and what options it sends:

1. Run the agent manually against a test MCP server
2. Capture the `session/request_permission` params
3. Check the `options` array — find the `"kind": "allow_always"` entry
4. Confirm the `optionId` value for that option (usually `"approve_for_session"`)

The ACP spec response format is fixed:
```json
{
  "jsonrpc": "2.0",
  "id": <request_id>,
  "result": {
    "outcome": {
      "outcome": "selected",
      "optionId": "<option_id_from_request>"
    }
  }
}
```

This is handled by `acp.rs::handle_rpc_request()` — you typically do not need to change it. But verify the agent actually sends `options` with the expected `kind` values.

---

### Step 4: Run DM-002 and watch the logs

```bash
# Build
cargo build

# Start a test server
nohup ./target/debug/chorus serve --port 3102 --data-dir /tmp/chorus-test-3102 \
  > /tmp/chorus-3102.log 2>&1 &

# Run the e2e test
cd qa/cases/playwright
CHORUS_BASE_URL=http://127.0.0.1:3102 \
CHORUS_RUNTIME=<runtime> \
CHORUS_MODEL=<model> \
npx playwright test DM-002.spec.ts --reporter=list --timeout=180000
```

Expected: test passes in under 60s. The agent should:
1. Receive the DM message
2. Call `check_messages` + `list_server` (possibly with permission approval)
3. Call `send_message` with a response containing the test token
4. End the turn

If the test times out, tail the log and check which phase stalled:

```bash
tail -f /tmp/chorus-3102.log | grep -E "→|←|TurnEnd|session/|permission"
```

---

### Step 5: Verify permission persistence (if applicable)

If the runtime uses `session/request_permission`, send a second message after the first completes. The second message should NOT trigger permission requests for the same tools.

If it does, `approve_for_session` is not being stored — re-check the response format.

---

### Step 6: Commit

```
feat(<runtime>): add ACP driver for <runtime>

- spawn_args: <what flags and why>
- session_new_params: <what fields the agent requires>
- requires_session_id_in_prompt: <true/false and why>

Verified: DM-002 passes with <runtime>/<model> in <N>s

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>
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
