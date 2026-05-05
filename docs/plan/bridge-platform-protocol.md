# Bridge ↔ Platform Protocol (Phase 3)

**Purpose:** Define the wire contract between a local Chorus bridge and
the (eventually cloud-hosted) Chorus platform. Two transports, by
function:

- **Plain HTTPS POSTs** for the runtime data plane — the 10
  chat/task/decision RPCs that already exist as Axum handlers, plus
  file upload/view. Bearer auth replaces loopback trust; no new
  endpoints, no transport rewrite.
- **One persistent WebSocket** for the bridge control plane —
  handshake, state recovery, lifecycle commands, chat-message wake.
  WS protocol-level ping/pong is the heartbeat. No SSE, no separate
  POSTs for bridge state.

**Status:** Design — not yet implemented. Reviewers: please push back
on the frame catalog and the four Open Decisions at the bottom.

---

## 1. Goals

- Let a bridge run on any host that can dial outbound HTTPS to the
  platform; replaces today's localhost-only bridge↔platform coupling.
- Reuse the existing 10 chat/task/decision Axum handlers for all
  runtime-initiated traffic. Bearer auth replaces loopback trust; no
  transport rewrite.
- One WebSocket carries the entire bridge control plane: identity,
  state, lifecycle, push wake, heartbeat. WS protocol-level ping/pong
  is the heartbeat — no separate endpoint needed.
- Survive bridge restart (state recovery via `bridge.hello` as first
  WS frame, with full target_agents reply) and platform restart
  (bridge reconnects with backoff).

## 2. Non-goals (this doc)

- Postgres migration. Protocol does not depend on DB engine.
- Cloud Run / GH Actions deploy.
- `chorus connect <preview-url>` CLI flow. Separate plan.
- Workspace / multi-tenant boundary. Protocol routes by `machine_id`;
  whether two `machine_id`s see each other's data is one layer above.
- Phase 4 backend abstraction (Slack/Discord).

## 3. Operation surface

Two views of the same data: §3.1 is the before/after table for every
operation that crosses the boundary; §3.2 is the WS frame catalog.

### 3.1 Operation catalog — before / after

| Op / Process               | Today (Phase 2)                                              | After (Phase 3)                                              | Source                |
| -------------------------- | ------------------------------------------------------------ | ------------------------------------------------------------ | --------------------- |
| **B→P chat / tasks / decisions (HTTPS, authenticated)** ||||
| `send_message`             | POST `/internal/agent/{id}/send` (loopback)                  | same endpoint, bearer auth                                   | `backend.rs:228`      |
| `read_history`             | GET  `/internal/agent/{id}/history`                          | same, bearer auth                                            | `backend.rs:314`      |
| `list_server`              | GET  `/internal/agent/{id}/server`                           | same, bearer auth                                            | `backend.rs:496`      |
| `list_tasks`               | GET  `/internal/agent/{id}/tasks`                            | same, bearer auth                                            | `backend.rs:522`      |
| `create_tasks`             | POST `/internal/agent/{id}/tasks`                            | same, bearer auth                                            | `backend.rs:593`      |
| `claim_tasks`              | POST `/internal/agent/{id}/tasks/claim`                      | same, bearer auth                                            | `backend.rs:641`      |
| `unclaim_task`             | POST `/internal/agent/{id}/tasks/unclaim`                    | same, bearer auth                                            | `backend.rs:707`      |
| `update_task_status`       | POST `/internal/agent/{id}/tasks/update-status`              | same, bearer auth                                            | `backend.rs:741`      |
| `resolve_channel`          | POST `/internal/agent/{id}/resolve-channel`                  | same, bearer auth                                            | `backend.rs:787`      |
| `dispatch_decision`        | POST `/internal/agent/{id}/decisions`                        | same, bearer auth                                            | `backend.rs:1003`     |
| **B→P files (HTTPS, unchanged)** ||||
| `upload_file`              | POST `/internal/agent/{id}/upload` (multipart)               | same endpoint, bearer auth                                   | `backend.rs:849`      |
| `view_file`                | GET  `/api/attachments/{id}`                                 | same                                                         | `backend.rs:935`      |
| **B→P bridge control plane (WS frames)** ||||
| `bridge.hello`             | n/a (in-process)                                             | WS frame; first frame after connect; re-sent on state change | (new)                 |
| `agent.state`              | `AgentManager` observes session handles directly             | WS frame on each transition; carries `runtime_pid` discriminator | `manager.rs`     |
| `chat.ack`                 | implicit (long-poll consume advanced platform cursor)        | WS frame after each delivery batch is buffered locally       | (new)                 |
| **P→B bridge control plane (WS frames)** ||||
| `bridge.target`            | n/a (in-process)                                             | WS frame; reply to hello; pushed when desired state changes  | (new)                 |
| `chat.message.received`    | bridge polls GET `/internal/agent/{id}/receive?block=true`   | WS frame; push wake replaces long-poll                       | `backend.rs:281`      |
| **Connection-level** ||||
| Heartbeat                  | n/a                                                          | WS protocol-level ping/pong every 15 s, both directions      | (new)                 |
| Discovery                  | `~/.chorus/bridge.json` (PID + loopback port)                | `~/.chorus/preview.toml` (platform URL + bearer token)       | `bridge/discovery.rs` |
| Auth                       | none (loopback only)                                         | Bearer on WS upgrade + every HTTPS POST                      | (new)                 |

