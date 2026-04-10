# Operations

**How to run, test, ship, and recover this thing.** Task-shaped docs for
the operator (often the same person as the code writer, but wearing a
different hat).

| File | Covers | When to read |
|---|---|---|
| `development.md` | Prerequisites, run locally (backend + Vite proxy), testing (Rust + vitest + `/gstack-qa`), killing/restarting the server, branch and commit workflow, verification policy, troubleshooting | Before running Chorus locally. Start here if you're new. |

## Scope

This folder holds *task-shaped* docs. Each file answers "how do I do X
right now". They are not tutorials; they are checklists and commands.

For *how to write code* see `docs/conventions/`. For *how a subsystem
works* see `docs/mechanisms/`.

## When to add a new file

When a new operational task emerges that doesn't fit into `development.md`
and is cited in more than one review. Expected future candidates:

- `deploy.md` — when Chorus has a deployment target
- `release.md` — when version cadence formalizes beyond manual /ship runs
- `incident.md` — fire-drill playbook, once there are enough shared
  incidents to have a playbook worth writing

Do not fragment `development.md` just because it's growing. Split only
when a new reader-type emerges (e.g., release engineer vs developer).
