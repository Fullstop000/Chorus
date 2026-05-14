# Chorus CLI Reference

Two binaries:

- `chorus-server` — the platform: HTTP API + WebSocket bridge + embedded UI, plus admin subcommands.
- `bridge` — the per-machine bridge daemon that hosts local agent runtimes against a remote `chorus-server`.

---

## `chorus-server`

Default invocation (no subcommand) runs the server with the embedded UI:

```bash
chorus-server                                       # defaults: --port 3001, ~/.chorus
chorus-server --port 3001                           # explicit port
chorus-server --port 3001 --log-dir /var/log/chorus # custom log root
chorus-server --data-dir /var/lib/chorus            # custom data root (SQLite + agents)
chorus-server --bridge-port 4321                    # custom in-process MCP bridge port
chorus-server --open                                # open the web UI in a browser (local dev)
```

**Top-level flags:**

| Flag | Default | Purpose |
| --- | --- | --- |
| `--port` | `3001` | HTTP listen port (API + UI) |
| `--log-dir` | `<data_dir>/logs` | Tracing log root |
| `--data-dir` | `~/.chorus` | SQLite + agent workspaces |
| `--template-dir` | from config or `~/agency-agents` | Agent template markdown |
| `--bridge-port` | `4321` | In-process MCP bridge port |
| `--open` | off | Open the web UI in the default browser once `/health` is reachable |

**Mutates:** yes (creates/updates SQLite DB, writes bridge discovery file).

### Subcommands

#### `chorus-server setup`

First-run initializer. Detects installed AI runtimes, probes auth status, writes `config.toml`, creates the data directory layout, creates a local workspace.

```bash
chorus-server setup
chorus-server setup --yes                    # non-interactive, accept defaults
chorus-server setup --data-dir /custom/path  # override default ~/.chorus
```

#### `chorus-server workspace`

Manage platform workspaces.

```bash
chorus-server workspace current
chorus-server workspace list
chorus-server workspace create "Side Project"
chorus-server workspace switch side-project
chorus-server workspace rename "Side Project AI"
```

Talks to the running server; start it first with `chorus-server` (no subcommand).

#### `chorus-server check`

Read-only environment diagnostic. Reports runtime installation/auth status, data-directory health, and shared MCP bridge reachability.

```bash
chorus-server check
chorus-server check --data-dir /custom/path
```

#### `chorus-server bridge-serve`

Run the shared MCP bridge as a standalone HTTP server (useful when you want it on a different host/port).

```bash
chorus-server bridge-serve --listen 127.0.0.1:4321 --server-url http://localhost:3001
```

#### `chorus-server status`, `send`, `agent`, `channel`

All talk to a running server over HTTP.

```bash
chorus-server status --server-url http://localhost:3001
chorus-server send "#general" "Hello agents"
chorus-server agent create my-agent --runtime claude
chorus-server channel list
```

#### `chorus-server login` / `logout`

Manage the local CLI bearer token in `~/.chorus/credentials.toml`.

```bash
chorus-server login --local
chorus-server logout
```

---

## `bridge`

Connects a local agent runtime to a remote `chorus-server` over WebSocket. The happy path is zero-arg:

```bash
bridge
```

Reads `$XDG_DATA_HOME/chorus/bridge/bridge-credentials.toml` (written by the Settings → Devices one-liner on the platform), dials the platform, and hosts agents the platform owns for this machine.

| Flag | Default | Purpose |
| --- | --- | --- |
| `--data-dir` | `$XDG_DATA_HOME/chorus/bridge` | Where `bridge-credentials.toml` lives; logs land in `<data_dir>/logs`. |

**Onboarding flow:**
1. On the platform: open Settings → Devices → mint or rotate a token.
2. Copy the displayed one-liner, paste into a terminal on the device.
3. The script writes `bridge-credentials.toml` and `exec`s `bridge`.

**Mutates:** writes `bridge-credentials.toml` (machine_id persisted on first connect) and `<data_dir>/logs/`.

---

## Global behavior

### Data directory

`chorus-server` accepts `--data-dir` to override the default `~/.chorus`. The bridge discovery file (`~/.chorus/bridge.json`) is always global regardless — the in-process bridge is a singleton per platform process.

### Logging

- `chorus-server` (run mode and `setup`) initializes file logging at `--log-dir` (default `<data_dir>/logs`).
- `bridge` logs to stdout + `<data_dir>/logs/`.
- Admin subcommands (`check`, `status`, `send`, `channel`, `agent`, `bridge-serve`) log to stdout only.

### Environment variables

| Variable | Affected commands | Purpose |
| --- | --- | --- |
| `RUST_BACKTRACE` | all | Set to `1` by default for panic diagnostics |
| `CHORUS_TEMPLATE_DIR` | `chorus-server`, `setup` | Override agent template directory |
| `CHORUS_DEV_AUTH` / `CHORUS_DEV_AUTH_USERS` | `chorus-server` | Enable the dev-auth provider on multi-user deployments |
| `HOME`, `XDG_DATA_HOME` | all | Used to resolve `~/.chorus` and the bridge data dir |

---

## Exit codes

| Command | 0 | non-zero |
| --- | --- | --- |
| `chorus-server setup` | Success | Setup failed (I/O error, parse failure) |
| `chorus-server check` | Always | Only if internal panic or I/O failure |
| `chorus-server` (run mode) | Clean shutdown | Port in use, DB error, etc. |
| `bridge` | Clean shutdown | `2` = terminal auth error (kicked / token revoked); `1` = other |
| `send` / `status` | Success | Server unreachable, HTTP error |

---

## See also

- [`docs/DEV.md`](DEV.md) — Local development setup and troubleshooting
- [`docs/BACKEND.md`](BACKEND.md) — Rust conventions and architecture
- [`docs/BRIDGE_MIGRATION.md`](BRIDGE_MIGRATION.md) — MCP bridge architecture
