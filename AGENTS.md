# Chorus

Chorus is an AI agent collaboration platform. Agents run as real OS processes and communicate through a Slack-like chat interface.

This file is the working contract for agents in this repository. Read it before making changes, and keep it aligned with shipped behavior.

## Code Principles

### 1. Naming

**Names are the primary documentation.**
A well-named function or variable eliminates the need for a comment. If you need a comment to explain what a name means, rename it.

**Use domain language consistently.**
Pick one word per concept across the entire codebase. Don't alternate between `fetch`/`get`/`retrieve`/`load` for the same operation. Establish a shared vocabulary with your team.

**Booleans should read as questions.**
`isLoading`, `hasError`, `canSubmit` — not `loading`, `error`, `submit`. This also applies to boolean-returning functions: `isEmpty()`, `hasPermission()`.

**Avoid abbreviations and cryptic shortcuts.**
Code is read far more than it is written. Saving keystrokes now creates cognitive overhead forever. Exceptions: universally understood shorthands (`i`, `j`, `id`, `url`, `err`) are fine in narrow scopes.

--

### 2. Structure & Modularity

**Single Responsibility Principle (SRP).**
Every module, class, and function should have exactly one reason to change. If you describe it with "and", split it. A 500-line file almost certainly violates this.

**Organize by feature, not by type.**
Group code by what it does together, not by what kind of code it is.

```
# Avoid (for larger apps)
src/models/  src/controllers/  src/views/

# Prefer
src/auth/  src/billing/  src/dashboard/
```

**Keep files short and scannable.**
Files over ~300 lines are a signal to refactor. Files over 500 lines are almost always a problem. Short files are easier to review, test, and understand.

**Enforce clear layer boundaries.**
Define distinct layers (presentation, business logic, data access) and enforce that dependencies only flow in one direction. UI should not contain SQL. Business logic should not format display strings.

---

### 3. Functions & Methods

**Functions should do one thing.**
A function that does one thing is easy to name, easy to test, and easy to reuse. The ideal function is 5–15 lines. Nested conditionals deeper than 2 levels almost always need extraction.

**Limit function arguments to 3 or fewer.**
Functions with many parameters are hard to call, hard to test, and a sign the function does too much. Use an options object when you need more context.

```js
// Bad — hard to read at call site
createUser("Alice", 30, "admin", true, "UTC");

// Good — self-documenting
createUser({ name: "Alice", age: 30, role: "admin" });
```

**Prefer pure functions wherever possible.**
Pure functions (same input → same output, no side effects) are trivial to test, reason about, and reuse. Isolate side effects (I/O, mutations, randomness) at the edges of your system.

**Return early to reduce nesting.**
Guard clauses at the top of a function eliminate deep indentation and make the happy path obvious.

```js
// Deep nesting (avoid)
if (user) { if (user.active) { if (hasPermission) { ... } } }

// Guard clauses (prefer)
if (!user) return;
if (!user.active) return;
if (!hasPermission) return;
// happy path here
```

---

### 4. State & Data Flow

**Minimize mutable state.**
Every piece of mutable state is a potential bug. Prefer immutable data and derive values from a single source of truth. Prefer `const` over `let`; prefer derived values over stored copies.

**Colocate state with its consumers.**
State should live as close as possible to the code that uses it. Avoid hoisting state globally unless truly shared. Global state is a shared mutable dependency — the hardest kind to reason about.

**Make invalid states unrepresentable.**
Design data models so impossible situations can't exist in the type system. Prevent bugs structurally, not defensively.

```ts
// Bad: both can be true simultaneously
{ isLoading: true, hasError: true }

// Good: mutually exclusive states
type Status = 'idle' | 'loading' | 'error' | 'success';
```

---

### 5. Dependencies & Coupling

**Depend on abstractions, not concretions.**
Code should depend on interfaces and contracts, not on specific implementations (Dependency Inversion Principle). Inject dependencies; don't hardcode them.

**Keep coupling loose, cohesion high.**
Modules that change together should live together. Modules that don't depend on each other shouldn't know about each other.

**Treat third-party libraries as risks.**
Wrap external dependencies behind thin adapters. This decouples your code from library internals and makes migration painless. Don't scatter calls to a logging or HTTP library across 80 files — create a thin abstraction; change it in one place.

