# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
