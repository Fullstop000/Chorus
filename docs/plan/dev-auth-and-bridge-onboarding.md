# Dev Auth + User-Scoped Bridge Tokens + Device Onboarding

**Purpose:** Make a remote-deployed Chorus install (GCP VM, homelab,
etc.) end-to-end usable for a solo operator without OAuth wiring.
Covers three couplings that have to land together:

1. A dev auth provider, allowlist-gated, that lets the UI accept
   browser logins from a non-loopback host without rolling real OAuth.
2. A user-scoped bridge token model (one token per user, shared across
   that user's machines), with a `bridge_machines` registry table for
   live tracking.
3. A device-onboarding flow: log in, click "Add device" once, copy the
   shown script into each device's terminal, paste, done.

**Status:** Design — refined through a grill session on 2026-05-13.
Not yet implemented.

---

## 1. Goals

- Run `chorus serve` on a non-loopback host and use the UI from any
  browser the operator owns, without standing up real OAuth.
- Onboard a new device with one paste from Settings → Devices to the
  device's terminal. `chorus bridge` comes up zero-arg.
- Honest revocation: Rotate kicks every device sharing the bearer;
  Kick disconnects a single device until Forget.
- Don't paint the auth surface into a corner that real OAuth (Google
  / GitHub / email) would have to re-shape later.

## 2. Non-goals

- Real OAuth providers — covered by `accounts.auth_provider` already;
  not delivered in this PRD.
- TLS termination inside `chorus serve`. Continue to expect an
  external reverse proxy in front for production-ish use.
- Multi-user / multi-tenant boundaries. Same workspace rules apply
  regardless of how the bearer was obtained.
- Bridge tokens reusable across users. Each token binds to one user.
- `install.sh` / release-binary distribution. Operator installs
  `chorus` on each device themselves; the onboarding script checks
  for `chorus` on PATH and prints `cargo install --git …` if missing.
  Swap for release-binary install when release CI ships.

## 3. Dev auth provider

### 3.1 Endpoint

`POST /api/auth/dev-login { username }` — mounted only when
`CHORUS_DEV_AUTH=1` is set. Otherwise unrouted (404).

When mounted, on a valid call:

- Looks up `username` against `CHORUS_DEV_AUTH_USERS` (comma-separated
  allowlist). If not present, returns 403.
- Finds-or-creates a User + Account pair with
  `(auth_provider='dev', email=<username>@dev.local)`. (Email column
  reused as the `UNIQUE(auth_provider, email)` lookup key — see
  `src/store/schema.sql` `accounts`.)
- Inserts a `sessions` row and returns the standard `chorus_sid`
  HttpOnly + SameSite=Strict + Path=/ cookie that the local-session
  endpoint returns today.

The route has **no Origin / loopback gate** — that's the point. The
existing `/api/auth/local-session` loopback-only path is unchanged.

### 3.2 Refuse-to-start

`chorus serve` exits at startup if `CHORUS_DEV_AUTH=1` and
`CHORUS_DEV_AUTH_USERS` is empty or unset. An empty allowlist means
nobody can log in, which is always a misconfiguration.

Sidecar effects when dev auth is enabled (and the allowlist is
non-empty, so the server starts):

- WARN log: `dev auth enabled for users: [...]. This install is
  access-controlled by network reachability only.`
- Non-dismissible yellow banner in UI: `Dev auth enabled.`
- `/health` response includes `"dev_auth": true` so external
  watchdogs can alert if the flag stays on past expected windows.

### 3.3 What it unlocks

- The Chorus UI works from a non-loopback browser against the
  operator's GCP install (today blocked by `local-session`'s
  loopback gate).
- The bridge-onboarding flow has somewhere for the operator to land
  ("log into Settings → Devices") on a remote install.

## 4. User-scoped bridge tokens

### 4.1 Model

- **One bridge token per user**, materialized at first
  `POST /api/devices/mint`.
- **Many machines can share the same token** — each machine
  registers in `bridge_machines` on first `bridge.hello`.
- **One bridge per (token, machine_id) at a time.** Conflicts are
  handled by supersede (existing `bridge_registry` behavior) unless
  the row was explicitly Kicked, in which case the new connection
  is rejected.
- **Tokens never expire** — only revocable via Rotate (revoke +
  mint-new) or per-device Kick.

### 4.2 Disambiguating CLI vs bridge tokens

Today `api_tokens` distinguishes CLI from bridge tokens by
`machine_id IS NULL / NOT NULL`. User-scoped bridge tokens have
`machine_id IS NULL`, which would collide with CLI tokens. Add an
explicit discriminator:

```sql
-- new column on api_tokens
provider TEXT NOT NULL CHECK (provider IN ('local','bridge'))
```

The auth resolver in `src/server/bridge_auth.rs` now branches by
`(provider, machine_id)`:

| `provider` | `machine_id` | Outcome                                                                                                                                       |
| ---------- | ------------ | --------------------------------------------------------------------------------------------------------------------------------------------- |
| `local`    | NULL         | CLI bearer. `/api/*` as the User; `/internal/agent/{id}` only for agents owned by the User.                                                   |
| `bridge`   | Some(m)      | Pre-PRD legacy bridge token. Only agents with `agents.machine_id = m`.                                                                        |
| `bridge`   | NULL         | **New.** User-scoped bridge token. On `bridge.hello`, `machine_id` comes from the frame. Registers in `bridge_machines`. Rules per §4.1, §4.3. |

Values match what's already passed to `generate_raw_token()`
(`"local"`, `"bridge"`); the bearer prefix `chrs_local_…` /
`chrs_bridge_…` becomes self-documenting.

### 4.3 `bridge_machines` table

```sql
CREATE TABLE IF NOT EXISTS bridge_machines (
    token_hash       TEXT NOT NULL REFERENCES api_tokens(token_hash) ON DELETE CASCADE,
    machine_id       TEXT NOT NULL,
    hostname_hint    TEXT,                     -- what the bridge self-reported on first hello
    first_seen_at    TEXT NOT NULL DEFAULT (datetime('now')),
    last_seen_at     TEXT NOT NULL DEFAULT (datetime('now')),
    disconnected_at  TEXT,                     -- NULL = live WS; set = not currently connected
    kicked_at        TEXT,                     -- set = operator clicked Kick; reconnect rejected
    PRIMARY KEY (token_hash, machine_id)
);

CREATE INDEX IF NOT EXISTS idx_bridge_machines_active
    ON bridge_machines(token_hash) WHERE disconnected_at IS NULL;
```

State machine:

| Event                                                            | Effect on row                                                                  |
| ---------------------------------------------------------------- | ------------------------------------------------------------------------------ |
| First `bridge.hello` for an unseen `(token, machine_id)`         | Insert; `disconnected_at = NULL`, `kicked_at = NULL`.                          |
| `bridge.hello` for an existing row, `disconnected_at IS NULL`    | Supersede the live WS (existing `bridge_registry` behavior); bump `last_seen_at`. |
| `bridge.hello` for an existing row, `kicked_at IS NOT NULL`      | Reject with WS close 4004 `kicked`. Row unchanged.                              |
| `bridge.hello` for an existing row, `disconnected_at IS NOT NULL` but `kicked_at IS NULL` | Clear `disconnected_at`; bump `last_seen_at`. Normal reconnect.    |
| Clean WS drop, last sender for that pair                          | Stamp `disconnected_at = now()`.                                                |
| Operator clicks **Kick** in Settings                              | `disconnected_at = now()`, `kicked_at = now()`; close live WS with 4004.        |
| Operator clicks **Forget** in Settings                            | Hard-delete the row. (Future reconnect re-creates it.)                          |
| Operator clicks **Rotate**                                        | Revoke the `api_tokens` row (set `revoked_at`); sweep all live WS bound to the token with 4005. Rows in `bridge_machines` stay (filtered out in UI by `JOIN api_tokens ON ... WHERE revoked_at IS NULL`). |

### 4.4 One-shot reveal of the raw bearer

The raw bearer is **never stored** server-side. It's generated in the
response handler for `POST /api/devices/mint`, baked into the returned
script body, and discarded when the response finishes streaming.

- **First `POST /api/devices/mint`** for a user: server mints token
  (hash to `api_tokens`), constructs the script body with the raw
  bearer literal, returns it. Raw never touches the DB.
- **Subsequent calls** (an unrevoked bridge token already exists for
  this user): server returns 410 Gone. Raw is unrecoverable. The
  operator must Rotate to get a new one.

UI consequence: Settings → Devices, on first visit ever for a user,
shows a big yellow "Copy this once — you won't see it again" code
block with the script body. After the operator copies and navigates
away (or refreshes), the only path to a script again is Rotate.

This matches OpenAI / Anthropic / GitHub PAT semantics. Sharp edge
("refreshed too fast → must Rotate") is shipped intentionally —
same friction every API platform ships, for the same reason.

## 5. Wire-protocol delta

Existing protocol: `docs/plan/bridge-platform-protocol.md`. Only the
deltas below.

### 5.1 `bridge.hello` machine_id binding

| Token shape                                | Behavior                                                                                                                                |
| ------------------------------------------ | --------------------------------------------------------------------------------------------------------------------------------------- |
| `provider='bridge', machine_id=set` (legacy) | Unchanged. `hello.machine_id` must match `token.machine_id` or close.                                                                   |
| `provider='bridge', machine_id=NULL` (new)   | `machine_id` taken from `hello.machine_id`. Apply the `bridge_machines` state machine in §4.3.                                          |

