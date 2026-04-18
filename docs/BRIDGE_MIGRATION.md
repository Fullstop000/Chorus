# Bridge Migration Guide

Chorus runs a single shared HTTP bridge that serves all agents. This guide covers how
it works, how to use it, how to implement a driver against it, and what comes next.

For deep architectural context, see the design rationale in this guide
(Section 4, "For Architects") and the phased migration plan below.

---

## 1. Overview

### Architecture

One shared HTTP MCP bridge runs alongside the Chorus server and serves all agents.
Each agent pairs with the bridge to obtain a per-agent opaque token, then connects via
Streamable HTTP to `{bridge_url}/token/{token}/mcp`. The bridge routes tool calls to
the correct agent based on the token. Session management is handled per-agent inside
the same process.

The long-term goal is for the bridge to become a **local agent control plane**: a
durable daemon that decouples agent identity (inbox, history, tasks, permissions) from
the transient runtime process (Claude session #47, Codex run abc123). Runtimes connect,
do their work, and can crash or be swapped without losing agent state. The same bridge
will eventually talk to Chorus local, Chorus cloud, Slack, Discord, or any other IM
backend — one bridge, all agents, any platform.

Drivers never fall back to stdio — the shared HTTP bridge is the only transport. If
the bridge is not running when an agent starts, the agent start fails loudly rather
than silently spawning a per-agent process.

---

## 2. For Users (running Chorus)

### Starting the bridge

`chorus serve` starts the shared bridge in the same process automatically:

```bash
chorus serve --port 3001
```

By default the bridge listens on `127.0.0.1:4321`. Override with `--bridge-port`:

```bash
chorus serve --port 3001 --bridge-port 4400
```

The standalone `chorus bridge-serve` command is still available for users who want to
run the bridge in a separate process (for example, to share one bridge across multiple
chorus instances):

```bash
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

---

## 3. For Driver Authors (implementing a driver)

### The contract

`AgentSpec::bridge_endpoint` is always set to a live bridge URL before the driver is
attached. The driver:

1. Requests a per-agent pairing token via `request_pairing_token(endpoint, agent_key)`
   (defined in `src/agent/drivers/v2/mod.rs`). This `POST`s to `{endpoint}/admin/pair`
   and returns an opaque token that is consumed on the first MCP `initialize`.
2. Constructs the runtime's MCP config using
   `{endpoint}/token/{token}/mcp` as the URL.
3. Spawns the runtime with that config.

### Pattern

```rust
let token = super::request_pairing_token(&self.spec.bridge_endpoint, &self.key)
    .await
    .context("failed to pair with shared bridge")?;

let mcp_config = serde_json::json!({
    "mcpServers": {
        "chat": {
            "type": "http",   // exact key depends on the runtime's config format
            "url": format!(
                "{}/token/{}/mcp",
                self.spec.bridge_endpoint.trim_end_matches('/'),
                token
            )
        }
    }
});
```

Codex (app-server) passes this via `-c` overrides instead of a config file, but the
shape is the same — see `src/agent/drivers/v2/codex.rs`.

### Per-runtime config format

Each runtime expresses HTTP MCP transport differently. Matching the right shape is
load-bearing — sending the wrong keys produces confusing "invalid params" errors:

| Runtime | Config shape |
|---|---|
| Claude Code | `{"type": "http", "url": "…"}` in `mcpServers.chat` (config file) |
| Codex (app-server) | `-c mcp_servers.chat.url="…"` plus `enabled=true` / `required=true` |
| Kimi (file) | `{"transport": "http", "url": "…"}` in `mcpServers.chat` |
| Kimi (ACP inline) | `{"type": "http", "name": "chat", "url": "…", "headers": []}` in `session/new` params |
| OpenCode | `{"type": "remote", "url": "…"}` in `mcp.chat` |

### Driver connection failures

The bridge can crash or be restarted. When that happens:
- All HTTP connections from runtimes are dropped
- The session registry (in-memory) is lost
- Runtimes that hold a live MCP session will see a connection error

Drivers should handle this with exponential backoff retry.

### `bridge_endpoint` at runtime

`AgentManager::start_agent` reads `~/.chorus/bridge.json` via
`chorus::bridge::discovery::read_bridge_info()` and fails loudly if no bridge is
running. `chorus serve` starts the bridge in-process on startup, so under normal
operation the discovery file is always present. If it's missing, start the bridge
explicitly via `chorus bridge-serve` or restart `chorus serve`.

---

## 4. For Architects (phased migration plan)

### Phase 1 — Shared HTTP transport

**Status:** Shipped.

What is in place:
- `chorus serve` starts the bridge in-process; standalone `chorus bridge-serve` also
  available for users who want to run the bridge separately
- Routes: `/token/{token}/mcp` (MCP Streamable HTTP), `/admin/pair` (token issue),
  `/health` (plain text)
- Per-agent `StreamableHttpService<ChatBridge>` created lazily on first request after
  a successful pairing — no pre-registration of agent keys required
- `~/.chorus/bridge.json` written on startup (port, PID, timestamp); removed on clean
  shutdown
- `read_bridge_info()` in `src/bridge/discovery.rs` validates the PID is alive before
  returning the info, preventing stale-file confusion
- `chorus bridge-smoke-test` for "it works" confirmation
- No stdio fallback — drivers fail loudly if the bridge is unavailable

### Phase 2 — Pairing tokens and opaque identity

**Status:** Shipped.

What is in place:
- Driver `start()` calls `request_pairing_token(endpoint, agent_key)` which hits
  `/admin/pair` and receives an opaque token
- Token has a TTL; expires unused after a short grace period
- Bridge maps token → agent persona at session setup time; consumed on the first
  successful `InitializeResult`
- `agent_key` is not visible in the transport layer — the URL path carries the
  opaque token
- Legacy per-agent stdio bridge code path removed

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

There is no stdio bridge to fall back to. If the shared bridge fails to start, fix
the underlying cause (port conflict, permissions on `~/.chorus/bridge.json`, etc.);
drivers will not silently spawn per-agent bridges.

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
to the PID and returns `None` if the process is gone) and `AgentManager::start_agent`
will fail loudly. To clean up manually:

```bash
rm ~/.chorus/bridge.json
```

Then restart `chorus serve` (which will start a fresh bridge) or start
`chorus bridge-serve` standalone.

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
