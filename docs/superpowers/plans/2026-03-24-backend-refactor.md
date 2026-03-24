# Backend Structural Refactor — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Eliminate `src/models.rs` and split two oversized files (`handlers.rs`, `bridge.rs`) into focused domain modules — zero behavior changes.

**Architecture:** Types move to the store sub-module that owns them; API DTOs move to their handler file; `handlers.rs` converts to a `handlers/` directory; `bridge.rs` gets its param structs and formatters extracted into sibling files.

**Tech Stack:** Rust, Axum, SQLite (rusqlite), rmcp

**Spec:** `docs/superpowers/specs/2026-03-24-backend-refactor-design.md`

---

## File Map

### Created
- `src/server/handlers/mod.rs` — shared infra (AppState, error helpers, TransitionGuard) + whoami/server-info handlers
- `src/server/handlers/agents.rs` — agent CRUD/lifecycle handlers + agent API DTOs
- `src/server/handlers/channels.rs` — channel CRUD handlers + channel inline structs
- `src/server/handlers/messages.rs` — send/receive/history handlers + message API DTOs
- `src/server/handlers/tasks.rs` — task handlers + task API DTOs
- `src/server/handlers/attachments.rs` — upload/download handlers
- `src/server/handlers/workspace.rs` — workspace handlers + WorkspaceFileParams
- `src/server/handlers/knowledge.rs` — remember/recall handlers
- `src/bridge/mod.rs` — ChatBridge tool impls + run_bridge()
- `src/bridge/types.rs` — all MCP param structs
- `src/bridge/format.rs` — to_local_time, format_target, format_attachments

### Modified
- `src/store/mod.rs` — gains `Attachment`, `ServerInfo`, `ChannelInfo`, `AgentInfo`, `HumanInfo`; sub-modules changed from `mod` to `pub mod`; `use crate::models::*` replaced
- `src/store/channels.rs` — gains `Channel`, `ChannelType`, `ChannelMember`
- `src/store/messages.rs` — gains `Message`, `SenderType`, `ReceivedMessage`, `AttachmentRef`, `HistoryMessage`, `ActivityMessage`
- `src/store/tasks.rs` — gains `Task`, `TaskStatus`, `TaskInfo`, `ClaimResult`
- `src/store/agents.rs` — gains `Agent`, `AgentStatus`, `AgentEnvVar`, `AgentConfig`, `Human`
- `src/store/knowledge.rs` — gains `RememberRequest/Response`, `RecallQuery/Response` (KnowledgeEntry was already imported)
- `src/activity_log.rs` — gains `ActivityEntry`, `ActivityLogEntry`, `ActivityLogResponse`; removes models import
- `src/agent/manager.rs` — `use crate::models::*` replaced with explicit store/activity_log imports
- `src/agent/drivers/mod.rs` — `AgentConfig` bare path updated to `crate::store::agents::AgentConfig`
- `src/agent/drivers/claude.rs` — `use crate::models::AgentConfig` updated
- `src/agent/drivers/codex.rs` — `use crate::models::AgentConfig` updated
- `src/agent/drivers/prompt.rs` — `use crate::models::AgentConfig` updated
- `src/server/mod.rs` — `use crate::models::*` replaced with explicit imports (Step 1 only; struct unchanged)
- `src/main.rs` — `use chorus::models::*` replaced with explicit imports
- `src/lib.rs` — removes `pub mod models`
- `tests/e2e_tests.rs` — `use chorus::models::{ChannelType, SenderType}` updated
- `tests/store_tests.rs` — `use chorus::models::*` updated
- `tests/driver_tests.rs` — `use chorus::models::AgentConfig` updated
- `tests/server_tests.rs` — `use chorus::models::*` updated

### Deleted
- `src/models.rs`
- `src/server/handlers.rs` (replaced by handlers/ directory)
- `src/bridge.rs` (replaced by bridge/ directory)

---

## Task 1: Distribute types from `models.rs` (atomic)

This task is **one atomic operation** — all sub-steps must be done before running `cargo build`. Do not run `cargo build` mid-task.

