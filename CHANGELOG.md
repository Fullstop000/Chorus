# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.0.4.0] - 2026-04-28

### Added
- **Member-joined chips in chat** — when someone joins a channel, the human chat now shows a centered `[actor] joined [target]` row with clickable name and channel chips, replacing the plain hairline divider. Routes to the actor's profile or the target channel on click.
- **Structural system-notice primitive** — a single `SystemNotice` component renders any `[actor] [verb] [target?]`-shaped event so future kinds (task_claimed, agent_started, …) work the day they ship without changing the renderer.

### Changed
- **One column for all structured system payloads** — `messages.notice` is renamed to `messages.payload`, and task-event JSON moves out of `content` into the same column. `content` is now always a human-readable English fallback. Two encodings became one.
- **Audience-based agent filter** — agents stop seeing ambient channel-state markers (member-joined chips) via a structural `payload.audience != 'humans'` SQL check, not a kind allowlist. Operational events like task creation/claim still flow to agents.
- **Frontend payload type collapses** to a loose `MessagePayload` (`{kind, [k]: unknown}`) with runtime narrowing in each renderer, instead of a fixed `Notice` interface that locked in one shape.

### Fixed
- **Auto-join system message on agent creation** — moved out of inner helper so the join always emits a chip in `#all`.

### Removed
- **`format_message_for_agent` bridge formatter** — agents now read `content` directly because producers always write a clean English sentence. Less indirection, no JSON-in-content reparsing.

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
