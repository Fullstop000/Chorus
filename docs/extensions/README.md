# Extensions

**How to add new implementations of existing extension points.** Each
file here walks an extender through the full steps of adding one concrete
thing.

| File | Covers | When to read |
|---|---|---|
| `driver-guide.md` | How to add a new agent runtime driver (alongside the existing Claude, Codex, Kimi, etc. drivers in `src/agent/drivers/`) | When adding a new runtime backend |

## Scope

Extension docs are *recipe-shaped*. They assume you know the codebase
in general but not the specific extension point. They list files to
touch, patterns to follow, and verification steps.

For *house-style* conventions see `docs/conventions/`. For the
*internal architecture* of an extension point see
`docs/mechanisms/`. Extension docs sit in between: "here's how to add
one more of these".

## When to add a new file

When a new first-class extension point exists and has had at least two
implementations. Expected future candidates:

- `template-guide.md` — how to author a new agent template for the
  template gallery
- `channel-type-guide.md` — if channel types ever become pluggable
- `mcp-tool-guide.md` — how to add a new MCP tool available to agents

Do not write an extension doc for a one-off addition. One implementation
is a feature; two is a pattern; three deserves a guide.
