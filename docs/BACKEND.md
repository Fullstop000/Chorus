# Chorus Backend Conventions

How the Rust side of Chorus is organized, written, and tested. Read this
before any backend change. Update it in the same PR when you introduce a new
pattern.

For the visual side, see `docs/DESIGN.md`. For how to run the app, see
`docs/DEV.md`.

---

## Error handling

- **Application errors:** `anyhow::Result` with `anyhow!(...)` and
  `.context("...")` for adding detail as errors bubble up.
- **Library-shaped APIs:** typed errors using `thiserror` (rare in this
  codebase today; use when you add a new module that deserves a real error
  type).
- **Never `unwrap()` or `expect()` in library code.** OK in tests, CLI
  one-shots, and genuinely impossible cases (document the invariant in a
  comment).
- **Context when re-throwing:** match the pattern in `src/store/messages/*`
  of adding a one-line `.context("failed to …")` at each layer.

```rust
// Good
let channel = Self::get_channel_by_name_inner(conn, name)?
    .ok_or_else(|| anyhow!("channel not found: {name}"))?;

// Bad
let channel = Self::get_channel_by_name_inner(conn, name).unwrap().unwrap();
```

---

## Type system

**Make invalid states unrepresentable.** Reach for enums before booleans or
strings.

```rust
// Good
pub enum SenderType { Human, Agent, System }

// Bad
pub struct Sender { is_human: bool, is_agent: bool, is_system: bool }
```

**Enum additions are additive and safe** — verified with `rg "match.*SenderType"`
producing zero results. Every consumer uses `.as_str()`, `==`, or
`from_sender_type_str`, so adding a variant never breaks exhaustiveness.
`SenderType::System` was added exactly this way (see commit `73496d5`).

**Avoid `String` for closed sets.** If a field has a finite set of values
(sender kind, channel type, task status, runtime name), promote it to an
enum. The one exception is SQL column types where SQLite stores text and
we round-trip through `from_*_str`.

---

## Logging

- Use the `tracing` crate. **Never `eprintln!` / `println!` in library code.**
- `RUST_LOG=chorus=info ./target/debug/chorus serve --port 3001` is the
  standard invocation for dev.
- `RUST_LOG=chorus=debug` for verbose output.
- Prefer structured fields over string interpolation:
  ```rust
  tracing::info!(agent = %name, channel = %channel_name, "joined channel");
  ```
  not
  ```rust
  tracing::info!("{name} joined {channel_name}");
  ```

---

## Database and schema

The single most important thing to know: **read-model state lives in SQL
VIEWs, not in Rust queries.**

### The schema file is the source of truth

`src/store/schema.sql` contains every table and every view. `init_schema` in
`src/store/mod.rs` calls:

```rust
conn.execute_batch(include_str!("schema.sql"))?;
```

on every startup. The schema file uses:

- `CREATE TABLE IF NOT EXISTS` for tables — idempotent, runs every boot,
  preserves existing data
- `DROP VIEW IF EXISTS ... CREATE VIEW` for views — rebuilt every boot from
  the current file contents

**Implication:** view changes are **zero-migration**. Edit the view in
`schema.sql`, restart the server, done. Table changes need a proper
migration in `src/store/migrations.rs`.

### Read models that live in views

The inbox read model is the biggest one to know about.
`inbox_conversation_state_view` in `schema.sql:205-268` computes:

- `unread_count` — top-level messages for the viewing member, excluding
  their own messages and messages with `sender_type='system'`
- `thread_unread_count` — thread replies, same exclusions

When you need to change what counts as unread, change the view.
**Do not bypass the view with ad-hoc Rust queries** that re-implement
the logic — you will drift.

`conversation_messages_view` (`schema.sql:147-162`) is the history
projection — just `messages` joined to `channels`, no filtering. Most
history reads go through this view.

### Thread notifications are the exception

`get_thread_notification_state_by_channel_id_inner` in
`src/store/inbox.rs` has its own SQL query that parallels the thread-level
subquery in the main view. When you change one, change both. They are
intentionally kept in sync for a different call path.

### When to add a view vs a method

- **Use a view** when the projection is reused across multiple callers or
  when it materializes cross-table state (joins, aggregates, windowing).