**Files:**
- Modify: `src/store/mod.rs`, `src/store/channels.rs`, `src/store/messages.rs`, `src/store/tasks.rs`, `src/store/agents.rs`, `src/store/knowledge.rs`
- Modify: `src/activity_log.rs`, `src/agent/manager.rs`, `src/agent/drivers/mod.rs`, `src/agent/drivers/claude.rs`, `src/agent/drivers/codex.rs`, `src/agent/drivers/prompt.rs`
- Modify: `src/server/mod.rs`, `src/server/handlers.rs`, `src/main.rs`, `src/lib.rs`
- Modify: `tests/e2e_tests.rs`, `tests/store_tests.rs`, `tests/driver_tests.rs`, `tests/server_tests.rs`
- Delete: `src/models.rs`

---

- [ ] **Step 1a: Add domain types to `src/store/channels.rs`**

At the top of `src/store/channels.rs`, replace `use crate::models::*;` with:

```rust
use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::Store;

// ── Types owned by this module ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Channel {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub channel_type: ChannelType,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelType {
    Channel,
    Dm,
    /// System-managed channels (e.g. #shared-memory). Not listed in the UI channel list.
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelMember {
    pub channel_id: String,
    pub member_name: String,
    pub member_type: super::messages::SenderType,
    pub last_read_seq: i64,
}
```

Note: `ChannelMember` references `SenderType` from the messages module — use `super::messages::SenderType` or add `use super::messages::SenderType;` at the top.

---

- [ ] **Step 1b: Add domain types to `src/store/messages.rs`**

