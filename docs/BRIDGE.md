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

See `docs/DRIVERS.md` for the driver-author contract and pairing-token
flow.

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

## Bridge ↔ Platform protocol

Specification for the `chorus bridge` → `chorus-server` wire. Two
transports by function:

- **Plain HTTPS POSTs** carry the runtime data plane — the existing
  chat/task/decision Axum handlers (`/internal/agent/...`), plus file
  upload/view. Bearer auth on every request; no app-level error
  envelope.
- **One persistent WebSocket** (`GET /api/bridge/ws`) carries the
  bridge control plane — handshake, state recovery, lifecycle commands,
  chat-message wake. WS protocol-level ping/pong every 15 s is the
  heartbeat.

The runtime ↔ bridge MCP layer (described above) is unchanged. Only
the bridge ↔ platform leg is specified here.

### Connection model

```
                             bearer-auth on every request
              ┌──────────────────────────────────────────────────────────────┐
              │                                                              │
┌─────────────▼────────┐                                            ┌────────▼─────────┐
│  chorus bridge       │  HTTPS POST  ────────────────────────────▶ │  chorus-server   │
│  (any host, NAT OK)  │   /internal/agent/...                      │  (cloud or local)│
│                      │   /api/attachments/{id}                    │                  │
│                      │                                            │                  │
│                      │  GET /api/bridge/ws  (WS upgrade) ───────▶ │                  │
│                      │  ◀── persistent full-duplex ────────────── │                  │
│                      │       JSON frames + WS ping/pong           │                  │
└──────────────────────┘                                            └──────────────────┘
```

- **Direction:** bridge dials outbound for both transports.
  NAT-friendly; no inbound port on the bridge host.
- **Cardinality:** at most one WS per `machine_id`. A second WS
  arrival supersedes (4002 close on the older).
- **Auth:** Bearer token on WS upgrade and every HTTPS POST. Token is
  opaque, bound to a single `machine_id`; platform looks up
  `machine_id` from the token directly (no separate header).
- **Reconnect:** exponential backoff (250 ms → cap 30 s, jitter ±20 %).
  On reconnect, bridge sends `bridge.hello` as the first frame, gets
  `bridge.target` back, reconciles locally. Server-initiated clean WS
  close is routine; bridge treats it as a normal reconnect trigger.
- **Liveness:** WS protocol-level ping/pong every 15 s in both
  directions. Either side closes (4001 stale) after 45 s without a
  pong.

### Wire format

WS frames are JSON over text frames, envelope:

```json
{ "v": 1, "type": "bridge.hello", "data": { "machine_id": "...", "agents_alive": [...] } }
```

- `v: 1` for forward compat. Bumps via subprotocol negotiation.
- `type` selects the schema for `data` (see frame catalog).
- Receivers ignore unknown `type` values (logged + skipped, not fatal).
- Frames are independent — no request/response correlation IDs. State
  flows as a stream of authoritative events; each `bridge.hello` →
  `bridge.target` exchange is itself the reconcile primitive.

HTTPS POSTs use standard `Content-Type: application/json` with:

| Header                       | Purpose                                                                                |
| ---------------------------- | -------------------------------------------------------------------------------------- |
| `Authorization: Bearer <t>`  | Auth, every request. Token is opaque; platform looks up `machine_id` from token.       |
| `Idempotency-Key: <ulid>`    | Optional on writes; platform dedupes for 5 min.                                        |

Standard HTTP status codes; no app-level error envelope.

### Frame catalog

Bridge → Platform:

| Frame type     | When sent                                                                                | `data` fields                                                                                                                            |
| -------------- | ---------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------- |
| `bridge.hello` | First frame after every WS connect; re-sent if the bridge wants to re-declare its state  | `{ machine_id, bridge_version, supported_frames, agents_alive: [{ agent_id, state, runtime_pid, last_acked_seq, started_at }] }`         |
| `agent.state`  | Agent transition (started, stopped, sleeping, crashed)                                   | `{ agent_id, state, reason?, ts, runtime_pid }` — `runtime_pid` is the instance discriminator (see State recovery)                       |
| `chat.ack`     | After bridge has buffered a `chat.message.received` batch into the agent's local mailbox | `{ agent_id, last_seq, ts }`                                                                                                              |

Platform → Bridge:

| Frame type                 | When sent                                                                                 | `data` fields                                                                                                                                  |
| -------------------------- | ----------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------- |
| `bridge.target`            | In reply to `bridge.hello`; **also pushed** whenever desired state changes (UI/decision)  | `{ target_agents: [{ agent_id, runtime, model, system_prompt, env_vars, init_directive?, pending_prompt? }] }`                                |
| `chat.message.received`    | Whenever `event_bus` emits a delivery for an agent on this bridge                         | `{ agent_id, messages: [...] }`                                                                                                                |

### State recovery

On bridge WS (re)connect:

1. Bridge upgrades to WS with `Authorization: Bearer <token>`.
   Platform validates and records the connection in its in-memory
   bridge registry keyed by `machine_id`.
2. Bridge sends `bridge.hello` as its first frame, including
   `agents_alive[]` with current runtime state and per-agent
   `last_acked_seq` (from its local mailbox state).
3. Platform reconciles `agents_alive` against its DB view of agents
   owned by this `machine_id`:
   - **Platform expects running, hello omits → mark crashed**, persist
     transition, notify viewers via `event_bus`.
4. Platform sends `bridge.target` with the desired runtime config for
   every agent that should run on this bridge. **`bridge.target` is
   the authoritative reconcile mechanism.**
5. Bridge reconciles `target_agents` against `agents_alive` locally:
   - **In target, not alive → start** (use the provided config + any
     `init_directive`).
   - **Alive, not in target → stop**.
   - **Both → no-op.** If `pending_prompt` is set on a target_agent
     entry whose process is already alive, bridge calls
     `session.prompt(pending_prompt)` and emits the matching
     `agent.state` frame on completion.
6. Platform replays any chat messages with `seq > last_acked_seq` for
   owned agents as `chat.message.received` frames.
7. **Stale `agent.state` filtering:** every `agent.state` frame
   carries `runtime_pid`. Platform tracks the current `runtime_pid`
   per agent (set on the most recent `started` transition) and drops
   frames whose `runtime_pid` doesn't match. Prevents a delayed
   terminal report from a previous instance (stop → start in quick
   succession) from silently marking the live new instance crashed.

Subsequent in-band updates:

- Platform pushes `bridge.target` whenever desired state changes (UI
  start/stop, agent edit, decision-resolve sets a `pending_prompt`).
- Bridge pushes `agent.state` on each transition, with `runtime_pid`.
- Bridge pushes `chat.ack` after each delivery batch is buffered.
- Bridge may re-send `bridge.hello` periodically (~5 min) as
  belt-and-suspenders self-correction; the diff against the previous
  `bridge.target` is normally empty.

### Backpressure

- **WS outbound (both sides):** bounded mpsc per connection (default
  256 frames). All frame types are load-bearing and never drop. If
  the queue overflows past that, the side closes the WS with 4003
  overrun and forces a reconnect + hello reconcile.
- **HTTPS POSTs:** standard HTTP semantics; no protocol-level
  backpressure. Platform may rate-limit via standard middleware.

### Errors

WS close codes:

| Code | Meaning                                                          |
| ---- | ---------------------------------------------------------------- |
| 4001 | `stale` — no pong within the liveness window.                    |
| 4002 | `superseded` — newer WS arrived for same `machine_id`.           |
| 4003 | `overrun` — outbound frame queue overflowed.                     |
| 4004 | `unauthorized` — token revoked or invalidated mid-stream.        |

HTTPS status codes:

| Status | Meaning                                                          |
| ------ | ---------------------------------------------------------------- |
| 401    | Unauthorized — token missing/invalid.                            |
| 404    | Not found — channel/agent/task missing.                          |
| 409    | Conflict — idempotency-key collision with different payload.     |
| 422    | Unprocessable — payload schema invalid.                          |
| 5xx    | Internal — platform bug.                                         |

### Versioning

- `v: 1` in every WS frame and POST body.
- `bridge.hello.supported_frames` lets the platform avoid pushing
  frame types a stale bridge doesn't know.
- Receivers ignore unknown fields. Unknown WS frame `type` on the
  bridge is logged and skipped, not fatal.
- Major bumps negotiated on the WS upgrade `Sec-WebSocket-Protocol`
  subprotocol header (e.g., `chorus.bridge.v1` → `chorus.bridge.v2`).

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