The runtime ↔ bridge MCP layer is unchanged. Only the bridge ↔ platform
leg changes.

### 3.2 WS frame catalog

All frames are JSON over text frames. Envelope:

```json
{ "v": 1, "type": "<frame-type>", "data": { ... } }
```

Bridge → Platform frames:

| Frame type     | When sent                                                                                | `data` fields                                                                                                                            |
| -------------- | ---------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------- |
| `bridge.hello` | First frame after every WS connect; re-sent if the bridge wants to re-declare its state  | `{ machine_id, bridge_version, supported_frames, agents_alive: [{ agent_id, state, runtime_pid, last_acked_seq, started_at }] }`         |
| `agent.state`  | Agent transition (started, stopped, sleeping, crashed)                                   | `{ agent_id, state, reason?, ts, runtime_pid }` — `runtime_pid` is the instance discriminator (see §6)                                   |
| `chat.ack`     | After bridge has buffered a `chat.message.received` batch into the agent's local mailbox | `{ agent_id, last_seq, ts }`                                                                                                              |

Platform → Bridge frames:

| Frame type                 | When sent                                                                                 | `data` fields                                                                                                                                  |
| -------------------------- | ----------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------- |
| `bridge.target`            | In reply to `bridge.hello`; **also pushed** whenever desired state changes (UI/decision)  | `{ target_agents: [{ agent_id, runtime, model, system_prompt, env_vars, init_directive?, pending_prompt? }] }`                                |
| `chat.message.received`    | Whenever `event_bus` emits a delivery for an agent on this bridge                         | `{ agent_id, messages: [...] }`                                                                                                                |

Heartbeat: WS protocol-level ping/pong, every 15 s in both directions.
Implementation handled by standard WS libraries (axum, tokio-tungstenite,
etc.). No app-level frame.

## 4. Connection model

```
                             bearer-auth on every request
              ┌──────────────────────────────────────────────────────────────┐
              │                                                              │
┌─────────────▼────────┐                                            ┌────────▼─────────┐
│  chorus bridge       │  HTTPS POST  ────────────────────────────▶ │  chorus platform │
│  (any host, NAT OK)  │   /internal/agent/...   (10 existing RPCs) │  (cloud or local)│
│                      │   /api/attachments/{id}                    │                  │
│                      │                                            │                  │
│                      │  GET /api/bridge/ws  (WS upgrade) ───────▶ │                  │
│                      │  ◀── persistent full-duplex ────────────── │                  │
│                      │       JSON frames + WS ping/pong           │                  │
└──────────────────────┘                                            └──────────────────┘
```

- **Direction:** bridge dials outbound for both transports. Standard
  NAT-friendly pattern; no inbound port on the bridge host.
- **Cardinality:** at most one WS per `machine_id`. A second WS
  arrival supersedes (4002 close on the older).
- **Auth:** Bearer token on WS upgrade and every HTTPS POST. Token is
  opaque, bound to a single `machine_id`; platform looks up
  `machine_id` from the token directly (no separate header).
- **Reconnect:** exponential backoff (250 ms → cap 30 s, jitter ±20 %).
  On reconnect, bridge sends `bridge.hello` as the first frame, gets
  `bridge.target` back, reconciles locally. **Server-initiated clean
  WS close is routine** (Cloud Run caps requests at ~60 min); bridge
  treats it as a normal reconnect trigger, not an error.
- **Liveness:** WS protocol-level ping/pong every 15 s in both
  directions. Either side closes (4001 stale) after 45 s without a
  pong.

## 5. Wire format

### 5.1 WS frames

JSON over text frames, envelope:

```json
{ "v": 1, "type": "bridge.hello", "data": { "machine_id": "...", "agents_alive": [...] } }
```

- `v: 1` for forward compat. Bumps via subprotocol negotiation.
- `type` selects the schema for `data`. See §3.2 catalog.
- Receivers ignore unknown `type` values (logged + skipped, not fatal).

Frames are independent — no request/response correlation IDs. State
flows as a stream of authoritative events; each `bridge.hello` →
`bridge.target` exchange is itself the reconcile primitive.

### 5.2 HTTPS POSTs

Standard `Content-Type: application/json`. Headers:

| Header                       | Purpose                                                                                |
| ---------------------------- | -------------------------------------------------------------------------------------- |
| `Authorization: Bearer <t>`  | Auth, every request. Token is opaque; platform looks up `machine_id` from token.       |
| `Idempotency-Key: <ulid>`    | Optional on writes; platform dedupes for 5 min.                                        |