### 5.2 `machine_id` source

Bridge resolves `machine_id` in this order:

1. `bridge-credentials.toml` `machine_id` line if present (persisted
   from a previous successful hello — survives hostname changes).
2. `hostname` command output (sanitized — lowercase, `[a-z0-9-]+`,
   truncated to 32 chars).
3. Random `mch_<base32>` fallback if `hostname` unavailable.

Server **echoes back** the accepted `machine_id` in the `bridge.target`
reply (new `bridge.target.assigned_machine_id` field). On hostname
collisions across the user's devices, the server suffix-disambiguates
(`mbp` → `mbp-2`). Bridge writes the assigned value back to its
credentials file; subsequent runs use the persisted value.

No user-facing rename in v1. For pretty names, set sensible hostnames
before onboarding.

### 5.3 New close codes

Extending the 4001 / 4002 / 4003 catalog from
`docs/plan/bridge-platform-protocol.md` §4:

| Code | Name           | When emitted                                                                                          |
| ---- | -------------- | ----------------------------------------------------------------------------------------------------- |
| 4004 | `kicked`       | `bridge.hello` arrived for a `(token, machine_id)` row whose `kicked_at IS NOT NULL`                  |
| 4005 | `token_revoked`| The bearer was Rotated; server is sweeping live WS bound to the old token                              |

### 5.4 Bridge client-side reaction

| Server signal                  | Bridge logs                                                                                          | Exit |
| ------------------------------ | ---------------------------------------------------------------------------------------------------- | ---- |
| HTTP 401 on WS upgrade          | `Token rotated or revoked. Get a new script from Settings → Devices on https://<host>.`              | 2    |
| WS close 4004 `kicked`          | `Disconnected from the platform. Re-onboard from Settings → Devices to reconnect.`                   | 2    |
| WS close 4005 `token_revoked`   | `Bridge token rotated. Get the new script from Settings → Devices on https://<host>.`                | 2    |
| Anything else                   | (current behavior — backoff + retry)                                                                  | n/a  |

Exit code 2 means "this will not recover by retrying." Process
supervisors and humans both treat it as actionable.

## 6. Device onboarding — UI + CLI

### 6.1 HTTP surface

| Method   | Path                                | Purpose                                                                                            |
| -------- | ----------------------------------- | -------------------------------------------------------------------------------------------------- |
| `POST`   | `/api/auth/dev-login`                | Dev-auth login (mounted only when `CHORUS_DEV_AUTH=1`)                                              |
| `POST`   | `/api/devices/mint`                  | First-mint: returns the onboarding script body with bearer embedded (once)                          |
| `GET`    | `/api/devices`                       | List current user's onboarded devices                                                              |
| `DELETE` | `/api/devices/{machine_id}`          | Kick (set `disconnected_at` + `kicked_at`, close live WS with 4004, block reconnect until Forget)   |
| `DELETE` | `/api/devices/{machine_id}?forget=1` | Forget (hard-delete the row, future reconnect re-creates it)                                        |
| `POST`   | `/api/devices/rotate`                | Revoke + mint-new (kicks all live bridges with 4005, returns fresh script body once)                |

### 6.2 Settings → Devices page

**First visit ever (no bridge token row yet for this user):**

```
┌──────────────────────────────────────────────────────────────┐
│ Devices                                                      │
│                                                              │
│ ⚠ You'll only see this once. Save it somewhere safe          │
│   (password manager, snippet store).                         │
│                                                              │
│ ┌──────────────────────────────────────────────────────────┐ │
│ │ #!/usr/bin/env bash                                      │ │
│ │ set -euo pipefail                                        │ │
│ │ ... [full script, ~15 lines, bearer literal]             │ │
│ └──────────────────────────────────────────────────────────┘ │
│ [Copy script]   [I've saved it — close]                      │
└──────────────────────────────────────────────────────────────┘
```

The "I've saved it — close" button is the operator's
acknowledgement. Closing the modal removes the script from the
DOM immediately. There is **no** "show me again" affordance.

**All subsequent visits:**

```
┌──────────────────────────────────────────────────────────────┐
│ Devices                                                      │
│                                                              │
│ ▸ laptop-zht    active   last seen 2s ago     [Kick]         │
│ ▸ homelab-01    active   last seen 14s ago    [Kick]         │
│ ▸ mbp-2         offline  last seen 2d ago     [Forget]       │
│                                                              │
│ [Rotate token]   Disconnects all devices.                    │
└──────────────────────────────────────────────────────────────┘
```

Rotate prompts: `Rotate disconnects every active device (N) and
requires re-pasting the new onboarding script on each. Continue?`
On confirm: `POST /api/devices/rotate` returns a fresh script body
once, same modal UX as first-visit.

