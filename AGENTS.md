# Chorus

AI agent collaboration platform. Agents run as OS processes and communicate through a Slack-like chat interface.

This file is the routing table for agents working in this repo. Read it first, follow the pointers.

---

## Principles

1. **The next reader is you, six months from now.** Optimize for recognition, not cleverness.
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

## Skill Routing

All skills prefixed with `/gstack-` (`SKILL_PREFIX=true`).
When a request matches a skill, invoke it as the FIRST action.

| Workflow | Skill | When |
|----------|-------|------|
| Think | `/gstack-office-hours` | Product ideas, brainstorming, "is this worth building" |
| Think | `/gstack-plan-eng-review` | Architecture review |
| Dev | `/gstack-investigate` | Bugs, errors, "why is this broken" |
| Dev | `/gstack-review` | Code review, check my diff |
| Dev | `/gstack-health` | Code quality, health check |
| Design | `/gstack-design-consultation` | Design system, brand |
| Design | `/gstack-design-review` | Visual audit, design polish |
| Release | `/gstack-ship` | Ship, deploy, push, create PR |
| Release | `/gstack-qa` | QA, test the site, find bugs |
| Doc | `/gstack-document-release` | Update docs after shipping |
| Doc | `/gstack-retro` | Weekly retro |
| Util | `/gstack-checkpoint` | Save progress, checkpoint, resume |

Browser: use `/gstack-browse`. Never use `mcp__claude-in-chrome__`* tools.
Discovery: run `/gstack-upgrade` to update skill inventory.

---

## Stats

Rule citation tracking. Rules uncited for 12 months are deletion candidates.

| Rule | Last cited | Count |
|------|-----------|-------|
| Principles §1 — next reader | — | 0 |
| Principles §2 — root cause | 2026-04-11 | 1 |
| Principles §3 — stop and ask | — | 0 |
| `general.md` — code rules | — | 0 |
| `general.md` — errors | 2026-04-11 | 1 |
| `general.md` — verification | — | 0 |
| `general.md` — doc governance | — | 0 |
| `backend.md` | — | 0 |
| `design.md` | — | 0 |

Update this table in the same PR when a rule is cited in a review or commit.
Annual audit: delete any row with 0 citations and last-cited older than 12 months.
