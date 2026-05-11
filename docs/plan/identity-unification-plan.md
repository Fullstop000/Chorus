# Identity Unification (Issue #144) — Plan

Goal: collapse the duplicated "who is the local human?" resolution paths and the
cached-name drift surface into one well-defined story, **without** adopting the
larger `Actor` reframing proposed in the comment thread until a real second case
forces it.

## Problem (verified from code)

1. **Two resolvers can produce different identities for the same `(config, DB)`
   input.**
   - `ensure_setup_local_human` (`src/cli/setup.rs:521`) — four-step fallback,
     **writes** to both store and `config.toml`.
   - `resolve_local_human_identity` (`src/server/mod.rs:340`) — three-step
     fallback, returns `(id, name)`, does **not** update config, panics on
     create failure.
   - Concrete divergence scenarios:
     - `cfg.local_human.id = Some("human_abc")` but DB row missing →
       setup falls through to configured_name, creates `human_xyz`, **rewrites
       cfg to the new id**. Server, with the same inputs, picks
       `get_humans().pop()` (`name` ORDER BY name) or creates a fresh row with
       `whoami::username()`, and **does not update config**. Next setup run
       sees the now-stale id and recreates again.
     - `cfg.local_human.id = None`, DB has multiple humans → setup picks
       `into_iter().next()` (alphabetically first). Server picks `humans.pop()`
       (alphabetically last). Different humans.

2. **`LocalHumanConfig` caches `name` alongside `id`.** The `humans` table
   already owns the name (`humans.name TEXT NOT NULL UNIQUE`). The config copy
   is a denormalized state column that can drift on rename and adds no behavior
   we can't read from the DB. Per the project memory *"question state-columns
   aggressively"*, this is a smell.

3. **`UserError` is a CLI surface error misnamed.** Its only call sites are in
   `src/cli/{agent,channel,send}.rs` and the `main.rs:6` downcast that
   distinguishes user-friendly CLI failures from internal errors. It is not
   "user" in any of the senses the comment thread enumerates.

4. **Comment language is inconsistent.** Mix of "human user" and "user" where
   "human" alone is meant. Minor but real cognitive tax.

## The deeper reframing — and why I'm not landing it now

The comment proposes lifting the existing `SenderType { Human, Agent, System }`
into a full `Actor` abstraction, and migrating `workspaces.created_by_human_id`
to `(created_by_id, created_by_type)` so agents can be workspace creators.

`SenderType` already does this structurally — it's the same three variants
under a different name. The proposal is therefore:
- A **rename** with cross-cutting touch (every `sender_type` JSON key the
  frontend reads, every store method that builds `SenderType` values, every
  test fixture). No behavior change.
- Plus a **schema migration** for workspace ownership to support agent-created
  workspaces — a feature we don't ship today and have no concrete pull for.

Per YAGNI and *"name for what it is today, not what it might become"* (project
memory), I'm deferring both. Revisit when:
- A second creator role for workspaces lands (agent-spawned workspaces, scripted
  workspace setup with a service identity, etc.). At that point the migration
  pays for itself.
- Three or more "who did this" call sites accumulate friction from the
  `SenderType` name. Today the name reads fine in context.

