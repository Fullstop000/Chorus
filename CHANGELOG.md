# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.0.12.0] - 2026-05-17

### Changed
- **URL-driven navigation.** Channels, agents, tasks, settings, and inbox now live at routes (`/c/:channel`, `/c/:channel/tasks/:n`, `/dm/:agent`, `/agent/:agent/{profile,activity,workspace}`, `/inbox`, `/settings/:section`). Deep links, browser back/forward, reload-restores-view, and bookmarks all work. Names (not ids) appear in URLs.

## [0.0.11.0] - 2026-05-13

### Added
- **Dev auth provider for remote installs.** `CHORUS_DEV_AUTH=1 CHORUS_DEV_AUTH_USERS=zht chorus serve` mounts `POST /api/auth/dev-login`, finds-or-creates an allowlisted user, and returns the same `chorus_sid` cookie `local-session` does — without the loopback gate. Designed for solo operators on access-controlled hosts (GCP VM, homelab) so the UI is reachable from a non-loopback browser without standing up OAuth. Refuses to start with an empty allowlist; logs a WARN + shows a non-dismissible yellow banner; `/health` reports `dev_auth: true`.
- **User-scoped bridge tokens.** `provider='bridge', machine_id=NULL` row shape — one bearer per user, shared across that user's machines. Each machine registers in a new `bridge_machines` table on first `bridge.hello`.
- **Settings → Devices page.** Mints the user's bridge token (one-shot reveal — bearer never stored raw, shown once at first-mint), lists onboarded devices with active/offline/kicked status, supports Kick (mark disconnected, reject reconnect) / Forget (hard-delete) / Rotate (revoke + remint, kicks every device).
- **Zero-arg `chorus bridge`.** Reads `$XDG_DATA_HOME/chorus/bridge/bridge-credentials.toml` (host + token), derives WS/HTTP URLs from `host`, auto-detects `machine_id` from hostname with random fallback. Two new WS close codes 4004 `kicked` + 4005 `token_revoked` → bridge exits non-zero with an actionable message instead of looping in backoff.

### Changed
- `GET /api/devices` returns `{ has_token, devices[] }` (envelope, not bare array) so the UI can tell "no token yet" apart from "token exists but no machine has connected."
- `chorus bridge` CLI surface shrank from 6 args to 1: `--data-dir <path>` (optional). Host + token come from the credentials file, not flags.

### Migration
- Existing data dirs from <0.0.11.0 must wipe `~/.chorus` and re-run `chorus setup`. The new `provider` column on `api_tokens` is NOT NULL with no default.

## [0.0.10.0] - 2026-05-13

