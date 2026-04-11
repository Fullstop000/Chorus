# Chorus

AI agent collaboration platform. Agents run as OS processes and communicate through a Slack-like chat interface.

This file is the routing table. Read it first, follow the pointers.

---

## Principles

1. **Read before you write.** Read the file, the surrounding code, the existing tests. Never speculate about a bug without reading the relevant code first.
2. **Fix root causes, not symptoms.** No silent fallbacks. Surface errors; the caller decides.
3. **When in doubt, stop and ask.** The human has context you don't. Silent guessing is not an answer.

---

## Docs

| Reader-type | Folder              | When to read                                                   |
| ----------- | ------------------- | -------------------------------------------------------------- |
| Code writer | `docs/conventions/` | Before writing code. `general.md`, `backend.md`, `design.md`. |
| Operator    | `docs/operations/`  | Before running, testing, shipping. `development.md`.           |
| Mechanic    | `docs/mechanisms/`  | When modifying a subsystem. `inbox.md` and growing.            |
| Extender    | `docs/extensions/`  | When adding a driver, template, or plugin. `driver-guide.md`.  |
| Historian   | `docs/adr/`         | When wondering "why did we choose X over Y".                   |

Two-hop lookup: this table → category README → file.

---

## Quick Reference

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

Full setup details: `docs/operations/development.md`.

---

## Chorus Workflows

All skills prefixed with `/gstack-` (`SKILL_PREFIX=true`).
When a request matches a skill, ALWAYS invoke it using the Skill tool as the FIRST action.
Do NOT answer directly or use other tools first. The skill has specialized workflows.

### Spec

| Skill | When |
|-------|------|
| `/gstack-office-hours` | New feature idea, "is this worth building", problem framing |
| `/gstack-plan-eng-review` | Architecture review before implementation |
| `/gstack-plan-ceo-review` | Scope challenge, dream state mapping, expansion decisions |

### Develop

| Skill | When |
|-------|------|
| `superpowers:executing-plans` | Implement a plan with review checkpoints |
| `superpowers:subagent-driven-development` | Parallel implementation of independent tasks |
| `/gstack-investigate` | Agent won't start, message not delivered, driver error, any bug |
| `/gstack-review` | Code review, check my diff before shipping |
| `/gstack-health` | Code quality dashboard, test coverage, dead code |

### Polish

| Skill | When |
|-------|------|
| `/gstack-design-consultation` | Design system, brand, typography, color |
| `/gstack-design-review` | Visual audit, spacing issues, design polish |

### Ship

| Skill | When |
|-------|------|
| `/gstack-ship` | Create PR, push, deploy |
| `/gstack-qa` | Test the live site, find bugs, verify fix |

### Maintain

| Skill | When |
|-------|------|
| `/gstack-document-release` | Update docs after shipping |
| `/gstack-retro` | Weekly retro, what shipped, what broke |
| `/gstack-checkpoint` | Save progress, resume later |

Browser: use `/gstack-browse`. Never use `mcp__claude-in-chrome__`* tools.
Run `/gstack-upgrade` to update skill inventory.

---

## Rules for This File

1. **Every rule earns its place by preventing a real problem.** No rule without an incident.
2. **Adding a rule means deleting a weaker one.** Fixed budget. Growth is not progress.
3. **Update in the same PR that made you wish it said something.**
4. **Annual audit.** Read every rule, every doc pointer. Delete what's stale. If you didn't delete anything, you didn't audit.
