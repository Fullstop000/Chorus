# Chorus

AI agent collaboration platform. Agents run as OS processes and communicate through a Slack-like chat interface.

---

## Principles

1. **Read before you write.** Read the file, the surrounding code, the existing tests. Never speculate about a bug without reading the relevant code first.
2. **Fix root causes, not symptoms.** No silent fallbacks. Surface errors; the caller decides.
3. **When in doubt, stop and ask.** The human has context you don't. Silent guessing is not an answer.
4. **YAGNI.** Build the smallest thing that satisfies the current goal. No speculative abstractions, no "while I'm here" refactors, no defending against scenarios that can't happen, no flags or shims for futures we haven't committed to. Three similar lines beats a premature abstraction. Delete the layer when the second caller doesn't show up.
5. **No cheating for the goal.** *Hard constraint.* Never fake the result to satisfy a checklist. Don't drive a mechanism manually and claim the agent did it. Don't dictate the payload and claim the model produced it. Don't skip, mock past, or `#[ignore]` a failing test to make it green. Don't merge a PR by working around the very thing the PR is meant to verify. If the goal can't be reached honestly, stop and surface it.

---

## Getting Started

```bash
# Run
cargo run --bin chorus-server -- --port 3001   # platform server (HTTP API + embedded UI)
cargo run --bin chorus -- bridge                # bridge daemon (per-machine, agent runtime host)
cargo run --bin chorus -- setup                 # local CLI (admin actions)
cd ui && npm run dev                            # frontend (proxies to :3001)

# Test
cargo test                             # all Rust tests
cargo test --test e2e_tests            # e2e (message/agent flows)
cd ui && npm run test                  # vitest (all frontend tests)
cd ui && npx tsc --noEmit              # typecheck only

# Build
cargo build                            # backend
cd ui && npm run build                 # frontend production build
```

Use this doc index before touching a subsystem or workflow:

| Doc | Covers | Read Before |
| --- | --- | --- |
| `[docs/DEV.md](docs/DEV.md)` | Setup, prerequisites, run/test/build loops, and local troubleshooting | First local run, environment setup, or when local tooling is acting up |
| `[docs/CLI.md](docs/CLI.md)` | CLI command reference — flags, exit codes, environment variables | Adding or changing a CLI command |
| `[docs/workspace.md](docs/workspace.md)` | Workspace background, restrictions, architecture, and data model | Adding or changing workspace behavior, workspace-scoped resources, or active workspace semantics |
| `[qa/README.md](qa/README.md)` | Authoritative QA SOP: run modes, Playwright workflow, failure classification, evidence handling | Running QA, debugging QA failures, or updating QA process |
| `[qa/QA_CASES.md](qa/QA_CASES.md)` | Static case catalog index and area-by-area case map | Choosing coverage for a change or mapping a failure to an existing case |
| `[docs/BACKEND.md](docs/BACKEND.md)` | Rust — error handling, enums, logging, schema/views, tests, Axum handlers | Any backend change |
| `[docs/DESIGN.md](docs/DESIGN.md)` | Frontend — tokens, typography, components, interaction states, motion, a11y | Any UI change |
| `[docs/INBOX.md](docs/INBOX.md)` | Inbox delivery mechanism — how messages reach agents | Modifying message delivery |
| `[docs/ACP.md](docs/ACP.md)` | Agent Client Protocol — JSON-RPC handshake, session lifecycle | Modifying ACP driver |
| `[docs/DRIVERS.md](docs/DRIVERS.md)` | Runtime drivers and template types | Adding or changing a runtime driver or template type |
| `[docs/BRIDGE.md](docs/BRIDGE.md)` | Shared MCP bridge — architecture, per-runtime MCP config table, discovery file, troubleshooting | Wiring a runtime's MCP transport, debugging bridge connectivity / stale discovery, or auditing the in-process bridge layer |
| `[docs/plan/bridge-platform-protocol.md](docs/plan/bridge-platform-protocol.md)` | Phase 3 bridge ↔ platform wire protocol — `chorus bridge` runtime, frame contract, ownership rules | Running `chorus bridge`, splitting Chorus across machines, or modifying the WS protocol |
| `[docs/KNOWLEDGE.md](docs/KNOWLEDGE.md)` | Decisions, bug postmortems, project facts, patterns | Debugging non-obvious behavior or revisiting architecture choices |
| `[docs/DRIVER_GUIDE.md](docs/DRIVER_GUIDE.md)` | Step-by-step guide for implementing a new driver | Adding a new driver |

---

## Conventions

How we write code. Read the relevant doc from the index above before touching that subsystem.

For UI work, `docs/DESIGN.md` is authoritative. All font choices, colors,
spacing, and aesthetic direction are defined there. Do not deviate without
explicit user approval. In QA mode, flag any code that doesn't match
`docs/DESIGN.md`.

Cross-cutting rules (apply everywhere):

- **Match the neighborhood.** Enum-first types, SQL views for read models, mono chat content, zero-radius UI. Check existing patterns before inventing new ones.
- **Make invalid states unrepresentable.** Enums over booleans. Typed errors over `null`. Required args over optional flags.
- **Names are documentation.** `isLoading` not `loading`. One concept = one word.
- **One thing, done well.** One function = one job. One file = one concept (300 lines = signal, 500 = problem).
- **Fail loudly with context.** Never swallow exceptions. `anyhow!("channel not found: {name}")`. No silent retry logic.
- **Explain why, not what.** Comments justify decisions the code cannot express.
- **Verification matches risk.** Backend → `cargo test`. Data path → `cargo test --test e2e_tests`. UI → `/gstack-qa`.

---

## Chorus Workflows

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


### Issues

When creating GitHub issues, use the matching form in `.github/ISSUE_TEMPLATE/`:

| Form | Use For |
| --- | --- |
| `bug_report.yml` | Broken shipped behavior, regressions, or suspected defects |
| `product_design_request.yml` | Product behavior, UX/design critique, onboarding, copy, and visual refinements |
| `architecture_investigation.yml` | Technical design notes, root-cause writeups, debt, or systemic issues before concrete work is planned |

If an investigation confirms a defect, open or link a Bug Report instead of
continuing to track it only as an architecture note.


Browser: use `/gstack-browse`. Never use `mcp__claude-in-chrome__`* tools.
Run `/gstack-upgrade` to update skill inventory.

---

## Rules for This File

1. **Every rule earns its place by preventing a real problem.** No rule without an incident.
2. **Adding a rule means deleting a weaker one.** Fixed budget. Growth is not progress.
3. **Update in the same PR that made you wish it said something.**


---

## Completion Checklist

Before stopping, confirm:

- [ ] Change lives in correct subsystem and file
- [ ] Verification matches risk of change
- [ ] Required e2e/browser QA run for user-facing critical paths, or gap called out
- [ ] `AGENTS.md` or related docs updated if shipped behavior/workflow changed

## Health Stack

- typecheck: cd ui && tsc --noEmit
- lint: cargo clippy -- -D warnings
- test: cargo test
- test-ui: cd ui && npm run test
- shell: shellcheck dev.sh
