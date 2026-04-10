# Chorus

AI agent collaboration platform. Agents run as OS processes and communicate through a Slack-like chat interface.

This file is the working contract for agents. Read it before making changes.

---

## General Rules

Listed in priority order. When two rules point different directions, the earlier rule wins.

1. **The next reader is you, six months from now.** Every naming choice, every comment, every commit message is a letter to that person. Optimize for recognition, not cleverness.
2. **Read before you write.** Read the file before editing. Read the surrounding code before inventing a pattern. Read the existing tests before adding a new one. Never speculate about a bug without reading the relevant code first. If the cause is unclear, say so.
3. **Match the neighborhood.** Chorus has strong conventions: enum-first types, SQL views for read models, mono chat content, zero-radius UI, kicker labels, horizontal-rule dividers. Before adding a new pattern, check whether an existing one covers the job. Inconsistency is a tax every future agent pays. See `docs/conventions/` for the written versions.
4. **Make invalid states unrepresentable.** Reach for enums before booleans. Typed errors before `null`. Required arguments before optional flags. `type Status = 'idle' | 'loading' | 'error'` not `{ isLoading: true, hasError: true }`. In Rust: `enum SenderType { Human, Agent, System }`.
5. **Names are documentation.** `isLoading` not `loading`. `hasPermission()` not `checkPermission()`. Booleans read as questions. One concept = one word, don't alternate fetch/get/retrieve/load. Names outlive their authors, so spend the extra minute.
6. **One thing, done well.** One function = one job (5-15 lines ideal, nested conditionals > 2 levels = extract). One file = one concept (300 lines = signal, 500 = problem). One commit = one logical change. One PR = one feature. Guard clauses over deep nesting. Pure functions over side effects. When in doubt, split.
7. **Fail loudly with context.** Never swallow exceptions. Never return `null` for failures. Add one line of context when rethrowing: `anyhow!("channel not found: {name}")`. A silent failure is a future 3am debugging session.
8. **Explain why, not what.** Comments justify decisions the code cannot express. Outdated comments are worse than no comments. Commit messages, PR descriptions, and TODOs all answer "why", never "what". State the bug, show the fix, stop.
9. **Verification matches risk.** Do not claim done without running the matching verification. Backend change -> focused Rust tests. Data path -> `cargo test --test e2e_tests`. User-visible change -> `/gstack-qa`. If verification cannot run, state it clearly; do not claim "fully verified".
10. **When in doubt, stop and ask.** The human has context you don't. "I don't know" is a valid answer. Silent guessing is not. Every agent in this repo overcommits occasionally; the reliable escape hatch is asking.

---

## Project Organization

**Docs (indexed by reader-type, not by topic):**


| Reader-type | Folder              | When to read                                                                                                       |
| ----------- | ------------------- | ------------------------------------------------------------------------------------------------------------------ |
| Code writer | `docs/conventions/` | Before writing new code in any subsystem. Contains `backend.md` (Rust) and `design.md` (frontend visual language). |
| Operator    | `docs/operations/`  | Before running, testing, shipping, or recovering. Contains `development.md`.                                       |
| Mechanic    | `docs/mechanisms/`  | When you need to understand a subsystem deeply enough to modify it. Contains `inbox.md` and will grow.             |
| Extender    | `docs/extensions/`  | When adding a new runtime driver, template type, or plugin. Contains `driver-guide.md`.                            |
| Historian   | `docs/adr/`         | When wondering "why did we choose X over Y" — architecture decision records.                                       |


Each folder has a `README.md` that lists its files with one-line summaries.
Agents follow the two-hop lookup: this table → category README → file.
New docs slot into an existing category without touching this table.

---


## GStack

- **Browser access:** use `/gstack-browse` for all web browsing. **Never** use `mcp__claude-in-chrome__`* tools.
- **Skill prefix:** this project has `SKILL_PREFIX=true`, so invoke skills with the `/gstack-` prefix (`/gstack-qa`, `/gstack-ship`, `/gstack-investigate`, etc.).
- **Discovery:** skill inventory lives in gstack itself. Run `/gstack-upgrade` to keep them current. The routing rules below cover the common entry points — not an exhaustive list.

---

## Completion Checklist

Before stopping, confirm:

- Change lives in correct subsystem and file
- Verification matches risk of change
- Required e2e/browser QA run for user-facing critical paths, or gap called out
- `AGENTS.md` or related docs updated if shipped behavior/workflow changed

---

## Skill routing

When the user's request matches an available skill, ALWAYS invoke it using the Skill
tool as your FIRST action. Do NOT answer directly, do NOT use other tools first.
The skill has specialized workflows that produce better results than ad-hoc answers.

Key routing rules (all prefixed with `/gstack-` — see `SKILL_PREFIX=true` above):

- Product ideas, "is this worth building", brainstorming → `/gstack-office-hours`
- Bugs, errors, "why is this broken", 500 errors → `/gstack-investigate`
- Ship, deploy, push, create PR → `/gstack-ship`
- QA, test the site, find bugs → `/gstack-qa`
- Code review, check my diff → `/gstack-review`
- Update docs after shipping → `/gstack-document-release`
- Weekly retro → `/gstack-retro`
- Design system, brand → `/gstack-design-consultation`
- Visual audit, design polish → `/gstack-design-review`
- Architecture review → `/gstack-plan-eng-review`
- Save progress, checkpoint, resume → `/gstack-checkpoint`
- Code quality, health check → `/gstack-health`

---

## Rules for docs

How the `docs/` tree stays sharp over years instead of collapsing into
noise. Enforced on the annual audit (see "Rules for this file" below).

1. **Index by reader-type, not by topic.** The `docs/` index above has
  five rows (conventions, operations, mechanisms, extensions, adr) and
   never grows. New docs slot into an existing category.
2. **Default to extending an existing doc.** Split a doc only when one of:
  it crosses ~500 lines (same budget as source files), a new reader-type
   emerges (rare — maybe once every two years), or a subsection gets cited
   in reviews more often than its parent doc.
3. **A new doc is indexed in its category's `README.md` in the same PR
  it's born.** A doc that isn't in a README doesn't exist. Orphan docs
   get deleted on the annual audit.
4. **Delete before you add.** Before writing a new doc, check if an
  existing one is obsolete or redundant. Shrink before you grow.
5. **A doc uncited and untouched for 18 months is a deletion candidate.**
  Same pressure as the audit rule for `AGENTS.md`. Annual audit runs it.
6. **A category `README.md` is a table, not a wall of text.** One line
  per file: what it covers, when to read it. If the README starts
   explaining instead of pointing, the docs have failed and need
   reorganizing.

## Rules for this file

1. **Every rule earns its place by preventing a real problem.** If you
  can't cite the incident it came from, it doesn't belong.
2. **Adding a rule means deleting a weaker one.** This file has a fixed
  budget. Growth is not progress.
3. **Update this file in the same PR that made you wish it said
  something.** Drift happens in the gap between "we should document this"
   and "someone should document this".
4. **The owner of this file runs an annual audit.** Read every rule and
  every doc pointer. Delete what's stale. Rewrite what's unclear. If
   you didn't delete anything, you didn't audit carefully enough.
5. **If a rule hasn't been cited in a review in a year, delete it.**
  Rules that go unused are noise. This file should shrink over time,
   not grow.

