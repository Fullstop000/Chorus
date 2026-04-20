# Project Knowledge

Knowledge you can't infer from reading the current code. Decisions record
*why* we chose X over Y. Bugs record non-obvious root causes and lessons.
Facts record things that are true but not discoverable from code alone.

If you can figure it out by reading the source, it doesn't belong here.
Update in the same PR that created the knowledge.

---

## Decisions

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
