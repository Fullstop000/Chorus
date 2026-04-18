# Channel CLI Subcommands — Design

**Date:** 2026-04-18
**Status:** Approved, ready for implementation plan

## Problem

`chorus channel <name>` is a single flat command that creates a channel by writing to the local SQLite store. Users also need to delete, join, list, and read history of channels from the CLI. Today those require either the web UI or cobbled-together commands (`chorus history`, `chorus status`). The CLI is inconsistent: `channel` bypasses the server while `history`/`status`/`send`/`agent` all go through it.

## Goals

1. Unified `chorus channel` subcommand group: `create`, `del`, `join`, `list`, `history`.
2. One transport: all subcommands hit the running Chorus server over HTTP.
3. Consistent UX with existing subcommands (`--server-url`, JSON error surfacing, `tracing::info!` output).
4. No duplicated logic between CLI and server handlers.

## Non-goals

- Leaving a channel (not requested; can be added later via a new server endpoint).
- Archiving (`archive` exists server-side; not part of this CLI surface).
- Offline operation. All subcommands require `chorus start` to be running.

## CLI Surface

```
chorus channel create <name> [--description TEXT] [--server-url URL]
chorus channel del    <name> [--yes] [--server-url URL]
chorus channel join   <name> [--server-url URL]
chorus channel list   [--all] [--server-url URL]
chorus channel history <name> [--limit N] [--server-url URL]
```

- `<name>` is accepted with or without a leading `#`; the server already normalizes via `normalize_channel_name`.
- `--server-url` defaults to `http://localhost:3001`, matching existing subcommands.
- `--limit` for `history` defaults to `20`, matching the current top-level `history` command.
- `list` without `--all` shows only channels the current OS user has joined (`member=<user>` filter). `--all` omits the filter.
- `del` prompts `Delete #<name>? [y/N]` on stdin unless `--yes` is passed. Non-TTY stdin without `--yes` is an error (no silent assume-yes).

## Architecture

New directory `src/cli/channel/` replaces the current `src/cli/channel.rs` file:

```
src/cli/channel/
├── mod.rs       # ChannelCommands enum, dispatch
├── create.rs    # POST /api/channels
├── delete.rs    # resolve name→id via list, then DELETE /api/channels/{id}
├── join.rs      # resolve name→id, POST /api/channels/{id}/members {memberName: $USER}
├── list.rs      # GET /api/channels?member=$USER (or no filter with --all)
└── history.rs   # GET /internal/agent/$USER/history?channel=<name>&limit=<n>
```

### `cli/mod.rs` changes

```rust
/// Manage channels
Channel {
    #[command(subcommand)]
    cmd: ChannelCommands,
},
```

The old `History { channel, limit, server_url }` top-level variant and the `mod history` declaration are removed. `src/cli/history.rs` moves to `src/cli/channel/history.rs` (the body is unchanged — same endpoint and output format).

### `ChannelCommands` enum

`--server-url` is defined once as a **global arg** on the `channel` parent (not repeated on each subcommand), per eng-review finding:

```rust
/// Manage channels
Channel {
    /// Chorus server URL (inherited by all channel subcommands)
    #[arg(long, global = true, default_value = "http://localhost:3001")]
    server_url: String,
    #[command(subcommand)]
    cmd: ChannelCommands,
},

#[derive(Subcommand)]
pub(crate) enum ChannelCommands {
    Create  { name: String, #[arg(long)] description: Option<String> },
    Del     { name: String, #[arg(long)] yes: bool },
    Join    { name: String },
    List    { #[arg(long)] all: bool },
    History { name: String, #[arg(long, default_value = "20")] limit: i64 },
}
```

Dispatch in `cli/mod.rs` destructures both `server_url` and `cmd`, then passes `&server_url` to each subcommand's `run` function.

### Name→ID resolution

`del` and `join` need a channel id (server routes are id-scoped). A small shared helper in `cli/channel/mod.rs`:

```rust
pub(super) async fn resolve_channel_id(client: &reqwest::Client, server_url: &str, name: &str) -> anyhow::Result<String>
```

Calls `GET /api/channels` (no member filter, `include_archived=false`), finds the channel whose normalized name matches the argument, returns its `id`. `anyhow!("channel not found: #{name}")` on miss.