### Added
- **Identity & auth redesign (#157).** `users` + `accounts` + `sessions` + `api_tokens` replaces the `humans`-keyed identity. CLI uses bearer tokens via `credentials.toml`; browser bootstraps a `chorus_sid` cookie via loopback-only `POST /api/auth/local-session`; bridges share `api_tokens` with machine-bound rows in `bridge-credentials.toml`.
- `chorus login --local` and `chorus logout` for token rotation.

### Changed (breaking)
- CLI commands that hit the server now require credentials. `chorus setup` writes them; automation can set `CHORUS_TOKEN`.
- `CHORUS_BRIDGE_TOKENS` env path replaced by `bridge-credentials.toml` (or `CHORUS_BRIDGE_TOKEN`).
- Local-mode UI is loopback-only — remote browsers can no longer hit a local install.

### Migration
- Existing data dirs from <0.0.10.0 cannot upgrade in place. Wipe `~/.chorus` and re-run `chorus setup`.

## [0.0.9.0] - 2026-05-07

### Added
- **Cross-process Chorus.** `chorus bridge --platform-ws ws://host:3001/api/bridge/ws --platform-http http://host:3001 --machine-id <id>` runs agent runtimes on a different machine from the platform. The bridge dials in over a WebSocket, reconciles desired agents from `bridge.target`, hosts an embedded MCP bridge for local agent tool-calls, and tunnels chat + lifecycle back to the platform.
- **Bearer-token auth on the bridge WS.** Configure `CHORUS_BRIDGE_TOKENS="token-1:machine-alpha,token-2:machine-beta"`. With tokens set, `/api/bridge/ws` requires `Authorization: Bearer <token>` and pins each token to one `machine_id`. With no tokens, auth is disabled (loopback default unchanged).
- **Bridge ownership on agents.** `POST /api/agents` accepts `machineId`; matching bridges get the agent in their next `bridge.target`. Bridge-hosted agents are no longer started by the platform — the bridge owns the runtime.

### Changed
- Standing system prompt is shorter — removed the codex-only "your process stays alive across turns" note; the universal prompt already covers it.

## [0.0.8.0] - 2026-05-03

### Fixed
- Codex, opencode, kimi, and gemini agents no longer silently no-op when their resume target is gone. Each driver now checks the runtime's session storage before issuing `thread/resume` or `session/load`, and falls back to a fresh session with a warning if the file is missing — same pattern claude got in v0.0.7.0.

### Added
- Run-completion logs include `tool_calls=N`. A run that finishes `Natural` with `tool_calls=0` is now logged at `warn!` with a likely-cause hint instead of looking identical to a normal turn end.

## [0.0.7.0] - 2026-05-01

### Added
- **Decision Inbox.** Agents can now hand a verdict request to the human via `dispatch_decision` (PR review, A-vs-B choices, config flags). Human picks an option in the sidebar inbox; the agent's session resumes with the picked option's body and acts on it.

### Fixed
- Claude `--resume` no longer errors silently when the session file is missing locally. The driver checks the file and falls back to a fresh session with a warning.

## [0.0.6.0] - 2026-04-30

### Fixed
- Agent creation is now non-blocking. Previously, creating an agent with Codex or ACP-native runtimes blocked the HTTP response for 500ms–3s while the server waited for driver handshake round-trips. The agent now appears in the sidebar immediately and starts up in the background. (#127)

## [0.0.5.0] - 2026-04-28

### Added
- New agents introduce themselves in `#all` on creation. (#108)

## [0.0.4.0] - 2026-04-28

### Added
- **Member joined chips in chat** — when someone joins a channel, you see a clean `[name] joined #channel` row with clickable chips, instead of a plain divider.

### Changed
- **System messages share one structured format** so future event types (task claimed, agent started, …) render as chips with no extra plumbing.

## [0.0.3.0] - 2026-04-27

### Added
- **Standing system prompt across every runtime** — agents now start knowing the chat protocol, message format, task board, and MEMORY.md convention.

### Fixed
- **Codex agents now start successfully** — previously every `thread/start` was rejected by Codex because the system prompt was sent as the wrong field type.

## [0.0.2.0] - 2026-04-24

### Added
- **Empty-run warnings** — when an agent completes a run without sending a message, the system now posts a diagnostic warning to the channel. This helps users identify auth failures, runtime errors, or other silent failures instead of staring at silence. (fixes #97)

## [0.0.1.0] - 2026-04-23

### Fixed
- **Setup no longer hangs indefinitely** — auth probes (`gemini auth status`, etc.) now use async `tokio::process::Command` with a 5-second timeout instead of blocking `std::process::Command`.
- **Channel delete alias** — `chorus channel delete <name>` now works alongside `chorus channel del <name>`.
- **Status shows system channels** — `chorus status` now iterates `system_channels` so channels like `#all` are visible.
- **Clean CLI errors** — mundane mistakes (e.g., forgetting `--yes` on `channel del`) now print a one-line error without a full backtrace, via the new `UserError` type.
- **Strict channel name validation** — channel names are now validated against an allow-list (`[a-z0-9_-]+`) on both the server and CLI, preventing routing bugs from spaces and special characters.
- **Empty messages rejected** — sending empty or whitespace-only messages now returns `400 Bad Request`.
- **Claude headless spurious WARN** — the `"status"` system event subtype is now ignored alongside `"hook_started"` and `"hook_response"`.
- **Template frontmatter log noise** — YAML parse failures in external user templates are now logged at `info!` instead of `warn!`.

### Changed
- **Simpler `start --help` wording** — removed confusing `serve` alias reference.
- **Vite upgraded** — `vite` dev dependency bumped from `5.4.2` to `6.4.2` (transitively upgrades `esbuild`), resolving `npm audit` vulnerabilities.

### Added
- **`bridge-smoke-test` CLI command** — restored smoke test for the shared MCP bridge.