Standard HTTP status codes; no app-level error envelope.

## 6. State recovery

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
- Bridge may re-send `bridge.hello` periodically (every ~5 min) as a
  belt-and-suspenders self-correction; the diff against the previous
  `bridge.target` is normally empty.

## 7. Backpressure

- **WS outbound (both sides):** bounded mpsc per connection (default
  256 frames). All frame types in §3.2 are load-bearing and never
  drop. If the queue overflows past that, the side closes the WS with
  4003 overrun and forces a reconnect + hello reconcile.
- **HTTPS POSTs:** standard HTTP semantics; no protocol-level
  backpressure. Platform may rate-limit via standard middleware.

## 8. Errors

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

## 9. Versioning

- `v: 1` in every WS frame and POST body.
- `bridge.hello.supported_frames` lets the platform avoid pushing
  frame types a stale bridge doesn't know.
- Receivers ignore unknown fields. Unknown WS frame `type` on the
  bridge is logged and skipped, not fatal.
- Major bumps negotiated on the WS upgrade `Sec-WebSocket-Protocol`
  subprotocol header (e.g., `chorus.bridge.v1` → `chorus.bridge.v2`).

## 10. Sequence diagram

Connect → hello/target → chat round trip → crash:

```
Bridge                                                 Platform
  │  GET /api/bridge/ws  (upgrade, bearer)  ─────────▶│
  │  ◀── 101 Switching Protocols ────────────────────│
  │                                                    │
  │  WS frame: bridge.hello {machine_id, agents_alive} ▶│ reconciles in DB
  │  ◀── WS frame: bridge.target {target_agents} ──── │
  │  (bridge starts target ∖ alive, stops alive ∖ target)
  │                                                    │
  │  ◀── WS ping  (every 15s) ──────────────────────  │
  │  WS pong ────────────────────────────────────────▶│
  │                                                    │
  │                                                    │ (a viewer types a chat msg
  │                                                    │  for agent agt_abc)
  │  ◀── WS frame: chat.message.received {agent_id,…} │
  │  (bridge buffers; runtime drains via check_messages MCP)
  │  WS frame: chat.ack {agent_id, last_seq} ─────────▶│ advance last_delivered_seq
  │                                                    │
  │  (runtime sends reply via MCP)                     │
  │  POST /internal/agent/agt_abc/send {target,content} ▶│
  │  ◀── 200 {messageId, seq} ────────────────────── │
  │                                                    │
  │  (UI clicks "stop agent agt_abc")                  │
  │  ◀── WS frame: bridge.target {target_agents: []}  │
  │  (bridge stops agt_abc, emits agent.state)         │
  │  WS frame: agent.state {agent_id, "stopped", pid} ▶│ persists, notifies viewers
```

## 11. Open decisions

| ID  | Question                                                       | Recommendation                                              | Why                                                                |
| --- | -------------------------------------------------------------- | ----------------------------------------------------------- | ------------------------------------------------------------------ |
| D-A | Token binding granularity — single `machine_id` vs account-wide? | Single `machine_id`.                                        | Token leak ≠ identity hijack; matches the existing `machine_id`-per-host model. |
| D-B | Orphan-agent reconcile — adopt or stop?                        | Stop.                                                       | Agents the platform can't observe shouldn't run.                  |
| D-C | WS frame format — JSON v1 vs CBOR/MsgPack?                     | JSON v1; revisit after measurement.                         | Debug-friendly with `wscat` and devtools; volume is control-plane scale; serde-native. |
| D-D | Periodic re-hello cadence (belt-and-suspenders self-correction) | Every ~5 min.                                               | Push-on-change covers the hot path; the periodic re-hello catches any drift between platform's `target_agents` and what the bridge actually has. |

## 12. What this enables (and explicitly leaves for later)

**Enables, in roughly this order:**

1. **Phase 3-local** — both halves on localhost, WS + HTTPS replaces
   today's loopback HTTP. De-risks the protocol with no cloud or auth
   complexity.
2. **`chorus connect <preview-url>`** — CLI flow that pairs a bridge to
   a remote platform. Separate plan; uses this protocol.
3. **Multi-bridge** — multiple `machine_id`s connected to one
   platform, each owning the agents on its host.

**Explicitly out of scope (separate plans):**

- DB migration to Postgres.
- Cloud Run / GH Actions deploy.
- Workspace / multi-tenant boundary on top of `machine_id` routing.
- Phase 4 backend abstraction.
- **Token rotation and revocation.** Preview-env tokens are
  bearer-and-forget; the WS 4002 supersede gives an attacker
  immediate eviction of the legitimate bridge if a token leaks. A
  production deployment needs a revoke endpoint, expiry on tokens,
  and anomaly detection on connection origin (IP flapping). Out of
  scope for the wire contract, in scope for the eventual cloud
  deploy.
- **`agent.runtime.log` streaming.** Live runtime trace from the
  bridge to the platform UI. Worth its own event topic on the WS
  later, but not load-bearing for v1.
