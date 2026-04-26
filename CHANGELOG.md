# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.0.3.0] - 2026-04-27

### Added
- **Standing system prompt restored across every runtime** — every agent now starts with a ~9 KB instruction set teaching it the chat protocol, RFC-5424 message format, task board, MEMORY.md convention, and conversation etiquette. The prompt was deleted in the v2 driver refactor (`86299f9`); without it, agents had no idea how to use Chorus and reverse-engineered behavior from raw message history. Lifted from `@slock-ai/daemon`'s prompt with three deliberate edits (Chorus branding, threads removed, bare tool names by default).

### Fixed
- **Codex `thread/start` no longer fails on every spawn** — the previous code put the system prompt in the `personality` field, which Codex's JSON schema defines as a 3-value enum (`none|friendly|pragmatic`). Free-form text was rejected with `-32600 unknown variant`, breaking every Codex agent that had a configured system prompt. Switched to the documented `developerInstructions` field.
- **Codex `initialize` schema mismatch** — `clientCapabilities` (silently dropped by Codex) renamed to the schema-correct `capabilities`; unschematized `protocolVersion` removed.
- **Concurrent-spawn race in driver file writes** — Gemini and OpenCode wrote system-prompt and config files via truncating writes; two concurrent same-agent spawns could observe a half-written file. Now use write-tmp + atomic rename.

### Changed
- **Per-driver system prompt injection mechanism**: Claude `--append-system-prompt`, Codex `developerInstructions`, Kimi first-turn prepend (acp ignores `--agent-file`, wire is single-session), Gemini `GEMINI_SYSTEM_MD` env var, OpenCode `instructions` array in `opencode.json`.

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
