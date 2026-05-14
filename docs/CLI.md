# Chorus CLI Reference

Two binaries, by location.

- **`chorus-server`** — server-side daemon. HTTP API + WebSocket bridge
  + embedded UI in one process. No subcommands.
- **`chorus`** — device-side binary. Operator subcommands plus the
  bridge daemon mode (`chorus bridge`).

For per-subcommand flags and usage, `chorus --help` and
`chorus <cmd> --help` are canonical. This doc covers cross-cutting
behavior — anything `--help` doesn't.

---

## chorus-server

Top-level invocation runs the platform; there are no subcommands.

| Flag | Default | Purpose |
| --- | --- | --- |
| `--port` | `3001` | HTTP listen port (API + UI) |
| `--log-dir` | `<data_dir>/logs` | Tracing log root |
| `--data-dir` | `~/.chorus` | SQLite + agent workspaces |
| `--template-dir` | from config or `~/agency-agents` | Agent template markdown |
| `--bridge-port` | `4321` | In-process MCP bridge port |

The in-process MCP bridge always binds `127.0.0.1`; it has no auth
beyond loopback. Cross-machine connectivity goes through
`chorus bridge`, which authenticates over the WS upgrade.

## chorus

Device-side binary. Two roles:

- **Operator subcommands** (`setup`, `agent`, `send`, `status`,
  `channel`, `workspace`, `login`, `logout`, `check`) — one-shot HTTP
  clients against a running `chorus-server`.
- **`chorus bridge`** — long-running daemon. Reads
  `$XDG_DATA_HOME/chorus/bridge/bridge-credentials.toml` (written by the
  Settings → Devices one-liner on the platform), dials the platform's
  `/api/bridge/ws`, and hosts the agents the platform owns for this
  machine.

---

## Logging

- `chorus-server` initializes file logging at `--log-dir`
  (default `<data_dir>/logs`).
- `chorus bridge` file-logs into `<bridge data_dir>/logs/`.
- `chorus setup` file-logs into `<data_dir>/logs/` — it touches the
  on-disk layout, so first-run problems are diagnosable.
- All other `chorus` subcommands log to stdout only; they're one-shot
  HTTP clients with no persistent state.

## Environment variables

| Variable | Affects | Purpose |
| --- | --- | --- |
| `RUST_BACKTRACE` | all | Set to `1` by default so panics carry backtraces |
| `CHORUS_TEMPLATE_DIR` | `chorus-server`, `chorus setup` | Override agent template directory |
| `CHORUS_DEV_AUTH` / `CHORUS_DEV_AUTH_USERS` | `chorus-server` | Enable the dev-auth provider on multi-user deployments |
| `CHORUS_TOKEN` | `chorus` | Bearer token override (skips reading `~/.chorus/credentials.toml`) |
| `HOME`, `XDG_DATA_HOME` | all | Resolve `~/.chorus` and the bridge data dir |

## Exit codes

| Command | 0 | non-zero |
| --- | --- | --- |
| `chorus setup` | Success | I/O error, parse failure |
| `chorus check` | Always | Internal panic only |
| `chorus-server` | Clean shutdown | Port in use, DB error, etc. |
| `chorus bridge` | Clean shutdown | **`2`** = terminal auth error (kicked / token revoked); `1` = other |
| `chorus send` / `status` / `agent` / `channel` / `workspace` | Success | Server unreachable, HTTP error |

The exit-2 contract on `chorus bridge` is for process supervisors:
restart loops should stop on 2 because the token was rotated/kicked and
no amount of retry will recover.

---

## See also

- [`docs/DEV.md`](DEV.md) — local development setup
- [`docs/BACKEND.md`](BACKEND.md) — Rust conventions
- [`docs/BRIDGE.md`](BRIDGE.md) — bridge architecture and troubleshooting
