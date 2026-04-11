# General Conventions

Cross-cutting rules for all code in Chorus. Backend-specific rules live in
`backend.md`, visual rules in `design.md`. This file covers what applies
everywhere.

---

## Code

1. **Match the neighborhood.** Chorus has strong conventions: enum-first types, SQL views for read models, mono chat content, zero-radius UI, kicker labels, horizontal-rule dividers. Before adding a new pattern, check whether an existing one covers the job. Inconsistency is a tax every future agent pays. See `docs/conventions/` for the written versions.
2. **Make invalid states unrepresentable.** Reach for enums before booleans. Typed errors before `null`. Required arguments before optional flags. `type Status = 'idle' | 'loading' | 'error'` not `{ isLoading: true, hasError: true }`. In Rust: `enum SenderType { Human, Agent, System }`.
3. **Names are documentation.** `isLoading` not `loading`. `hasPermission()` not `checkPermission()`. Booleans read as questions. One concept = one word, don't alternate fetch/get/retrieve/load. Names outlive their authors, so spend the extra minute.
4. **One thing, done well.** One function = one job (5-15 lines ideal, nested conditionals > 2 levels = extract). One file = one concept (300 lines = signal, 500 = problem). One commit = one logical change. One PR = one feature. Guard clauses over deep nesting. Pure functions over side effects. When in doubt, split.

## Errors

5. **Fail loudly with context.** Never swallow exceptions. Never return `null` for failures. Add one line of context when rethrowing: `anyhow!("channel not found: {name}")`. A silent failure is a future 3am debugging session. Fix root causes, not symptoms. No silent fallback/retry logic that masks real errors. Surface the error; the caller decides.

## Communication

6. **Explain why, not what.** Comments justify decisions the code cannot express. Outdated comments are worse than no comments. Commit messages, PR descriptions, and TODOs all answer "why", never "what". State the bug, show the fix, stop.

## Verification

7. **Verification matches risk.** Do not claim done without running the matching verification. Backend change -> focused Rust tests. Data path -> `cargo test --test e2e_tests`. User-visible change -> `/gstack-qa`. If verification cannot run, state it clearly; do not claim "fully verified".

## Completion Checklist

Before stopping, confirm:

- Change lives in correct subsystem and file
- Verification matches risk of change
- Required e2e/browser QA run for user-facing critical paths, or gap called out
- `AGENTS.md` or related docs updated if shipped behavior/workflow changed

---

## Doc Governance

How the `docs/` tree stays sharp over years.

1. **Index by reader-type, not by topic.** The `docs/` index has five rows (conventions, operations, mechanisms, extensions, adr) and never grows. New docs slot into an existing category.
2. **Default to extending an existing doc.** Split only when it crosses ~500 lines, a new reader-type emerges, or a subsection gets cited more often than its parent.
3. **A new doc is indexed in its category's `README.md` in the same PR it's born.** A doc that isn't in a README doesn't exist.
4. **Delete before you add.** Check if an existing doc is obsolete or redundant. Shrink before you grow.
5. **A doc uncited and untouched for 18 months is a deletion candidate.**
6. **A category `README.md` is a table, not a wall of text.** One line per file: what it covers, when to read it.
