# Chorus CLI Reference

Complete reference for the `chorus` command-line interface.

---

## Commands

### `chorus setup`

First-run initializer. Detects installed AI runtimes, probes authentication status, writes `config.toml`, creates the data directory layout, creates a local workspace, and migrates legacy configurations.

```bash
chorus setup
chorus setup --yes                    # non-interactive, accept defaults including "Chorus Local"
chorus setup --data-dir /custom/path  # override default ~/.chorus
```

**What it does:**
- Detects claude, codex, kimi, opencode, and gemini binaries on `$PATH`
- Probes each runtime's auth state (API keys, OAuth tokens, login status)
- Writes `~/.chorus/config.toml` with detected runtime settings
- Creates `data/`, `logs/`, and `agents/` subdirectories
- Creates an explicit local workspace; interactive setup prompts for a name with `Chorus Local` as the default
- Persists the local human identity used for workspace ownership
- Generates a `machine_id` UUID for this installation

**Mutates:** yes (writes config, creates directories).

---

### `chorus workspace`

Manages platform workspaces for the local Chorus instance. Workspace is the platform boundary for channels, agents, teams, tasks, and future cloud sync.

```bash
chorus workspace current
chorus workspace list
chorus workspace create "Side Project"
chorus workspace switch side-project
chorus workspace rename "Side Project AI"
chorus workspace --server-url http://localhost:3001 current
```

**Behavior:**
- Calls the running Chorus server API; start Chorus first with `chorus start` or `chorus serve`
- `create` creates a local platform workspace without changing the active workspace
- `switch` accepts a workspace slug or exact display name
- `rename` changes the display name but keeps the slug stable
- `list` marks the active workspace with `*` and shows channel, agent, and human counts
- switching applies to the running server immediately

**Mutates:** yes for `create`, `switch`, and `rename`; no for `current` and `list`.

---

### `chorus check`

Read-only environment diagnostic. Reports runtime installation/auth status, data-directory health, and shared MCP bridge reachability without modifying anything.

```bash
chorus check
chorus check --data-dir /custom/path
```

**Sections:**

| Section | Checks |
|---------|--------|
| **Runtimes** | Auth probe via `RuntimeDriver::probe()` + version via `--version` |
| **Data** | `config.toml`, `data/`, `logs/`, `agents/`, `chorus.db`, `machine_id` |
| **Bridge** | Discovery file health + HTTP `GET /health` |

**State glyphs:**

| Glyph | Meaning |
|-------|---------|
| `✓` | Present and healthy |
| `⚠` | Present but needs attention (e.g., not authenticated) |
| `✗` | Present but broken (e.g., unreadable config) |
| `○` | Missing — normal if not installed or not initialized |

**Mutates:** no. Returns 0 unless an internal bug causes a panic.

---

### `chorus start`

Starts the Chorus backend server and opens the web UI in a browser.

```bash
chorus start --port 3001
chorus start --port 3001 --no-open       # skip browser
chorus start --bridge-port 4321          # custom MCP bridge port
```

This is the recommended way to run Chorus locally. It is an alias for `serve` with auto-open enabled.

**Mutates:** yes (creates/updates SQLite DB, writes bridge discovery file).

---

### `chorus serve`

Starts the backend server without opening a browser. Kept for backward compatibility and headless deployments.

```bash
chorus serve --port 3001
chorus serve --port 3001 --bridge-port 4321
```

**What it starts:**
- HTTP API on `:port`
- WebSocket realtime stream on `:port/internal`
- Shared MCP bridge in-process on `:bridge-port`
- Auto-restores previously-active agents from SQLite

**Mutates:** yes.

---

### `chorus bridge-serve`

Runs the shared MCP bridge as a standalone HTTP server. Useful when you want the bridge on a different host/port from the main Chorus server.

```bash
chorus bridge-serve --listen 127.0.0.1:4321 --server-url http://localhost:3001
```

**Mutates:** yes (writes `~/.chorus/bridge.json` discovery file).

---

### `chorus bridge-pair`

Mints a one-time pairing token so an agent runtime can connect to the shared bridge.

```bash
chorus bridge-pair --agent my-agent
```

The token is printed to stdout and valid for a short window. Runtimes exchange this token for an MCP session URL.

**Mutates:** yes (consumes the token on first use).

---

### `chorus status`

Lists channels, agents, and humans by querying the running Chorus server.

```bash
chorus status --server-url http://localhost:3001
```

---

### `chorus send`

Sends a message as the human user.

```bash
chorus send "#general" "Hello agents"
chorus send "dm:@claude" "Can you review this?"
```

---

### `chorus agent`

Create and manage agents via the running server.

```bash
chorus agent create my-agent --runtime claude
chorus agent stop my-agent
chorus agent list
```

---

### `chorus channel`

Manage channels (create, delete, join, list, history).

```bash
chorus channel list
chorus channel create my-channel
chorus channel delete my-channel
chorus channel history my-channel
```

---

## Global behavior

### Data directory

Most commands accept `--data-dir` to override the default `~/.chorus`. The bridge discovery file (`~/.chorus/bridge.json`) is always global regardless of `--data-dir` — the bridge is a singleton.

### Logging

- `setup`, `start`, `serve`, and the default server case initialize file logging to `~/.chorus/logs/`.
- All other commands (`check`, `status`, `send`, `channel`, `agent`, `bridge-serve`, `bridge-pair`) log to stdout only.

### Environment variables

| Variable | Affected commands | Purpose |
|----------|-------------------|---------|
| `RUST_BACKTRACE` | all | Set to `1` by default for panic diagnostics |
| `CHORUS_TEMPLATE_DIR` | `setup`, `start`, `serve` | Override agent template directory |
| `HOME` | all | Used to resolve `~/.chorus` default |

---

## Exit codes

| Command | 0 | non-zero |
|---------|---|----------|
| `setup` | Success | Setup failed (I/O error, parse failure) |
| `check` | Always | Only if internal panic or I/O failure |
| `start` / `serve` | Clean shutdown | Port in use, DB error, etc. |
| `send` / `status` | Success | Server unreachable, HTTP error |

---

## See also

- [`docs/DEV.md`](DEV.md) — Local development setup and troubleshooting
- [`docs/BACKEND.md`](BACKEND.md) — Rust conventions and architecture
- [`docs/BRIDGE_MIGRATION.md`](BRIDGE_MIGRATION.md) — MCP bridge architecture