This decision is explicitly called out in the issue close-out (see "Issue close
plan" below) so it doesn't get re-litigated silently.

## What lands (one PR)

### 1. One identity module with two intentional entry points

New file `src/store/local_identity.rs` (alongside `humans.rs`). Two functions
sharing internals, because **setup and serve legitimately have different
policies** — setup is a bootstrap/repair command and may write; serve must not
silently bootstrap.

```rust
pub struct LocalIdentity {
    pub id: String,
    pub name: String,
}

impl LocalIdentity {
    /// Bootstrap/repair path. Used by `chorus setup` only.
    ///
    /// May seed a `humans` row and rewrite `cfg.local_human.id`.
    /// Resolution order:
    ///   1. cfg.local_human.id present + row exists  → use it.
    ///   2. cfg.local_human.id present + row missing → seed a fresh row,
    ///      rewrite cfg with the new id (tracing::warn the recovery).
    ///   3. cfg.local_human.id absent + DB has exactly one human → adopt it,
    ///      persist its id to cfg.
    ///   4. cfg.local_human.id absent + DB empty → create_local_human(hint),
    ///      persist id.
    ///   5. cfg.local_human.id absent + DB has >1 human → fail. Caller must
    ///      pass a hint or the user must edit cfg.
    pub fn resolve_or_seed(
        store: &Store,
        cfg: &mut ChorusConfig,
        default_name_hint: &str,
    ) -> anyhow::Result<Self>;

    /// Runtime path. Used by `chorus serve` only.
    ///
    /// Read-only against the existing identity; never seeds, never writes cfg.
    /// Fails loudly if cfg and DB disagree — that's an operator-visible
    /// inconsistency, not something `serve` should paper over.
    pub fn resolve_strict(
        store: &Store,
        cfg: &ChorusConfig,
    ) -> anyhow::Result<Self>;
}
```

Behavior changes vs today:
- `serve` no longer silently creates a new human row when config is empty.
  It fails with a hint pointing the operator at `chorus setup`. This is the
  *"no silent fallbacks that mask real errors"* line in the user's feedback
  memory.
- `serve` no longer silently picks among multiple humans by alphabetical order
  when `cfg.local_human.id` is missing. Same fail-loud treatment.
- `setup` collapses its four-step ladder into the order above and writes the
  authoritative id to cfg in every branch (it already does, but now uniformly).

These are behavior changes. They are intentional and the right side of the
*root-cause-fixes / no silent fallbacks* rule. Call them out in the PR
description.

### 2. Drop `name` from `LocalHumanConfig`

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct LocalHumanConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}
```

- Legacy `config.toml` files with `name = "..."` still parse cleanly (serde
  ignores unknown fields; `deny_unknown_fields` is not set on this struct).
- All display-side reads go through `Store::get_human_by_id` and use
  `humans.name` directly. There's one such read today in
  `src/server/handlers/mod.rs:216` — adjust to call the store.
- Removes the drift mechanism entirely.

### 3. Rename `UserError` → `CliError`

- Move type from `src/cli/mod.rs:28`.
- Update `main.rs:6` downcast.
- Update ~10 construction sites under `src/cli/`.
- Pure rename; no behavior change. Belongs in this PR because the doc comment
  for the type currently reads "User error", which is part of the same
  terminology mess the issue lists.

### 4. Comment language sweep

- `Human` doc comment in `src/store/humans.rs:9`: "Registered human user" →
  "Registered human."
- `SenderType::Human` doc in `src/store/messages/types.rs:14`: "Human user row"
  → "Human row".
- Any `// human user` / `// user` (when referring to humans) in code comments
  under `src/` — find/replace pass.
- Reserve the word "user" for CLI/UI operator context (e.g. the `prompt_for_*`
  helpers in `setup.rs`, error messages addressed to whoever is typing the
  command). One word, one meaning.

This sweep is ~30 minutes of mechanical edits and is in scope because it is
exactly what the issue body item #1 lists ("Terminology drift").

## Tests

Add `tests/local_identity_tests.rs` (or extend `tests/store_tests.rs`):

| Case | Expected |
| --- | --- |
| `resolve_or_seed` with valid configured id | returns that human; cfg unchanged |
| `resolve_or_seed` with stale id, no DB row | seeds new row, rewrites cfg |
| `resolve_or_seed` no id, one human in DB | adopts, writes id to cfg |
| `resolve_or_seed` no id, empty DB | creates human using hint, writes id |
| `resolve_or_seed` no id, multiple humans | returns Err with disambiguation hint |
| `resolve_strict` with valid id matching DB row | succeeds |
| `resolve_strict` with stale id (no row) | returns Err pointing at `chorus setup` |
| `resolve_strict` with no id, DB has rows | returns Err — strict mode never adopts |
| `resolve_strict` with empty DB | returns Err — strict mode never seeds |

## Verification matrix

- `cargo test` (unit + store tests)
- `cargo test --test e2e_tests` — exercises setup → serve flow against a real
  store; the unified resolver must keep that flow green.
- Manual smoke: `rm -rf ~/.chorus && cargo run -- setup` then
  `cargo run -- serve --port 3001` and confirm `/api/whoami` returns the id
  written into `~/.chorus/config.toml`.
- Manual regression: hand-edit `~/.chorus/config.toml` to clear `local_human.id`,
  then `cargo run -- serve` and confirm it fails with the expected hint
  instead of silently generating a new identity.

## Migration / compat notes

