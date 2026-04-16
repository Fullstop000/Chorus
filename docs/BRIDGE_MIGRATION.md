# Bridge Migration Guide

Chorus is migrating from per-agent stdio bridge processes to a single shared HTTP
bridge. This guide covers what changed, how to use the new bridge, how to convert a
driver, and what comes next.

For deep architectural context, see the design doc:
`~/.gstack/projects/Fullstop000-Chorus/bytedance-main-design-20260416-013721.md`

---

## 1. Overview

### What changed and why

**Before:** Each agent spawned its own `chorus bridge --agent-id <key> --server-url
<url>` stdio process. The bridge process held a hardcoded agent identity and handled
all MCP tool calls for that one agent. N agents = N bridge processes, each with its
own OS event loop and HTTP connection pool. Every driver duplicated the same
boilerplate to construct the MCP config.

**After:** One `chorus bridge-serve` daemon runs on localhost and serves all agents.
Each agent connects to a unique URL path (`/<agent_key>/mcp`) over Streamable HTTP.
The bridge routes tool calls to the correct agent based on the URL path. Session
management is handled per-agent inside the same process.

The long-term goal is for the bridge to become a **local agent control plane**: a
durable daemon that decouples agent identity (inbox, history, tasks, permissions) from
the transient runtime process (Claude session #47, Codex run abc123). Runtimes connect,
do their work, and can crash or be swapped without losing agent state. The same bridge
will eventually talk to Chorus local, Chorus cloud, Slack, Discord, or any other IM
backend — one bridge, all agents, any platform.

### Which architecture is active

Both paths are active simultaneously:

| Path | Status | How it works |
|---|---|---|
| Old stdio bridge | Default (unchanged) | Driver spawns `chorus bridge --agent-id <key>` per agent |
| New shared HTTP bridge | Opt-in per driver | Driver sets `bridge_endpoint`, bridge-serve must be running |

Migration is per-driver. When a driver is converted, all agents of that runtime use the
shared bridge. Other runtimes continue using the stdio bridge until their driver is
converted.

---

## 2. For Users (running Chorus)

### Starting the shared bridge

Run `chorus bridge-serve` alongside `chorus serve`:

```bash
# Terminal 1 — Chorus backend
chorus serve --port 3001

# Terminal 2 — Shared bridge (connects to the backend)
chorus bridge-serve --listen 127.0.0.1:4321 --server-url http://localhost:3001
```

The bridge writes discovery information to `~/.chorus/bridge.json` so drivers can
find it automatically:

```json
{
  "port": 4321,
  "pid": 12345,
  "started_at": "2026-04-16T01:37:21Z"
}
```

### Verifying the bridge works

```bash
# Quick health check
curl http://127.0.0.1:4321/health

# Full MCP handshake smoke test (no Chorus server required)
chorus bridge-smoke-test
```

`bridge-smoke-test` starts a temporary bridge, sends a real MCP `initialize` request,
verifies the session ID comes back, then shuts down. Passes = the MCP layer is working.

### When to use the shared bridge vs the old stdio bridge

**Shared bridge (`chorus bridge-serve`):**
- Running multiple agents and want fewer OS processes
- Driver for your runtime has been converted to `bridge_endpoint`
- Debugging the bridge itself

**Old stdio bridge (default, no action needed):**
- Single-agent setups
- Using a runtime whose driver has not yet been converted
- Backward-compatibility requirement — the stdio path is stable and unchanged

You do not need to change anything to keep using the old stdio bridge. It remains the
default until `bridge_endpoint` is explicitly set.

---

## 3. For Driver Authors (converting a driver)

### The change in one sentence

Replace the hardcoded stdio MCP config with a branch: if `bridge_endpoint` is set,
point at the shared HTTP bridge; otherwise fall back to the old stdio spawn.

### Before (current pattern — all four drivers look like this)

```rust
// claude.rs, kimi.rs, opencode.rs, codex.rs — same idea in each
let mcp_config = serde_json::json!({
    "mcpServers": {
        "chat": {
            "command": &self.spec.bridge_binary,
            "args": ["bridge", "--agent-id", &self.key, "--server-url", &self.spec.server_url]
        }
    }
});
```

Codex (app-server) does this via `-c` flags instead of a config file, but it is the
same conceptual pattern: stdio transport, per-agent binary, hardcoded agent identity
in the spawn args.

### After (shared bridge path)

```rust
let mcp_config = if let Some(endpoint) = &self.spec.bridge_endpoint {
    // Shared HTTP bridge — agent identity lives in the URL path, not the spawn args.
    serde_json::json!({
        "mcpServers": {
            "chat": {
                "url": format!("{}/{}/mcp", endpoint, self.key),
                "type": "http"   // exact key depends on the runtime's config format
            }
        }
    })
} else {
    // Legacy stdio bridge — unchanged from before.
    serde_json::json!({
        "mcpServers": {
            "chat": {
                "command": &self.spec.bridge_binary,
                "args": ["bridge", "--agent-id", &self.key, "--server-url", &self.spec.server_url]
            }
        }
    })
};
```

The `bridge_endpoint` field is already present on `AgentSpec` as
`pub bridge_endpoint: Option<String>`. It defaults to `None` in all constructors. No
schema or database changes are needed to add the field to a driver — it's already
there.

### What `bridge_endpoint` looks like at runtime

When `bridge-serve` is running on the default port:

```
bridge_endpoint = Some("http://127.0.0.1:4321")
```

For an agent with `key = "bot-1"`, the driver constructs:

```
http://127.0.0.1:4321/bot-1/mcp
```

The bridge lazily creates a `StreamableHttpService<ChatBridge>` for each new
`agent_key` it sees. If the key has never connected before, the session is created on
the first `initialize` request.

### Per-runtime notes

Not all runtimes support Streamable HTTP MCP natively. Check the MCP config format for
each runtime before converting its driver:

| Runtime | MCP config key | HTTP support | Notes |
|---|---|---|---|
| Claude Code | `"type": "http"` (in `mcpServers`) | Needs verification | Uses `stream-json` over stdio by default |
| Codex (app-server) | `-c mcp_servers.chat.type="http"` | Needs verification | Config passed via `-c` flags, not a file |
| Kimi | `"type"` field in `mcpServers` | Needs verification | — |
| OpenCode | `"type": "local"` or `"http"` | Needs verification | Config written to `opencode.json` |

If a runtime does not support HTTP MCP natively, a stdio-to-HTTP adapter is needed
(see Phase 2 in section 4). Do not convert that driver until the adapter exists or
native HTTP support is confirmed.

### Driver connection failures

The bridge can crash or be restarted. When that happens:
- All HTTP connections from runtimes are dropped
- The session registry (in-memory) is lost
- Runtimes that hold a live MCP session will see a connection error

Drivers should handle this with exponential backoff retry. The stdio bridge has the
same failure mode — the bridge process crashes, the runtime sees a broken pipe. The
error surface is identical; only the transport changes.

### Wiring `bridge_endpoint` from the manager

The manager constructs `AgentSpec` and currently always sets `bridge_endpoint: None`.
To enable the shared bridge for an agent, populate this field before passing the spec
to the driver:

```rust
// In AgentManager or wherever the spec is built:
let bridge_endpoint = chorus::bridge::discovery::read_bridge_info()
    .map(|info| format!("http://127.0.0.1:{}", info.port));

let spec = AgentSpec {
    // ... other fields ...
    bridge_binary: self.bridge_binary.clone(),
    server_url: self.server_url.clone(),
    bridge_endpoint,
};
```

`read_bridge_info()` returns `None` if the file does not exist or if the recorded PID
is not alive (stale file). In that case the driver falls back to stdio automatically.

---

## 4. For Architects (phased migration plan)

### Phase 1 — Shared HTTP transport (current)

**Status:** Shipped.

What is in place:
- `chorus bridge-serve --listen <addr> --server-url <url>` starts the daemon
- Routes: `/{agent_key}/mcp` (MCP Streamable HTTP), `/health` (plain text)
- Each `agent_key` gets its own `StreamableHttpService<ChatBridge>` created on first
  request — no pre-registration required
- `~/.chorus/bridge.json` written on startup (port, PID, timestamp); removed on clean
  shutdown
- `read_bridge_info()` in `src/bridge/discovery.rs` validates the PID is alive before
  returning the info, preventing stale-file confusion
- `chorus bridge-smoke-test` for "it works" confirmation
- Old stdio `chorus bridge --agent-id <key>` remains as fallback — unchanged

Identity model: `agent_key` is visible in the URL path. Any local process that can
read the MCP config file can connect to any agent's endpoint. This is acceptable for
Phase 1 (same attack surface as the old stdio bridge).

Security: localhost-only (`127.0.0.1`). No authentication. Single-user, single-machine
threat model.

### Phase 2 — Pairing tokens and opaque identity

What changes:
- `chorus bridge-pair --agent <key>` generates a one-time opaque token
- Token has a TTL (default 5 minutes); `bridge-pair` can regenerate if it expires
- Driver passes the token in the MCP `initialize` request (custom header or init params)
- Bridge maps token → agent persona at session setup time; token is consumed after a
  successful `InitializeResult`
- `agent_key` is no longer visible in the transport layer — the URL path carries the
  opaque token instead
- Agent personas survive runtime crash. A new runtime re-pairs to the same persona
- Convert remaining drivers (Codex, Kimi, OpenCode)
- Remove old stdio bridge

Open question: how does the token get from `chorus bridge-pair` output into the driver
config? Options: (a) environment variable injected by the driver at spawn time, (b)
driver reads a well-known file written by `bridge-pair`, (c) driver config references
a shell command that calls `bridge-pair`.

### Phase 3 — Bidirectional Platform connection

What changes:
- Persistent WebSocket between bridge and Platform (Chorus server or cloud)
- Platform pushes lifecycle operations (create, delete, restart agent) to bridge
- Bridge pushes messages and events upstream
- `machine_id` (already in `config.toml`, generated by `chorus setup`) is used to
  route lifecycle commands to the correct bridge in multi-machine setups
- Supports both local-server and remote-server targets

Multi-machine scenario enabled by this phase: Machine A has agents 1–2, Machine B has
agent 3, all in the same channel. "Restart agent 3" from the web UI routes to Machine
B's bridge, not Machine A's.

### Phase 4 — Backend abstraction

What changes:
- `src/bridge/backend.rs` grows a trait-based backend interface
- Trait captures: send message, receive messages, list channels, manage tasks, upload
  files
- Chorus backend implements the trait (already exists as `ChorusBackend`)
- Slack, Discord, custom backends can implement the same trait
- Bridge becomes the universal agent-to-IM gateway — same MCP tool surface regardless
  of upstream backend

---

## 5. Rollback

If the shared bridge has issues and you need to revert a driver to the stdio path:

1. **Revert the driver code** — either `git revert` the commit that added the
   `bridge_endpoint` branch, or manually change the driver back to always use the stdio
   config and remove the `bridge_endpoint` check.

2. **Ensure `bridge_endpoint: None` in the spec** — the default is already `None` in
   all `AgentSpec` constructors. If you wired auto-discovery into the manager, revert
   that too.

3. **No data migration needed** — agent records in `chorus.db` do not store
   `bridge_endpoint`. The field lives only in the in-memory `AgentSpec` built at
   runtime. Reverting the code is sufficient.

4. **Stop `chorus bridge-serve`** — once no drivers reference the shared bridge, the
   daemon can be shut down.

---

## 6. Troubleshooting

### "Bridge unreachable" or connection refused

The shared bridge is not running. Check:

```bash
# Is the discovery file there?
cat ~/.chorus/bridge.json

# Is the process alive? (PID from bridge.json)
ps -p <pid>

# Try the health endpoint directly
curl http://127.0.0.1:4321/health
```

If `bridge.json` does not exist, start `chorus bridge-serve`. If the file exists but
the health check fails, the process recorded in `bridge.json` may have died and left a
stale file — see below.

### "Session not found" or tool calls return session errors

The bridge restarted (clean shutdown or crash) and lost its in-memory session registry.
Runtimes that had active MCP sessions must re-initialize. This is handled automatically
if the driver implements reconnect with backoff. Manual fix: stop and restart the
affected agent runtime, which will send a fresh `initialize` request.

### Stale discovery file

The bridge crashed without running its cleanup hook, leaving `~/.chorus/bridge.json`
pointing at a dead PID. The `read_bridge_info()` helper detects this (it sends signal 0
to the PID and returns `None` if the process is gone), so drivers will fall back to
stdio automatically. To clean up manually:

```bash
rm ~/.chorus/bridge.json
```

Then start `chorus bridge-serve` fresh.

### Debug commands

```bash
# Check bridge discovery state
cat ~/.chorus/bridge.json

# Test bridge health
curl http://127.0.0.1:4321/health

# Full MCP layer smoke test (starts its own temporary bridge)
chorus bridge-smoke-test

# Watch bridge logs in real time (if started in a terminal)
chorus bridge-serve --listen 127.0.0.1:4321 --server-url http://localhost:3001
# → structured tracing output includes agent_key for each new session
```

### Verifying per-agent isolation

If you suspect cross-agent message contamination (Agent A receiving Agent B's
messages), the concurrency test in the test suite covers this:

```bash
cargo test --test e2e_tests bridge
```

The test starts two agents through one bridge and asserts: (1) `check_messages` for
Agent A never returns Agent B's messages, (2) `send_message` from Agent A is attributed
to Agent A in the Chorus server, (3) simultaneous sends from both agents complete
independently.
