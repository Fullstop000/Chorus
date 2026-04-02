# Chorus

AI agent collaboration platform. Agents run as OS processes and communicate through a Slack-like chat interface.

This file is the working contract for agents. Read it before making changes.

---

## General Rules

- Simplest working solution. No over-engineering.
- No abstractions for single-use operations.
- No speculative features or "you might also want..."
- Read the file before modifying it. Never edit blind.

| Rule                       | Example                                                              |
| -------------------------- | -------------------------------------------------------------------- |
| Names are documentation    | `isLoading` not `loading`; `hasPermission()` not `checkPermission()` |
| Name for the reason, not the control | `isRuntimeAvailable` not `canSelectRuntime`              |
| One word per concept       | Don't alternate fetch/get/retrieve/load                              |
| Booleans read as questions | `isLoading`, `hasError`, `canSubmit`                                 |
| No cryptic shortcuts       | `id`, `url`, `err` ok in narrow scopes only                          |
| Functions do one thing     | Ideal: 5-15 lines. Nested conditionals > 2 levels = extract          |
| Max 3 arguments            | Use options object: `createUser({ name, age, role })`                |
| Return early               | Guard clauses > deep nesting                                         |
| Pure functions preferred   | Isolate I/O, mutations, randomness at edges                          |

**Structure:**

- Organize by feature: `src/auth/`, `src/billing/` — not `src/models/`, `src/controllers/`
- Files over ~300 lines = refactor signal. Over 500 lines = problem.
- UI contains no SQL. Business logic contains no display strings.

**State:**

- Minimize mutable state. Prefer `const` over `let`.
- Colocate state with consumers. No global state unless truly shared.
- Make invalid states unrepresentable: `type Status = 'idle' | 'loading' | 'error'` not `{ isLoading: true, hasError: true }`

**Error Handling:**

- Fail fast and loudly. Never swallow exceptions.
- Add context when re-throwing: `throw new Error(\`Failed to load user ${userId}: ${e.message}\`)`
- Use typed errors, not `null` for failures.

**Testing:**

- Follow Arrange–Act–Assert (AAA).
- Test behavior, not implementation.
- One assertion per test (ideally).

**Debugging:**

- Never speculate about a bug without reading the relevant code first.
- State what you found, where, and the fix. One pass.
- If cause is unclear: say so. Do not guess.

** Review **

- State the bug. Show the fix. Stop.
- No suggestions beyond the scope of the review.
- No compliments on the code before or after the review.

**Comments:**

- Explain _why_, not _what_.
- Outdated comments are worse than no comments.

> **Meta-Principle:** Code is written once, read hundreds of times. Optimize for the next reader (often you, six months from now).

---

## Project Organization

**Backend:**
| Path | Purpose |
|------|---------|
| `src/main.rs` | CLI entrypoint, `serve` bootstrap |
| `src/lib.rs` | Crate-level module exports |
| `src/agent/` | Agent lifecycle, process management, activity log, collaboration, workspace. Runtime drivers: `src/agent/drivers/` |
| `src/bridge/` | MCP bridge, request/response formatting |
| `src/server/` | Axum router assembly in `mod.rs`. Handlers grouped by domain under `src/server/handlers/` |
| `src/store/` | SQLite persistence. Modules: `agents`, `channels`, `messages`, `tasks`, `teams` |

**Frontend:**
| Path | Purpose |
|------|---------|
| `ui/src/App.tsx` | Top-level shell |
| `ui/src/api.ts` | Browser-to-server API calls |
| `ui/src/store.tsx` | Client app state and selection logic |
| `ui/src/hooks/` | Reusable data-loading and interaction hooks |
| `ui/src/components/` | UI grouped by panel, modal, component responsibility |
| `ui/src/channelList.ts`, `types.ts` | Shared UI-side derivation and types |

## Project Conventions

**UI:**

- Component styles in co-located `.css` files
- Design tokens in CSS variables in `App.css`
- Icons: `lucide-react` (13px inline, 16px panel)
- No global state mutations outside `ui/src/store.tsx`
- API calls through `ui/src/api.ts`
- Do not introduce a second visual style for shared dialogs, forms, or selects
- Do not separate labels from their focusable controls
- Do not use the browser viewport for read visibility

**Logging:**

- `RUST_LOG=chorus=debug` for verbose output
- Use `tracing`; never `eprintln!` or `println!` in library code

---

## Dev Workflow

**Branch Workflow:**

1. Check worktree dirty before switching branches
2. If local changes exist, ask user to commit/stash/move
3. Start from up-to-date `main` based on `origin/main`
4. Create branch with `{agent}/` prefix (`codex/`, `claude/`, `gemini/`, etc.)
5. Don't carry unrelated changes into new branch

**Commits:**

- Use conventional style with scope: `feat(settings):`, `fix(command):`, `refactor(config):`, `docs(agent):`, `ci:`

---

## Verification Policy

Do not claim complete without matching verification.

**Minimum:**

1. Run focused Rust tests for affected modules
2. Run `cargo test --test e2e_tests` when backend message/task/DM/thread/agent flow affected
3. For user-facing changes, run browser QA pass in `qa/README.md`

**Escalation:**

- Backend/data-path changes: Rust tests first
- Core user process changes: headless-browser e2e testing mandatory
- Core paths: channel messaging, DM flows, thread replies, task board, agent loops
- Backend tests alone insufficient for user-visible changes
- If headless-browser verification cannot run, state it clearly; don't claim fully verified

---

## QA Workflow

Authoritative workflow in `qa/README.md`. Case catalog and templates under `qa/`.

---

## Extension Points

**Adding A New Driver:**

Follow `docs/DRIVER_GUIDE.md`

---

## Completion Checklist

Before stopping, confirm:

- [ ] Change lives in correct subsystem and file
- [ ] Verification matches risk of change
- [ ] Required e2e/browser QA run for user-facing critical paths, or gap called out
- [ ] `AGENTS.md` or related docs updated if shipped behavior/workflow changed
