# Identity and Auth Redesign

**Status:** Design locked. Decisions: D1=A (no session expiry locally),
D2=A (prefixed token format), D3=A (`created_by_user_id`), D4=A (rename
wire `"human"` → `"user"`), D5=B (single token kind — no `kind` column on
`api_tokens`).
**Supersedes:** [`identity-unification-plan.md`](identity-unification-plan.md)
(narrower refactor; eng-review found that fixing the resolver in isolation
was treating symptoms — the underlying ontology was the problem).
**Tracks:** [#144](https://github.com/Fullstop000/Chorus/issues/144)

---

## 1. Goal

Replace the current ad-hoc `humans + LocalHumanConfig + state.local_human_id`
identity model with a real, production-shaped auth model that works the same
way in local and cloud deployments.

Today the server *caches an identity at boot* (`AppState.local_human_id`),
populated from `~/.chorus/config.toml`. Every handler reads from that cached
field. There is no per-request auth — local mode trusts the loopback. This
shape cannot extend to cloud, and it is the source of #144's drift: any
disagreement between config and DB silently changes who the "local operator"
is.

After this change:

- **Users** are the identity layer. Every actor reference uses `user_id`.
- **Accounts** are how a User authenticates. 1..N per User. `auth_provider`
  distinguishes `'local'` from cloud providers (`'google'`, `'github'`, …).
- **Credentials** are per-request: cookies for browser UI, bearer tokens
  for CLI/bridge. Issued from Accounts. Validated by an auth middleware.
- **The server holds no boot-time identity state.** `state.local_human_id`
  is deleted. Every handler reads the actor from `request.extensions`.

Local mode and cloud mode share the entire auth pipeline; they differ only
in *how the first credential is obtained* (loopback shortcut vs OAuth).

## 2. Non-goals

- Not implementing cloud auth providers in this PR. The schema and middleware
  support `auth_provider != 'local'`, but only `'local'` issuance and the
  loopback browser path ship.
- Not introducing the `Actor` reframing for `SenderType` from #144's comment
  thread. `SenderType::Human` is renamed to `SenderType::User` to match the
  new noun; the polymorphic shape is unchanged.
- Not migrating `workspaces.created_by_human_id` to a generic actor. Stays
  user-specific (renamed to `created_by_user_id`); agent-creates-workspace
  is still deferred.
- Not implementing token rotation, scopes, fine-grained permissions, MFA,
  or session expiry policies. `expires_at` is a column; the policy that
  populates it is future work.

## 3. Ontology

```
                    ┌─────────────────────┐
                    │       User          │  (the person)
                    │  id, name, ...      │
                    └──────────┬──────────┘
                               │ 1..N
                               ▼
                    ┌─────────────────────┐
                    │      Account        │  (how the User authenticates)
                    │  id, user_id,       │
                    │  auth_provider,     │
                    │  email, disabled_at │
                    └──────────┬──────────┘
                               │ 1..N
              ┌────────────────┼────────────────┐
              ▼                                 ▼
   ┌─────────────────────┐           ┌─────────────────────┐
   │      Session        │           │      ApiToken       │
   │  (browser cookie)   │           │  (CLI / bridge)     │
   └─────────────────────┘           └─────────────────────┘
```

**Reading rules.** A User is identity (the person, what gets stamped on
messages). An Account is one way that User proves who they are (local
machine, Google sign-in, GitHub, …). A Session or ApiToken is a specific
credential issued for one Account. Many Sessions per Account is allowed
(browser + another browser tab + …); many ApiTokens per Account is allowed
(laptop CLI + CI token + bridge token).

**Local-mode invariant.** Exactly one `auth_provider='local'` Account per
installation. Setup creates User + this Account + the initial CLI token in
one transaction.

## 4. Schema

```sql
-- Identity layer.
CREATE TABLE users (
    id          TEXT PRIMARY KEY,                       -- usr_<uuid>
    name        TEXT NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
    -- name is NOT UNIQUE: cloud-era collaborators may share display names.
);

-- Authentication layer.
CREATE TABLE accounts (
    id              TEXT PRIMARY KEY,                   -- acc_<uuid>
    user_id         TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    auth_provider   TEXT NOT NULL,                      -- 'local' | 'google' | ...
    email           TEXT,
    disabled_at     TEXT,
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(auth_provider, email)
    -- (auth_provider='local', email IS NULL): exactly one row per install.
);
CREATE INDEX idx_accounts_user_id ON accounts(user_id);

-- Browser UI auth: session cookies.
CREATE TABLE sessions (
    id              TEXT PRIMARY KEY,                   -- ses_<uuid>; opaque cookie value
    account_id      TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    last_seen_at    TEXT NOT NULL DEFAULT (datetime('now')),
    expires_at      TEXT,                               -- NULL = no expiry (local mode)
    revoked_at      TEXT
);
CREATE INDEX idx_sessions_account_id ON sessions(account_id);

-- CLI / bridge auth: bearer tokens.
CREATE TABLE api_tokens (
    token_hash      TEXT PRIMARY KEY,                   -- SHA-256(raw); raw never stored
    account_id      TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    kind            TEXT NOT NULL,                      -- 'cli' | 'bridge'
    label           TEXT,                               -- 'Local CLI', 'Laptop bridge'
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    last_used_at    TEXT,
    revoked_at      TEXT
);
CREATE INDEX idx_api_tokens_account_id ON api_tokens(account_id);
```

**What's removed:**
- `humans` table — replaced by `users`. The columns `auth_provider`, `email`,
  `disabled_at` move to `accounts`. `name` stays on `users`. `created_at`
  stays on both (they're independent lifecycles).

**What's renamed in existing tables:**

| Old | New |
| --- | --- |
| `workspaces.created_by_human_id` | `workspaces.created_by_user_id` |
| `workspace_members.human_id` | `workspace_members.user_id` |
| `messages.sender_type = 'human'` | `messages.sender_type = 'user'` (data sweep) |
| `channel_members.member_type = 'human'` | `channel_members.member_type = 'user'` |
| `inbox_read_state.member_type = 'human'` | `inbox_read_state.member_type = 'user'` |
| `team_members.member_type = 'human'` | `team_members.member_type = 'user'` |
| `tasks.created_by_type = 'human'` | `tasks.created_by_type = 'user'` |
| `tasks.claimed_by_type = 'human'` | `tasks.claimed_by_type = 'user'` |

The `sender_type` etc. columns stay polymorphic; only the literal `'human'`
changes to `'user'` in storage *and* on the wire. Frontend will be updated
in the same PR — no compat shim.

## 5. Request flow

```
┌───────────────────────────────────────────────────────────────────────────┐
│ INCOMING REQUEST                                                          │
└──────────────────────────────────┬────────────────────────────────────────┘
                                   ▼
┌───────────────────────────────────────────────────────────────────────────┐
│ AuthLayer (Axum middleware)                                               │
│                                                                           │
│   1. Has Cookie `chorus_sid=ses_…`?                                       │
│        sessions WHERE id = ? AND revoked_at IS NULL                       │
│             → accounts → users                                            │
│        UPDATE sessions SET last_seen_at = now()                           │
│        Inject Actor { user_id, account_id, kind: SessionAuth } ──────┐    │
│                                                                      │    │
│   2. Else has Authorization: Bearer chrs_…?                          │    │
│        api_tokens WHERE token_hash = sha256(raw) AND revoked_at NULL │    │
│             → accounts → users                                       │    │
│        UPDATE api_tokens SET last_used_at = now()                    │    │
│        Inject Actor { user_id, account_id, kind: TokenAuth(kind) } ──┤    │
│                                                                      │    │
│   3. Else: 401 (with WWW-Authenticate hint for clients)              │    │
│                                                                      │    │
│   Exempt: GET /health, GET /api/auth/local-session (loopback only),  │    │
│           static UI assets.                                          │    │
└──────────────────────────────────┬───────────────────────────────────┘    │
                                   ▼                                        │
┌───────────────────────────────────────────────────────────────────────────┐
│ Handler                                                                   │
│   let Actor { user_id, .. } = req.extensions().get::<Actor>().unwrap();   │
│   store.send_message(channel_id, user_id, content)                        │
└───────────────────────────────────────────────────────────────────────────┘
```

The `Actor` type:

```rust
#[derive(Debug, Clone)]
pub struct Actor {
    pub user_id: String,
    pub account_id: String,
    pub auth: AuthKind,
}

#[derive(Debug, Clone)]
pub enum AuthKind {
    Session,           // Browser cookie
    ApiToken(TokenKind),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind { Cli, Bridge }
```

Lives at `src/server/auth/mod.rs`. The middleware is `AuthLayer`; the
extractor is `Extension<Actor>`.

## 6. Local-mode flows

### 6.1 First-run setup

```
chorus setup:
   1. INSERT INTO users (id, name)
      VALUES ('usr_'||lower(hex(randomblob(16))), $whoami_hint)
   2. INSERT INTO accounts (id, user_id, auth_provider)
      VALUES ('acc_'||..., ^.id, 'local')
   3. Mint token: raw = "chrs_local_" || base64url(random(32))
      INSERT INTO api_tokens (token_hash, account_id, kind, label)
      VALUES (sha256(raw), ^.id, 'cli', 'Local CLI')
   4. Write ~/.chorus/credentials.toml (0600):
          token = "chrs_local_…"
          server = "http://127.0.0.1:3001"
   5. cfg.toml writes machine_id, runtime binaries, template paths.
      cfg.local_human is GONE.
```

All four DB writes run in a single transaction. If the credentials file
write fails, the transaction rolls back — partial state would leave a User
whose only token is unreachable on disk.

### 6.2 CLI request

```
chorus send "hi":
   1. Read raw_token from ~/.chorus/credentials.toml
   2. POST {server}/api/messages/send
      Authorization: Bearer {raw_token}
   3. Server:
        AuthLayer → api_tokens → account → user → inject Actor
        handler → store.send_message(channel, actor.user_id, "hi")
```

### 6.3 Browser UI bootstrap (local mode)

```
GET http://127.0.0.1:3001/ (no cookie yet)
   → serves index.html (exempt from AuthLayer)

UI JS on load:
   POST /api/auth/local-session
        Origin: http://127.0.0.1:3001
   → Server: check remote_addr.is_loopback() && origin matches host
   → Find the single auth_provider='local' Account
   → INSERT INTO sessions (id, account_id) VALUES (...)
   → Set-Cookie: chorus_sid=ses_…; HttpOnly; SameSite=Strict; Path=/
   → 200 OK { user: { id, name } }

Subsequent GET /api/whoami:
   Cookie: chorus_sid=ses_…
   → AuthLayer → sessions → account → user → inject Actor
   → handler returns { id, name }
```

The `/api/auth/local-session` endpoint is **gated on loopback origin**.
In cloud mode it returns 404. Local mode only.

### 6.4 Logout

```
chorus logout:
   POST /api/auth/logout
        Authorization: Bearer {raw_token}
   → Server: UPDATE api_tokens SET revoked_at = now() WHERE token_hash = ...
   → Delete ~/.chorus/credentials.toml

Next chorus send:
   No credentials → CLI prints "not logged in; run `chorus login --local`"
                    (exits non-zero before making any HTTP call)
```

`chorus login --local` mints a new token against the existing local Account.
Idempotent if a non-revoked token already exists (offer to reuse vs replace).

## 7. Cloud-mode flows (sketched — not implemented this PR)

Same tables, same middleware, same handler code.

```
chorus login --provider google:
   1. Open browser to ${SERVER}/auth/google
   2. Server runs OAuth → receives id_token
   3. Resolve user by (auth_provider='google', email)
        → match: use existing User+Account
        → no match: INSERT User + Account
   4. Mint api_tokens row (kind='cli'), return raw token to CLI via callback
   5. CLI writes ~/.chorus/credentials.toml
```

Browser web app in cloud mode follows standard session-cookie OAuth: hit
`/auth/google` → callback sets a `sessions` row + cookie. Identical to the
local flow except for the "obtain account" step.

Bridge in cloud mode: user generates a bridge token via the web UI
(`POST /api/tokens` with `kind='bridge'`), pastes it into the bridge's env.
Same `api_tokens` table.

## 8. Bridge auth — DEFERRED to a follow-up PR

Originally scoped here as "subsume bridge_auth into api_tokens." On
closer reading the subsumption is invasive: bridge tokens bind to a
`machine_id` (the bridge instance), while CLI tokens bind only to an
Account. A clean unification needs:

  - `machine_id` column on `api_tokens` (NULL for CLI, set for bridge)
  - A `mint_bridge_token(account_id, machine_id, label)` issuance path
  - A migration for existing `CHORUS_BRIDGE_TOKENS` env-driven bridges
  - Rewiring `bridge_auth::require_bridge_auth` to look up via
    `api_tokens` instead of its in-memory `HashMap`

`bridge_auth` and the new `require_auth` layer don't conflict today:
they cover disjoint route prefixes. `/api/bridge/ws` lives in the open
router (its own auth layer downstream); `/internal/*` keeps using
`bridge_auth`. Unifying them is a real follow-up but doesn't block the
identity-and-auth redesign from landing.

A note in `src/server/bridge_auth.rs` documents the deferral inline so
any future change-author sees it.

## 8b. Schema rename — DEFERRED

`humans` → `users` (table), `sender_type='human'` → `'user'` (data + wire),
`Human` struct → `User`: deferred to a follow-up. Reasons:

  - The `humans` table is a passive mirror in this PR: production handlers
    never read from it (they go through `accounts.user_id → users.id`). The
    workspace tables still FK-reference `humans(id)`, but every row written
    has matching ids in both tables.
  - The rename is a coordinated change across ~20 files plus a backfill
    `UPDATE` for existing data and a frontend sweep for `sender_type ===
    "human"`. None of it changes behaviour; it's purely cosmetic.
  - The PR is already large. Per "one PR per goal", the goal is the new
    auth model; the rename is a separate goal.

A `DEPRECATED` comment lives on the `humans` table in `schema.sql`
pointing at this section.

## 9. What this kills

| Today | After |
| --- | --- |
| `humans` table | `users` table |
| `Human` struct | `User` struct |
| `LocalHumanConfig` in cfg.toml | DELETED |
| `cfg.local_human.id` | DELETED |
| `cfg.local_human.name` | DELETED |
| `ensure_setup_local_human()` (setup) | DELETED |
| `resolve_local_human_identity()` (server) | DELETED |
| `AppState.local_human_id` | DELETED |
| `AppState.local_human_name` | DELETED |
| `fetch_local_human_identity()` (CLI) | replaced by `read_credentials_and_call(/api/whoami)` |
| `LocalHumanIdentity` struct | DELETED |
| `UserError` | renamed to `CliError` (from old plan) |
| Issue #144's A1/A2/A3/Q1 from eng review | unreachable by construction |
| `bridge_auth` as separate registry | thin wrapper over `api_tokens` |
| `SenderType::Human` | `SenderType::User` |
| `"sender_type":"human"` in wire format | `"sender_type":"user"` (frontend updated in same PR) |

`humans.id` values become `users.id` values during the rename (literal table
rename + column renames in references). No id format change.

## 10. Migration policy

Same as today's `validate_schema_shape` pattern (`src/store/mod.rs:119`):
the validator detects pre-redesign DBs by checking for the `humans` table
and refuses to open with an explicit "delete `~/.chorus/data/chorus.db` and
re-run `chorus setup`" hint.

No live migration. No backfill. No compat shim in `LocalHumanConfig` to
read old `name = "..."` lines — serde already ignores unknown fields when
`deny_unknown_fields` is absent (it is), so the cfg loader naturally drops
the obsolete keys on next save.

Existing users (you, anyone else with a local install) re-run setup once.
The OS username hint stays the same; their workspace data is lost — this
is acceptable pre-1.0. Document it in the PR description.

## 11. Code organization

```
src/
├── store/
│   ├── users.rs            (NEW — was humans.rs, restructured)
│   ├── accounts.rs         (NEW)
│   ├── sessions.rs         (NEW — distinct from agent sessions in this file's
│   │                        sibling; rename existing sessions.rs → agent_sessions.rs)
│   ├── api_tokens.rs       (NEW)
│   └── schema.sql          (rewritten)
├── server/
│   ├── auth/
│   │   ├── mod.rs          (NEW — Actor, AuthKind, AuthLayer)
│   │   ├── cookie.rs       (NEW — cookie codec + session lookup)
│   │   ├── token.rs        (NEW — sha256 token hash + api_tokens lookup)
│   │   └── local_session.rs (NEW — loopback shortcut endpoint)
│   ├── handlers/
│   │   ├── auth.rs         (NEW — /api/auth/login, /logout, /local-session, /whoami)
│   │   ├── tokens.rs       (NEW — /api/tokens CRUD for "manage my tokens" UI)
│   │   └── (existing handlers: ~30 reads of state.local_human_id rewritten
│   │                            to read Actor from req.extensions)
│   └── mod.rs              (state.local_human_id deleted; router gets AuthLayer)
├── cli/
│   ├── credentials.rs      (NEW — read/write ~/.chorus/credentials.toml)
│   ├── login.rs            (NEW — chorus login --local)
│   ├── logout.rs           (NEW — chorus logout)
│   ├── mod.rs              (LocalHumanIdentity deleted; UserError → CliError)
│   └── setup.rs            (creates User+Account+token in one tx; writes
│                            credentials.toml; cfg.local_human gone)
└── config.rs               (LocalHumanConfig deleted)

ui/                          (sender_type === 'human' → 'user' sweep; whoami
                              response shape unchanged; local-session bootstrap
                              call added to UI load)
```

## 12. Commit sequence (one PR)

Each commit independently green; PR is the landing unit.

| # | Commit | Why first |
| --- | --- | --- |
| 1 | `feat(store): add users/accounts/sessions/api_tokens tables and CRUD` | Schema + plain-CRUD with full test coverage. No callers yet; nothing breaks. |
| 2 | `feat(server): AuthLayer + Actor extractor` | Middleware exists, registered as permissive in this commit (falls through if no auth — so step 6 still works). |
| 3 | `feat(server): /api/auth/local-session endpoint (loopback only)` | Browser UI cookie path; gated on loopback origin. |
| 4 | `feat(cli): credentials file + chorus login --local + chorus logout` | CLI client side of token auth, not yet required. |
| 5 | `refactor(cli): chorus setup creates User+Account+token; writes credentials.toml` | Setup is the only place that creates local identity; rewrite it. |
| 6 | `refactor(server): handlers read Actor from request, not state.local_human_id` | The ~30-site sweep. AuthLayer flips to required after this lands. |
| 7 | `refactor(server): subsume bridge_auth into api_tokens (kind='bridge')` | One auth registry. |
| 8 | `chore: rename humans→users, Human→User, sender_type 'human'→'user'` | Mechanical sweep. Frontend update bundled here. |
| 9 | `refactor: drop LocalHumanConfig, resolve_local_human_identity, ensure_setup_local_human` | The actual delete pass. Possible only after step 6. |
| 10 | `chore: rename UserError → CliError; comment sweep` | Last cleanup. |

This is large. Per `feedback_one_commit_per_goal` memory: hard line is at
the PR boundary; multi-commit on one branch is fine. The goal is single
(unify identity + introduce auth); the commits exist for review legibility.

## 13. Test plan

### 13.1 Unit (store layer)

- `users`: create + get_by_id + duplicate-id rejection.
- `accounts`: create + get_by_id + UNIQUE(auth_provider, email) enforcement;
  cascading delete on user removal.
- `sessions`: create + lookup + revoke + last_seen update; expired session
  not returned by lookup.
- `api_tokens`: create-with-hash + lookup-by-hash + revoke + last_used
  update; raw token never stored; case-insensitive hash lookup.

### 13.2 Middleware

- Cookie request → Actor injected with `AuthKind::Session`.
- Token request → Actor injected with `AuthKind::ApiToken(Cli|Bridge)`.
- Both present → cookie wins (browser fallback semantics).
- Revoked credential → 401.
- No credential, non-exempt route → 401.
- Exempt routes (`/health`, `/api/auth/local-session`, static assets) → no
  Actor required.

### 13.3 Local flows (integration)

- `chorus setup` writes credentials.toml with mode 0600; DB has exactly one
  User, one local Account, one CLI token.
- `chorus send` after setup → message persisted with `sender_id =
  users.id` and `sender_type='user'`.
- `chorus logout` then `chorus send` → 401, no message persisted.
- `chorus login --local` after logout → mints a new token, reuses existing
  User+Account.
- Loopback `POST /api/auth/local-session` → returns user info + sets cookie;
  remote-origin same request → 404.
- After setup, `GET /api/whoami` (with token) returns the User from the
  one local Account.

### 13.4 Bridge

- Bridge auth using `api_tokens` with `kind='bridge'` → bridge endpoints
  accept; CLI endpoints reject (`kind != cli`).

### 13.5 Regression (per eng review)

- T1: Lose `~/.chorus/credentials.toml`, DB intact → CLI prompts to log in
  (no silent identity drift, because the server no longer has a
  "remembered" identity to fall through to).
- T2: setup → serve identity continuity: e2e test that runs setup, starts
  serve, hits `/api/whoami` with the minted token, asserts user id matches
  the row setup wrote.
- T3: `/api/system-info.local_human` is replaced by `/api/whoami` and the
  test asserts the new endpoint returns the expected shape.
- T4: `UserError → CliError` rename: `main.rs` downcast still distinguishes
  CLI-friendly errors.
- T5: Old config.toml with `[local_human]\nname = "..."` still parses cleanly
  (unknown fields ignored).

### 13.6 Manual smoke checklist

```
[ ] rm -rf ~/.chorus && cargo run -- setup
[ ] cat ~/.chorus/credentials.toml          # token present, 0600
[ ] cat ~/.chorus/config.toml               # no [local_human] section
[ ] cargo run -- serve --port 3001
[ ] curl -H "Authorization: Bearer $(grep token ~/.chorus/credentials.toml | cut -d'"' -f2)" \
       http://127.0.0.1:3001/api/whoami      # returns user
[ ] curl http://127.0.0.1:3001/api/whoami    # 401
[ ] open http://127.0.0.1:3001               # UI loads, no manual login
[ ] cargo run -- logout && cargo run -- send "hi"   # 401, helpful error
```

## 14. Failure modes

| Codepath | Failure | Test? | Handling | User sees |
| --- | --- | --- | --- | --- |
| Credentials file 0600 perms violated | Token leak | yes (perm assertion in setup test) | refuse to start CLI if perms loose | clear error |
| Credentials file present but token revoked | CLI requests fail | yes | 401 → "log in again" | clear message |
| Setup interrupted between DB tx and credentials write | DB has User but credentials don't | yes | transaction rolls back on credentials write failure | re-run setup |
| Multiple local Accounts (cloud-era mis-install) | local-session endpoint ambiguous | yes | refuse, log: "multiple local accounts; not supported" | error |
| `auth_provider='local'` Account disabled | local-session fails | yes | 403 | clear message |
| Token hash collision (theoretical) | wrong account resolved | n/a | SHA-256, infeasible | n/a |

## 15. Open decisions

These need user calls before implementation starts.

### D1 — Session expiry policy in local mode

- **A** (recommended): no expiry (`expires_at = NULL`). Local sessions
  persist until explicit logout or DB reset. Matches the "single user owns
  this machine" reality.
- **B**: 30-day rolling expiry. Closer to cloud defaults but adds a "your
  local UI suddenly asks you to re-auth" moment for no real-world threat
  model.

### D2 — Token format

- **A** (recommended): `chrs_local_<base64url(32 random bytes)>` for local,
  `chrs_<provider>_<base64url(32)>` for cloud. Prefix encodes provenance for
  human-readable token strings (helps in shell history audits).
- **B**: opaque random string, no prefix. Simpler; no provenance signal.

### D3 — Workspace ownership rename

- **A** (recommended): `created_by_human_id` → `created_by_user_id`. User-
  specific column, matches the new noun. Agent-creates-workspace remains
  deferred.
- **B**: generic `(created_by_id, created_by_type)` to match other tables.
  Bigger sweep, no caller asks for it today.

### D4 — Frontend `sender_type` wire string

- **A** (recommended): rename `"human"` → `"user"` in the JSON wire format
  too; update frontend in the same PR. Honest: the schema layer and the
  wire layer move together.
- **B**: keep `"human"` on the wire via `#[serde(rename = "human")]` so the
  frontend doesn't need to change. Saves a sweep but plants a discrepancy
  between Rust `User` and JSON `"human"`.

### D5 — Bridge token `kind` value

- **A** (recommended): `kind='bridge'` distinct from `kind='cli'`. Middleware
  can enforce "bridge endpoints reject CLI tokens" for accidental misuse.
- **B**: single `kind='api'` for both. Simpler; loses the misuse guard.

Default if no answer is given: pick all the (A) options.

## 16. NOT in scope

| Deferred | Why |
| --- | --- |
| Cloud auth providers (Google, GitHub, …) | Schema and middleware support it; issuance flows are not built. |
| Token rotation / refresh | `expires_at` column exists; policy is future work. |
| Per-token scopes / permissions | One token = full account access today. Future PR adds scope strings if needed. |
| Multi-account UI ("switch account") | Local mode has exactly one local account. Cloud-era UI feature. |
| Bridge protocol changes | The bridge keeps speaking its current WS protocol; only the auth header changes. The bridge-platform-protocol plan (`docs/plan/bridge-platform-protocol.md`) is orthogonal. |
| Migrating workspace ownership to a generic Actor | Same reasoning as #144's deferred items. |
| Backwards-compat path for existing local DBs | Per project convention, schema changes ship via "delete the DB" not migrations. |

## 17. Verification

| Layer | Command |
| --- | --- |
| Store unit tests | `cargo test --lib` |
| Handler + middleware | `cargo test` |
| End-to-end | `cargo test --test e2e_tests` |
| Frontend type check | `cd ui && npx tsc --noEmit` |
| Frontend tests | `cd ui && npm run test` |
| Manual smoke | section 13.6 |
| Health-stack baseline | `cargo clippy -- -D warnings` |

The eng-review skill's "regression rule" (T1–T5) is captured in 13.5.

## 18. Risks

1. **PR size.** ~10 commits, ~1500-2000 LOC changed. Mitigated by
   independent-green commits and explicit review-order in section 12.
2. **Frontend churn.** Section 11 lists the UI changes; they're mechanical
   but touch every component that renders `sender_type`. If D4 picks (B),
   this risk halves at the cost of a wire-vs-storage discrepancy.
3. **Auth middleware regressions.** Every endpoint now goes through the
   layer. Mitigated by: (a) shipping it permissive in commit 2, (b) flipping
   to required only after commit 6 lands and the sweep is verified.
4. **Bridge breakage.** Commit 7's bridge-auth subsume happens after all
   handlers migrate; bridges run as today through commits 1-6.

## 19. What this resolves

- **#144 entirely.** The terminology mess (`Human`/`User`/`local_human`)
  collapses into `User`/`Account`. The two resolvers vanish. The schema
  inconsistency (`created_by_human_id` vs generic patterns) gets the rename
  to `created_by_user_id`. The cached `name` field gets deleted.
- **Eng review's A1/A2/A3/Q1.** All four are symptoms of "server caches
  identity at boot." Removing that pattern removes all four.
- **Foundation for cloud.** Auth middleware + Sessions + ApiTokens are
  exactly what cloud needs. The local-mode flows are the special cases of a
  general system, not the system itself.

---

## Awaiting

D1–D5. Default if no answer: all (A). Once decided, I write a one-line
"Decisions: D1=A, D2=A, …" header and start commit 1.
