# Chorus

AI agent collaboration platform. Agents run as OS processes and communicate through a Slack-like chat interface.

---

## Principles

1. **Read before you write.** Read the file, the surrounding code, the existing tests. Never speculate about a bug without reading the relevant code first.
2. **Fix root causes, not symptoms.** No silent fallbacks. Surface errors; the caller decides.
3. **When in doubt, stop and ask.** The human has context you don't. Silent guessing is not an answer.

---

## Part I — Getting Started

```bash
# Run
cargo run -- serve --port 3001        # backend
cd ui && npm run dev                   # frontend (proxies to :3001)

# Test
cargo test                             # all Rust tests
cargo test --test e2e_tests            # e2e (message/agent flows)
cd ui && npm run test                  # vitest (all frontend tests)
cd ui && npx tsc --noEmit              # typecheck only

# Build
cargo build                            # backend
cd ui && npm run build                 # frontend production build
```

Full setup, prerequisites, and dev workflow: `[docs/DEV.md](docs/DEV.md)`

---

## Part II — Conventions

How we write code. Read the relevant doc before touching that subsystem.


| Doc                                  | Covers                                                                      | Read before        |
| ------------------------------------ | --------------------------------------------------------------------------- | ------------------ |
| `[docs/BACKEND.md](docs/BACKEND.md)` | Rust — error handling, enums, logging, schema/views, tests, Axum handlers   | Any backend change |
| `[docs/DESIGN.md](docs/DESIGN.md)`   | Frontend — tokens, typography, components, interaction states, motion, a11y | Any UI change      |


Cross-cutting rules (apply everywhere):

- **Match the neighborhood.** Enum-first types, SQL views for read models, mono chat content, zero-radius UI. Check existing patterns before inventing new ones.
- **Make invalid states unrepresentable.** Enums over booleans. Typed errors over `null`. Required args over optional flags.
- **Names are documentation.** `isLoading` not `loading`. One concept = one word.
- **One thing, done well.** One function = one job. One file = one concept (300 lines = signal, 500 = problem).
- **Fail loudly with context.** Never swallow exceptions. `anyhow!("channel not found: {name}")`. No silent retry logic.
- **Explain why, not what.** Comments justify decisions the code cannot express.
- **Verification matches risk.** Backend → `cargo test`. Data path → `cargo test --test e2e_tests`. UI → `/gstack-qa`.

---

## Part III — Architecture

Deep knowledge for modifying subsystems.


| Doc                                  | Covers                                                        | Read when                   |
| ------------------------------------ | ------------------------------------------------------------- | --------------------------- |
| `[docs/INBOX.md](docs/INBOX.md)`     | Inbox delivery mechanism — how messages reach agents          | Modifying message delivery  |
| `[docs/ACP.md](docs/ACP.md)`         | Agent Client Protocol — JSON-RPC handshake, session lifecycle | Modifying ACP driver        |
| `[docs/DRIVERS.md](docs/DRIVERS.md)` | Adding a new runtime driver or template type                  | Adding a new runtime        |
| `[docs/KNOWLEDGE.md](docs/KNOWLEDGE.md)` | Decisions, bug postmortems, project facts, patterns      | Debugging non-obvious behavior, architecture choices |


---

## Part IV — Chorus Workflows

All skills prefixed with `/gstack-` (`SKILL_PREFIX=true`).
When a request matches a skill, ALWAYS invoke it using the Skill tool as the FIRST action.
Do NOT answer directly or use other tools first.

### Spec


| Skill                     | When                                                        |
| ------------------------- | ----------------------------------------------------------- |
| `/gstack-office-hours`    | New feature idea, "is this worth building", problem framing |
| `/gstack-plan-eng-review` | Architecture review before implementation                   |
| `/gstack-plan-ceo-review` | Scope challenge, dream state mapping, expansion decisions   |


### Develop


| Skill                                     | When                                                            |
| ----------------------------------------- | --------------------------------------------------------------- |
| `superpowers:executing-plans`             | Implement a plan with review checkpoints                        |
| `superpowers:subagent-driven-development` | Parallel implementation of independent tasks                    |
| `/gstack-investigate`                     | Agent won't start, message not delivered, driver error, any bug |
| `/gstack-review`                          | Code review, check my diff before shipping                      |
| `/gstack-health`                          | Code quality dashboard, test coverage, dead code                |


