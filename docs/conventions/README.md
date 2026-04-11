# Conventions

**How we write code here.** Read the relevant file before touching code in
that subsystem. Update it in the same PR when you introduce a new pattern.

| File | Covers | When to read |
|---|---|---|
| `general.md` | Cross-cutting rules — naming, structure, errors, verification, doc governance, completion checklist | Before any change |
| `backend.md` | Rust conventions — error handling, enum-first types, logging, schema/views read model, test layout, Axum handlers | Before any backend change |
| `design.md` | Frontend visual language — tokens, typography, component families, interaction states, motion, accessibility | Before any UI change |

## Scope

This folder holds *code-writing* conventions: how to name things, how to
structure modules, which patterns to prefer. Each file here is authoritative
for its subsystem.

For *how to run* the code see `docs/operations/`. For *how a subsystem
works internally* see `docs/mechanisms/`. For *why* a specific design was
chosen see `docs/adr/`.

## When to add a new file

Only when a new reader-type emerges for code writers. Expected future
candidates:

- `frontend.md` — React/state/hooks conventions when they diverge from the
  visual `design.md` concerns enough to deserve a separate doc
- `testing.md` — test strategy and conventions when they outgrow the
  Testing sections in `backend.md`

Do not add a file here for a single subsystem's quirk. That belongs in
`docs/mechanisms/` or inline in the code.
