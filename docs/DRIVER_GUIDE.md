# Adding A New Driver

This guide is the practical checklist for adding a new agent runtime to Chorus and verifying that it actually works in the live product.

## ACP-Based Architecture (Driver 2.0)

All drivers now use the Agent Client Protocol (ACP) as the base transport layer. ACP is a JSON-RPC 2.0 protocol over stdio that standardizes communication between Chorus and agent CLIs.

**Architecture:**
- `src/agent/drivers/acp.rs` — shared ACP protocol handler (`AcpRuntime` trait + `AcpDriver<R>`)
- `src/agent/drivers/<runtime>.rs` — thin `AcpRuntime` implementation per runtime (~50-80 lines)

Adding a new runtime means implementing the `AcpRuntime` trait (binary name, CLI args, MCP config, auth detection, model listing). You do **not** need to write a custom stdout parser, stdin encoder, or tool display mapper — ACP handles all of that uniformly.

The goal is not just "the process starts." The goal is:

- the runtime can be spawned reliably
- the runtime can call the Chorus MCP bridge tools
- the runtime receives messages correctly
- the runtime replies through `send_message`, not raw stdout text
- the runtime does not regress the user-visible DM and activity flows

## Where Driver Support Lives

New driver work usually touches these files:

- `src/agent/drivers/acp.rs`
  - shared ACP protocol: `AcpRuntime` trait definition, `AcpDriver<R>` implementation, JSON-RPC parsing, tool display names
- `src/agent/drivers/<runtime>.rs`
  - thin `AcpRuntime` impl: binary name, ACP CLI args, MCP config, env overrides, auth detection, model list
- `src/agent/drivers/mod.rs`
  - module registration
- `src/agent/manager.rs`
  - driver selection, session handling, and subprocess lifecycle
- `src/main.rs`
  - CLI defaults or runtime-specific model choices when needed
- `src/server/handlers/dto.rs`
  - API runtime enum validation when needed
- `src/store/agents.rs`
  - persisted runtime/model support when needed
- `ui/src/components/AgentConfigForm.tsx`
  - runtime exposed in the create/edit UI
- `ui/src/components/ProfilePanel.tsx`
  - runtime/model rendering if the profile badges or labels need updates
- `tests/driver_tests.rs`
  - prompt and parser regression tests
- `tests/server_tests.rs`
  - runtime create/update API coverage when needed
- `qa/cases/playwright/AGT-002.spec.ts`
  - create/start matrix coverage
- `qa/cases/playwright/<RUNTIME-CASE>.spec.ts`
  - runtime-specific DM reply verification

## Phase 1: Discover The Runtime Protocol First

Do not start by copying Claude or Codex behavior and hoping it matches.

Before writing much code, answer these questions from the real runtime:

1. What stdin shape does the runtime expect in print/non-interactive mode?
2. What stdout shape does it emit for:
   - assistant text
   - tool calls
   - tool results
   - errors
3. What MCP tool names does it expose?
   - namespaced like `mcp__chat__send_message`
   - bare like `send_message`
   - something else
4. How does it represent tool-call arguments?
   - embedded JSON object
   - JSON string inside another object
   - content array blocks
5. How is session resume expressed?

### Recommended Probe

Use a tiny one-off probe before integrating fully:

1. Create a temporary MCP config that points at:
   - `chorus bridge --agent-id <test-agent> --server-url <local-server>`
2. Run the runtime directly in its print/JSON mode
3. Send one minimal prompt that asks it to call `send_message`
4. Capture raw stdout and stderr

You want at least one saved raw sample for:

- a successful tool call
- a tool result
- a plain text assistant response

That sample becomes the source of truth for the parser tests.

## Phase 2: Implement The Driver

Create `src/agent/drivers/<runtime>.rs` and implement the `Driver` trait.

The key pieces are:

- `spawn()`
  - construct the real subprocess command
  - pass the bridge MCP config
  - enable piped stdin/stdout/stderr
- `parse_line()`
  - convert raw runtime output into Chorus `ParsedEvent`s
- `encode_stdin_message()`
  - format stdin notifications in the runtime's actual expected shape
- `build_system_prompt()`
  - teach the runtime the Chorus tool contract using the runtime's real tool names
- `tool_display_name()` and `summarize_tool_input()`
  - make activity log output readable

### Prompt Rules That Matter

The prompt must be explicit about these points:

- all human-visible replies must go through `send_message`
- raw assistant text is not a valid user-visible reply
- after `wait_for_message()` or `check_messages()` returns a real user message, the agent must either:
  - reply, or
  - deliberately explain why no reply is needed
- when idle, the agent must return to `wait_for_message()`

### Tool Naming Rule

Use the runtime's actual MCP-exposed names in the prompt and wake-up instructions.

Do not assume they match another runtime.

If the runtime emits bare names like `send_message`, the prompt should teach bare names. If the parser wants to normalize internally, do that in `parse_line()`, not in the prompt text.