---

### 6. Error Handling

**Fail fast and fail loudly.**
An error that surfaces immediately is far easier to debug than one that propagates silently. Never swallow exceptions. Catch errors at the right boundary, handle them meaningfully.

**Make error paths as clear as happy paths.**
Every function that can fail should communicate that clearly. Avoid returning `null` for failures — it's impossible to tell if `null` means "not found" or "something broke." Use typed errors or Result types.

**Add context when re-throwing.**
Stack traces alone are rarely enough for production debugging. Always add context.

```js
// Bad
} catch (e) { throw e; }

// Good
} catch (e) {
  throw new Error(`Failed to load user ${userId}: ${e.message}`);
}
```

---

### 7. Testing

**Write code that is testable by design.**
If code is hard to test, it's hard to understand and hard to change. Testability is a proxy for good architecture. Hard-to-test code usually has hidden dependencies, global state, or does too many things.

**Follow the Arrange–Act–Assert (AAA) pattern.**
Every test has three phases: set up the scenario, execute the code under test, verify the outcome. Keep these phases visually distinct. Tests are documentation — they show how code is intended to be used.

**Test behavior, not implementation.**
Tests should verify what a unit does, not how it does it internally. Tests coupled to implementation break on every refactor. Assert on public outcomes: return values, side effects, state changes — not internal method calls.

**One assertion per test (ideally).**
Tests with multiple assertions obscure which behavior failed. One focused assertion makes failures unambiguous and fast to diagnose.

---

### 8. Comments & Documentation

**Comments explain _why_, not _what_.**
Code should explain itself through naming and structure. Comments are for business rules, historical context, non-obvious trade-offs, and warnings — not for restating what the code already says.

**Delete dead code; don't comment it out.**
Commented-out code is noise that erodes trust in the codebase. Version control exists precisely to recover old code. Delete it.

**Keep comments synchronized with code.**
A comment that contradicts the code is worse than no comment. Outdated comments actively mislead. If you change code, update its comments immediately.

---

### 9. Consistency & Style

**Automate style enforcement.**
Use linters, formatters, and pre-commit hooks. Style debates are a waste of engineering time. Let tools decide; let humans think.

**Follow the principle of least surprise.**
Code should do exactly what its name, signature, and context imply. Surprising behavior is a bug, even when it's intentional.

**Be consistent above all else.**
A codebase that consistently follows a mediocre convention is easier to work in than one that inconsistently follows great ones. Consistency reduces the cognitive load of context-switching across files.

---

### The Meta-Principle

> **Code is written once, read hundreds of times.**

Every decision — naming, structure, commenting, testing — should optimize for the next person who reads it. That person is often you, six months from now. Write accordingly.

---

##  Architecture Design Principles

> Architecture is the set of decisions that are hard to reverse. Make them deliberately, document them explicitly, and revisit them regularly.

### 1. Design for Change, Not for Perfection

**Defer irreversible decisions as long as possible.**
The cost of a wrong architectural decision compounds over time. Gather real requirements before committing to a structure. "We might need this later" is not a requirement.

**Prefer reversible over irreversible choices.**
All else equal, choose the option that is easier to undo. Monolith-first is easier to split later than microservices are to merge. SQL is easier to move off than a proprietary cloud-native store.

**Evolve architecture incrementally.**
Big-bang rewrites almost always fail. Introduce architectural changes through the strangler fig pattern — wrap, migrate, retire — so the system remains shippable at every step.

---

### 2. Separate Concerns at Every Level

**Domain logic must not leak into infrastructure.**
Business rules should be expressible and testable without a database, HTTP server, or message queue. If your domain model imports a framework, something is wrong.

**Define clear boundaries between bounded contexts.**
Each major domain area (e.g. billing, identity, notifications) should own its data and expose a deliberate interface to the outside world. Cross-context data access is the root of most large-scale coupling problems.

**Apply the Ports & Adapters (Hexagonal) pattern.**
Your application core defines ports (interfaces it needs). Adapters implement those ports for specific infrastructure (Postgres, S3, Stripe). This makes the core independently testable and infrastructure swappable.

