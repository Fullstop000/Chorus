# Chorus Development Guide

How to run, test, and iterate on Chorus locally. Everything a new contributor
(human or agent) needs to get productive in 5 minutes.

For code conventions see `docs/BACKEND.md` (Rust) and
`docs/DESIGN.md` (frontend). For the code map see the tables
in `AGENTS.md` § Project Organization.

---

## Prerequisites

- **Rust** — stable toolchain, whatever `rust-toolchain.toml` pins (check
  the file)
- **Node.js + npm** — for the Vite dev server and vitest. Bun also works for
  running tests but the `package.json` scripts assume npm
- **SQLite** — bundled via rusqlite, no system install needed

---

## Run the app locally

Chorus is a two-process app: a Rust backend and a Vite dev server for the
React frontend. Run both in parallel.

**Backend** (terminal 1):

```bash
RUST_LOG=chorus=info cargo run -- serve --port 3001
```

Output ends with `Chorus running at http://localhost:3001`. The server:

- Opens / creates a SQLite DB at `~/.chorus/chorus.db`
- Serves the HTTP API at `:3001/api/*`
- Opens a WebSocket at `:3001/internal/*` for the realtime event stream
- Auto-restores previously-active agents on startup — if you killed
  `chorus serve` while agents were running, they come back on the next
  boot without manual intervention

Override the port with `--port <N>`. Override the template directory with
`--template-dir <PATH>` or `CHORUS_TEMPLATE_DIR=<PATH>`.

**Frontend** (terminal 2):

```bash
cd ui && npm run dev
```

Vite starts on `http://localhost:5173`. It proxies `/api/*` and
`/internal/*` to the backend, default target `http://localhost:3001`.

**Point the proxy at a different backend port:**

```bash
CHORUS_API_PORT=3002 npm run dev
```

See `ui/vite.config.ts` for the proxy configuration.

**Shared MCP bridge:**

`chorus serve` starts the shared bridge in-process automatically (default port 4321,
configurable via `--bridge-port`). You don't need to run anything extra. If you want
to run the bridge standalone:

```bash
chorus bridge-serve --listen 127.0.0.1:4321 --server-url http://localhost:3001
```

```bash
# Verify the bridge layer works (no Chorus server required)
chorus bridge-smoke-test
```

See `docs/BRIDGE_MIGRATION.md` for the full architecture and driver implementation
guide.

### CLI commands

See [`docs/CLI.md`](CLI.md) for the full command reference. Quick cheatsheet:

```bash
chorus setup                    # first-run initializer
chorus check                    # read-only environment diagnostic
chorus start --port 3001        # start server + open browser
chorus serve --port 3001        # start server, no browser
chorus bridge-serve ...         # standalone MCP bridge
chorus bridge-pair --agent ...  # mint pairing token
```

---

## Testing

### Rust

```bash
cargo test                    # everything
cargo test -p chorus store::  # store modules only
cargo test --test e2e_tests   # one integration file
cargo test <name>             # single test by name
```

The test suite is ~190 tests across 6 integration files. It uses real
SQLite tempdirs via the `make_store()` helper in `tests/store_tests.rs` —
not mocks. See `docs/BACKEND.md` § Testing for conventions and file
layout.

### Frontend

```bash
cd ui && npm run test           # vitest run (all tests)
cd ui && npx vitest              # watch mode
cd ui && npx tsc --noEmit       # typecheck only (no test run)
cd ui && npm run build           # production build (tsc + vite build)
```

Vitest covers ~48 tests across hooks, inbox, sidebar filters, and pure
function helpers. There is currently **no React component testing
infrastructure** (no testing-library, no jsdom) — component rendering is
verified via `/gstack-qa` browser QA, not unit tests.

### Browser QA

```bash
/gstack-qa
```

This is the authoritative path for user-facing verification. It boots a
headless Chromium, exercises real flows, captures screenshots, and can
fix bugs it finds. The case catalog and templates still live under
`qa/` — `/gstack-qa` consumes them as input.

---

## Killing and restarting the server

`chorus serve` auto-restores previously-active agents on startup. This
means:

- **Safe to kill during QA or rebuild.** `pkill -f 'chorus serve'`, rebuild,
  restart — the agents come back automatically.
- **Agent state survives.** Agents are stored in SQLite; the running
  processes are just bridges. Restarting the server re-spawns the bridges
  against the same agent records.
- **Warning:** if you have a long-running multi-turn conversation in
  progress with an agent, killing the server interrupts the in-flight
  turn but does not lose history. The next message to that agent resumes
  cleanly.

---

## Branch and commit workflow

### Branches

1. Check the worktree is clean before switching branches.
2. If local changes exist, commit, stash, or move them before switching.
3. Start from an up-to-date `main` based on `origin/main`.
4. Create branches with an `{agent}/` prefix: `claude/`, `codex/`, `kimi/`,
   `opencode/`, etc. Use your agent name.
5. Don't carry unrelated changes into a new branch.

### Commits

Conventional commits with a scope:

- `feat(templates):` — new feature
- `fix(store):` — bug fix
- `refactor(ui):` — structural change, no behavior change
- `docs(backend):` — documentation only
- `style(rust):` — formatting only (cargo fmt)
- `test(qa):` — test-only changes
- `chore:` — tooling, config, dependencies
- `ci:` — CI/CD configuration

Each commit should be **one logical change** that's independently valid.
Bisectable commits are the goal: if a bug is introduced, `git bisect`
should land on exactly the commit that caused it.

---

## Verification policy

Do not claim complete without matching verification.

**Minimum:**

1. Run focused Rust tests for affected modules
2. Run `cargo test --test e2e_tests` when backend message / task / DM /
   thread / agent flow is affected
3. For user-facing changes, run `/gstack-qa` (authoritative browser QA)

**Escalation:**

- Backend or data-path changes: Rust tests first, then e2e
- Core user process changes: `/gstack-qa` mandatory — backend tests alone
  are not sufficient
- Core paths: channel messaging, DM flows, thread replies, task board,
  agent loops
- If `/gstack-qa` cannot run (no dev server, external blocker), state it
  clearly; don't claim "fully verified"

---

## Troubleshooting

### "Port already in use" on `cargo run -- serve --port 3001`

Another `chorus serve` is running. `pkill -f 'chorus serve'` then retry.

### Vite proxy 502 / 504

The backend isn't running or is on a different port. Check with
`curl http://localhost:3001/api/whoami`. Restart the backend or set
`CHORUS_API_PORT` to match.

### SQLite schema out of sync after editing `schema.sql`

Views are rebuilt on every startup (`DROP VIEW IF EXISTS ... CREATE VIEW`).
Restart `chorus serve` and the new view definition takes effect. Tables
are additive via `CREATE TABLE IF NOT EXISTS`, so column additions need a
real migration in `src/store/migrations.rs`.

### `cargo fmt` touches files you didn't change

Pre-existing drift from commits that didn't run `cargo fmt`. Commit the
formatting fix in a separate `style(rust): apply cargo fmt` commit before
your feature work, so your feature diff stays clean.