## Phase 3: Register It End To End

Wire the runtime through the full product surface:

1. Add the module in `src/agent/drivers/mod.rs`
2. Add the runtime selection branch in `src/agent/manager.rs`
3. Add API/runtime validation updates where needed
4. Expose the runtime in the UI create/edit form
5. Make sure runtime/model labels render correctly in the profile UI

If the runtime needs session resume support, ensure the persisted session ID format is correct and that restart logic reuses it.

## Phase 4: Add The Right Tests

### Unit / Parser Tests

In `tests/driver_tests.rs` and the driver's own unit tests, cover:

- prompt contains the runtime's real tool names
- prompt forbids raw stdout replies
- parser handles:
  - assistant text
  - tool-call blocks
  - top-level `tool_calls`
  - runtime-specific argument encoding
- stdin notifications use the runtime's actual JSON shape

### Store / Lifecycle Tests

Add focused regressions for any runtime-specific delivery behavior that shows up during debugging.

Example from Kimi:

- sender read position must advance on send
- an agent must not receive its own outbound message back as unread

### API / Server Tests

Cover:

- runtime accepted by `POST /api/agents`
- runtime shows up in list/detail responses when needed
- restart/session persistence if the runtime is resumable

## Phase 5: Verify The Live Runtime, Not Just The Parser

This is the most important phase.

Start a fresh local server with:

- a fresh temp data dir
- `RUST_LOG=chorus=debug`
- the current branch build

Then reproduce one real DM flow against the live app.

### Required Live Checks

Verify all of these:

1. Agent creates successfully
2. Agent starts successfully
3. Agent receives the DM
4. Agent replies in the DM
5. Activity log shows `Sending message…`
6. Raw subprocess output shows a real `send_message` tool call
7. The agent does not only emit raw assistant text
8. The agent does not consume the message silently
9. The agent does not receive its own outbound reply back as unread

### Raw Logging Recommendation

When bringing up a new driver, temporarily log:

- raw stdout lines from that runtime
- raw stderr lines from that runtime

This makes it possible to distinguish:

- tool injection failure
- parser mismatch
- prompt/tool-choice failure
- lifecycle/read-position bugs

Without raw logs, new-driver debugging is mostly guesswork.

## Phase 6: Browser QA

Every new runtime should have two Playwright checks:

### 1. Runtime-Specific DM Reply Case

Create a case like `qa/cases/playwright/<RUNTIME-001>.spec.ts` that proves:

- a direct DM to the runtime gets a reply
- the reply contains an exact token
- the reply is visible in chat
- the activity log contains a real `Sending message` tool event
- success is not granted if only raw text output exists

### 2. Create/Start Matrix Case

Update `qa/cases/playwright/AGT-002.spec.ts` so the runtime is covered in the creation matrix.

This catches:

- runtime enum mismatches
- model default issues
- UI/runtime wiring regressions

## Common Failure Modes

These are the mistakes most likely to waste time:

### 1. Wrong stdin protocol

Symptom:

- runtime starts but never behaves correctly
- no useful stdout

Cause:

- sending another runtime's JSON envelope instead of the runtime's real input shape

### 2. Wrong tool names in the prompt

Symptom:

- runtime can call one tool sometimes but ignores the reply contract

Cause:

- prompt teaches namespaced tools but runtime sees bare names, or vice versa

### 3. Parser only handles one tool-call shape

Symptom:

- runtime logs raw JSON with tool calls, but Chorus only records text output

Cause:

- parser handles content-array tool blocks but not top-level `tool_calls`, or the reverse

### 4. Runtime emits plain text after reading a message

Symptom:

- logs show the runtime understood the user message
- chat never receives a reply
- activity only shows `text output`

Cause:

- prompt contract is too weak, or the tool result needs a stronger local reply instruction

### 5. Sender self-echo

Symptom:

- after replying, the agent receives its own outbound DM back through `wait_for_message()`

Cause:

- sender `last_read_seq` is not advanced on send

## Completion Checklist

Do not call a new driver "done" until all items below are true:

- runtime protocol was confirmed from real raw samples
- prompt uses the runtime's actual tool names
- parser covers the runtime's actual output shapes
- create/start works through the API and UI
- live DM reply path uses `send_message`
- activity log proves the tool path
- no self-echo/unread loop remains
- focused Rust tests pass
- browser QA passes for:
  - runtime-specific DM case
  - `AGT-002`
- `AGENTS.md` or related docs were updated

## Recommended Fast Path Next Time

If you want the shortest reliable path for the next runtime, do this in order:

1. capture one raw runtime probe
2. write parser tests from the raw sample
3. implement spawn + parser + prompt
4. verify one live DM with raw stdout/stderr logging
5. add the runtime-specific Playwright case
6. add the runtime to `AGT-002`
7. only then polish UI and defaults

That order avoids spending time debugging UI or API layers when the real bug is still at the runtime wire-protocol level.
