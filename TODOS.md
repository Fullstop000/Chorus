# TODOS

Living list of follow-ups that are out of scope for the current branch but worth tracking.

## Design

- [x] **Write `DESIGN.md`** — written to `docs/DESIGN.md` and indexed in `CLAUDE.md`. Covers philosophy, palette, shape rules, typography, layout tokens, global patterns (graph-paper background, translucent app shell, kickers, badges, dividers, embossed buttons, color-hashed avatars, empty states, scrollbar), interaction states (hover invert, focus, disabled), motion (two keyframes), component families, a11y baseline, and a "when you change the design" workflow. Completed 2026-04-10. Reverse-engineered directly from `ui/src/index.css` + `ui/src/App.css` + component CSS files.

## Testing

- [ ] **Audit `src/store/` test coverage** — list every `pub fn` in the store module tree and cross-reference against `tests/store_tests.rs`. Surface all zero-coverage functions.
  - **Why:** During the eng review for the system-message-redesign spec, we discovered `create_system_message` had zero tests despite being in production use (the Launch Trio kickoff path). It was found opportunistically, not systematically. A one-time audit will surface every similar gap.
  - **Pros:** Inventory all untested store paths. Cheap with AI assistance (~10 min with CC). Creates a prioritized list of regression risks.
  - **Cons:** Produces work. May surface gaps in paths you don't care about (dead code, admin-only).
  - **How to start:** `rg "pub fn" src/store/ | cut -d: -f3` to list functions; for each, grep `tests/store_tests.rs` and `tests/e2e_tests.rs` for references. Output a table: function → has_test? → risk.
  - **Depends on:** Nothing. Independent one-shot.
  - **Discovered:** 2026-04-09 during `/gstack-plan-eng-review` of the system-message-redesign spec.

## CLI

- [ ] **`chorus channel leave <name>`** — let users leave a channel they've joined.
  - **Why:** The `channel` subcommand group ships with `create`/`del`/`join`/`list`/`history` but no inverse of `join`. Users who joined by mistake (or want to clean up noisy channels) have no CLI escape hatch today.
  - **Pros:** Closes the obvious symmetry gap. Small scope — a new server endpoint + CLI command.
  - **Cons:** Server-side doesn't yet expose a self-leave endpoint. Need to decide semantics for channel owners and system channels.
  - **How to start:** Add `DELETE /api/channels/{id}/members/{name}` handler (or similar), then a thin CLI wrapper mirroring `join.rs`. Tests: lifecycle (join → leave → list confirms gone).
  - **Depends on:** Nothing.
  - **Discovered:** 2026-04-18 during channel CLI subcommand refactor (spec non-goal).

- [ ] **`chorus channel del` / `join` should find archived channels by name** — `resolve_channel_id` in `src/cli/channel/mod.rs` calls `GET /api/channels` without `include_archived=true`, so archived rows are invisible to the CLI. The server's `handle_delete_channel` accepts archived user channels, but the CLI turns them into "channel not found" and leaves no way to clean up by name.
  - **Why:** Edge case today, but the failure mode is silent and confusing: the channel exists, the server can delete it, yet the CLI claims it doesn't exist.
  - **Pros:** One-line fix (append `?include_archived=true` to the resolver URL).
  - **Cons:** Slightly larger response payloads for the helper. Immaterial at Chorus's scale.
  - **How to start:** Change `let url = format!("{server_url}/api/channels");` in `resolve_channel_id` to include the query param. Add an integration test: create → archive via store → `chorus channel del <name> --yes` succeeds.
  - **Depends on:** Nothing.
  - **Discovered:** 2026-04-18 during Codex review of PR #61.

- [ ] **`chorus channel create` auto-joins the server-side OS user, not the CLI caller** — `handle_create_channel` in `src/server/handlers/channels.rs` calls `whoami::username()` on the server process. When the CLI runs on machine A against a server on machine B (or a daemon under a different account), `create` reports success but the caller isn't a member of their own new channel.
  - **Why:** Today Chorus mostly runs as the same OS user who invokes the CLI, so this hasn't bitten anyone. It will the moment someone runs `chorus serve` as a system daemon or points `--server-url` at a shared host.
  - **Pros:** Removes a foot-gun for distributed setups. Unblocks "CLI against remote server" as a real workflow.
  - **Cons:** Requires the CLI to pass the joining identity, or a new `/api/channels` request field. Touches the server.
  - **How to start:** Option A: extend `CreateChannelRequest` with an optional `joiner` field the CLI fills with `whoami::username()`; server joins that member instead of its own. Option B: CLI chains a `POST /api/channels/{id}/members` after create, mirroring `join`. Option A is cleaner but server-side; B is CLI-only and works today.
  - **Depends on:** Nothing blocking.
  - **Discovered:** 2026-04-18 during Codex review of PR #61.

- [ ] **`chorus channel members <name>`** — list who's in a channel from the CLI.
  - **Why:** Server already exposes `GET /api/channels/{id}/members`. Today only the web UI consumes it. Scripters and debuggers would benefit from CLI access.
  - **Pros:** Zero new server work — the endpoint exists. Small CLI addition.
  - **Cons:** None material.
  - **How to start:** New `src/cli/channel/members.rs` that resolves name→id via the shared helper, GETs `/api/channels/{id}/members`, and prints a formatted list (mirror `status.rs` row style).
  - **Depends on:** Nothing.
  - **Discovered:** 2026-04-18 during channel CLI subcommand refactor (spec non-goal).

## Architecture

- [ ] **Non-atomic team creation handler** — `handle_create_team` performs SQLite writes, filesystem mutations (`init_team`, `init_team_memory`), and agent process restarts without a transaction or rollback mechanism. If a late-stage failure occurs (e.g. agent restart fails), DB records persist while FS state is partial.
  - **Why:** Pre-existing limitation. All Chorus handlers that touch both DB and FS share this pattern. Fixing it requires either (a) SQLite transactions that span filesystem operations (impossible without WAL + external coordination), or (b) a two-phase commit / compensation rollback system.
  - **Pros:** Eliminates partial-state corruption on failure.
  - **Cons:** High complexity. Chorus's local-first, single-user model makes this low-probability.
  - **How to start:** Survey all handlers that touch DB+FS. Design a shared `UnitOfWork` abstraction that stages DB writes and FS mutations, then commits or rolls back atomically.
  - **Depends on:** Nothing blocking.
  - **Discovered:** 2026-04-23 during Gemini CLI review of dogfooding fixes #87–#92.

- [ ] **`whoami::username()` as identity assumption** — Server handlers (`handle_create_team`, `handle_create_channel`, etc.) use `whoami::username()` to determine the caller's identity. This assumes the server runs as the same OS user who performs the action, which breaks for multi-user or hosted deployments.
  - **Why:** Pre-existing architectural decision (local-first design). A proper fix requires passing identity via request context (auth token, session cookie, or explicit request field) through the entire API surface.
  - **Pros:** Unlocks hosted/multi-user Chorus.
  - **Cons:** Touches every server handler, all CLI commands, and the auth subsystem. Massive scope.
  - **How to start:** Design an `Identity` extractor for Axum that resolves from a session token or API key. Migrate one handler at a time.
  - **Depends on:** Auth/session system design.
  - **Discovered:** 2026-04-23 during Gemini CLI review of dogfooding fixes #87–#92.
