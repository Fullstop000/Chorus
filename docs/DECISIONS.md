# Decision Inbox

Mechanism that lets an agent ask the human to pick between alternatives without blocking on a chat reply. The agent calls one MCP tool (`chorus_create_decision`), ends its turn cleanly, and gets resumed with a self-contained envelope as the new turn prompt when the human picks via the inbox UI.

Read this before:
- Adding a new payload field — both Rust types and the TS mirror must move together.
- Touching the resume path — see `Lifecycle` below for how envelopes flow back.
- Writing a tool that an agent will call instead of `send_message` — match the conventions here.

Source-of-truth design: [`chorus-design-reviews/explorations/2026-04-30-pr-review-vertical-slice/design.md`](https://github.com/Fullstop000/chorus-design-reviews/blob/main/explorations/2026-04-30-pr-review-vertical-slice/design.md) (r7 · commit `3a38b22`).

---

## Lifecycle

```
agent: chorus_create_decision(payload)
       └─▶ bridge attaches X-Agent-Id from auth context
       └─▶ bridge validates payload (3 structural rules + 6 length caps)
       └─▶ bridge POSTs to internal handler (Day 3)
       └─▶ server INSERTs row in `decisions` (status=Open) (Day 3)
       └─▶ tool returns { decision_id }
       └─▶ agent ends turn cleanly (the system prompt enforces this)

─── (later, possibly hours; agent process can exit and be re-spawned) ───

human picks via UI (click an option)
       └─▶ POST /api/decisions/:id/resolve { picked_key, note? } (Day 3)
       └─▶ server: CAS UPDATE WHERE status='Open' — 0 rows → 409
       └─▶ build self-contained envelope text containing original
           headline + question + picked option's label + body + note
       └─▶ AgentLifecycle::resume_with_prompt(session_id, envelope) (Day 3)
       └─▶ on failure: revert row to Open, return 5xx (human re-picks)

agent: receives envelope as new ACP `session/prompt` → acts (gh pr review, etc.)
```

The envelope is **self-contained**. The agent can act on it without recalling prior session state. Runtime context restoration via `RuntimeDriver::resume_session` (`src/agent/drivers/mod.rs:548-593`) is a UX bonus, not a requirement.

---

## Implementation status

| Phase | Day | Status |
|---|---|---|
| Types + validator + tests + TS mirror + drift fixture | 1 | shipping in this PR |
| Bridge tool registration + system-prompt patch | 1 | shipping in this PR (handler is a 501 stub) |
| Driver integration (`claude.rs`) | 2 | not started |
| Storage (`decisions` table, handlers, resolve, `resume_with_prompt`) | 3 | not started |
| React inbox (list → click → pick) | 4 | not started |
| Meta-circular dogfood (land the PR via this mechanism) | 5 | not started |

When you call `chorus_create_decision` today, the bridge validates the payload (good — surfaces shape errors loudly) and the backend stub returns a 501 ServerError. The Day-3 PR replaces the stub with the real handler.

---

## MCP tool

Registered on the shared chat bridge. One tool. Most drivers see the bare name `chorus_create_decision`; Claude binds it as `mcp__chat__chorus_create_decision` via the `tool_prefix` field of `PromptOptions` (`src/agent/drivers/prompt.rs`).

```jsonc
{
  "name": "chorus_create_decision",
  "description": "Create a structured decision for the human to pick. Returns decision_id; END YOUR TURN immediately. Resolution arrives as a new prompt.",
  "inputSchema": {
    "type": "object",
    "required": ["headline", "question", "options", "recommended_key", "context"],
    "properties": {
      "headline":        { "type": "string", "maxLength": 80 },
      "question":        { "type": "string", "maxLength": 120 },
      "options": {
        "type": "array", "minItems": 2, "maxItems": 6,
        "items": {
          "type": "object",
          "required": ["key", "label", "body"],
          "properties": {
            "key":   { "type": "string", "maxLength": 2 },
            "label": { "type": "string", "maxLength": 40 },
            "body":  { "type": "string", "maxLength": 2048 }
          }
        }
      },
      "recommended_key": { "type": "string", "maxLength": 2 },
      "context":         { "type": "string", "maxLength": 4096 }
    }
  },
  "outputSchema": {
    "type": "object",
    "required": ["decision_id"],
    "properties": { "decision_id": { "type": "string" } }
  }
}
```

The schema's `maxLength` is informational for the MCP client. **Serde does not enforce it server-side**, so the bridge's `crate::decision::validate(...)` call (`src/bridge/mod.rs`) is load-bearing — the manual length checks in `src/decision/validator.rs` are the actual gate.

The Rust types that serialize to this schema live in `src/decision/types.rs`:

- `DecisionPayload` — agent-emitted (headline, question, options, recommended_key, context)
- `OptionPayload` — one of 2..=6 (key, label, body)
- `Decision` — server-stored row (the 11-column shape; `payload_json` is the serialized `DecisionPayload`)
- `Status` — `Open | Resolved`
- `ResolvePayload` — body of the resolve handler (picked_key, note?)

Both Rust and the TS mirror in `ui/src/data/decisions.ts` parse the canonical fixture at `src/decision/fixtures/payload.json`. If either side's shape changes without the other, one of the two parses fails and the build breaks.

---

## Validator rules (load-bearing)

Three structural rules + six length caps. From `src/decision/validator.rs`:

1. `options.len()` in `2..=6`
2. option keys are unique
3. `recommended_key` matches one option's `key`
4. `headline` ≤ 80 chars
5. `question` ≤ 120 chars
6. `context` ≤ 4096 chars
7. each option's `key` length in `1..=2`
8. each option's `label` ≤ 40 chars
9. each option's `body` ≤ 2048 chars

**Deliberately NOT in the validator** (deferred until the loop runs once):
- slug-format rule on a `kind` field (no `kind` field in v1 — headline carries the category)
- reserved-key blacklist (no UI keyboard shortcuts in v1)
- schema versioning (`version: u8`) — v1 has one shape
- urgency / deadline / expiry — v1 stays Open until picked
- per-agent calibration tracking, confidence, reversibility — v1 has no UX gates
- typed `Evidence` / `Risk` / `Delta` / `ResolveEffect` — folded into the agent's `context` markdown

---

## Agent system prompt

The driver injects a `## Decision Inbox` section into the standing system prompt (`src/agent/drivers/prompt.rs`). It tells the agent:

- When to use the tool (PR review verdicts, alternative implementations, config flags, "should I do X or Y?")
- When NOT to use it (information requests, brainstorming, things actable unilaterally)
- The payload shape and quality bar (one-line headline; the human should be able to pick in <10 s without expanding context)
- The contract: end your turn after `chorus_create_decision` returns; the resolution arrives as a new prompt

The existing `send_message` rule is amended with a single exception: when a human choice between concrete alternatives is needed, use `chorus_create_decision` instead, then end your turn. No other paths to `send_message` are weakened.

---

## Context Convention (zero-schema)

The agent fills `context: String` with markdown. Suggested H2 sections (all optional):

| Heading | What goes there |
|---|---|
| `## Why now` | One line, why this needs attention now vs. later. |
| `## Evidence` | Bullets prefixed by `[verified · source]` / `[inferred]` / `[agent]`. |
| `## Risk` | First-line audience prefix `[external]` / `[team]` / `[private]`, then blast description. |
| `## Pressure` | Non-obvious dynamics (deferral count, blocked agents, gates). |
| `## History` | Prior decisions in the same lineage. |
| `## Dep tree` | ASCII tree if topology is non-obvious. |
| `## Related` | Links to PRs, files, prior decisions. |

**v1 renders markdown plainly** — no H2 parsing, no inline-prefix styling. The convention is documented for the agent's authoring discipline. v2 may add UI rendering hooks if Day-5 dogfood reveals drivers consistently follow the convention.

---

## Codebase touchpoints

| File | Role |
|---|---|
| `src/decision/types.rs` | The 5 types: `DecisionPayload`, `OptionPayload`, `Decision`, `Status`, `ResolvePayload`. Round-trip-tested + drift-tested via `fixtures/payload.json`. |
| `src/decision/validator.rs` | The 9 validation rules. Manual length checks compensate for serde's lack of `maxLength` enforcement. |
| `src/decision/fixtures/payload.json` | Canonical fixture shared by Rust + TS drift tests. |
| `src/bridge/mod.rs` | `chorus_create_decision` tool dispatch + `crate::decision::validate` call. |
| `src/bridge/backend.rs` | `Backend::create_decision` trait method + Day-1 stub on `ChorusBackend`. |
| `src/agent/drivers/prompt.rs` | `## Decision Inbox` section + the `send_message` exception, rendered through the existing `t(...)` template helper so Claude's `mcp__chat__` prefix doesn't break references. |
| `ui/src/data/decisions.ts` | TS mirror of the Rust types. Hand-maintained; CI catches drift. |
| `ui/src/data/decisions.test.ts` | Drift test — parses `src/decision/fixtures/payload.json` against the TS interfaces. |

Day 3 will add:
- `src/store/schema.sql` — the `decisions` table (`CREATE TABLE IF NOT EXISTS`, 11 columns).
- `src/server/handlers/decisions.rs` — `GET /api/decisions`, `POST /api/decisions/:id/resolve`, plus the internal `POST /internal/agent/{agent_key}/decisions` the bridge calls.
- `src/agent/lifecycle.rs` — new `AgentLifecycle::resume_with_prompt(session_id, envelope)` method wiring `start_agent(..., init_directive=Some(envelope))` for asleep agents and `handle.prompt(envelope)` for live ones.

---

## What v2 adds (gated on Day-5 dogfood)

Each of these waits until the loop runs once and reveals the need:

- Keyboard shortcuts in the inbox UI (Enter, j/k, option keys)
- Confidence + reversibility per option, with UX gates
- Server-side reversibility overrides for known-dangerous patterns
- Backoff + max-attempts + `delivery_failed` terminal state
- Background reroute task / long-poll subscription
- Per-session FIFO queue for multiple in-flight resolutions
- `decision_acks` ledger / explicit ack endpoint
- Per-agent calibration tracking
- Driver-level "stop after this tool" hook
- Schema versioning, slug validation on `kind`, reserved-key blacklist
- Course-correct as a separate API endpoint
- `ts-rs` codegen drift detection
- Multi-driver, multi-decision-type, deadline / urgency / expiry
- H2-section parsing + inline-prefix styling in the renderer

Add to v2 only after one real decision has round-tripped through Chorus.
