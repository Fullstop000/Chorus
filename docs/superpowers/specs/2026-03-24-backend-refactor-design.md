# Backend Refactor Design

**Date:** 2026-03-24
**Status:** Approved
**Scope:** Pure structural re-organization — zero behavior changes, zero public API changes.

---

## Motivation

Three files dominate the backend and violate single-responsibility:

| File | Lines | Problem |
|---|---|---|
| `src/models.rs` | 542 | All domain types + all API DTOs in one flat file |
| `src/server/handlers.rs` | 1,251 | All 25+ HTTP handlers across every domain in one file |
| `src/bridge.rs` | 1,216 | Formatting helpers, param structs, and 12+ MCP tool impls mixed |

The `src/store/` directory already demonstrates the right pattern — one file per domain. This refactor extends that pattern upward into handlers and types.

---

## Design Principles Applied

- **Organize by feature, not by type** — files group by domain (agents, channels, messages, tasks, knowledge, workspace, attachments), not by layer.
- **Files over ~300 lines are a signal** — each resulting file targets 100–250 lines.
- **Single Responsibility** — each file has one reason to change.
- **Depend on abstractions** — types flow one direction: store → handlers, never the reverse.

---

## Change 1: Remove `src/models.rs`

Delete `src/models.rs`. Distribute all types into the module that owns their persistence or primary logic.

**This step is atomic.** All types must land in their new home modules and all `use crate::models::*` imports updated in one pass before the intermediate `cargo build` check is run.

### Domain types → store modules

| Type(s) | Destination |
|---|---|
| `Channel`, `ChannelType`, `ChannelMember` | `src/store/channels.rs` |
| `Message`, `SenderType`, `ReceivedMessage`, `AttachmentRef` | `src/store/messages.rs` |
| `HistoryMessage`, `ActivityMessage` | `src/store/messages.rs` (store constructs these directly) |
| `Task`, `TaskStatus` | `src/store/tasks.rs` |
| `TaskInfo`, `ClaimResult` | `src/store/tasks.rs` (store constructs these directly) |
| `Agent`, `AgentStatus`, `AgentEnvVar`, `AgentConfig`, `Human` | `src/store/agents.rs` |
| `Attachment` | `src/store/mod.rs` |
| `KnowledgeEntry`, `RememberRequest`, `RememberResponse`, `RecallQuery`, `RecallResponse` | `src/store/knowledge.rs` |
| `ServerInfo`, `ChannelInfo`, `AgentInfo`, `HumanInfo` | `src/store/mod.rs` (returned by `get_server_info`) |
| `ActivityEntry`, `ActivityLogEntry`, `ActivityLogResponse` | `src/activity_log.rs` |

**Rationale for `TaskInfo`, `ClaimResult`, `HistoryMessage`, `ActivityMessage` staying in store:**
The store submodules construct and return these types directly. Placing them in handler files would require the store to import from handlers, creating a circular dependency Rust will refuse to compile.

### API DTOs → handler files

These types are only constructed by handlers and never touched by the store:

| Type(s) | Destination |
|---|---|
| `ErrorResponse` | `src/server/handlers/mod.rs` |
| `CreateAgentRequest`, `UpdateAgentRequest`, `RestartAgentRequest`, `DeleteAgentRequest`, `RestartMode`, `DeleteMode`, `AgentDetailResponse`, `AgentEnvVarPayload` | `src/server/handlers/agents.rs` |
| `UnclaimTaskRequest`, `UpdateTaskStatusRequest`, `CreateTasksRequest`, `ClaimTasksRequest` | `src/server/handlers/tasks.rs` |
| `SendRequest`, `SendResponse`, `ReceiveResponse`, `HistoryResponse`, `ResolveChannelRequest`, `ResolveChannelResponse` | `src/server/handlers/messages.rs` |

### Visibility audit

Every type moved out of `models.rs` must be declared `pub` in its new home. Where a type is used outside its parent module, it must be re-exported from the parent's `mod.rs` so callers outside `store/` can reach it.

After removing `pub mod models` from `src/lib.rs`, verify that every previously-exported type is individually reachable via its new module path (e.g. `crate::store::tasks::Task`, `crate::activity_log::ActivityEntry`).

### Files that must be updated in Step 1 (beyond the store modules)

The following files import from `crate::models` / `chorus::models` and must be updated as part of the atomic Step 1 pass — they cannot wait for Steps 2 or 3:

- **`src/main.rs`** — `use chorus::models::*` (uses `ChannelType`, `SenderType`, `AgentStatus`). Replace with targeted imports from new locations.
- **`src/server/mod.rs`** — `use crate::models::*` (uses `ReceivedMessage`, `ActivityEntry`, `ActivityLogResponse` in `AgentLifecycle` trait signatures). Replace with explicit imports from `store/messages.rs` and `activity_log.rs`. Note: `server/mod.rs` stays structurally unchanged in Step 2; this import update is a Step 1 obligation only.
- **`src/activity_log.rs`** — `use crate::models::{ActivityEntry, ActivityLogEntry, ActivityLogResponse}`. After Step 1 these types live in `activity_log.rs` itself, so this import line is removed.
- **`src/bridge.rs`** — check for any `use crate::models` imports and update before Step 3.
- **`src/agent/manager.rs`** — uses `use crate::models::*`; update to explicit imports.

### Private functions that travel with their types