```
[ UI / CLI / API ]       ← Adapter (driving)
        ↓
[ Application Core ]     ← Pure domain + use cases
        ↓
[ DB / Queue / Email ]   ← Adapter (driven)
```

---

### 3. Design Explicit Contracts

**Every public API is a promise.**
Once an interface is consumed externally, changing it has a cost. Version APIs from day one. Deprecate explicitly; don't silently break consumers.

**Specify behavior, not structure.**
Contracts should describe what a component guarantees — inputs, outputs, invariants, error conditions — not how it is implemented internally. This preserves the freedom to refactor.

**Use types and schemas as living contracts.**
Define data shapes at boundaries with types (TypeScript), schemas (JSON Schema, Zod, Pydantic), or protobufs. These are machine-checkable and serve as documentation that cannot go stale.

---

### 4. Design for Observability from Day One

**Treat logging, metrics, and tracing as first-class concerns.**
Observability is not a post-launch concern. A system you cannot observe in production is a system you cannot safely operate. Design structured logs, emit meaningful metrics, and propagate trace IDs across service boundaries from the start.

**Make the system's health visible.**
Every service should expose a health check. Every critical operation should emit a metric. Every failure should be distinguishable from a success in your logs.

**Design for debuggability, not just correctness.**
Code that works is not enough — you need to be able to understand _why_ it works and _why_ it fails. Instrument the decision points, not just the outcomes.

---

### 5. Manage Complexity Deliberately

**Complexity is the root cause of most software failures.**
There are two kinds: essential complexity (inherent to the problem) and accidental complexity (introduced by our solutions). Ruthlessly eliminate accidental complexity. Acknowledge and isolate essential complexity.

**Prefer simple over clever.**
A solution the entire team can understand and modify is more valuable than an elegant one only its author can maintain. Cleverness has a carry cost.

**Document architectural decisions with ADRs.**
For every significant architectural choice, write a short Architecture Decision Record (ADR): the context, the options considered, the decision made, and the trade-offs accepted. Future engineers — including yourself — will need this context.

```markdown
# ADR-001: Use PostgreSQL for primary data store

## Status: Accepted

## Context: Need a reliable relational store with strong consistency guarantees.

## Decision: PostgreSQL over MySQL due to superior JSON support and extension ecosystem.

## Consequences: Operationally familiar; requires managed hosting or DBA attention at scale.
```

---

### 6. Security and Resilience Are Not Features

**Design for failure at every layer.**
Every network call will eventually fail. Every disk will eventually fill. Every dependency will eventually be unavailable. Design with timeouts, retries, circuit breakers, and graceful degradation — not as afterthoughts, but as first-class requirements.

**Apply least privilege everywhere.**
Services, users, and processes should have access to exactly what they need — nothing more. Over-provisioned permissions are a security debt that compounds silently.

**Validate all input at trust boundaries.**
Never trust data crossing a boundary you don't control. Validate, sanitize, and type-check at every ingress point — APIs, file uploads, message queues, user input.

---

## Architecture

### Overview

Chorus is a multi-agent collaboration platform with three layers: a React/Vite frontend, a Rust/Axum backend, and external agent processes that communicate via MCP (Model Context Protocol).

```
┌─────────────────┐      HTTP/WebSocket       ┌─────────────────┐
│   React/Vite    │ ◄───────────────────────► │   Rust/Axum     │
│    (ui/)        │                           │    (src/)       │
└─────────────────┘                           └────────┬────────┘
                                                       │
                              ┌────────────────────────┼────────────────────────┐
                              │                        │                        │
                              ▼                        ▼                        ▼
                       ┌─────────────┐        ┌─────────────┐          ┌─────────────┐
                       │   SQLite    │        │   Agent     │          │   Bridge    │
                       │  (store/)   │        │  Processes  │          │  (MCP)      │
                       │             │        │             │          │             │
                       │ • messages  │        │ • Claude    │◄────────►│ • mcp__     │
                       │ • channels  │        │ • Codex     │          │   chat__    │
                       │ • agents    │        │ • Kimi      │          │   send_msg  │
                       │ • tasks     │        │             │          │ • mcp__     │
                       │ • teams     │        │             │          │   chat__    │
                       │             │        │             │          │   recv_msg  │
                       └─────────────┘        └─────────────┘          └─────────────┘
```

### Components

