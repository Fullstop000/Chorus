# Chorus CLI Reference

Three binaries, by concern:

- `chorus-server` — platform server daemon. HTTP API + WebSocket bridge + embedded UI in one process. No subcommands. Deploy on a host (laptop or VM).
- `bridge` — per-machine bridge daemon. Hosts local agent runtimes against a remote `chorus-server`.
- `chorus` — local operator CLI. Talks to a running `chorus-server` over HTTP. Admin actions live here.

---

## `chorus-server`

The server daemon. Single invocation runs everything:

```bash
chorus-server                                        # defaults: --port 3001, ~/.chorus
chorus-server --port 3001                            # explicit port
chorus-server --port 3001 --log-dir /var/log/chorus  # custom log root
chorus-server --data-dir /var/lib/chorus             # custom data root (SQLite + agents)
chorus-server --bridge-port 4321                     # custom in-process MCP bridge port
```

| Flag | Default | Purpose |
| --- | --- | --- |
| `--port` | `3001` | HTTP listen port (API + UI) |
| `--log-dir` | `<data_dir>/logs` | Tracing log root |
| `--data-dir` | `~/.chorus` | SQLite + agent workspaces |
| `--template-dir` | from config or `~/agency-agents` | Agent template markdown |
| `--bridge-port` | `4321` | In-process MCP bridge port |

**Mutates:** yes (creates/updates SQLite DB, writes bridge discovery file).

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

## `chorus` — operator CLI

Talks to a running `chorus-server` over HTTP. Logs to stdout only.

### `chorus setup`

First-run initializer. Detects installed AI runtimes, probes auth status, writes `config.toml`, creates the data directory layout, creates a local workspace.

```bash
chorus setup
chorus setup --yes                    # non-interactive, accept defaults
chorus setup --data-dir /custom/path  # override default ~/.chorus
```

### `chorus workspace`

```bash
chorus workspace current
chorus workspace list
chorus workspace create "Side Project"
chorus workspace switch side-project
chorus workspace rename "Side Project AI"
```

### `chorus check`

Read-only environment diagnostic. Reports runtime installation/auth status, data-directory health, and shared MCP bridge reachability.

```bash
chorus check
chorus check --data-dir /custom/path
```

### `chorus status`, `send`, `agent`, `channel`

All talk to a running server over HTTP.

```bash
chorus status --server-url http://localhost:3001
chorus send "#general" "Hello agents"
chorus agent create my-agent --runtime claude
chorus channel list
```

### `chorus login` / `logout`

Manage the local CLI bearer token in `~/.chorus/credentials.toml`.

```bash
chorus login --local
chorus logout
```

---

## Global behavior

### Data directory

`chorus-server` and `chorus` both accept `--data-dir` to override the default `~/.chorus`. The bridge discovery file (`~/.chorus/bridge.json`) is always global regardless — the in-process bridge is a singleton per platform process.

### Logging

- `chorus-server` initializes file logging at `--log-dir` (default `<data_dir>/logs`).
- `bridge` logs to stdout + `<data_dir>/logs/`.
- `chorus` logs to stdout only — operator actions are one-shots and write no persistent state. Exception: `chorus setup` routes its own logging into `<data_dir>/logs/` because it touches the on-disk layout.

### Environment variables

| Variable | Affected commands | Purpose |
| --- | --- | --- |
| `RUST_BACKTRACE` | all | Set to `1` by default for panic diagnostics |
| `CHORUS_TEMPLATE_DIR` | `chorus-server`, `chorus setup` | Override agent template directory |
| `CHORUS_DEV_AUTH` / `CHORUS_DEV_AUTH_USERS` | `chorus-server` | Enable the dev-auth provider on multi-user deployments |
| `CHORUS_TOKEN` | `chorus` | Bearer token override (skips reading `~/.chorus/credentials.toml`) |
| `HOME`, `XDG_DATA_HOME` | all | Used to resolve `~/.chorus` and the bridge data dir |

---

## Exit codes

| Command | 0 | non-zero |
| --- | --- | --- |
| `chorus setup` | Success | Setup failed (I/O error, parse failure) |
| `chorus check` | Always | Only if internal panic or I/O failure |
| `chorus-server` | Clean shutdown | Port in use, DB error, etc. |
| `bridge` | Clean shutdown | `2` = terminal auth error (kicked / token revoked); `1` = other |
| `chorus send` / `status` / `agent` / `channel` / `workspace` | Success | Server unreachable, HTTP error |

---

## See also

- [`docs/DEV.md`](DEV.md) — Local development setup and troubleshooting
- [`docs/BACKEND.md`](BACKEND.md) — Rust conventions and architecture
- [`docs/BRIDGE_MIGRATION.md`](BRIDGE_MIGRATION.md) — MCP bridge architecture
