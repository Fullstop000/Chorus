# Runtime MCP Transport Support

Status of HTTP vs stdio MCP support for each Chorus runtime, for Phase 2 bridge conversion planning.

## Summary

| Runtime | Current Chorus config | Native HTTP MCP? | Phase 2 path |
|---------|----------------------|------------------|--------------|
| Claude Code | stdio command in `.chorus-claude-mcp.json` (`mcpServers.chat.command`) | Yes — `"type": "http"` with `"url"` field, full OAuth + header support | Direct (no adapter needed) |
| Codex | stdio via `-c mcp_servers.chat.command=…` CLI flags | Yes — `url` field in `[mcp_servers.<name>]` TOML, with bearer token and header support; known stability issues in some versions | Direct with caution (verify version) |
| Kimi | stdio command in `.chorus-kimi-mcp.json` (`mcpServers.chat.command`) | Yes — `"url"` field in `~/.kimi/mcp.json`, `kimi mcp add --transport http <name> <url>` | Direct (no adapter needed) |
| OpenCode | stdio via `opencode.json` `mcp.chat.type = "local"` | Yes — `"type": "remote"` with `"url"` field in `opencode.json` | Direct (no adapter needed) |

## Detailed findings

### Claude Code

**Current Chorus config** — writes `.chorus-claude-mcp.json` to the working directory and passes `--mcp-config <path>` on the CLI:

```json
{
  "mcpServers": {
    "chat": {
      "command": "<bridge_binary>",
      "args": ["bridge", "--agent-id", "<key>", "--server-url", "<server_url>"]
    }
  }
}
```

**Native HTTP MCP support** — Yes, fully supported. Claude Code accepts `"type": "http"` with a `"url"` field in the same `mcpServers` JSON format:

```json
{
  "mcpServers": {
    "chat": {
      "type": "http",
      "url": "https://example.com/<agent_key>/mcp"
    }
  }
}
```

OAuth and custom headers (`"headers": {}`) are also supported. The `--mcp-config` CLI flag continues to work with this format.

**Recommendation** — Swap the `command`/`args` keys for `"type": "http"` + `"url"`. No adapter needed.

---

### Codex

**Current Chorus config** — passes inline TOML overrides via `-c` flags to `codex app-server`:

```
-c mcp_servers.chat.command="<bridge_binary>"
-c mcp_servers.chat.args=["bridge","--agent-id","<key>","--server-url","<url>"]
-c mcp_servers.chat.type="stdio"
-c mcp_servers.chat.enabled=true
-c mcp_servers.chat.required=true
```

**Native HTTP MCP support** — Yes, documented as supported. The `[mcp_servers.<name>]` TOML block accepts a `url` field for streamable HTTP:

```toml
[mcp_servers.chat]
url = "https://example.com/<agent_key>/mcp"
enabled = true
required = true
```

Bearer token and custom header support are also available (`bearer_token_env_var`, `http_headers`).

**Caveats** — Several GitHub issues (openai/codex #4707, #5208, #11284) report instability with streamable HTTP in certain versions. An experimental `experimental_use_rmcp_client` flag may be required on older builds. Verify the version deployed before removing the stdio path.

**Recommendation** — Direct conversion using `-c mcp_servers.chat.url=<url>`, but pin to a version where HTTP MCP is stable and run the e2e tests before removing the stdio fallback.

---

### Kimi

**Current Chorus config** — writes `.chorus-kimi-mcp.json` and passes `--mcp-config-file <path>` to `kimi acp`:

```json
{
  "mcpServers": {
    "chat": {
      "command": "<bridge_binary>",
      "args": ["bridge", "--agent-id", "<key>", "--server-url", "<server_url>"]
    }
  }
}
```

The ACP handshake also sends `mcpServers` inline in the `session/new` JSON-RPC params (same stdio format).

**Native HTTP MCP support** — Yes. Kimi CLI stores MCP config in `~/.kimi/mcp.json` and supports streamable HTTP via the CLI or a JSON entry with `"url"`:

```json
{
  "mcpServers": {
    "chat": {
      "url": "https://example.com/<agent_key>/mcp"
    }
  }
}
```

CLI: `kimi mcp add --transport http chat https://example.com/<agent_key>/mcp`

Optional header auth: `--header "Authorization: Bearer <token>"`

**Recommendation** — Update both the file-based config and the ACP `session/new` params. No adapter needed, but the inline ACP params path will also need updating.

---

### OpenCode

**Current Chorus config** — writes `opencode.json` in the working directory with `"type": "local"` (stdio):

```json
{
  "model": "<model_id>",
  "mcp": {
    "chat": {
      "type": "local",
      "command": ["<bridge_binary>", "bridge", "--agent-id", "<key>", "--server-url", "<server_url>"]
    }
  }
}
```

OpenCode uses ACP over stdio (`opencode acp`); the `mcpServers` field in the `session/new` JSON-RPC params is currently an empty array — MCP is wired entirely through `opencode.json`.

**Native HTTP MCP support** — Yes. OpenCode natively supports remote MCP via `"type": "remote"` with a `"url"` field:

```json
{
  "model": "<model_id>",
  "mcp": {
    "chat": {
      "type": "remote",
      "url": "https://example.com/<agent_key>/mcp"
    }
  }
}
```

Optional `"headers"` and `"oauth": false` fields are also supported. SSE-based servers have known issues; streamable HTTP is recommended.

**Recommendation** — Change `"type": "local"` → `"type": "remote"` and swap `"command"` for `"url"`. No adapter needed; the `opencode.json` file path already exists in the driver.

---

## Recommended Phase 2 conversion order

1. **OpenCode** — single config file (`opencode.json`), one key swap (`local` → `remote`), no ACP param changes needed. Cleanest diff, lowest risk.
2. **Claude Code** — single JSON config file, one key swap (`command`/`args` → `type`/`url`). Well-documented, stable HTTP MCP support.
3. **Kimi** — also a single JSON config file, but requires updating both the file-based config _and_ the ACP `session/new` inline params. Two touch points but both are straightforward.
4. **Codex** — documented HTTP support but the most active bug history around streamable HTTP. Convert last, after the other three are validated. Keep the stdio fallback path available until a stable version is confirmed.

## References

- [Claude Code MCP docs](https://code.claude.com/docs/en/mcp)
- [Codex MCP docs](https://developers.openai.com/codex/mcp)
- [Codex config reference](https://developers.openai.com/codex/config-reference)
- [Kimi CLI MCP docs](https://moonshotai.github.io/kimi-cli/en/customization/mcp.html)
- [OpenCode MCP servers docs](https://opencode.ai/docs/mcp-servers/)
- [openai/codex issue #4707 — streamable HTTP not working properly](https://github.com/openai/codex/issues/4707)
- [openai/codex PR #4317 — Add support for streamable HTTP MCP servers](https://github.com/openai/codex/pull/4317)
- [MCP spec — Transports (2025-03-26)](https://modelcontextprotocol.io/specification/2025-03-26/basic/transports)
