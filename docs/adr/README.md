# Architecture Decision Records

**Why we chose X over Y.** A chronological log of strategic decisions that
shaped the codebase. Each ADR captures the context, the options
considered, and the reasoning — enough that a future contributor (or
agent) can tell whether the reasoning still applies.

## Files

_(empty — this folder is seeded but holds no records yet. Start writing
them as real decisions get made.)_

## ADR format

Each file is named `NNN-short-title.md` with a zero-padded 3-digit number
starting from 001. The file follows this template:

```markdown
# ADR-NNN: Short title

**Status:** Proposed | Accepted | Superseded by ADR-NNN | Deprecated
**Date:** YYYY-MM-DD

## Context

What situation made this decision necessary? What constraints were real?
Include enough history that a reader in 3 years understands the
environment, not just the outcome.

## Decision

What did we choose? One paragraph, specific enough to be actionable.

## Consequences

What does this decision commit us to? What does it rule out? What
follow-up work does it imply?

## Alternatives considered

What else was on the table? Why was each rejected? Be fair to the
rejected options — the agents of three years from now will need to
evaluate whether the reasoning still holds.
```

## When to write an ADR

- A decision that will outlive its author's memory
- A choice between two reasonable options where the "why" isn't obvious
  from reading the code
- An architectural constraint that would be hard to reverse
- A convention that the code enforces but doesn't explain

## When NOT to write an ADR

- Routine implementation choices covered by `docs/conventions/`
- Bug fixes (those belong in commit messages)
- Personal preferences (those belong in a review thread)
- Decisions that only affect one PR

## Retroactive ADRs

If you find a load-bearing decision in the codebase that never got an
ADR, write one retroactively. Mark it `Status: Accepted (retroactive)`
and note the approximate date. Examples worth writing for Chorus:

- Why read models live in SQL views, not Rust queries
- Why `SenderType` is enum-first (no `match` anywhere in the codebase,
  making additions purely additive)
- Why the UI uses IBM Plex Mono for chat content (intentional
  retro/terminal aesthetic)

Retroactive ADRs are not busywork — they capture tribal knowledge that
would otherwise disappear with the people who made the decisions.
