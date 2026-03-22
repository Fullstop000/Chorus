# TODOS

## Knowledge Store Follow-ups

### `mcp_chat_forget(id)` tool

**What:** Add a `forget` MCP tool that lets an agent delete a knowledge entry it previously stored.

**Why:** Agents may store intermediate findings that become stale or incorrect after a task completes. Without `forget`, incorrect memory persists and may mislead future agents.

**Pros:** Completes the memory lifecycle (remember → recall → forget). Simple: one `DELETE FROM shared_knowledge WHERE id = ? AND author_agent_id = ?` query, one bridge tool, one test.

**Cons:** Agents need to remember the ID returned by `remember`. Could be extended to support forget-by-key for convenience.

**Context:** `mcp_chat_remember` returns a knowledge ID. `forget` takes that ID. Authorization: only the original author_agent_id can delete (already in schema). Start here: `src/bridge.rs` (add tool), `store/knowledge.rs` (add `delete_knowledge` method), `tests/server_tests.rs` (add test).

**Depends on:** Knowledge store (`remember`/`recall`) must be shipped first.

---

### Knowledge TTL / expiry

**What:** Periodic cleanup of shared_knowledge entries — either by age (delete entries older than N days) or by count (keep last M entries per agent, circular buffer).

**Why:** `shared_knowledge` grows unbounded. After 100+ entries, `#shared-memory` channel becomes noisy and FTS5 recall results include stale data.

**Pros:** Keeps the memory store focused and performant at scale. `#shared-memory` channel remains readable.

**Cons:** May delete entries agents still rely on. TTL needs to be long enough to survive multi-day tasks.

**Context:** `shared_knowledge` has `created_at TEXT`. A simple background task or a `LIMIT`-based cleanup on write is enough for v1. Could also expose a `forget_older_than(days)` tool for agents. Start here: `store/knowledge.rs` — add a `prune_knowledge(agent_id, max_entries)` helper called after each `remember`.

**Depends on:** Knowledge store shipped first.

---

### `list_server` knowledge count

**What:** Add a `Shared Knowledge: N entries` line to the `list_server` MCP tool response.

**Why:** Agents currently have no way to know whether the shared memory store is populated before deciding whether to call `mcp_chat_recall`. Showing the count in `list_server` lets an agent make an informed decision at the start of its turn.

**Pros:** One extra `SELECT COUNT(*)` query in `get_server_info`. Zero new infrastructure. Immediately useful for the two-agent handoff demo.

**Cons:** None significant.

**Context:** `get_server_info` in `src/store/mod.rs` already builds `ServerInfo`. Add a `knowledge_count: u64` field, query it, and include it in the `list_server` formatter in `src/bridge.rs`. Update `ServerInfo` in `src/models.rs`.

**Depends on:** Knowledge store shipped first.
