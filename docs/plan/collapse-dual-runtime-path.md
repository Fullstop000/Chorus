# Collapse the dual runtime path — every agent bridge-owned

> **Status:** done. Phases 1, 2+3, and 4 shipped on `main` via
> [#150](https://github.com/Fullstop000/Chorus/pull/150),
> [#152](https://github.com/Fullstop000/Chorus/pull/152), and
> [#153](https://github.com/Fullstop000/Chorus/pull/153). The remaining
> "phase 5" cleanup (this doc, the `bridge-hosted` comments and dead
> branches that survived) lands as housekeeping. The original goal — no
> more `if machine_id.is_some()` runtime branches in handlers — is met.

Tracking issue: [#149](https://github.com/Fullstop000/Chorus/issues/149).
Built on: [#147](https://github.com/Fullstop000/Chorus/pull/147) (id-keyed AgentManager).

---

## Goal

`chorus serve` stops being a runtime owner. Every agent — local or remote —
is owned by a bridge client that reconciles desired state from
`bridge.target` frames. The platform's only job becomes: persist agents,
broadcast targets, route chats. Handlers no longer branch on
`agent.machine_id.is_some()`.

Outcome: one ownership rule ("every agent has exactly one owner: a bridge
client"), one start/stop code path, the wrong-key-to-lifecycle bug class
(#147) becomes structurally impossible because handlers don't call
lifecycle methods.

## Non-goals

- Cross-machine deployments. Single-machine `chorus serve` stays the
  default; the in-process bridge client is what powers it.
- Reworking the bridge wire protocol. We re-use `bridge.target`,
  `chat.message.received`, `agent.state` as-is.
- Replacing `AgentManager`. It moves under bridge ownership unchanged.
- Tearing out the activity-log / trace observability surface. That
  stays where it is — only the lifecycle methods move.

## End state

```
                      ┌───────────────────┐
   chorus serve  ───► │ HTTP server       │  store + broadcast_target_update
                      └────────┬──────────┘
                               │ ws://127.0.0.1:<port>/api/bridge/ws
                      ┌────────▼──────────┐
                      │ in-proc bridge    │  reconcile → AgentManager
                      │ client            │  (the only AgentManager in serve)
                      └────────┬──────────┘
                               │ stdio / acp / mcp
                          [ agents ]
```

Bridge clients are interchangeable: the in-process one boots inside
`chorus serve`, remote ones boot from `chorus bridge`. Both speak the
same WS frames against the same `/api/bridge/ws` endpoint.

## Phases

Each phase is a separately mergeable PR that keeps the tree green.

### Phase 1 — `local_machine_id` is real

**Why first:** the rest of the plan depends on every agent row having
a non-NULL `machine_id`. Doing this without the runtime flip lets us
land the migration and the config plumbing without touching handlers.

- `ChorusConfig::machine_id` becomes mandatory; serve generates one on
  first run and writes it to `config.toml` (we already write `machine_id`
  there optionally — flip it to required + auto-fill).
- Migration at startup: `UPDATE agents SET machine_id = ?1 WHERE
  machine_id IS NULL` keyed by the local id.
- Handler `create_and_start_agent`: when the request omits `machine_id`,
  fill with `state.local_machine_id`. Behavior unchanged — local agents
  still take the `start_agent` branch — but every row now has a value.

Verification: `cargo test`, manual smoke (create agent, restart serve,
verify rows have machine_id).

### Phase 2 — In-process bridge client wired into `chorus serve`

**Why:** prove the bridge client can drive lifecycle for the local
machine before we ask handlers to trust it.

- `chorus serve` spawns a bridge client (`run_bridge_client`) on
  startup with `local_machine_id`, dialing its own
  `ws://127.0.0.1:<port>/api/bridge/ws`.
- `AgentManager` ownership moves: the platform's `AppState.lifecycle`
  still points at the same manager instance for now (so the dual path
  still works), but the bridge client gets a *handle* to that same
  manager rather than constructing a second one. One manager, two
  callers, both id-keyed.
- Boot order: bridge client must be ready to receive `bridge.target`
  before HTTP accepts external traffic, otherwise the first
  `POST /api/agents` broadcasts to nobody. Either (a) start the HTTP
  server, then have the bridge client connect, then run a one-shot
  initial reconcile by re-broadcasting all current agents; or (b)
  delay accepting external requests until the bridge client emits
  `bridge.hello`. Going with (a) — simpler, same final state.

Verification: with the bridge client live, manually trigger an agent
create and confirm `bridge.target` reaches the in-process client and
`agent.state{started}` comes back. Existing tests still pass because
handlers still use the direct path.

### Phase 3 — Handlers stop calling lifecycle methods

**This is the load-bearing flip.** Six call sites in `agents.rs`, one
in `messages.rs::deliver_message_to_agents`, one in `decisions.rs`.

- Drop the `if params.machine_id.is_some() { skip } else { start }`
  shape. Always: insert/update row → `broadcast_target_update`. Never
  call `lifecycle.start_agent` / `stop_agent` / `notify_agent` from
  the handler.
- `deliver_message_to_agents`: remove the `machine_id.is_some()`
  early-continue. Always emit `chat.message.received` for any agent
  with subscribers — the bridge client decides whether to wake.
- `resume_with_prompt`: the tight-loop synchronous case becomes
  asynchronous. Persist the envelope as a pending `init_directive`
  on the `agents` row (new column, or piggyback on an existing
  per-agent state field). Bridge client picks it up on next
  reconcile and either pipes to live session or starts the agent
  with that directive. The decision handler returns 202 Accepted;
  UI watches the realtime stream for confirmation.

HTTP API contract change: `POST /api/agents` returns 202 with
`status: "pending"` instead of 200/`start_error`. UI updates required
in same PR. (If we want to stage this, we can keep returning 200 with
no `start_error` field while the UI catches up — flip to 202 in a
follow-up.)

Verification: full Playwright smoke (`qa/cases/playwright`), MSG-001
through MSG-006 with real LLMs (CHORUS_E2E_LLM=1), the same set we ran
for #147. Add a dedicated Playwright case that verifies the agent
lifecycle path under network drop of the bridge client.

### Phase 4 — `AgentLifecycle` trait shrinks to observability

- Trait keeps: `get_activity_log_data`, `get_all_agent_activity_states`,
  `active_run_id`, `set_run_channel`, `run_channel_id`,
  `process_state` (read-only inspection).
- Trait drops: `start_agent`, `stop_agent`, `notify_agent`,
  `resume_with_prompt`. These move to bridge-client-internal APIs.
- `AppState.lifecycle` becomes `AppState.observability` (or splits in
  two). Handlers only use it to render activity feeds and runtime
  status badges.
- `MockLifecycle` in tests becomes `MockObservability`; tests that
  verified "lifecycle.start_agent was called" become "verify a
  `bridge.target` was broadcast with the expected agent". Largest
  test refactor of the plan — but mechanical.

Verification: `cargo test`, `cargo test --test e2e_tests`, `cd ui &&
npm run test`.

### Phase 5 — Cleanup

- Delete the `if machine_id.is_some()` dead code paths and their
  comments.
- Delete `AppState.transitioning_agents` if the bridge client now owns
  per-agent transition serialization (it already does via the WS frame
  ordering — we just need to confirm the same `CONFLICT` UX still works
  for double-clicks; if not, keep the guard).
- Update `docs/BRIDGE_MIGRATION.md` and `docs/plan/bridge-platform-protocol.md`
  to mark this work done and document "the in-process bridge client is
  not a special case, it's the only client `chorus serve` ships with".

## Decisions to lock before phase 1

These are the questions whose answers shape the plan. I have leanings,
none are settled.

| Decision | Lean | Why |
|---|---|---|
| In-process bridge client speaks WS to localhost vs. in-memory channel | **WS** | Free integration coverage on every `chorus serve`; the WS hop is loopback so cost is negligible. Issue #149 explicitly recommends this. |
| `resume_with_prompt` — persist as pending `init_directive` column vs. broadcast a one-shot frame | **Column** | Survives bridge client reconnect; doesn't require a new frame type; matches existing `init_directive` shape on creation. |
| `POST /api/agents` 200/`start_error` → 202/`pending` in same PR or staged | **Same PR (Phase 3)** | The UI is in the same repo; staging adds shim code that immediately becomes dead. Easier to land + revert as one unit. |
| `local_machine_id` source — generate fresh UUID vs. derive from hostname | **Generate UUID, persist in config.toml** | Hostname collisions across machines (laptops named `mbp.local`) would be silent and weird; UUID is unambiguous. |
| Migrate NULL machine_id rows in place vs. require manual reset | **In place at startup** | Existing dev DBs would otherwise need explicit attention; the migration is a single UPDATE keyed off the local id. |

## Risks

- **Boot order race.** First request after start must not broadcast to
  a missing client. Mitigated by initial reconcile after bridge client
  connects (Phase 2 design).
- **Test surface.** ~30+ tests use `MockLifecycle`. Phase 4 is the bulk
  of the test refactor; if we discover a class of test that resists the
  flip, we may need an in-test "synthetic bridge" helper.
- **Decision flow latency.** Today `resume_with_prompt` is synchronous;
  the bridge round-trip adds ~5–20ms typical, more under contention. UX
  on `Resolve` button changes from instant to "spinner for a beat".
  Acceptable, but worth verifying with QA.
- **`stop_agent` ordering.** Today the handler stops the agent then
  deletes the row. Under the new flow we delete the row, broadcast,
  bridge client stops it. If the platform crashes between row delete
  and broadcast, the bridge client never learns to stop — orphaned
  process. Mitigation: bridge client treats "running but not in target"
  as stop signal on every reconcile (already true in `reconcile.rs`).

## Success criteria

- Zero call sites of `lifecycle.start_agent` / `stop_agent` /
  `notify_agent` / `resume_with_prompt` in `src/server/handlers/`.
- Zero `if machine_id.is_some()` branches in `src/server/handlers/`.
- `agents.machine_id` is `NOT NULL` (schema-enforced).
- Full Playwright smoke + LLM-gated MSG suite green on a single PR
  representing the merged phases.
- `chorus bridge` (the standalone binary) and `chorus serve` (the
  in-process one) share 100% of their lifecycle code path.

## What this is not committing to yet

- The exact PR boundaries between phases — phases are the natural
  break points but we may bundle 4+5 together if the test refactor is
  small.
- The exact `AppState.lifecycle` rename or split. Phase 4 will reveal
  whether one trait or two reads better.
- Whether `AppState.transitioning_agents` survives. Decided in Phase 5.
