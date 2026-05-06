# Bridge Cases

Cases covering the shared MCP bridge and live agent-runtime round-trips.

## Smoke

### BRG-001 Bridge Health

- Suite: smoke
- Goal: Bridge HTTP server starts on a random loopback port and responds to `GET /health`.
- Script: [`tests/bridge_serve_tests.rs`](../../tests/bridge_serve_tests.rs) :: `bridge_starts_and_health_check`
- Steps:
  1. Bind a random loopback port and start the bridge router.
  2. Send `GET http://127.0.0.1:<port>/health`.
  3. Verify status 200 and body `"ok"`.
- Expected: `/health` returns 200 with body `ok`.
- Failure evidence:
  - bind error or HTTP response status/body when non-200

---

### BRG-002 Isolated MCP Sessions

- Suite: smoke
- Goal: Bridge creates one `StreamableHttpService` per agent_key with no cross-talk.
- Script: [`tests/bridge_serve_tests.rs`](../../tests/bridge_serve_tests.rs) :: `two_agents_get_separate_sessions`
- Steps:
  1. Start the bridge.
  2. Send MCP `initialize` to `/agent-a/mcp`; capture `Mcp-Session-Id`.
  3. Send MCP `initialize` to `/agent-b/mcp`; capture `Mcp-Session-Id`.
  4. Verify both return 200 with valid JSON-RPC responses.
  5. Verify the two session IDs are distinct.
- Expected: Each agent_key gets its own MCP session; no session ID collision.
- Failure evidence:
  - SSE response bodies and observed `Mcp-Session-Id` values for both requests

---

### BRG-003 E2E send_message Through Bridge

- Suite: smoke
- Goal: Full data path from MCP client through bridge to Chorus server SQLite store.
- Script: [`tests/bridge_serve_tests.rs`](../../tests/bridge_serve_tests.rs) :: `bridge_sends_message_to_chorus_server`
- Steps:
  1. Start an in-process Chorus server with a temp SQLite store.
  2. Create agent `bot1`, channel `general`, add agent to channel.
  3. Start the bridge pointing at the Chorus server URL.
  4. Perform MCP handshake on `/bot1/mcp` (initialize + initialized notification).
  5. Send `tools/call` for `send_message` with `{"target":"#general","content":"Hello from bridge test!"}`.
  6. Parse the SSE response and confirm no JSON-RPC error.
  7. Query `store.get_history("general", ...)` and verify message present with `sender_name == "bot1"`.
- Expected: Tool call succeeds; message row in store with correct sender and content.
- Failure evidence:
  - JSON-RPC request/response body from the bridge
  - Chorus server error log if `/internal/agent/.../send` failed

---

## Regression

### LRT-001 Live Runtime Bridge [matrix]

- Suite: regression
- Supersedes: LRT-002, LRT-003, LRT-004
- Execution mode: hybrid (Rust CLI)
- Goal: Each runtime, configured with `bridge_endpoint`, pairs with the bridge and round-trips a message through MCP HTTP transport.
- Script: [`tests/live_runtime_tests.rs`](../../tests/live_runtime_tests.rs)

All tests are `#[ignore]` — run with `--ignored`. They skip cleanly when the binary or auth is missing.

#### `CHORUS_RUNTIME` matrix

| Runtime   | Test function                                       | Binary   | Auth env / setup                                                        | Known issues                                                                                         |
| --------- | --------------------------------------------------- | -------- | ----------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------- |
| opencode  | `opencode_agent_replies_through_shared_bridge`      | `opencode` | `opencode auth login`; optional `OPENCODE_MODEL` (default `opencode/gpt-5-nano`) | MCP config shape (`mcp.chat.type = "remote"`) must match driver output                              |
| claude    | `claude_agent_replies_through_shared_bridge`        | `claude`   | `ANTHROPIC_API_KEY` or `claude login`; optional `CHORUS_TEST_CLAUDE_MODEL` (default `sonnet`) | Config key shape / case sensitivity; token consumed before session opens                            |
| codex     | `codex_agent_replies_through_shared_bridge`         | `codex`    | `OPENAI_API_KEY` or `codex login`; optional `CHORUS_TEST_CODEX_MODEL` (default `gpt-5.4`) | HTTP-MCP instability on older versions (see `RUNTIME_MCP_SUPPORT.md`); `command`/`args` vs `url` conflict |
| kimi      | `kimi_agent_replies_through_shared_bridge`          | `kimi`     | Moonshot creds in `~/.kimi/credentials/kimi-code.json`; optional `CHORUS_TEST_KIMI_MODEL` | ACP `session/new` param shape (`type` not `transport`); missing `headers` array                     |

- Steps:
  1. Set env vars for the target runtime (binary path, auth, optional model).
  2. Start in-process Chorus server + bridge.
  3. Create agent `<runtime>-live-bot` in the store; add to `#general`.
  4. Seed a user DM prompting a reply containing `"hello world"`.
  5. Instantiate the runtime driver with `bridge_endpoint: Some(bridge_url)`, attach, and call `start`.
  6. Verify the bridge accepts the pairing and the MCP session connects.
  7. Poll `store.get_history` up to 60s for a reply from `<runtime>-live-bot` containing `"hello world"`.
  8. Verify the agent's session is cleaned up after stop.
- Expected: Bridge connects; agent replies via `send_message`; message lands in store within 60s; session cleaned up.
- Failure evidence:
  - runtime stderr (last 200 lines)
  - written MCP config file contents
  - runtime-specific log (last 100 lines): `~/.opencode/logs/`, `~/.claude/logs/`, `~/.codex/log/`, `~/.kimi/logs/kimi.log`
  - observed channel history at timeout
- Common failure signals:
  - auth missing or expired → runtime reports auth error on stderr
  - MCP config shape wrong → no tool call ever made
  - bridge URL reachable but session ID mismatch → 401 on tool call

---

### INT-001 `chorus serve --shared-bridge` auto-wires agents

- Suite: regression
- Execution mode: hybrid
- Goal: `chorus serve --shared-bridge` starts the bridge in-process, writes the discovery file, and `AgentManager::start_agent` auto-populates `bridge_endpoint`.
- Coverage matrix:
  - Bridge HTTP layer: `tests/bridge_serve_tests.rs`
  - Discovery file I/O: `src/bridge/discovery.rs` unit tests
  - URL-formatting: `src/agent/manager.rs::bridge_endpoint_from_info_formats_url`
  - Driver + bridge round-trip: `tests/live_runtime_tests.rs` (LRT-001)
- Preconditions:
  - At least one runtime binary installed for the round-trip layer
- Steps:
  1. Run `chorus serve --shared-bridge --port 3101 --bridge-port 14321`.
  2. Verify both listeners bind.
  3. Check `~/.chorus/bridge.json` contains the bridge port and PID.
  4. Start an agent and verify `bridge_endpoint: Some("http://127.0.0.1:14321")` without explicit config.
- Expected: Both ports bind; discovery file written; agent auto-discovers bridge endpoint.
- Failure evidence:
  - output of `curl http://127.0.0.1:<bridge-port>/health`
  - contents of `~/.chorus/bridge.json`
  - the agent's `AgentSpec.bridge_endpoint` value at start time
- Common failure signals:
  - discovery file absent → agent fell back to stdio
  - stale PID → `is_pid_alive` false → agent fell back to stdio
  - bridge failed to bind → main server up but no agent pairing
