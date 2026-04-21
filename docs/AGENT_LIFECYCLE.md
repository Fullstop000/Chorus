# Agent / Process / Session

[Stub — full ontology section added in Task 7.1.]

## Per-runtime resume support

| Runtime | RESUME_SESSION capability | Behavior of SessionIntent::Resume(id) |
|---|---|---|
| claude | no — probe returns `MODEL_LIST \| LOGIN` only | attaches to prior session; stores id on `preassigned_session_id` + `resumed_session_id`, `run_inner` passes `--resume <id>` to `claude -p` child; resumed child echoes same id via `system.init`, driving `SessionAttached { session_id: <same id> }` |
| codex | no — probe returns `MODEL_LIST` only | attaches to prior session; stores id on `resume_session_id`, `run_inner` sends `thread/resume` (vs `thread/start` for New); errors surface synchronously — `start_or_resume_thread` bails on `AppServerEvent::Error`; `SessionAttached` carries the thread id confirmed by the server |
| kimi | no — probe returns `MODEL_LIST` only | attaches to prior session; stores id on `preassigned_session_id`, `run_inner` sends ACP `session/load` (vs `session/new` for New); `SessionAttached` carries the id the runtime returns, falling back to the caller-supplied id if kimi-cli omits `sessionId` in the response |
| opencode | no — probe returns `MODEL_LIST` only | attaches to prior session; bootstrap path stores id on `preassigned_session_id` and sends `session/load` at run time; secondary path (live child) calls `request_load_session` synchronously in `open_session`; `SessionAttached` carries the runtime-confirmed id, falling back to caller-supplied id if omitted |
| fake | no — probe returns `MODEL_LIST` only | attaches to prior session (test fixture); stores id on both `preassigned_session_id` and `resumed_session_id`; `run()` precedence `resumed_session_id > preassigned > literal`; `SessionAttached` carries the resumed id verbatim (covered by `open_session_resume_run_first_event_is_session_attached`) |

If a driver silently drops the `id` and starts a fresh session, the
new agent_sessions table still records the latest session id reported
on SessionAttached — semantics are unchanged from today. If a driver
errors on stale id, callers (manager.start_agent) must catch and
retry with SessionIntent::New.

## Stale session recovery

If a session pointer becomes stale (rare — most drivers retain server-side state for the lifetime of the agent), the agent will fail to start on next message. Recovery is via `restart {mode: reset_session}` which clears the active row in `agent_sessions` and forces `SessionIntent::New` on the next start.

Invalid resume tokens surface asynchronously through driver events / process exit codes, not as synchronous return values from `open_session`. Stale-session recovery is intentionally out of scope for this refactor; async event-driven detection would be the correct design, and if it becomes a real operational concern, file a follow-up.
