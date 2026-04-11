# Project Knowledge

Institutional knowledge for Chorus. Agents: search this file when debugging
non-obvious behavior, making architecture decisions, or wondering "why is it
this way?" Update it in the same PR that created the knowledge.

---

## Decisions

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
**Decision:** Added a separate `system_prompt` field to `AgentConfig`. Description
stays short for UI; system_prompt carries the full prompt body.
**Rejected:** Overloading description — forces a choice between UI label and prompt quality.

---

## Bugs

### Kimi ACP agent fails on startup: "Session not found"
**Date:** 2026-04-10
**Root cause:** `manager.rs:89` generated a random UUID for all Kimi drivers (raw + ACP).
ACP driver sent `session/load` with that fake UUID. Kimi server rejected it.
**Fix:** `Driver::needs_pregenerated_session_id()` — `src/agent/drivers/mod.rs`,
`src/agent/drivers/kimi_raw.rs`, `src/agent/manager.rs`.
**Lesson:** When adding a new driver mode (ACP vs raw), check whether existing manager
assumptions apply. The manager's session logic was written for raw drivers only.

---

## Facts

- `CLAUDE.md` is a symlink to `AGENTS.md`. Always edit AGENTS.md. (2026-04-11)
- `Store::create_system_message` does NOT fan out to agent receive queues. Agent delivery is a separate path. `src/store/messages/posting.rs` (2026-04-08)
- `AgentConfig.description` is injected verbatim into the system prompt at `src/agent/drivers/prompt.rs:271`. (2026-04-08)
- Rust `std::path::Path` does not expand `~`. Use `dirs::home_dir()` for any CLI flag or env var with tilde. (2026-04-08)

---

## Patterns

### Enum-first driver dispatch
**When:** Adding new driver types or runtime variants.
**Pattern:** Add variant to `AgentRuntime` enum, match exhaustively. The compiler finds every call site.
**Anti-pattern:** `if driver_name == "kimi"` string matching.

### SQL views for read models
**When:** Adding new query endpoints or read-heavy features.
**Pattern:** Create a SQL view, query against it. Write schema stays normalized, read schema is denormalized for the UI.
**Anti-pattern:** Joining 4+ tables in application-layer Rust code.