### Data flow per subcommand

| Subcommand | HTTP call                                                       | Output |
| ---        | ---                                                             | ---    |
| `create`   | `POST /api/channels` `{name, description}`                      | `Channel #<name> created.` |
| `del`      | resolve → `DELETE /api/channels/{id}`                           | `Channel #<name> deleted.` |
| `join`     | resolve → `POST /api/channels/{id}/members` `{memberName:$USER}` (see note) | `Joined #<name> as @<user>.` |
| `list`     | `GET /api/channels?member=$USER` (omit `member` if `--all`)     | Aligned table: `#<name>  [joined/not]  <description>` (same format as `status`) |
| `history`  | `GET /internal/agent/$USER/history?channel=<name>&limit=<n>`    | `[<timestamp>] @<sender>: <content>` (unchanged from today) |

> **Note on `join`:** the server exposes only `handle_invite_channel_member` for channel membership writes; there is no dedicated "self-join" endpoint. Chorus runs without auth on localhost, so the invite endpoint functions correctly when the OS user invites themselves. If auth is added later, this path needs revisiting.

### TTY detection

`del` uses `std::io::IsTerminal` (stable since Rust 1.70) on `stdin()` to decide between prompting and refusing. No new dependency.

### Error handling

- Connection refused / DNS failure: print `is the Chorus server running at <server_url>?` and exit 1.
- HTTP 4xx/5xx: parse the standard `ErrorResponse` JSON (`{code, message}`) and surface `<code>: <message>`; exit 1.
- `del`/`join` when name not found in channel list: `channel not found: #<name>`, exit 1.
- `del` without `--yes` on non-TTY stdin: `refusing to delete #<name> without --yes on non-interactive stdin`, exit 1.
- No silent retries, no fallbacks. Match existing CLI conventions.

## Backward compatibility

This is a clean break (pre-1.0):

1. **Removed:** top-level `chorus history <channel>`. Use `chorus channel history <channel>` instead.
2. **Removed:** bare `chorus channel <name>`. Use `chorus channel create <name>` instead.
3. **Semantics changed:** `chorus channel create` now requires a running server (previously wrote SQLite directly). `docs/DEV.md` and any inline help/docs referencing the old forms must be updated in the same PR.

## Testing

Tests required by eng-review; all must land in this PR.

### 1. E2E lifecycle (integration)

Extend `tests/e2e_tests.rs` (or new `tests/channel_cli_tests.rs` if the file is getting large) with a `channel_lifecycle` test:

1. Start the server fixture.
2. Shell out to the built binary: `channel create foo --description "bar"`.
3. `channel list` → asserts `#foo` appears with `joined` status.
4. `channel history foo --limit 5` → asserts no error.
5. `channel del foo --yes` → asserts success.
6. `channel list` → asserts `#foo` gone.

### 2. `del` confirmation-prompt unit tests

Factor the prompt-read logic into a small pure function taking a `BufRead` and a `bool` (is_tty), returning `ConfirmOutcome::{Proceed, Abort, RefuseNonInteractive}`. Test directly — no subprocess needed:

- `is_tty=true`, stdin `"y\n"` → `Proceed`.
- `is_tty=true`, stdin `"n\n"` → `Abort`.
- `is_tty=true`, stdin `"\n"` (empty) → `Abort` (default No).
- `is_tty=false`, any stdin → `RefuseNonInteractive`.

The `run` function composes this with the real `stdin`/`IsTerminal` check.

### 3. Error-path tests (integration)

Two additional shell-out tests in the same file:

- **Name not found:** `channel del nope --yes` against a running fixture → exit code 1, stderr contains `channel not found: #nope`.
- **Server unreachable:** `channel list --server-url http://127.0.0.1:1` (closed port) → exit code 1, stderr contains `is the Chorus server running at`.

### 4. Manual smoke

Before merging: run `chorus channel del foo` in a real terminal; confirm the `[y/N]` prompt renders and `n` aborts without calling the server.

## Out-of-scope follow-ups

- `chorus channel leave` (requires a new server endpoint).
- `chorus channel archive` / `unarchive`.
- `chorus channel members` (list members) — server endpoint exists; command can be added later.