- **No DB migration.** Schema is unchanged.
- **Config compat:** old `local_human.name = "..."` lines are silently dropped
  on next save (the field is gone, serde won't emit it). The id field is
  preserved.
- **Behavior compat:** `chorus serve` against a config with no `local_human.id`
  will fail where it used to silently seed. This is intentional. Document in
  the PR description and in `docs/CLI.md` if that file currently advertises the
  serve-without-setup recovery path (it doesn't, last I checked, but verify).

## What this plan does NOT do (deferred)

| Deferred | Why |
| --- | --- |
| `enum Actor { Human, Agent, System }` rename of `SenderType` | Rename only; touches dozens of files and the wire/JSON contract. No functional payoff today. |
| `workspaces.created_by_human_id` → `(created_by_id, created_by_type)` | No incoming requirement for agent-created workspaces. Schema is honest about today's reality. |
| Unifying terminology in user-facing docs beyond code comments | Stays in scope for a follow-up doc PR if anyone hits a real confusion. The code-comment sweep is the high-ROI part. |
| `fetch_local_human_identity`'s `UserError` instances → `CliError` | Caught automatically by the rename. Not a separate item. |

## Issue close plan

When this PR lands:
- Mark issue #144 items 1, 3, 4, 5 from the comment's action list as done
  (rename, drop name, unify resolvers, comment sweep).
- Leave items 2 (workspace `Actor` migration) and 6 (broader doc sweep) open
  with a comment quoting the YAGNI reasoning above. Don't close the issue —
  keep it as the tracking ticket if a real second case appears.

---

## Open decisions from eng-review (2026-05-12)

The eng review flagged these — they need user calls before implementation
begins.

### Must-fix

- **A1 — `resolve_strict` policy on "cfg lost, DB intact"**
  - Current plan: fail loud.
  - Review recommendation: adopt-and-warn when DB has exactly one
    `auth_provider='local'` row and cfg.id is empty; fail only on ambiguity
    or empty DB. The "no silent fallbacks" rule is about masking errors,
    not blocking recoveries from recoverable state.
  - Status: **open**.

- **A3 — `LocalIdentity` vs existing `LocalHumanIdentity`**
  - Current plan: new struct.
  - Review recommendation: reuse `LocalHumanIdentity` (`src/cli/mod.rs:272`),
    hoist to `src/store/local_identity.rs`. Don't add a third id+name struct.
  - Status: **open**.

- **Q2 — `src/server/handlers/mod.rs:213-218` consumes `cfg.local_human.name`**
  - Current plan: handwave ("adjust to call the store").
  - Required: explicit migration (replace `.zip(cfg.local_human.name)` with
    a `state.store.get_human_by_id(&id)?` lookup) + regression test that
    `/internal/system-info.local_human` still populates after the field drop.
  - Status: **open**.

### Should-fix

- **A2 — Filter on `auth_provider='local'` throughout the resolver**
  - Adds `Store::get_local_humans()`. One-line filter today; removes a
    known cloud-rollout landmine.
  - Status: **open**.

- **Q1 — Free functions instead of `LocalIdentity::{resolve_or_seed, resolve_strict}`**
  - Methods imply "two views of one operation"; the two are different
    operations sharing a primitive that's already on `Store`.
  - Status: **open**.

- **Test gaps (T1–T5)** to fold in:
  - T1 [REGRESSION] cfg.id wiped, DB intact (depends on A1's resolution).
  - T2 [E2E] setup → serve identity continuity.
  - T3 [REGRESSION] `/system-info.local_human` after handler migration.
  - T4 [REGRESSION] `UserError → CliError` downcast in `main.rs:6`.
  - T5 [COMPAT] Old `config.toml` with `name = "..."` still parses.

### Suggested commit sequence (revised)

1. `feat(store): add local identity resolver primitives` — new module + tests
   (incl. T1–T5).
2. `refactor(cli): chorus setup uses ensure_local_identity`.
3. `refactor(server): chorus serve uses resolve_local_identity + update
   system-info handler` (covers Q2).
4. `refactor(config): drop LocalHumanConfig.name` (depends on 3).
5. `refactor(cli): rename UserError to CliError`.
6. `chore: comment sweep on human/user terminology`.

## GSTACK REVIEW REPORT

| Review | Trigger | Why | Runs | Status | Findings |
|--------|---------|-----|------|--------|----------|
| Eng Review | `/plan-eng-review` | Architecture & tests | 1 | issues_open | 6 issues (3 P1, 2 P2, 1 P3); 5 test gaps (1 critical regression); 2 critical failure-mode gaps |
| CEO Review | `/plan-ceo-review` | Scope & strategy | 0 | — | — |
| Design Review | `/plan-design-review` | UI/UX | 0 | n/a | non-UI plan |
| Outside Voice | codex / claude | Independent challenge | 0 | — | not run (single-reviewer scope) |

- **UNRESOLVED:** 6 open decisions (A1, A2, A3, Q1, Q2 + test-gap fold-in).
- **VERDICT:** ENG REVIEW found issues — resolve A1, A3, Q2 before implementation.