Replace `use crate::models::*;` with explicit imports and add type definitions. The file currently uses `use super::{sender_type_str, Store};` — keep that. Add at top of file:

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
```

Then add these type definitions before the `impl Store {` block:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SenderType {
    Human,
    Agent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub channel_id: String,
    pub thread_parent_id: Option<String>,
    pub sender_name: String,
    pub sender_type: SenderType,
    pub content: String,
    pub created_at: DateTime<Utc>,
    pub seq: i64,
    pub attachment_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceivedMessage {
    pub message_id: String,
    pub channel_name: String,
    pub channel_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_channel_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_channel_type: Option<String>,
    pub sender_name: String,
    pub sender_type: String,
    pub content: String,
    pub timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<AttachmentRef>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentRef {
    pub id: String,
    pub filename: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HistoryMessage {
    pub id: String,
    pub seq: i64,
    pub content: String,
    #[serde(rename = "senderName")]
    pub sender_name: String,
    #[serde(rename = "senderType")]
    pub sender_type: String,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "senderDeleted")]
    pub sender_deleted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<AttachmentRef>>,
    #[serde(rename = "replyCount", skip_serializing_if = "Option::is_none")]
    pub reply_count: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ActivityMessage {
    pub id: String,
    pub seq: i64,
    pub content: String,
    #[serde(rename = "channelName")]
    pub channel_name: String,
    #[serde(rename = "createdAt")]
    pub created_at: String,
}
```

---

- [ ] **Step 1c: Add domain types to `src/store/tasks.rs`**

Replace `use crate::models::*;` with explicit imports. Add at top of file:

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
```

Add type definitions before the `impl Store {` block:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub channel_id: String,
    pub task_number: i64,
    pub title: String,
    pub status: TaskStatus,
    pub claimed_by: Option<String>,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Todo,
    InProgress,
    InReview,
    Done,
}

impl TaskStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Todo => "todo",
            Self::InProgress => "in_progress",
            Self::InReview => "in_review",
            Self::Done => "done",
        }
    }

    pub fn from_status_str(s: &str) -> Option<Self> {
        match s {
            "todo" => Some(Self::Todo),
            "in_progress" => Some(Self::InProgress),
            "in_review" => Some(Self::InReview),
            "done" => Some(Self::Done),
            _ => None,
        }
    }

    pub fn can_transition_to(&self, to: Self) -> bool {
        matches!(
            (self, to),
            (Self::Todo, Self::InProgress)
                | (Self::InProgress, Self::InReview)
                | (Self::InProgress, Self::Done)
                | (Self::InReview, Self::Done)
                | (Self::InReview, Self::InProgress)
        )
    }
}

/// Returned by list_tasks and create_tasks — store constructs these directly.
#[derive(Debug, Serialize, Deserialize)]
pub struct TaskInfo {
    #[serde(rename = "taskNumber")]
    pub task_number: i64,
    pub title: String,
    pub status: String,
    #[serde(rename = "claimedByName")]
    pub claimed_by_name: Option<String>,
    #[serde(rename = "createdByName")]
    pub created_by_name: Option<String>,
}

/// Returned by claim_tasks — store constructs these directly.
#[derive(Debug, Serialize, Deserialize)]
pub struct ClaimResult {
    #[serde(rename = "taskNumber")]
    pub task_number: i64,
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}
```

---

- [ ] **Step 1d: Add domain types to `src/store/agents.rs`**

Replace `use crate::models::*;` with:

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
```

Add type definitions before `pub struct AgentRecordUpsert`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    pub id: String,
    pub name: String,
    pub display_name: String,
    pub description: Option<String>,
    pub runtime: String,
    pub model: String,
    pub reasoning_effort: Option<String>,
    pub env_vars: Vec<AgentEnvVar>,
    pub status: AgentStatus,
    pub session_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentEnvVar {
    pub key: String,
    pub value: String,
    pub position: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Active,
    Sleeping,
    Inactive,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub name: String,
    pub display_name: String,
    pub description: Option<String>,
    pub runtime: String,
    pub model: String,
    pub session_id: Option<String>,
    pub reasoning_effort: Option<String>,
    pub env_vars: Vec<AgentEnvVar>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Human {
    pub name: String,
    pub created_at: DateTime<Utc>,
}
```

---

- [ ] **Step 1e: Add types to `src/store/knowledge.rs`**

Replace `use crate::models::KnowledgeEntry;` with the type definition inline plus add the other knowledge types from `models.rs`. Replace that import line with:

```rust
use serde::{Deserialize, Serialize};
```

Then add type definitions at the top of the file (before the `impl Store` block):

```rust
/// A single entry in the shared knowledge store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeEntry {
    pub id: String,
    pub key: String,
    pub value: String,
    pub tags: String,
    pub author_agent_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_context: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Deserialize)]
pub struct RememberRequest {
    pub key: String,
    pub value: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default, rename = "channelContext")]
    pub channel_context: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RememberResponse {
    pub id: String,
}

#[derive(Debug, Deserialize)]
pub struct RecallQuery {
    pub query: Option<String>,
    pub tags: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RecallResponse {
    pub entries: Vec<KnowledgeEntry>,
}
```

---

- [ ] **Step 1f: Add types to `src/store/mod.rs`**

**First**, change the five private sub-module declarations to `pub mod` so their types are accessible from outside `store/`:

```rust
// Change these lines in store/mod.rs:
mod agents;      →  pub mod agents;
mod channels;    →  pub mod channels;
mod knowledge;   →  pub mod knowledge;
mod messages;    →  pub mod messages;
mod tasks;       →  pub mod tasks;
```

This makes paths like `crate::store::agents::AgentStatus` valid from any other crate module.

**Then**, replace `use crate::models::*;` with imports from sub-modules. At the top of `store/mod.rs`, replace the `use crate::models::*;` line with:

```rust
use crate::activity_log::ActivityLogResponse;

pub use agents::AgentRecordUpsert;
pub use agents::{Agent, AgentConfig, AgentEnvVar, AgentStatus, Human};
pub use channels::{Channel, ChannelMember, ChannelType};
pub use knowledge::{KnowledgeEntry, RecallQuery, RecallResponse, RememberRequest, RememberResponse};
pub use messages::{
    ActivityMessage, AttachmentRef, HistoryMessage, Message, ReceivedMessage, SenderType,
};
pub use tasks::{ClaimResult, Task, TaskInfo, TaskStatus};
```

Then add the types that live in `store/mod.rs` itself — `Attachment`, `ServerInfo`, `ChannelInfo`, `AgentInfo`, `HumanInfo`. Add them near the top of the file (after imports, before the `Store` struct):

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub id: String,
    pub filename: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub stored_path: String,
    pub uploaded_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ServerInfo {
    pub channels: Vec<ChannelInfo>,
    pub system_channels: Vec<ChannelInfo>,
    pub agents: Vec<AgentInfo>,
    pub humans: Vec<HumanInfo>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ChannelInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub name: String,
    pub description: Option<String>,
    pub joined: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AgentInfo {
    pub name: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(rename = "reasoningEffort", skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activity_detail: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HumanInfo {
    pub name: String,
}
```

Remove the old `pub use agents::AgentRecordUpsert;` line (it's now in the block above).

---

- [ ] **Step 1g: Move `ActivityEntry`, `ActivityLogEntry`, `ActivityLogResponse` to `src/activity_log.rs`**

Remove the line `use crate::models::{ActivityEntry, ActivityLogEntry, ActivityLogResponse};` from `activity_log.rs`.

Add the three type definitions at the top of `activity_log.rs` (after the `use` statements for std):

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ActivityEntry {
    Thinking { text: String },
    ToolStart { tool_name: String, tool_input: String },
    Text { text: String },
    MessageReceived { channel_label: String, sender_name: String, content: String },
    MessageSent { target: String, content: String },
    Status { activity: String, detail: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityLogEntry {
    pub seq: u64,
    pub timestamp_ms: u64,
    pub entry: ActivityEntry,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ActivityLogResponse {
    pub entries: Vec<ActivityLogEntry>,
    pub agent_activity: String,
    pub agent_detail: String,
}
```

---

- [ ] **Step 1h: Move API DTOs into `src/server/handlers.rs`**

`handlers.rs` will remain a single file until Task 2. Here we add the API DTOs from `models.rs` that belong to handlers, and replace the `use crate::models::*;` import with explicit store/activity_log imports.

Replace `use crate::models::*;` with:

```rust
use crate::activity_log::ActivityEntry;
use crate::store::agents::{Agent, AgentConfig, AgentEnvVar, AgentStatus};
use crate::store::channels::{Channel, ChannelType};
use crate::store::knowledge::{KnowledgeEntry, RecallQuery, RememberRequest};
use crate::store::messages::{ReceivedMessage, SenderType};
use crate::store::tasks::{ClaimResult, TaskInfo, TaskStatus};
use crate::store::{AgentInfo, ServerInfo};
```

Then add the API DTO type definitions that were in `models.rs`. Add them near the top of `handlers.rs`, after imports. Copy these from `models.rs`:

- `ErrorResponse` (pub struct with `error: String`)
- `SendRequest`, `SendResponse`
- `ReceiveResponse`
- `HistoryResponse`
- `ResolveChannelRequest`, `ResolveChannelResponse`
- `AgentDetailResponse`, `AgentEnvVarPayload`
- `CreateAgentRequest`, `UpdateAgentRequest`, `RestartAgentRequest`, `RestartMode`
- `DeleteAgentRequest`, `DeleteMode`
- `CreateTasksRequest`, `CreateTaskItem`, `ClaimTasksRequest`
- `UnclaimTaskRequest`, `UpdateTaskStatusRequest`
- `ActivityMessage` (re-check: this actually moved to store/messages.rs — do NOT add here)
- `ActivityLogResponse` (moved to activity_log.rs — do NOT add here)

Also copy the two private helpers:
```rust
fn default_runtime() -> String { "claude".to_string() }
fn default_model() -> String { "sonnet".to_string() }
```

---

- [ ] **Step 1i: Update `src/server/mod.rs` imports**

Replace `use crate::models::*;` with:

```rust
use crate::activity_log::{ActivityEntry, ActivityLogResponse};
use crate::store::messages::ReceivedMessage;
```

---

- [ ] **Step 1j: Update `src/agent/manager.rs` imports**

Replace `use crate::models::*;` with:

```rust
use crate::activity_log::ActivityEntry;
use crate::store::agents::{Agent, AgentConfig, AgentEnvVar, AgentStatus};
use crate::store::messages::ReceivedMessage;
```

---

- [ ] **Step 1k: Update `src/agent/drivers/` imports**

All four driver files import `AgentConfig` from models. Replace each:

**`src/agent/drivers/mod.rs`** — look for any bare `crate::models::AgentConfig` path or `use crate::models` import and replace with:
```rust
use crate::store::agents::AgentConfig;
```

**`src/agent/drivers/claude.rs`** — replace `use crate::models::AgentConfig;` with:
```rust
use crate::store::agents::AgentConfig;
```

**`src/agent/drivers/codex.rs`** — same replacement as above.

**`src/agent/drivers/prompt.rs`** — same replacement as above.

---

- [ ] **Step 1l: Update `src/main.rs` imports**

Replace `use chorus::models::*;` with:

```rust
use chorus::store::agents::AgentStatus;
use chorus::store::channels::ChannelType;
use chorus::store::messages::SenderType;
```

---

- [ ] **Step 1m: Update test files**

All four integration test files import from `chorus::models`. Replace each:

**`tests/store_tests.rs`** and **`tests/server_tests.rs`** — replace `use chorus::models::*;` with explicit imports for only the types actually used in each file. Scan each file for model types and add targeted imports such as:
```rust
use chorus::store::agents::{Agent, AgentConfig, AgentEnvVar, AgentStatus};
use chorus::store::channels::{Channel, ChannelType};
use chorus::store::messages::{ReceivedMessage, SenderType};
use chorus::store::tasks::{Task, TaskStatus};
```
(Add only what each file actually uses — don't import unused types.)

**`tests/e2e_tests.rs`** — replace `use chorus::models::{ChannelType, SenderType};` with:
```rust
use chorus::store::channels::ChannelType;
use chorus::store::messages::SenderType;
```

**`tests/driver_tests.rs`** — replace `use chorus::models::AgentConfig;` with:
```rust
use chorus::store::agents::AgentConfig;
```

---

- [ ] **Step 1n: Remove `pub mod models` from `src/lib.rs`**

Delete the line `pub mod models;` from `src/lib.rs`.

---

- [ ] **Step 1o: Delete `src/models.rs`**

```bash
rm src/models.rs
```

---

- [ ] **Step 1p: Build and test**

```bash
cargo build
```
Expected: zero errors, zero warnings about unused imports.

```bash
cargo test
```
Expected: all tests pass (same count as before this task).

If errors occur, fix them before committing — they will be import path issues (wrong module path for a type). Do NOT proceed to the next step with failing tests.

- [ ] **Step 1q: Commit**

```bash
git add src/store/ src/activity_log.rs src/agent/ src/server/ src/main.rs src/lib.rs tests/
git commit -m "refactor(models): distribute types into owning store modules"
```

---

## Task 2: Split `src/server/handlers.rs` into a directory

**Files:**
- Delete: `src/server/handlers.rs`
- Create: `src/server/handlers/mod.rs`, `handlers/agents.rs`, `handlers/channels.rs`, `handlers/messages.rs`, `handlers/tasks.rs`, `handlers/attachments.rs`, `handlers/workspace.rs`, `handlers/knowledge.rs`

---

- [ ] **Step 2a: Convert `handlers.rs` to `handlers/mod.rs`**

Rust allows a module to be either `path/to/module.rs` or `path/to/module/mod.rs`. To convert:

```bash
mkdir src/server/handlers
cp src/server/handlers.rs src/server/handlers/mod.rs
rm src/server/handlers.rs
```

At this point `cargo build` should still succeed — the module path `crate::server::handlers` is unchanged.

```bash
cargo build
```
Expected: compiles cleanly.

---

- [ ] **Step 2b: Create `src/server/handlers/agents.rs`**

Create a new file. Move the following from `handlers/mod.rs` into this file:

**Inline structs to move:**
- `ActivityParams` (the `#[derive(Deserialize)] struct ActivityParams`)
- `ActivityLogParams` (the `#[derive(Deserialize)] struct ActivityLogParams`)

**API DTOs to move (defined in mod.rs after Task 1):**
- `CreateAgentRequest`, `UpdateAgentRequest`, `RestartAgentRequest`, `RestartMode`
- `DeleteAgentRequest`, `DeleteMode`
- `AgentDetailResponse`, `AgentEnvVarPayload`
- `default_runtime()`, `default_model()`

**Private helpers to move:**
- `normalize_agent_env_vars()`
- `normalize_reasoning_effort()`
- `agent_info_from_agent()`

**Public handlers to move:**
- `handle_create_agent`, `handle_get_agent`, `handle_update_agent`, `handle_restart_agent`
- `handle_delete_agent`, `handle_agent_start`, `handle_agent_stop`
- `handle_agent_activity`, `handle_agent_activity_log`

The file needs these imports at the top:

```rust
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use tracing::info;
use uuid::Uuid;

use super::{api_err, conflict_err, internal_err, acquire_transition, AppState, ApiResult};
use crate::agent::workspace::AgentWorkspace;
use crate::store::agents::{Agent, AgentConfig, AgentEnvVar, AgentStatus};
use crate::store::{AgentInfo, AgentRecordUpsert};
```

---

- [ ] **Step 2c: Create `src/server/handlers/channels.rs`**

Move from `handlers/mod.rs`:

**Inline structs:** `CreateChannelRequest`, `UpdateChannelRequest`

**Private helpers:** `strip_channel_prefix()`, `normalize_channel_name()`, `validate_channel_mutation()`

**Public handlers:** `handle_create_channel`, `handle_update_channel`, `handle_archive_channel`, `handle_delete_channel`

Imports:

```rust
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use super::{api_err, AppState, ApiResult};
use crate::store::channels::{Channel, ChannelType};
use crate::store::messages::SenderType;
```

---

- [ ] **Step 2d: Create `src/server/handlers/messages.rs`**

Move from `handlers/mod.rs`:

**Inline structs:** `ReceiveParams`, `HistoryParams`

**API DTOs:** `SendRequest`, `SendResponse`, `ReceiveResponse`, `HistoryResponse`, `ResolveChannelRequest`, `ResolveChannelResponse`

**Private helpers:** `resolve_history_target()`, `content_preview()`, `activity_channel_label()`, `push_received_activity()`

**Public functions:** `handle_send`, `handle_receive`, `handle_history`, `handle_resolve_channel`, `deliver_message_to_agents` (keep as `pub(crate)`)

Imports:

```rust
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use tracing::{debug, info};

use super::{api_err, internal_err, AppState, ApiResult};
use crate::activity_log::ActivityEntry;
use crate::store::channels::ChannelType;
use crate::store::messages::{ReceivedMessage, SenderType};
use crate::store::Store;
```

---

- [ ] **Step 2e: Create `src/server/handlers/tasks.rs`**

Move from `handlers/mod.rs`:

**Inline structs:** `ListTasksParams` (the `#[derive(Deserialize)] struct ListTasksParams`)

**API DTOs:** `CreateTasksRequest`, `CreateTaskItem`, `ClaimTasksRequest`, `UnclaimTaskRequest`, `UpdateTaskStatusRequest`

**Public handlers:** `handle_list_tasks`, `handle_create_tasks`, `handle_claim_tasks`, `handle_unclaim_task`, `handle_update_task_status`

Imports:

```rust
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use super::{api_err, AppState, ApiResult};
use crate::store::tasks::TaskStatus;
```

**Important:** `handle_list_tasks` calls `strip_channel_prefix()`, which lives in `channels.rs`. Either:
- Import it: `use super::channels::strip_channel_prefix;`
- Or duplicate the trivial one-liner inline in `tasks.rs`

The simplest option: duplicate the one-liner `fn strip_channel_prefix(s: &str) -> &str { s.strip_prefix('#').unwrap_or(s) }` privately in tasks.rs, since it's just a string helper.

---

- [ ] **Step 2f: Create `src/server/handlers/attachments.rs`**

Move from `handlers/mod.rs`:

**Public handlers:** `handle_upload`, `handle_get_attachment`

Imports:

```rust
use axum::body::Bytes;
use axum::extract::{Multipart, Path, State};
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use tracing::info;
use uuid::Uuid;

use super::{api_err, internal_err, AppState, ApiResult};
```

---

- [ ] **Step 2g: Create `src/server/handlers/workspace.rs`**

Move from `handlers/mod.rs`:

**Inline structs:** `WorkspaceFileParams`

**Private helpers:** `sanitize_workspace_path()`, `collect_workspace_files()`

**Public handlers:** `handle_agent_workspace`, `handle_agent_workspace_file`

Imports:

```rust
use std::path::{Path, PathBuf};

use axum::extract::{Path as AxumPath, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use super::{api_err, internal_err, AppState, ApiResult};
```

Note: `Path` conflicts with `std::path::Path` — alias Axum's `Path` as `AxumPath`.

---

- [ ] **Step 2h: Create `src/server/handlers/knowledge.rs`**

Move from `handlers/mod.rs`:

**Public handlers:** `handle_remember`, `handle_recall`

Imports:

```rust
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use uuid::Uuid;

use super::{api_err, AppState, ApiResult};
use crate::store::knowledge::{KnowledgeEntry, RecallQuery, RememberRequest};
```

---

- [ ] **Step 2i: Update `src/server/handlers/mod.rs` to declare and re-export sub-modules**

At the top of `handlers/mod.rs`, add the module declarations and re-exports:

```rust
pub mod agents;
pub mod attachments;
pub mod channels;
pub mod knowledge;
pub mod messages;
pub mod tasks;
pub mod workspace;

pub use agents::*;
pub use attachments::*;
pub use channels::*;
pub use knowledge::*;
pub use messages::*;
pub use tasks::*;
pub use workspace::*;
```

Then remove all the functions that were moved to sub-modules from `mod.rs`. What remains in `mod.rs`:

- All `use` imports needed by mod.rs itself
- `pub type ApiResult<T>`
- `pub struct AppState`
- `fn api_err`, `fn internal_err`, `fn conflict_err`
- `struct TransitionGuard`, `impl Drop for TransitionGuard`, `fn acquire_transition`
- `pub async fn handle_whoami`
- `pub async fn handle_server_info`
- `pub async fn handle_ui_server_info`

---

- [ ] **Step 2j: Build and test**

```bash
cargo build
```
Expected: compiles cleanly.

```bash
cargo test
```
Expected: all tests pass.

Fix any import errors (typically "unresolved import" or "function not found in module"). Do NOT proceed with failures.

- [ ] **Step 2k: Commit**

```bash
git add -A
git commit -m "refactor(server): split handlers.rs into domain modules"
```

---

## Task 3: Split `src/bridge.rs` into a directory

**Files:**
- Delete: `src/bridge.rs`
- Create: `src/bridge/mod.rs`, `src/bridge/types.rs`, `src/bridge/format.rs`

---

- [ ] **Step 3a: Create the bridge directory and copy bridge.rs to mod.rs**

```bash
mkdir src/bridge
cp src/bridge.rs src/bridge/mod.rs
rm src/bridge.rs
```

Build to verify the module path `crate::bridge` is unchanged:

```bash
cargo build
```
Expected: compiles cleanly.

---

- [ ] **Step 3b: Create `src/bridge/types.rs`**

Move all `#[derive(Deserialize, JsonSchema)]` param structs from `bridge/mod.rs` into this new file. These are (lines ~77–210 of the original `bridge.rs`):

- `SendMessageParams`
- `ReceiveMessageParams`
- `fn default_true()`
- `EmptyParams`
- `ReadHistoryParams`
- `ListTasksParams`
- `TaskDef`
- `CreateTasksParams`
- `ClaimTasksParams`
- `UnclaimTaskParams`
- `UpdateTaskStatusParams`
- `UploadFileParams`
- `ViewFileParams`
- `RememberParams`
- `RecallParams`

The file needs:

```rust
use rmcp::schemars::{self, JsonSchema};
use serde::Deserialize;
```

---

- [ ] **Step 3c: Create `src/bridge/format.rs`**

Move the three formatting helper functions from `bridge/mod.rs` into this file:

- `fn to_local_time(iso: &str) -> String`
- `fn format_target(m: &serde_json::Value) -> String`
- `fn format_attachments(attachments: Option<&serde_json::Value>) -> String`

The file needs:

```rust
use serde_json::Value;
```

---

- [ ] **Step 3d: Update `src/bridge/mod.rs`**

Add at the top of `bridge/mod.rs`:

```rust
mod format;
mod types;

use format::{format_attachments, format_target, to_local_time};
use types::*;
```

Remove the moved functions and structs from `mod.rs`.

**Important:** `bridge/mod.rs` uses `rmcp::model::ServerInfo` in `fn get_info()`. Do NOT add `use crate::store::*;` glob. Import store types by name only, e.g.:

```rust
use crate::store::ServerInfo as AppServerInfo; // if needed to disambiguate
```

Or keep the existing code which doesn't import `crate::store::ServerInfo` at all in bridge.

---

- [ ] **Step 3e: Build and test**

```bash
cargo build
```
Expected: compiles cleanly.

```bash
cargo test
```
Expected: all tests pass.

```bash
cargo test --test e2e_tests
```
Expected: all e2e tests pass.

- [ ] **Step 3f: Commit**

```bash
git add -A
git commit -m "refactor(bridge): extract param types and format helpers into sub-modules"
```

---

## Final Verification

- [ ] `cargo build` — clean
- [ ] `cargo test` — all pass
- [ ] `cargo test --test e2e_tests` — all pass
- [ ] `git log --oneline -5` — three clean commits visible
- [ ] No HTTP routes changed (check `src/server/mod.rs` — route list identical)
- [ ] No MCP tool names changed (check bridge tool `#[tool]` attributes)
- [ ] No database schema changes
