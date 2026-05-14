# Bridge

Single shared HTTP MCP bridge runs in-process alongside `chorus-server`
and serves all agents. Each agent pairs with the bridge to obtain a
per-agent opaque token, then connects via Streamable HTTP to
`{bridge_url}/token/{token}/mcp`. The bridge routes tool calls to the
correct agent based on the token; session state is in-memory per agent.

Drivers never fall back to stdio — the shared HTTP bridge is the only
transport. If the bridge is not running when an agent starts, the agent
start fails loudly rather than silently spawning a per-agent process.

Cross-machine setups use the `chorus bridge` daemon, which proxies
tool-calls back over an authenticated WebSocket upgrade
(`/api/bridge/ws`) instead of binding a loopback MCP port on the
device.

---

## Per-runtime MCP config

Each runtime expresses the HTTP MCP transport differently. Matching the
right shape is load-bearing — sending the wrong keys produces confusing
"invalid params" errors:

| Runtime | Config shape |
| --- | --- |
| Claude Code | `{"type": "http", "url": "…"}` in `mcpServers.chat` (config file) |
| Codex (app-server) | `-c mcp_servers.chat.url="…"` plus `enabled=true` / `required=true` |
| Kimi (file) | `{"transport": "http", "url": "…"}` in `mcpServers.chat` |
| Kimi (ACP inline) | `{"type": "http", "name": "chat", "url": "…", "headers": []}` in `session/new` params |
| OpenCode | `{"type": "remote", "url": "…"}` in `mcp.chat` |

See `docs/DRIVER_GUIDE.md` for the full driver-author contract and
pairing-token flow.

---

## Discovery file

The bridge writes `~/.chorus/bridge.json` on startup so drivers can find
it without a hard-coded port:

```json
{
  "port": 4321,
  "pid": 12345,
  "started_at": "2026-04-16T01:37:21Z"
}
```

`AgentManager::start_agent` reads this via
`chorus::bridge::discovery::read_bridge_info()` and fails loudly if the
file is missing or the PID is dead. The discovery guard removes the file
on every exit path; a stale file means the bridge process crashed
without running its `Drop`.

---

## Troubleshooting

### "Bridge unreachable" / connection refused

The bridge is not running.

```bash
cat ~/.chorus/bridge.json       # discovery file present?
ps -p <pid>                     # process alive?
curl http://127.0.0.1:4321/health
```

If the file is missing, start `chorus-server` (the in-process bridge
launches automatically). If the file exists but the health probe fails,
the recorded PID is likely dead — see the next section.

### Stale discovery file

The bridge crashed without running its cleanup hook, leaving a stale
`~/.chorus/bridge.json` pointing at a dead PID. `read_bridge_info()`
detects this (`kill -0 <pid>` → ESRCH) and `start_agent` fails loudly.
Clean up manually and restart:

```bash
rm ~/.chorus/bridge.json
chorus-server --port 3001
```

### "Session not found" / tool calls return session errors

The bridge restarted and lost its in-memory session registry. Runtimes
that held a live MCP session must re-initialize. Drivers handle this
automatically via reconnect-with-backoff; manual fix is to restart the
affected agent runtime so it sends a fresh `initialize`.

### Verifying per-agent isolation

If you suspect cross-agent message contamination, the e2e test covers
it:

```bash
cargo test --test e2e_tests bridge
```

The test starts two agents through one bridge and asserts that
`check_messages` for agent A returns only A's messages, never B's.
