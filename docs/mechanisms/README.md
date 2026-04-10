# Mechanisms

**How subsystems actually work under the hood.** Deep-dive explanations
for when you need to modify a system, not just use it.

| File | Covers | When to read |
|---|---|---|
| `inbox.md` | How unread counts, read cursors, and the `inbox_conversation_state_view` SQL view fit together | Before changing anything in `src/store/inbox.rs` or the inbox view in `schema.sql` |

## Scope

Mechanism docs explain the *why* and the *shape* of a subsystem. They are
written for a reader who needs to change how it works, not just interact
with it. They go deeper than `conventions/` (which is about house style)
and narrower than `adr/` (which records strategic choices).

**Test for "does this belong here?"** — would a new contributor need this
file to confidently change the subsystem's behavior? If yes, write it
here. If the answer is "read the code", don't write a mechanism doc;
improve the code comments instead.

## When to add a new file

When a subsystem has enough non-obvious behavior that reading the code
isn't enough. Expected future candidates:

- `agent-lifecycle.md` — how agents start, stop, auto-restore, and
  communicate with their bridges
- `stream-events.md` — WebSocket event bus, payload shapes, sequence
  numbers, read cursors
- `bridge-protocol.md` — MCP bridge internals, request/response framing
- `task-board.md` — task state machine and claim/release flow

Name files after the subsystem, not the feature. A mechanism doc
outlives a feature name.
