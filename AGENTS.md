# Chorus

AI agent collaboration platform. Agents run as OS processes and communicate through a Slack-like chat interface.

---

## Principles

1. **Read before you write.** Read the file, the surrounding code, the existing tests. Never speculate about a bug without reading the relevant code first.
2. **Fix root causes, not symptoms.** No silent fallbacks. Surface errors; the caller decides.
3. **When in doubt, stop and ask.** The human has context you don't. Silent guessing is not an answer.
4. **YAGNI.** Build the smallest thing that satisfies the current goal. No speculative abstractions, no "while I'm here" refactors, no defending against scenarios that can't happen, no flags or shims for futures we haven't committed to. Three similar lines beats a premature abstraction. Delete the layer when the second caller doesn't show up.
5. **No cheating for the goal.** *Hard constraint.* Never fake the result to satisfy a checklist. Don't drive a mechanism manually and claim the agent did it. Don't dictate the payload and claim the model produced it. Don't skip, mock past, or `#[ignore]` a failing test to make it green. Don't merge a PR by working around the very thing the PR is meant to verify. If the goal can't be reached honestly, stop and surface it.
6. **Make invalid states unrepresentable.** Enums over booleans. Typed errors over `null`. Required args over optional flags.
7. **Names are documentation.** `isLoading` not `loading`. One concept = one word.
8. **One thing, done well.** One function = one job. One file = one concept (300 lines = signal, 500 = problem).
9. **Explain why, not what.** Comments justify decisions the code cannot express.
10. **Verification matches risk.** Backend → `cargo test`. Data path → `cargo test --test e2e_tests`. UI → `/gstack-qa`.

---

## Getting Started

```bash
# Run
cargo run --bin chorus-server -- --port 3001   # platform server (HTTP API + embedded UI)
cargo run --bin chorus -- bridge                # bridge daemon (per-machine, agent runtime host)
cargo run --bin chorus -- setup                 # local CLI (admin actions)
cd ui && npm run dev                            # frontend (proxies to :3001)

# Verify
cargo clippy --all-targets -- -D warnings       # rust lint
cargo test                                      # all Rust tests
cargo test --test e2e_tests                     # e2e (message/agent flows)
cd ui && npx tsc --noEmit                       # ui typecheck
cd ui && npm run test                           # vitest (all frontend tests)
shellcheck dev.sh                               # shell lint

# Build
cargo build                                     # backend
cd ui && npm run build                          # frontend production build
```

Use this doc index before touching a subsystem or workflow:

| Doc | Covers | Read Before |
| --- | --- | --- |
| [docs/DEV.md](docs/DEV.md) | Setup, prerequisites, run/test/build loops, and local troubleshooting | First local run, environment setup, or when local tooling is acting up |
| [docs/CLI.md](docs/CLI.md) | CLI command reference — flags, exit codes, environment variables | Adding or changing a CLI command |
| [docs/workspace.md](docs/workspace.md) | Workspace background, restrictions, architecture, and data model | Adding or changing workspace behavior, workspace-scoped resources, or active workspace semantics |
| [qa/README.md](qa/README.md) | Authoritative QA SOP: run modes, Playwright workflow, failure classification, evidence handling | Running QA, debugging QA failures, or updating QA process |
| [qa/QA_CASES.md](qa/QA_CASES.md) | Static case catalog index and area-by-area case map | Choosing coverage for a change or mapping a failure to an existing case |
| [docs/BACKEND.md](docs/BACKEND.md) | Rust — error handling, enums, logging, schema/views, tests, Axum handlers | Any backend change |
| [docs/DESIGN.md](docs/DESIGN.md) | Frontend — tokens, typography, components, interaction states, motion, a11y | Any UI change |
| [docs/INBOX.md](docs/INBOX.md) | Inbox delivery mechanism — how messages reach agents | Modifying message delivery |
| [docs/ACP.md](docs/ACP.md) | Agent Client Protocol — JSON-RPC handshake, session lifecycle | Modifying ACP driver |
| [docs/DRIVERS.md](docs/DRIVERS.md) | Runtime driver API + step-by-step guide for adding a new driver | Adding or changing a runtime driver |
| [docs/BRIDGE.md](docs/BRIDGE.md) | Shared MCP bridge — architecture, per-runtime MCP config table, discovery file, troubleshooting | Wiring a runtime's MCP transport, debugging bridge connectivity / stale discovery, or auditing the in-process bridge layer |
| [docs/KNOWLEDGE.md](docs/KNOWLEDGE.md) | Decisions, bug postmortems, project facts, patterns | Debugging non-obvious behavior or revisiting architecture choices |

---

## Completion Checklist

Before stopping, confirm:

- [ ] Change lives in correct subsystem and file
- [ ] Verification matches risk of change
- [ ] Required e2e/browser QA run for user-facing critical paths, or gap called out
- [ ] `AGENTS.md` or related docs updated if shipped behavior/workflow changed

---

## Rules for This File

1. **Every rule earns its place by preventing a real problem.** No rule without an incident.
2. **Adding a rule means deleting a weaker one.** Fixed budget. Growth is not progress.
