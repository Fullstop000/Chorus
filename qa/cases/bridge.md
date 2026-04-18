# Bridge and Runtime Cases

Cases covering the shared MCP bridge, pairing-token flow, and live agent-runtime round-trips. These are subprocess/integration tests, not browser tests.

ID prefixes:

- `BRG-NNN` — bridge HTTP layer (in-process tests)
- `LRT-NNN` — live runtime round-trips (real runtime binary)
- `INT-NNN` — full integration (`chorus serve --shared-bridge`)

All `LRT` cases are `#[ignore]` by default — run with `--ignored`. They require installed runtime binaries and valid auth; tests skip cleanly when either is missing.

See [`../README.md`](../README.md) → "Subprocess and External Runtime Tests" for the Iron Rule on failure evidence and the per-runtime log file reference.

---

### BRG-001 Bridge starts and responds to /health

- Suite: smoke
- Release-sensitive: yes when touching `src/bridge/serve.rs`
- Execution mode: subprocess (in-process listener)
- Goal:
  - prove the shared bridge HTTP server starts on a random loopback port and responds to `GET /health`
- Script:
  - [`tests/bridge_serve_tests.rs`](../../tests/bridge_serve_tests.rs) :: `bridge_starts_and_health_check`
- Preconditions:
  - none (fully in-process)
- Steps:
  1. Bind a random loopback port and start the bridge router.
  2. Send `GET http://127.0.0.1:<port>/health`.
  3. Verify the status is `200` and the body is `"ok"`.
- Expected:
  - server accepts the connection
  - `/health` returns 200 with body `ok`
- Failure evidence (required):
  - bind error if the listener failed to come up
  - HTTP response status code and body when non-200
  - server task panic if the spawned task exited early
- Common failure signals:
  - loopback-bind refused (kernel-level)
  - router wiring regression causing 404 on `/health`

---

### BRG-002 Two agents get isolated MCP sessions

- Suite: smoke
- Release-sensitive: yes when touching `src/bridge/serve.rs` session routing or `get_or_create_service`
- Execution mode: subprocess (in-process listener)
- Goal:
  - prove the bridge creates one `StreamableHttpService` per agent_key and sessions do not cross-talk
- Script:
  - [`tests/bridge_serve_tests.rs`](../../tests/bridge_serve_tests.rs) :: `two_agents_get_separate_sessions`
- Preconditions:
  - none
- Steps:
  1. Start the bridge.
  2. Send MCP `initialize` to `/agent-a/mcp`; capture the `Mcp-Session-Id` header.
  3. Send MCP `initialize` to `/agent-b/mcp`; capture the `Mcp-Session-Id` header.
  4. Verify both return 200 with valid JSON-RPC responses.
  5. Verify the two session IDs are distinct.
- Expected:
  - each agent_key gets its own MCP session
  - no session ID collision across agents
- Failure evidence (required):
  - the full SSE response bodies for both requests
  - both observed `Mcp-Session-Id` values
- Common failure signals:
  - session dispatch collapsed to one shared service
  - rmcp version mismatch producing unexpected envelope shape

---

### BRG-003 Pairing token lifecycle

- Suite: smoke
- Release-sensitive: yes when touching `src/bridge/pairing.rs` or the `/token/{token}/mcp` route
- Execution mode: subprocess (in-process listener)
- Goal:
  - prove tokens are issued, consumed once, rejected after reuse, and rejected after TTL expiry
- Script:
  - [`tests/bridge_serve_tests.rs`](../../tests/bridge_serve_tests.rs) :: `bridge_pair_issues_token`, `token_connects_to_agent_mcp`, `invalid_token_returns_unauthorized`, `expired_token_rejected`
- Preconditions:
  - none
- Steps:
  1. POST `/admin/pair` with `{"agent_key": "bot-1"}`; capture the returned token.
  2. POST MCP `initialize` to `/token/<token>/mcp`; verify 200 + session established.
  3. POST MCP `initialize` with the same token to a new session slot; expect 401 (token already consumed).
  4. Issue a fresh token with 100ms TTL; wait 200ms; expect 401 on use.
  5. Use a syntactically valid but never-issued token; expect 401.
- Expected:
  - `/admin/pair` returns a `token` field
  - first use of a fresh token succeeds
  - second use of the same token fails with 401
  - expired token fails with 401
  - unknown token fails with 401
- Failure evidence (required):
  - request + response body on each unexpected status code
  - the token string that failed validation
  - the `PairingTokenStore` contents at failure time when accessible
- Common failure signals:
  - token not consumed after first use (reuse succeeds)
  - TTL never enforced (expired token still works)

---

### BRG-004 End-to-end send_message through bridge lands in store