| Component | Responsibility | Key Files |
|-----------|---------------|-----------|
| **Frontend** | Chat UI, real-time updates, state management | `ui/src/store.tsx`, `ui/src/components/ChatPanel.tsx` |
| **Server** | HTTP API, WebSocket events, request routing | `src/server/handlers/`, `src/server/transport/` |
| **Store** | SQLite persistence, queries, migrations | `src/store/` |
| **Agent Manager** | Process lifecycle, spawn/kill, stdout parsing | `src/agent/manager.rs`, `src/agent/lifecycle.rs` |
| **Drivers** | Runtime-specific spawn/prompt/parse logic | `src/agent/drivers/{claude,codex,kimi}.rs` |
| **Bridge** | MCP server for agent tool calls | `src/bridge/mod.rs` |

### Data Flow

#### Human Sends Message → Agent Replies

```
Human (Browser)
      │
      ▼ POST /api/channels/{id}/messages
┌─────────────┐
│   Server    │ ──► SQLite (messages table)
│  (Axum)     │ ──► Broadcast channel notify
└──────┬──────┘
       │
       ▼ AgentManager::wake_agent()
┌─────────────┐
│Agent Process│ ◄── stdin: {"type":"notify"}
│  (Driver)   │
└──────┬──────┘
       │ stdout: tool call mcp__chat__receive_message
       ▼
┌─────────────┐
│    Bridge   │ ──► GET /internal/agent/{id}/receive (long-poll)
│   (MCP)     │ ◄── Returns queued message
└──────┬──────┘
       │ Agent generates reply
       ▼ stdout: tool call mcp__chat__send_message
┌─────────────┐
│    Bridge   │ ──► POST /internal/agent/{id}/send
└─────────────┘
```

#### Browser Receives Real-time Updates

1. UI opens **one** session-wide WebSocket at `/api/events/ws`
2. Server emits `message.created` events with `(channel_id, latest_seq)`
3. UI fetches incremental history via `GET /api/channels/{id}/history?after={seq}`
4. Messages merge into local cache; optimistic UI shows sending state
5. Read cursors update via `POST /internal/agent/{id}/read-cursor` when visible

### Agent Sessions

Each agent runs as a **single OS process** across all channels and DMs:

- **Session ID**: Persisted to SQLite on `SessionInit` and `TurnEnd`
- **Resume**: On server restart, agents auto-restart with `--resume <session_id>` (Claude), `codex exec resume <thread_id>` (Codex), or `kimi --session <session_id>` (Kimi)
- **Context Isolation**: Provided via `MEMORY.md` in agent workspace, not separate processes

### Key Design Decisions

| Decision | Rationale |
|----------|-----------|
| One process per agent (not per channel) | Lower memory footprint; shared context across channels |
| MCP bridge as separate binary | Isolates runtime tool calls from server; enables testing |
| Long-poll for agent receive | Simpler than WebSocket for CLI-based runtimes |
| Optimistic UI with server reconciliation | Responsive feel; eventual consistency with durable seq |
| Single WebSocket for all channels | Reduces connection overhead; subscription-based routing |

## Code Organization

Organize code by subsystem, not by request or one-off feature patches.

### Backend Layout

- `src/main.rs`
  - CLI entrypoint and `serve` bootstrap only
- `src/lib.rs`
  - crate-level module exports
- `src/agent/`
  - agent lifecycle, process management, activity log, collaboration logic, workspace handling
  - runtime-specific subprocess drivers live under `src/agent/drivers/`
- `src/bridge/`
  - MCP bridge implementation, request and response formatting, bridge-local types
- `src/server/`
  - Axum router assembly in `mod.rs`
  - HTTP handlers grouped by domain under `src/server/handlers/`
- `src/store/`
  - SQLite persistence and domain store modules (`agents`, `channels`, `messages`, `tasks`, `teams`, `knowledge`)

### Frontend Layout

- `ui/src/App.tsx`
  - top-level shell composition only
- `ui/src/api.ts`
  - browser-to-server API calls only
- `ui/src/store.tsx`
  - client app state and selection logic
- `ui/src/hooks/`
  - reusable data-loading and interaction hooks
- `ui/src/components/`
  - UI grouped by panel, modal, and component responsibility