`default_runtime()` and `default_model()` are private helper functions in `models.rs` used by `#[serde(default = "...")]` attributes on `CreateAgentRequest` and `UpdateAgentRequest`. They must move to `src/server/handlers/agents.rs` alongside those structs in Step 1.

---

## Change 2: Split `src/server/handlers.rs` into a directory

All inline `#[derive(Deserialize)]` query/request structs move alongside their handler function into the appropriate domain file — no exceptions.

```
src/server/handlers/
├── mod.rs
│   Public: AppState, ErrorResponse
│   Private: api_err, internal_err, conflict_err, TransitionGuard, acquire_transition
│   Handlers: handle_whoami, handle_server_info, handle_ui_server_info
│
├── agents.rs
│   Handlers: handle_create_agent, handle_get_agent, handle_update_agent,
│              handle_restart_agent, handle_delete_agent, handle_agent_start,
│              handle_agent_stop, handle_agent_activity, handle_agent_activity_log
│   Private helpers: normalize_agent_env_vars, normalize_reasoning_effort,
│                    agent_info_from_agent
│   Inline structs: ActivityParams (used by handle_agent_activity)
│   DTOs: CreateAgentRequest, UpdateAgentRequest, RestartAgentRequest,
│          DeleteAgentRequest, RestartMode, DeleteMode,
│          AgentDetailResponse, AgentEnvVarPayload
│
├── channels.rs
│   Handlers: handle_create_channel, handle_update_channel,
│              handle_archive_channel, handle_delete_channel
│   Private helpers: strip_channel_prefix, normalize_channel_name,
│                    validate_channel_mutation
│   Inline structs: CreateChannelRequest, UpdateChannelRequest
│
├── messages.rs
│   Handlers: handle_send, handle_receive, handle_history, handle_resolve_channel
│   Public helper: deliver_message_to_agents — stays pub(crate), lives here because
│                  it is only called from handle_send and uses AppState + AgentLifecycle
│   Private helpers: resolve_history_target, content_preview,
│                    activity_channel_label, push_received_activity
│   Inline structs: inline Deserialize query structs for receive/history handlers
│   DTOs: SendRequest, SendResponse, ReceiveResponse, HistoryResponse,
│          ResolveChannelRequest, ResolveChannelResponse
│
├── tasks.rs
│   Handlers: handle_list_tasks, handle_create_tasks, handle_claim_tasks,
│              handle_unclaim_task, handle_update_task_status
│   Inline structs: inline Deserialize query structs for task handlers
│   DTOs: CreateTasksRequest, ClaimTasksRequest, UnclaimTaskRequest,
│          UpdateTaskStatusRequest
│
├── attachments.rs
│   Handlers: handle_upload, handle_get_attachment
│
├── workspace.rs
│   Handlers: handle_agent_workspace, handle_agent_workspace_file
│   Private helpers: sanitize_workspace_path, collect_workspace_files
│   Inline structs: WorkspaceFileParams (used by handle_agent_workspace_file)
│
└── knowledge.rs
    Handlers: handle_remember, handle_recall
```

`src/server/mod.rs` is structurally unchanged in Step 2 — it does `use handlers::*` and constructs the router identically. The `pub use handlers::AppState` re-export stays in place. (Its `use crate::models::*` import was already replaced in Step 1.)

---

## Change 3: Split `src/bridge.rs` into a directory (light touch)

```
src/bridge/
├── mod.rs      — ChatBridge struct + impl, all tool implementations, run_bridge()
├── types.rs    — all #[derive(Deserialize, JsonSchema)] param structs
│                 (SendMessageParams, ReceiveMessageParams, ReadHistoryParams,
│                  TaskDef, CreateTasksParams, ClaimTasksParams, UnclaimTaskParams,
│                  UpdateTaskStatusParams, UploadFileParams, ViewFileParams,
│                  RememberParams, RecallParams, EmptyParams, ListTasksParams)
└── format.rs   — to_local_time(), format_target(), format_attachments()
```

`bridge/mod.rs` uses `use super::types::*` and `use super::format::*`. The public surface (`run_bridge`, `ChatBridge`) is unchanged — `src/main.rs` sees no diff.

**Important:** `bridge/mod.rs` imports `rmcp::model::ServerInfo` for the `get_info()` method. Do not introduce a `use crate::store::*` glob import into `bridge/mod.rs` — import all `crate::store` types by name only, to avoid shadowing `rmcp::model::ServerInfo`.

---

## Execution Order

1. **Distribute `models.rs` types (atomic step)** — add types to their destination modules, update all `use crate::models` imports across the entire codebase, delete `models.rs`, remove `pub mod models` from `lib.rs`. Run `cargo build` and `cargo test` to confirm green before proceeding.
2. **Split `handlers.rs`** — create `handlers/` directory, move handler functions, private helpers, inline structs, and DTOs per the layout above. Run `cargo build` and `cargo test`.
3. **Split `bridge.rs`** — create `bridge/` directory, extract `types.rs` and `format.rs`, update imports in `mod.rs`. Use named imports only for `crate::store` types. Run `cargo build` and `cargo test`.

---

## Verification

- `cargo build` clean after each of the 3 steps
- `cargo test` green after all 3 steps
- `cargo test --test e2e_tests` green at the end
- No changes to any HTTP route, MCP tool name, or database schema
- No changes to public types exported from `src/lib.rs` (other than removing `pub mod models`)