### Polish


| Skill                         | When                                        |
| ----------------------------- | ------------------------------------------- |
| `/gstack-design-consultation` | Design system, brand, typography, color     |
| `/gstack-design-review`       | Visual audit, spacing issues, design polish |


### Ship


| Skill          | When                                      |
| -------------- | ----------------------------------------- |
| `/gstack-ship` | Create PR, push, deploy                   |
| `/gstack-qa`   | Test the live site, find bugs, verify fix |


### Maintain


| Skill                      | When                                   |
| -------------------------- | -------------------------------------- |
| `/gstack-document-release` | Update docs after shipping             |
| `/gstack-retro`            | Weekly retro, what shipped, what broke |
| `/gstack-checkpoint`       | Save progress, resume later            |
| `/project-memory`          | Record a decision, bug postmortem, fact, or pattern |


Browser: use `/gstack-browse`. Never use `mcp__claude-in-chrome__`* tools.
Run `/gstack-upgrade` to update skill inventory.

---

## Rules for This File

1. **Every rule earns its place by preventing a real problem.** No rule without an incident.
2. **Adding a rule means deleting a weaker one.** Fixed budget. Growth is not progress.
3. **Update in the same PR that made you wish it said something.**
4. **Annual audit.** Read every rule, every doc pointer. Delete what's stale. If you didn't delete anything, you didn't audit.


---

## Extension Points

**Adding A New Driver:**

Follow `docs/DRIVER_GUIDE.md`

---

## GStack

Use the `/gstack-browse` skill from gstack for all web browsing. Never use `mcp__claude-in-chrome__*` tools.

**Available skills:**
`/gstack-office-hours`, `/gstack-plan-ceo-review`, `/gstack-plan-eng-review`, `/gstack-plan-design-review`, `/gstack-design-consultation`, `/gstack-design-shotgun`, `/gstack-design-html`, `/gstack-review`, `/gstack-ship`, `/gstack-land-and-deploy`, `/gstack-canary`, `/gstack-benchmark`, `/gstack-browse`, `/gstack-connect-chrome`, `/gstack-qa`, `/gstack-qa-only`, `/gstack-design-review`, `/gstack-setup-browser-cookies`, `/gstack-setup-deploy`, `/gstack-retro`, `/gstack-investigate`, `/gstack-document-release`, `/gstack-codex`, `/gstack-cso`, `/gstack-autoplan`, `/gstack-plan-devex-review`, `/gstack-devex-review`, `/gstack-careful`, `/gstack-freeze`, `/gstack-guard`, `/gstack-unfreeze`, `/gstack-upgrade`, `/gstack-learn`

---

## Completion Checklist

Before stopping, confirm:

- [ ] Change lives in correct subsystem and file
- [ ] Verification matches risk of change
- [ ] Required e2e/browser QA run for user-facing critical paths, or gap called out
- [ ] `AGENTS.md` or related docs updated if shipped behavior/workflow changed

## Design System
Always read DESIGN.md before making any visual or UI decisions.
All font choices, colors, spacing, and aesthetic direction are defined there.
Do not deviate without explicit user approval.
In QA mode, flag any code that doesn't match DESIGN.md.

## Skill routing

When the user's request matches an available skill, ALWAYS invoke it using the Skill
tool as your FIRST action. Do NOT answer directly, do NOT use other tools first.
The skill has specialized workflows that produce better results than ad-hoc answers.

Key routing rules:
- Product ideas, "is this worth building", brainstorming → invoke office-hours
- Bugs, errors, "why is this broken", 500 errors → invoke investigate
- Ship, deploy, push, create PR → invoke ship
- QA, test the site, find bugs → invoke qa
- Code review, check my diff → invoke review
- Update docs after shipping → invoke document-release
- Weekly retro → invoke retro
- Design system, brand → invoke design-consultation
- Visual audit, design polish → invoke design-review
- Architecture review → invoke plan-eng-review
- Save progress, checkpoint, resume → invoke checkpoint
- Code quality, health check → invoke health
