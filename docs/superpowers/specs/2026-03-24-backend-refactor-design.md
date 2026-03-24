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

Delete `src/models.rs`. Distribute all types into the module that owns their persistence or primary logic. API DTOs move to their handler file.

### Domain types → store modules

| Type(s) | Destination |
|---|---|
| `Channel`, `ChannelType`, `ChannelMember` | `src/store/channels.rs` |
| `Message`, `SenderType`, `ReceivedMessage`, `AttachmentRef` | `src/store/messages.rs` |
| `Task`, `TaskStatus` | `src/store/tasks.rs` |
| `Agent`, `AgentStatus`, `AgentEnvVar`, `AgentConfig`, `Human` | `src/store/agents.rs` |
| `Attachment` | `src/store/mod.rs` |
| `KnowledgeEntry`, `RememberRequest`, `RememberResponse`, `RecallQuery`, `RecallResponse` | `src/store/knowledge.rs` |
| `ServerInfo`, `ChannelInfo`, `AgentInfo`, `HumanInfo` | `src/store/mod.rs` (returned by `get_server_info`) |
| `ActivityEntry`, `ActivityLogEntry`, `ActivityLogResponse` | `src/activity_log.rs` |

### API DTOs → handler files

| Type(s) | Destination |
|---|---|
| `ErrorResponse` | `src/server/handlers/mod.rs` |
| `CreateAgentRequest`, `UpdateAgentRequest`, `RestartAgentRequest`, `DeleteAgentRequest`, `RestartMode`, `DeleteMode`, `AgentDetailResponse`, `AgentEnvVarPayload` | `src/server/handlers/agents.rs` |
| `TaskInfo`, `CreateTasksRequest`, `ClaimTasksRequest`, `ClaimResult`, `UnclaimTaskRequest`, `UpdateTaskStatusRequest` | `src/server/handlers/tasks.rs` |
| `SendRequest`, `SendResponse`, `ReceiveResponse`, `HistoryResponse`, `HistoryMessage`, `ActivityMessage`, `ResolveChannelRequest`, `ResolveChannelResponse` | `src/server/handlers/messages.rs` |

`src/lib.rs` removes its `pub mod models` declaration. All callers that previously did `use crate::models::*` are updated to import from the owning module.

---

## Change 2: Split `src/server/handlers.rs` into a directory

```
src/server/handlers/
├── mod.rs          — AppState, ErrorResponse, error helpers (api_err, internal_err,
│                     conflict_err), TransitionGuard, acquire_transition,
│                     handle_whoami, handle_server_info, handle_ui_server_info
├── agents.rs       — handle_create_agent, handle_get_agent, handle_update_agent,
│                     handle_restart_agent, handle_delete_agent, handle_agent_start,
│                     handle_agent_stop, handle_agent_activity, handle_agent_activity_log
│                     + normalize_agent_env_vars, normalize_reasoning_effort,
│                       agent_info_from_agent
├── channels.rs     — handle_create_channel, handle_update_channel,
│                     handle_archive_channel, handle_delete_channel
│                     + strip_channel_prefix, normalize_channel_name,
│                       validate_channel_mutation
├── messages.rs     — handle_send, handle_receive, handle_history,
│                     handle_resolve_channel, deliver_message_to_agents
│                     + content_preview, activity_channel_label,
│                       push_received_activity
├── tasks.rs        — handle_list_tasks, handle_create_tasks, handle_claim_tasks,
│                     handle_unclaim_task, handle_update_task_status
├── attachments.rs  — handle_upload, handle_get_attachment
├── workspace.rs    — handle_agent_workspace, handle_agent_workspace_file
│                     + sanitize_workspace_path, collect_workspace_files
└── knowledge.rs    — handle_remember, handle_recall
```

`src/server/mod.rs` is unchanged — it does `use handlers::*` and constructs the router identically. The `pub use handlers::AppState` re-export stays in place.

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

---

## Execution Order

1. **Distribute `models.rs` types** — add types to their destination modules, update all `use crate::models` imports across the codebase, delete `models.rs`.
2. **Split `handlers.rs`** — create `handlers/` directory, move handler functions and their private helpers, update `mod.rs` re-exports.
3. **Split `bridge.rs`** — create `bridge/` directory, extract `types.rs` and `format.rs`, update imports in `mod.rs`.

Run `cargo build` and `cargo test` after each step to confirm no regressions before proceeding.

---

## Verification

- `cargo build` clean after each of the 3 steps
- `cargo test` green after all 3 steps
- `cargo test --test e2e_tests` green at the end
- No changes to any HTTP route, MCP tool name, or database schema
- No changes to public types exported from `src/lib.rs` (other than removing `pub mod models`)