### 6.3 The onboarding script body

```bash
#!/usr/bin/env bash
set -euo pipefail

if ! command -v chorus >/dev/null 2>&1; then
  echo "Install Chorus first:"
  echo "  cargo install --git https://github.com/Fullstop000/Chorus chorus"
  exit 1
fi

DATA_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/chorus/bridge"
mkdir -p "$DATA_DIR" && chmod 700 "$DATA_DIR"
umask 077
cat > "$DATA_DIR/bridge-credentials.toml" <<EOF
host  = "chorus.your.host"
token = "chrs_bridge_..."
EOF

echo "Connecting → chorus.your.host …"
exec chorus bridge
```

The `cargo install` line is the v1 install instruction. Swap for a
release-binary install when release CI ships; that's a one-line edit
in the script template, no API or schema change.

### 6.4 `chorus bridge` CLI

```rust
Bridge {
    /// Override the default data dir
    /// ($XDG_DATA_HOME/chorus/bridge).
    #[arg(long)]
    data_dir: Option<String>,
},
```

That's it. Host + token come from `bridge-credentials.toml` inside
the data dir. `machine_id` is auto-derived (§5.2). Embedded MCP
loopback port defaults to `127.0.0.1:0`; override via
`CHORUS_BRIDGE_LISTEN` env if a specific port is required.

Zero-arg happy path:

```bash
chorus bridge
```

## 7. Migration

None. Schema changes land via direct edit to `src/store/schema.sql`
(matching how #157 landed). Existing local installs must wipe
`~/.chorus` to pick up the new tables and columns — same upgrade
posture as #157.

Pre-PRD legacy bridge tokens (`provider='bridge', machine_id=set`)
keep working through the auth resolver branch in §4.2. No forced
re-mint.

## 8. Test plan

Backend (`cargo test`):

- `api_tokens` round-trip with both shapes (`provider='local'`,
  `provider='bridge'+machine_id=NULL`).
- Auth resolver branches by `(provider, machine_id)`.
- `bridge_machines` state machine: insert / supersede / Kick /
  reconnect-after-Kick rejected / Forget-deletes-row / Rotate-sweeps.
- Dev-login allowlist enforcement + refuse-to-start on empty
  allowlist.
- `POST /api/devices/mint` returns script body once; second call
  returns 410.
- `POST /api/devices/rotate` revokes old, sweeps active WS with 4005.

Integration (`cargo test --test e2e_tests`):

- Bridge with user-scoped token registers two machines, sees both
  in `bridge_machines`.
- Kick → reconnect rejected (4004). Forget → reconnect accepted.
- Rotate → existing bridge sees 4005 + exits non-zero.

Browser (Playwright, `qa/cases/auth.md` additions):

- `AUTH-002` — dev-login mints session for an allowlisted user;
  refuses for a non-allowlisted user.
- `AUTH-003` — first-visit Settings → Devices flow: mint script,
  copy, exec against test server, device appears in list. Kick,
  Forget, Rotate operations covered.

## 9. Phasing

Two PRs.

**PR A — schema + backend.** Adds `provider` column, `bridge_machines`
table, dev-login route + guardrails, user-scoped bridge token mint
path, `bridge_ws` branch for `provider='bridge', machine_id=NULL`,
device-management endpoints, bridge CLI collapse, credentials-file
parsing. Includes the Rust-level + e2e tests in §8. UI not yet
shipped; exercised end-to-end via curl + `chorus bridge`.

**PR B — UI.** Settings → Devices page, dev-auth banner, Playwright
coverage, one-shot reveal modal.

Splitting this way keeps PR A reviewable (a backend story exercisable
from curl) and lets PR B move at its own UI iteration cadence.

## 10. Open questions

- **Real OAuth provider shape.** When email / Google / GitHub
  providers land, they slot into `accounts.auth_provider` alongside
  `'local'` and `'dev'`. Document the callback shape
  (`POST /api/auth/callback/{provider}`) before this PRD ships so
  reviewers can flag inconsistencies with the dev-auth route.
- **`bridge_machines` orphan cleanup.** Rotation soft-revokes the
  old `api_tokens` row but `ON DELETE CASCADE` only fires on hard
  delete. After rotation, old `bridge_machines` rows are orphans
  tied to a revoked token. UI filters them out by
  `JOIN api_tokens ON ... WHERE revoked_at IS NULL`; a periodic
  sweep hard-deletes them after a grace period. Not blocking;
  follow-up.
- **`hostname` quality on macOS.** Persistence of `machine_id` in
  `bridge-credentials.toml` after first hello mitigates hostname
  drift across reboots and network changes; the first hello is the
  fragile moment.
- **Release-binary install.** When release CI ships, swap the
  `cargo install --git` line in §6.3. No schema or API change.