- Suite: smoke
- Release-sensitive: yes when touching `src/bridge/backend.rs`, `src/bridge/mod.rs`, or any driver HTTP config
- Execution mode: subprocess (in-process Chorus server + bridge)
- Goal:
  - prove the full data path: MCP client → bridge HTTP → ChatBridge → ChorusBackend → Chorus server → SQLite store
- Script:
  - [`tests/bridge_serve_tests.rs`](../../tests/bridge_serve_tests.rs) :: `bridge_sends_message_to_chorus_server`
- Preconditions:
  - none (in-process Chorus server + store)
- Steps:
  1. Start an in-process Chorus server with a temp SQLite store.
  2. Create agent `bot1`, channel `general`, add agent to channel.
  3. Start the bridge pointing at the Chorus server URL.
  4. Perform MCP handshake on `/bot1/mcp` (initialize + initialized notification).
  5. Send `tools/call` for `send_message` with `{"target":"#general","content":"Hello from bridge test!"}`.
  6. Parse the SSE response and confirm no JSON-RPC error.
  7. Query `store.get_history("general", ...)` and verify the message is present with `sender_name == "bot1"`.
- Expected:
  - tool call returns success
  - message row exists in the store with the correct sender and content
- Failure evidence (required):
  - JSON-RPC request and response body from the bridge
  - Chorus server error log if the POST `/internal/agent/.../send` failed
  - full channel history at the moment of assertion failure
- Common failure signals:
  - `initialized` notification missing (handshake incomplete)
  - ChorusBackend URL-encoding regression for agent_key with special chars
  - server-side channel membership check rejected the sender

---

### LRT-001 OpenCode agent replies through shared bridge

- Suite: regression
- Release-sensitive: yes when touching OpenCode driver or the shared pairing helper
- Execution mode: subprocess (spawns real `opencode` binary)
- Goal:
  - prove an `OpenCode` runtime, configured with `bridge_endpoint`, pairs with the bridge and replies through MCP HTTP transport
- Script:
  - [`tests/live_runtime_tests.rs`](../../tests/live_runtime_tests.rs) :: `opencode_agent_replies_through_shared_bridge`
- Preconditions:
  - `opencode` binary on `PATH`
  - `opencode auth login` completed (or equivalent credential setup)
  - optional env: `OPENCODE_MODEL` (default `opencode/gpt-5-nano`)
- Steps:
  1. Start in-process Chorus server + bridge.
  2. Create agent `opencode-live-bot` in the store; add to `#general`.
  3. Seed a user DM: `@opencode-live-bot please reply with exactly: hello world`.
  4. Instantiate `OpencodeDriver`, build `AgentSpec` with `bridge_endpoint: Some(bridge_url)`, attach, and call `start`.
  5. Poll `store.get_history("general", ...)` up to 60s for a message whose `sender_name == "opencode-live-bot"` and content contains `"hello world"` (case-insensitive).
- Expected:
  - driver writes `opencode.json` with `mcp.chat.type = "remote"` + URL under `/token/<tok>/mcp`
  - agent replies via `send_message` tool call
  - reply lands in store within 60s
