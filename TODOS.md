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

- [x] **`chorus channel del` / `join` should find archived channels by name** — `resolve_channel_id` in `src/cli/channel/mod.rs` calls `GET /api/channels` without `include_archived=true`, so archived rows are invisible to the CLI. The server's `handle_delete_channel` accepts archived user channels, but the CLI turns them into "channel not found" and leaves no way to clean up by name.
  - **Why:** Edge case today, but the failure mode is silent and confusing: the channel exists, the server can delete it, yet the CLI claims it doesn't exist.
  - **Pros:** One-line fix (append `?include_archived=true` to the resolver URL).
  - **Cons:** Slightly larger response payloads for the helper. Immaterial at Chorus's scale.
  - **How to start:** Change `let url = format!("{server_url}/api/channels");` in `resolve_channel_id` to include the query param. Add an integration test: create → archive via store → `chorus channel del <name> --yes` succeeds.
  - **Depends on:** Nothing.
  - **Discovered:** 2026-04-18 during Codex review of PR #61.
  - **Fixed:** 2026-04-28 on `fix/channel-cli-archived-members`.

- [x] **`chorus channel create` auto-joins the server-side OS user, not the CLI caller** — fixed by the ID-first human identity foundation on `identity-foundation-id-first` (`559dfb6`) and verified by `/gstack-qa` on 2026-04-26. `handle_create_channel` now joins the configured local human ID from server state instead of calling `whoami::username()` on the server process. Hosted/multi-user caller identity remains tracked under the architecture TODO below.

- [x] **`chorus channel members <name>`** — list who's in a channel from the CLI.
  - **Why:** Server already exposes `GET /api/channels/{id}/members`. Today only the web UI consumes it. Scripters and debuggers would benefit from CLI access.
  - **Pros:** Zero new server work — the endpoint exists. Small CLI addition.
  - **Cons:** None material.
  - **How to start:** New `src/cli/channel/members.rs` that resolves name→id via the shared helper, GETs `/api/channels/{id}/members`, and prints a formatted list (mirror `status.rs` row style).
  - **Depends on:** Nothing.
  - **Discovered:** 2026-04-18 during channel CLI subcommand refactor (spec non-goal).
  - **Fixed:** 2026-04-28 on `fix/channel-cli-archived-members`.

## Decision Inbox follow-ups

- [x] **Codex/opencode `--resume` liveness guard.** Both sides landed on the `driver-resilience` branch (PR #134, 2026-05-02). Codex: `codex_thread_file` walks `~/.codex/sessions/<year>/<month>/<day>/rollout-*-<thread_id>.jsonl` newest-first (capped 7 days), drops resume on miss. Opencode: `opencode_session_file` checks `~/.local/share/opencode/storage/session_diff/<session_id>.json` via the new `AcpDriverConfig::session_liveness_check` hook. Both mirror the claude `claude_session_file` pattern from PR #131. Verified end-to-end on codex by deleting the rollout and confirming the agent recovers with a fresh thread instead of silently no-op'ing. Kimi + gemini still set `None` until we hit stale-session there.

- [ ] **rmcp `StreamableHttpService` session TTL or re-init.** Long agent turns (>20 min, e.g. gemini deep-thinking on a PR review) cause the bridge MCP session to expire mid-turn; the agent's eventual `dispatch_decision` returns `Unauthorized: Session not found` at the transport layer even though the payload was valid. Either raise the TTL on the bridge side or detect expiry and re-init the session. Discovered 2026-05-01 during decision-inbox dogfood.

- [ ] **Coverage gaps in v0.0.7.0 decision-inbox.** Bridge-side `validate_decision_payload` has 12 untested branches (length caps, duplicate keys, recommended_key not in options). `AgentManager::resume_with_prompt` real impl (Active/Asleep branching) is only covered through `MockLifecycle`. The resume-failure → revert path in `handle_resolve_decision` (the whole reason CAS+revert exists) has no e2e test. UI `DecisionsInbox.tsx` has zero tests. Add unit tests for the validator and a `MockLifecycle` failure-path test for revert. Discovered 2026-05-01 in /ship coverage audit.

## Architecture

- [ ] **Non-atomic team creation handler** — `handle_create_team` performs SQLite writes, filesystem mutations (`init_team`, `init_team_memory`), and agent process restarts without a transaction or rollback mechanism. If a late-stage failure occurs (e.g. agent restart fails), DB records persist while FS state is partial.
  - **Why:** Pre-existing limitation. All Chorus handlers that touch both DB and FS share this pattern. Fixing it requires either (a) SQLite transactions that span filesystem operations (impossible without WAL + external coordination), or (b) a two-phase commit / compensation rollback system.
  - **Pros:** Eliminates partial-state corruption on failure.
  - **Cons:** High complexity. Chorus's local-first, single-user model makes this low-probability.
  - **How to start:** Survey all handlers that touch DB+FS. Design a shared `UnitOfWork` abstraction that stages DB writes and FS mutations, then commits or rolls back atomically.
  - **Depends on:** Nothing blocking.
  - **Discovered:** 2026-04-23 during Gemini CLI review of dogfooding fixes #87–#92.

- [ ] **Server-local identity fallback before auth/session identity** — public handlers now use the configured local human ID from server state instead of `whoami::username()`, but Chorus still does not resolve a distinct caller identity per request. This is correct for the current local-first mode and remains insufficient for hosted or multi-user deployments.
  - **Why:** Hosted/multi-user Chorus needs request-scoped identity from auth/session context, not one server-local human.
  - **Pros:** Unlocks hosted/multi-user Chorus and remote CLI/API clients with distinct human identities.
  - **Cons:** Touches every public server handler, all CLI commands that need caller identity, and the auth subsystem. Massive scope.
  - **How to start:** Design an `Identity` extractor for Axum that resolves from a session token or API key. Migrate one handler at a time from `AppState.local_human_id` to request-scoped identity after auth/session support lands.
  - **Depends on:** Auth/session system design.
  - **Discovered:** 2026-04-23 during Gemini CLI review of dogfooding fixes #87–#92. Updated 2026-04-26 after ID-first human identity foundation QA.

- [x] **Restore standing system prompt across all runtimes** — restored on `feat/restore-system-prompt` (commits `a356d1d`..`c29215d`, 2026-04-27). The standing prompt was deleted in `86299f9` (v2 RuntimeDriver refactor) and Codex's per-driver injection was actively broken (sending free-form text into the 3-value `personality` enum, returning -32600 on every `thread/start`). Rewires every shipping driver — Claude `--append-system-prompt`, Codex `developerInstructions`, Kimi first-turn prepend, Gemini `GEMINI_SYSTEM_MD` env var, OpenCode `instructions` array — and folds in two pre-existing Codex schema fixes (`clientCapabilities`→`capabilities`, drop unschematized `protocolVersion`). 472 unit tests + integration suites pass; clippy clean.