- **Use a Rust method with inline SQL** when the query is one-shot,
  scoped to a single feature, or needs dynamic parameters the view
  can't express cleanly.

---

## Testing

### Rust test layout

- `tests/store_tests.rs` — store + SQL integration tests (~50 tests). Uses
  `make_store()` helper that spins up a real SQLite in a tempdir. The
  primary regression harness.
- `tests/e2e_tests.rs` — HTTP + WebSocket end-to-end tests. Use these when
  a change touches the server router or the event stream shape.
- `tests/realtime_tests.rs` — WebSocket / `StreamEvent` tests without the
  full server.
- `tests/server_tests.rs` — HTTP handler tests with a test router (does not
  mount static assets, which would block on `ServeDir::new("ui/dist")` for
  large UI builds).
- `tests/driver_tests.rs` — agent runtime driver tests.
- `tests/check_impl.rs` — type-level / trait-bound checks.
- Module-level `#[cfg(test)] mod tests` for pure-function unit tests inside
  `src/`.

### Conventions

- **Naming:** `test_<subject>_<behavior>`. Example:
  `test_channel_unread_count_excludes_system_messages`.
- **AAA** (Arrange, Act, Assert) with blank lines between phases.
- **One assertion per test** where possible. When a single behavior needs
  multiple asserts, group them with a clear "then" comment.
- **Real SQLite in tempdirs, not mocks.** Mocking the database hides bugs
  that only show up in real query planning. `make_store()` or
  `Store::open(":memory:")` are both fine.
- **Direct SQL via `conn_for_test()`** is allowed for exercising view
  or constraint paths that the public API cannot reach (example:
  `test_thread_view_excludes_system_replies` inserts a system thread
  reply directly because `create_system_message` does not expose a
  `thread_parent_id` parameter).
- **Run with:** `cargo test` (everything), `cargo test -p chorus store::`
  (store modules only), `cargo test --test e2e_tests` (one file).

### Verification policy

- Focused module tests on every backend change.
- `cargo test --test e2e_tests` when backend message/task/DM/thread/agent
  flow is affected.
- `/gstack-qa` for user-facing changes (authoritative browser QA).

Do not claim complete without running the matching verification.

---

## Module structure

Organize by feature, not by layer. `src/store/messages/` owns the messages
domain end-to-end (types, insertion, history, threads, cursors, targets)
rather than scattering "models" and "queries" across separate folders.

- **Module size:** 300-line file = refactor signal. 500-line file =
  problem. Current outliers are `src/store/inbox.rs` and
  `src/store/messages/posting.rs` — touch carefully.
- **Public API at `mod.rs`:** expose a minimal surface. Internal helpers
  are `pub(crate)` or private. `Self::*_inner` naming for
  transaction-accepting private twins of public methods.
- **One concept per module.** If a file grows a second concern, extract it
  before it becomes intertwined.

---

## Server (Axum) conventions

- Router assembly lives in `src/server/mod.rs`. Route definitions and
  middleware are composed here.
- Handlers are grouped by domain under `src/server/handlers/` — one file
  per domain (`agents.rs`, `channels.rs`, `messages.rs`, `tasks.rs`,
  `teams.rs`, `templates.rs`, `server_info.rs`).
- **Handler error returns use tuples:** `Result<_, (StatusCode, Json<ErrorResponse>)>`.
  Use the `api_err` / `internal_err` helpers in `src/server/handlers/mod.rs`
  to keep the pattern uniform.
- **Sender type resolution for client requests** always goes through
  `sender_type_for_actor` → `store.lookup_sender_type` → falls back to
  `SenderType::Human`. This is the security boundary that prevents a
  client from forging a `sender_type='system'` message. Do not bypass
  it when adding new endpoints.

---

## Adding a new runtime driver

See `docs/DRIVERS.md`.

---

## What not to do

- Do not `unwrap()` in library code.
- Do not use `eprintln!` / `println!` for logging.
- Do not reinvent a read model in Rust when a SQL view already owns it.
- Do not add a new file for a single-use helper. Put it next to its caller.
- Do not add backwards-compatibility shims for code that has no users
  outside the repo.
- Do not add `// removed` comments, renamed `_unused` variables, or
  deprecated aliases. Delete completely.
