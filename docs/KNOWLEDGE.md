# Project Knowledge

Knowledge you can't infer from reading the current code. Decisions record
*why* we chose X over Y. Bugs record non-obvious root causes and lessons.
Facts record things that are true but not discoverable from code alone.

If you can figure it out by reading the source, it doesn't belong here.
Update in the same PR that created the knowledge.

---

## Decisions

### Unified task lifecycle (2026-04-25)

Six-state forward-only enum (`proposed | dismissed | todo | in_progress | in_review | done`) on a single `tasks` table. Replaces what would have been a two-table proposal+task split. The proposal is just a task in `status='proposed'`; acceptance is the `→ todo` transition that mints the sub-channel. Owner is a label, not a gate — membership is the only authorization check. Decoupled claim from "start" so the user gets two distinct affordances on the card.

Wire surfaces: ONE `task_card` system message in the parent channel per task (host for the live-rendered card, JSON payload), `task_event` system messages in the sub-channel for post-acceptance actions, plus a cross-channel `TaskUpdateEvent` realtime broadcast so the parent card re-renders even when the viewer is not a member of the sub-channel. The `claimedBy` wire field is preserved verbatim for chat-history compat — in-memory rename to `owner` happens at the parser boundary.

Pointer-vs-truth: source-message snapshot is stored as four `snapshot_*` columns on the task row, with `source_message_id` as a separately nullable FK (`ON DELETE SET NULL`). A schema CHECK enforces all-or-none population on the four snapshot columns. Provenance survives source deletion.

Rejected alternatives: (i) two tables (proposal + task) with a join — would have made the "task is one row" mental model leak through the API; (ii) advancing status on claim — coupled two concepts the UI treats as separate; (iii) renaming `claimedBy` on the wire — would break persisted chat history.

PRs #93 and #96 (incremental v1+v2 proposal flow) were closed and superseded by this unified design. See `docs/plans/2026-04-24-task-lifecycle-unified-*.md`.

### Task = sub-channel (2026-04-22)

A task is a 1:1 child channel (`ChannelType::Task`) of its parent channel. Chosen over the earlier thread-based approach because (a) threads were ripped out in PR #63 and (b) reusing the channel primitive means every existing message / inbox / realtime path works unchanged. Membership tracks task state. Archive on `Done` preserves the trace without cluttering the sidebar. Existing migration backfills sub-channels for pre-existing task rows so the upgrade is seamless.

Navigation uses Zustand state (`currentTaskDetail`), not URL routes — matches the rest of the Chorus UI.

Rejected alternatives: (i) task-as-thread (thread primitive is gone), (ii) task-as-virtual-view (broke read cursors), (iii) URL routing (would require adding react-router, out of scope).

### Rename `AgentSessionHandle` → `Session`, `AttachResult` → `SessionAttachment`
**Date:** 2026-04-20
**Context:** The original names were accurate but verbose. After Tasks 1–10 removed all legacy API surface, the new names (`Session`, `SessionAttachment`) read more naturally at call sites and in doc comments.
**Decision:** Renamed both types globally. The `handle` field on `SessionAttachment` was renamed to `session` for consistency. `ManagedAgent.handle` (a different struct) was intentionally left unchanged — that field is internal and would require a separate behavior change.
**Rejected:** `DriverSession` (would have been the fallback if `Session` collided with another type) — collision check found only `Mcp-Session-Id` HTTP header strings, no type-level conflicts.

### Kimi Bootstrap/Secondary handle path unification
**Date:** 2026-04-20
**Context:** Before the refactor, Kimi's `open_session` distinguished "bootstrap" (first attach) from "secondary" (subsequent attach) with a `HandleRole` enum. Both paths wired up a `KimiHandle` differently and duplicated startup logic. The bootstrap/secondary distinction was an artifact of the pre-`AgentRegistry` era when each driver managed its own `OnceLock<Mutex<HashMap>>`.
**Decision:** Unified via shared `AgentRegistry<KimiAgentCore>`. `open_session` checks the registry: if no live process exists, the next handle becomes bootstrap (spawns the child in `run`); otherwise it multiplexes as secondary. The `HandleRole` enum is preserved for the cases where Claude spawns a per-session child process.
**Rejected:** Keeping per-driver `OnceLock` — it was duplicated ~4 times across claude/codex/kimi/opencode with near-identical eviction logic. `AgentRegistry` centralises this in one place.

### Use ACP session/new for new agents, not pre-generated UUIDs
**Date:** 2026-04-10
**Context:** Manager pre-generated a UUID session_id for all Kimi agents, but ACP
drivers negotiate sessions via session/new. The fake UUID caused session/load to fail.
**Decision:** Added `Driver::needs_pregenerated_session_id()` trait method. Only the
raw Kimi driver returns true. ACP drivers get `None` for new agents.
**Rejected:** Session/load fallback to session/new — masks real errors (auth failures,
network issues). Violates Principle §2.

### Separate system_prompt field from description
**Date:** 2026-04-08
**Context:** `AgentConfig.description` was injected verbatim into the system prompt.
Template-based agents have rich prompts that bloated the brief role description.
**Decision:** Added a separate `system_prompt` field. Description stays short for UI;
system_prompt carries the full prompt body.
**Rejected:** Overloading description — forces a choice between UI label and prompt quality.

---

## Bugs

### Kimi ACP agent fails on startup: "Session not found"
**Date:** 2026-04-10
**Root cause:** Manager generated a random UUID for all Kimi drivers (raw + ACP).
ACP driver sent `session/load` with that fake UUID. Kimi server rejected it.
**Lesson:** When adding a new driver mode (ACP vs raw), check whether existing
manager assumptions apply. The session logic was written for raw drivers only.

---

## Facts

- `CLAUDE.md` is a symlink to `AGENTS.md`. Always edit AGENTS.md. (2026-04-11)