- Failure evidence (required):
  - runtime stderr (last 200 lines, from the handle's captured output)
  - contents of the written `opencode.json` MCP config
  - path to `~/.opencode/logs/` and last 100 lines if readable
  - observed channel history at timeout
- Common failure signals:
  - auth missing or expired → runtime reports auth error on stderr
  - MCP config shape wrong → no tool call ever made
  - bridge URL reachable but session ID mismatch → 401 on tool call

---

### LRT-002 Claude Code agent replies through shared bridge

- Suite: regression
- Release-sensitive: yes when touching Claude driver
- Execution mode: subprocess (spawns real `claude` binary)
- Goal:
  - prove a Claude Code runtime, configured with `bridge_endpoint`, connects over HTTP MCP and replies through the bridge
- Script:
  - [`tests/live_runtime_tests.rs`](../../tests/live_runtime_tests.rs) :: `claude_agent_replies_through_shared_bridge`
- Preconditions:
  - `claude` binary on `PATH`
  - `ANTHROPIC_API_KEY` env var OR OAuth via `claude login`
  - optional env: `CHORUS_TEST_CLAUDE_MODEL` (default `sonnet`)
- Steps:
  - same shape as LRT-001 with `ClaudeDriver` and agent key `claude-live-bot`
- Expected:
  - Claude Code picks up `.chorus-claude-mcp.json` with `type: "http"` + URL
  - reply lands in store within 60s
- Failure evidence (required):
  - same as LRT-001 + `~/.claude/logs/` last 100 lines when readable
- Common failure signals:
  - MCP config not loaded (wrong key shape, case sensitivity)
  - token consumed before Claude Code opens the session

---

### LRT-003 Codex agent replies through shared bridge

- Suite: regression
- Release-sensitive: yes when touching Codex driver; watch for HTTP-MCP instability flagged in [`../../docs/RUNTIME_MCP_SUPPORT.md`](../../docs/RUNTIME_MCP_SUPPORT.md)
- Execution mode: subprocess (spawns real `codex` binary)
- Goal:
  - prove the Codex driver's `-c mcp_servers.chat.url=...` flag configuration successfully bridges Codex to the shared HTTP MCP server
- Script:
  - [`tests/live_runtime_tests.rs`](../../tests/live_runtime_tests.rs) :: `codex_agent_replies_through_shared_bridge`
- Preconditions:
  - `codex` binary on `PATH`
  - `OPENAI_API_KEY` env var OR `codex login`
  - optional env: `CHORUS_TEST_CODEX_MODEL` (default `gpt-5.4`)
- Steps:
  - same shape as LRT-001 with `CodexDriver` and agent key `codex-live-bot`
- Expected:
  - Codex starts with the HTTP MCP config flags
  - reply lands in store within 60s
- Failure evidence (required):
  - same as LRT-001 + `~/.codex/log/` last 100 lines when readable
  - the Codex version (`codex --version`) — HTTP MCP stability depends on it
- Common failure signals:
  - known HTTP-MCP instability on older Codex versions (see RUNTIME_MCP_SUPPORT.md)
  - stdio `command`/`args` flags conflicting with `url` flag in the same config key

---

### LRT-004 Kimi agent replies through shared bridge

- Suite: regression
- Release-sensitive: yes when touching Kimi driver's ACP `session/new` params or `.chorus-kimi-mcp.json` config
- Execution mode: subprocess (spawns real `kimi` binary in ACP mode)
- Goal:
  - prove the Kimi driver's two-touchpoint MCP config (file + ACP inline params) works with HTTP transport
- Script:
  - [`tests/live_runtime_tests.rs`](../../tests/live_runtime_tests.rs) :: `kimi_agent_replies_through_shared_bridge`
- Preconditions:
  - `kimi` binary on `PATH`
  - Moonshot credentials in `~/.kimi/credentials/kimi-code.json`
  - optional env: `CHORUS_TEST_KIMI_MODEL` (default `kimi-code/kimi-for-coding`)
- Steps:
  - same shape as LRT-001 with `KimiDriver` and agent key `kimi-live-bot`
- Expected:
  - Kimi picks up both the file config (with `transport: "http"`) and the ACP `session/new` `mcpServers` array (with `type: "http"` and empty `headers`)
  - reply lands in store within 60s
- Failure evidence (required):
  - same as LRT-001 + `~/.kimi/logs/kimi.log` last 100 lines (this file was the only source of signal during the original debug session)
- Common failure signals:
  - ACP `session/new` params with the wrong shape → `acp.exceptions.RequestError: Invalid params` in the log, 60s timeout in the test
  - `transport` field in ACP params (wrong spec — Kimi's FILE format uses `transport`, ACP spec uses `type`)
  - missing `headers` array in ACP params

---

### INT-001 `chorus serve --shared-bridge` auto-wires agents

- Suite: regression
- Release-sensitive: yes when touching `src/cli/serve.rs` shared-bridge startup or `src/agent/manager.rs` discovery auto-population
- Execution mode: subprocess (currently validated by unit + integration tests across three layers)
- Goal:
  - prove the full one-command flow: `chorus serve --shared-bridge` starts the bridge in-process, writes the discovery file, and `AgentManager::start_agent` auto-populates `bridge_endpoint` from it
- Coverage matrix (see [`tests/live_runtime_tests.rs`](../../tests/live_runtime_tests.rs) header):
  - Bridge HTTP layer: `tests/bridge_serve_tests.rs`
  - Discovery file I/O: `src/bridge/discovery.rs` unit tests
  - URL-formatting from discovery: `src/agent/manager.rs::bridge_endpoint_from_info_formats_url`
  - Driver + bridge round-trip: `tests/live_runtime_tests.rs` (LRT-001..004)
- Preconditions:
  - at least one runtime binary installed for the round-trip layer (LRT cases)
- Expected:
  - running `chorus serve --shared-bridge --port 3101 --bridge-port 14321` binds both listeners
  - `~/.chorus/bridge.json` contains the bridge port and PID
  - a subsequently-started agent has `bridge_endpoint: Some("http://127.0.0.1:14321")` without any explicit configuration
- Failure evidence (required):
  - output of `curl http://127.0.0.1:<bridge-port>/health` when the bridge should be up
  - contents of `~/.chorus/bridge.json`
  - the agent's `AgentSpec.bridge_endpoint` value at start time
- Common failure signals:
  - discovery file absent → `read_bridge_info` returned None → agent fell back to stdio
  - stale PID in discovery file → `is_pid_alive` returned false → agent fell back to stdio
  - bridge failed to bind → main server kept running but no agent pairing
