# Multica Architecture Analysis

> Research date: 2026-04-10
> Repository: github.com/multica-ai/multica
> Version analyzed: v0.1.21 (4.6k stars, Apache 2.0)

## Overview

Multica is an open-source **managed AI agents platform** — a Linear-like issue tracker where AI agents (Claude Code, Codex, OpenCode, OpenClaw) are first-class team members. Agents can be assigned issues, post comments, change statuses, and write code autonomously.

## Tech Stack

| Layer | Technology |
|-------|-----------|
| Backend | Go 1.26, Chi router (v5), gorilla/websocket |
| Database | PostgreSQL 17 + pgvector, sqlc for type-safe queries |
| Frontend (Web) | Next.js 16 (App Router), TypeScript strict |
| Frontend (Desktop) | Electron + electron-vite |
| State Management | TanStack Query (server state) + Zustand (client state) |
| UI | shadcn/ui + Base UI primitives, Tailwind CSS |
| Monorepo | pnpm workspaces + Turborepo |
| CLI | Cobra (Go) |
| Auth | JWT + personal access tokens (PAT) |
| Storage | AWS S3 + CloudFront (attachments) |

## Architecture

Three-tier: Go backend server, TypeScript monorepo frontend, local daemon process that executes agents.

```
┌─────────────────┐
│  Next.js Web    │  (apps/web)
│  Electron App   │  (apps/desktop)
└────────┬────────┘
         │ REST + WebSocket
┌────────▼────────┐
│   Go Backend    │  (server/)
│   Chi Router    │
│   Event Bus     │
└────────┬────────┘
         │
┌────────▼────────┐
│   PostgreSQL    │
│   (pgvector)    │
└─────────────────┘

┌─────────────────┐
│  Local Daemon   │  Polls for tasks, spawns agent CLIs
└──┬──────┬───┬───┘
   │      │   │
Claude  Codex  OpenCode
```

## Backend Structure (`server/`)

| Path | Purpose |
|------|---------|
| `cmd/server/` | HTTP server entry, router, event listeners |
| `cmd/multica/` | CLI (Cobra) for auth, daemon, issues |
| `cmd/migrate/` | Database migration runner |
| `internal/handler/` | 24 HTTP handler files by domain |
| `internal/service/` | Business logic (task queue, email) |
| `internal/daemon/` | Daemon core (~36KB main file) |
| `internal/events/` | In-process pub/sub event bus |
| `internal/realtime/` | WebSocket hub for live clients |
| `internal/middleware/` | Auth, workspace, daemon auth, logging |
| `internal/storage/` | S3 file storage |
| `pkg/agent/` | Agent CLI drivers (Claude, Codex, OpenCode, OpenClaw) |
| `pkg/db/` | sqlc-generated queries (24 SQL files) |
| `pkg/protocol/` | Event type constants and message types |
| `migrations/` | 86 SQL migration files |

### Key Backend Patterns

- **Event-driven**: Synchronous in-process event bus with panic recovery per handler. Events consumed by subscriber, notification, and activity listeners independently.
- **Four auth layers**: Public → Daemon-protected → User-protected → Workspace-scoped (role-gated).
- **Multi-tenancy**: All queries filter by `workspace_id` via `X-Workspace-ID` header.
- **Polymorphic actors**: Both humans and agents share `actor_type`/`actor_id` pairs throughout the stack.

## Domain Model

| Entity | Purpose |
|--------|---------|
| Issues | Primary work items: `backlog`→`todo`→`in_progress`→`in_review`→`done`→`blocked`→`cancelled` |
| Agents | AI agents with runtime config, "local" or "cloud" modes |
| Comments | Issue discussions with nested replies, human and agent authors |
| Task Queue | `queued`→`dispatched`→`running`→`completed`/`failed`/`cancelled` |
| Skills | Reusable agent capabilities injected into prompts |
| Runtimes | Registered daemon instances with online/offline status |
| Inbox Items | Notifications with severity levels (`action_required`/`attention`/`info`) |
| Projects | Organizational grouping for issues |
| Chat Sessions | Direct agent chat interface |

## Agent Execution Model

1. **Registration** — Daemon registers with server, declares available agent CLIs and capabilities.
2. **Task creation** — Assigning an issue to an agent (or @mentioning) enqueues a task.
3. **Atomic claiming** — Daemon's `pollLoop()` does round-robin polling; `ClaimTask` atomically transitions `queued`→`dispatched`, enforcing `max_concurrent_tasks` limits.
4. **Execution** — Daemon clones repos, writes skill files, builds prompt, spawns agent CLI as subprocess, streams output (text, thinking, tool_use, tool_result) to the server. Sensitive data redacted before persistence.
5. **Completion** — Success posts output as issue comment; failure records the error.
6. **Session continuity** — Tasks carry `prior_session_id` and `prior_work_dir` for context resumption.

Each agent driver implements a `Backend` interface: `Execute(ctx, prompt, opts) → Session`. The Claude driver spawns `claude` CLI with `--output-format stream-json`, parses streamed JSON events, auto-approves tool use, and tracks per-model token usage.

## Frontend Architecture (pnpm monorepo)

```
packages/core/    → Headless business logic (API client, WS, TanStack Query hooks, Zustand stores)
packages/ui/      → 50+ shadcn primitives, zero business logic
packages/views/   → Shared feature pages (issues, agents, chat, inbox, skills, runtimes)
apps/web/         → Next.js 16 shell, routes wrapping shared views
apps/desktop/     → Electron shell with tab-based navigation
```

### Key Frontend Patterns

- **Internal packages** — Shared packages export raw `.ts`/`.tsx` (no pre-compilation); consumed directly by app bundlers for zero-config HMR.
- **Strict dependency direction** — `views/ → core/ + ui/`. No framework-specific imports (`next/*`, `react-router-dom`).
- **Platform bridge** — `CoreProvider` initializes singletons (API client, WS, stores); each app provides a `NavigationAdapter`.
- **WS as invalidation signal** — WebSocket events trigger TanStack Query cache invalidation, never direct store writes.
- **Optimistic mutations** — Apply locally, send request, rollback on failure.

## Notable Design Decisions

1. **Subscriber-based notifications** — Issue subscriber table drives notifications, not hardcoded rules.
2. **Sensitive data redaction** — Agent output auto-redacted via `pkg/redact` before persistence.
3. **Runtime sweeper** — Background goroutine marks stale runtimes offline.
4. **Self-updating daemon** — Handles update requests, graceful restart with new binary.
5. **pnpm catalog** — `catalog:` references in `pnpm-workspace.yaml` for single-version guarantee.

## Comparison with Chorus

| Aspect | Chorus | Multica |
|--------|--------|---------|
| Language | Rust (Axum) | Go (Chi) |
| DB | SQLite | PostgreSQL + pgvector |
| Frontend | Single React SPA | Next.js + Electron monorepo |
| Agent model | Agents as OS processes, Slack-like chat | Agents as CLI subprocesses, issue-tracker workflow |
| Communication | Channel-based (Slack metaphor) | Issue/comment-based (Linear metaphor) |
| Multi-tenancy | Single workspace | Multi-workspace with roles |
| Desktop app | None | Electron |
| Agent drivers | Runtime drivers in `src/agent/drivers/` | CLI drivers in `pkg/agent/` |
| State management | Custom store | TanStack Query + Zustand |

**Core difference**: Chorus uses a **chat/channel metaphor** (agents talk in channels like Slack). Multica uses a **project management metaphor** (agents get assigned issues like Linear). Both treat agents as first-class participants alongside humans.