- `ui/src/channelList.ts` and `ui/src/types.ts`
  - shared UI-side derivation and types

### Organization Rules

- Put new HTTP handlers in the matching file under `src/server/handlers/`; do not grow `src/server/mod.rs` into a handler dump
- Put persistence logic in the matching `src/store/*.rs` module; do not hide DB writes in handlers
- Put agent runtime and subprocess behavior in `src/agent/`; do not mix it into HTTP or store modules
- Put bridge-only formatting and protocol glue in `src/bridge/`
- Keep frontend state changes in `ui/src/store.tsx`; components should call APIs and store actions, not invent parallel state systems
- Co-locate component styles with the component in `ui/src/components/`
- Treat `qa/` as its own execution layer; specs, plans, reports, and evidence stay under `qa/`, not mixed into app code

## Core Conventions

### Store Conventions

- Every DB operation uses `self.conn.lock().unwrap()`; the connection is `Mutex<Connection>`
- IDs are `uuid::Uuid::new_v4().to_string()`
- Timestamps are stored as ISO 8601 text and parsed via `chrono`

### UI Conventions

- Component styles live in co-located `.css` files (for example `ActivityPanel.tsx` + `ActivityPanel.css`)
- Design tokens are CSS variables defined in `App.css`
- Icons use `lucide-react`; keep sizes consistent (13px for inline tool icons, 16px for panel icons)
- No global state mutations outside `ui/src/store.tsx`
- API calls go through `ui/src/api.ts`
- The shell bootstraps `/api/server-info`, `/api/channels`, `/api/agents`, and `/api/teams` once and should not poll them again while idle; sidebar lists refresh only after explicit create/edit flows or other real user-triggered invalidation

### Logging

Use `RUST_LOG=chorus=debug` for verbose output. All logging uses `tracing`; never use `eprintln!` or `println!` in library code.


## Development Workflow

### Branch Workflow For Feature Work

When the user explicitly asks to implement a new feature or do a refactor:

1. Check whether the worktree is dirty before switching branches
2. If local changes exist, stop and ask the user whether to commit, stash, or move them aside
3. Start work from an up-to-date `main` based on `origin/main`
4. Create a new branch with the `{agent}/` prefix (`codex/`, `claude/`, `gemini/`, and so on)
5. Do not carry unrelated residual changes into the new branch without explicit user approval

### Commit Conventions

- Use conventional-style commit messages with a scope when practical
- Preferred patterns: `feat(settings): ...`, `fix(command): ...`, `refactor(config): ...`, `docs(agent): ...`, `ci: ...`

### Development Commands

```bash
# Full dev environment (backend + UI hot-reload)
./dev.sh

# Backend only
cargo build && ./target/debug/chorus serve

# UI only (needs backend running)
cd ui && npm run dev
```

## Verification Policy

Do not claim a task is complete without running verification that matches the risk of the change.

### Minimum Verification

1. Run focused Rust tests for the affected modules
2. Run `cargo test --test e2e_tests` when backend message flow, task flow, DM flow, thread flow, or agent lifecycle is affected
3. For core user-facing workflow changes, run the browser QA pass defined in `qa/README.md`

### Required Escalation

- For backend or data-path changes, run the relevant Rust tests first
- For any change that affects a core user process, verify the real flow with headless-browser end-to-end testing against the running app
- Core process verification is mandatory for user-facing critical paths such as channel messaging, DM flows, thread replies, task board actions, and agent interaction loops
- Backend integration tests alone are not sufficient when the user-visible flow changed
- If required headless-browser verification cannot be run, say so clearly and do not present the work as fully verified

## QA Workflow

The authoritative QA execution workflow lives in `qa/README.md`, with the case catalog and templates under `qa/`.

## Extension Points

### Adding A New Driver

See [`docs/DRIVER_GUIDE.md`](./docs/DRIVER_GUIDE.md) for the complete guide on adding a new agent runtime driver, including protocol discovery, implementation phases, testing, and required verification.

## Completion Checklist

Before stopping, confirm all of the following:

- The change lives in the correct subsystem and file
- Verification matches the risk of the change
- Required e2e or browser QA was run for user-visible critical paths, or the gap was called out explicitly
- `AGENTS.md` or related docs were updated if shipped behavior or workflow changed
