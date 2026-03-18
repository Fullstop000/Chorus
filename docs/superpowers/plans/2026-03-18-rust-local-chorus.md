# Rust Local Chorus Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rewrite the Slock Chorus daemon as a fully local, self-contained Rust application — no remote server needed. Humans and AI agents (Claude Code, Codex CLI) communicate through channels, DMs, threads, and task boards, all stored locally in SQLite.

**Architecture:** Single Rust binary with two modes: (1) `chorus` — starts the local HTTP API server + agent process manager + human CLI, and (2) `chorus bridge --agent-id <id>` — runs as an MCP stdio server spawned by AI agent processes, proxying tool calls to the local HTTP API. State is stored in SQLite (`~/.chorus/chorus.db`). Message delivery uses tokio broadcast channels for real-time notification.

**Tech Stack:** Rust, tokio, axum, rusqlite, rmcp (MCP SDK), serde/serde_json, clap, uuid, chrono

---

## File Structure

```
Chorus/
├── _old/                        # Original TS/JS files (reference only)
│   ├── src/
│   │   ├── index.ts
│   │   ├── connection.ts
│   │   ├── agentProcessManager.ts
│   │   ├── chat-bridge.ts
│   │   └── drivers/
│   │       ├── index.ts
│   │       ├── claude.ts
│   │       ├── codex.ts
│   │       └── systemPrompt.ts
│   └── shared/
│       └── src/
│           ├── index.ts
│           └── serverPermissions.ts
├── Cargo.toml
├── src/
│   ├── main.rs                  # Entry point: clap subcommands (serve, bridge, send, history, etc.)
│   ├── lib.rs                   # Re-exports all modules
│   ├── models.rs                # All data models (Channel, Message, Task, Agent, Human, Attachment)
│   ├── store.rs                 # SQLite store: schema init, all CRUD operations, message notification
│   ├── server.rs                # Axum HTTP server: all /internal/agent/* and /api/* routes
│   ├── agent_manager.rs         # Agent process lifecycle: start, stop, sleep, deliver, workspace ops
│   ├── drivers/
│   │   ├── mod.rs               # Driver trait definition + ParsedEvent enum
│   │   ├── claude.rs            # Claude Code CLI: spawn args, parse stream-json, stdin encoding
│   │   ├── codex.rs             # Codex CLI: spawn args, parse JSON events
│   │   └── prompt.rs            # System prompt builder (shared across drivers)
│   └── bridge.rs                # MCP chat bridge: rmcp server with all 10 tools, runs as subcommand
├── tests/
│   ├── store_tests.rs           # Unit tests for SQLite store operations
│   ├── server_tests.rs          # Integration tests for HTTP API endpoints
│   └── e2e_tests.rs             # Full end-to-end: start server, spawn mock agent, exchange messages
```

---

## Task 1: Project Scaffolding + Move Old Files

**Files:**
- Create: `Cargo.toml`
- Create: `src/main.rs` (stub)
- Create: `src/lib.rs` (stub)
- Move: `src/*.ts`, `shared/` → `_old/`

- [ ] **Step 1: Move old TS files to `_old/`**

```bash
cd /Users/bytedance/slock-daemon/Chorus
mkdir -p _old/src/drivers _old/shared/src
mv src/index.ts src/connection.ts src/agentProcessManager.ts src/chat-bridge.ts _old/src/
mv src/drivers/systemPrompt.ts src/drivers/claude.ts src/drivers/codex.ts src/drivers/index.ts _old/src/drivers/
mv shared/src/index.ts shared/src/serverPermissions.ts _old/shared/src/
# Clean up empty dirs
rmdir src/drivers shared/src shared 2>/dev/null || true
```

- [ ] **Step 2: Create `Cargo.toml`**

```toml
[package]
name = "chorus"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "chorus"
path = "src/main.rs"

[dependencies]
tokio = { version = "1", features = ["full"] }
axum = { version = "0.8", features = ["json", "multipart"] }
rusqlite = { version = "0.34", features = ["bundled"] }
rmcp = { version = "0.16", features = ["server", "transport-io"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
clap = { version = "4", features = ["derive"] }
uuid = { version = "1", features = ["v4"] }
chrono = { version = "0.4", features = ["serde"] }
anyhow = "1"
reqwest = { version = "0.12", features = ["json", "multipart"] }
whoami = "1"
urlencoding = "2"

[dev-dependencies]
tempfile = "3"
tower = "0.5"
```

- [ ] **Step 3: Create stub `src/main.rs`**

```rust
mod models;
mod store;
mod server;
mod agent_manager;
mod drivers;
mod bridge;

fn main() {
    println!("chorus stub");
}
```

- [ ] **Step 4: Create stub `src/lib.rs`**

```rust
pub mod models;
pub mod store;
pub mod server;
pub mod agent_manager;
pub mod drivers;
pub mod bridge;
```

- [ ] **Step 5: Create stub files for all modules**

Create empty stubs for: `src/models.rs`, `src/store.rs`, `src/server.rs`, `src/agent_manager.rs`, `src/drivers/mod.rs`, `src/drivers/claude.rs`, `src/drivers/codex.rs`, `src/drivers/prompt.rs`, `src/bridge.rs`.

- [ ] **Step 6: Verify it compiles**

Run: `cargo check`
Expected: compiles with no errors (empty modules).

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "chore: scaffold Rust project, move TS sources to _old/"
```

---

## Task 2: Data Models

**Files:**
- Create: `src/models.rs`

All data types used across the application. Every struct derives `Debug, Clone, Serialize, Deserialize`. Models are plain data — no behavior.

- [ ] **Step 1: Write all model structs**

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ── Channel ──

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
}

// ── Message ──

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SenderType {
    Human,
    Agent,
}

// ── Task ──

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

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "todo" => Some(Self::Todo),
            "in_progress" => Some(Self::InProgress),
            "in_review" => Some(Self::InReview),
            "done" => Some(Self::Done),
            _ => None,
        }
    }

    /// Returns true if transitioning from self to `to` is valid.
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

// ── Agent ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    pub id: String,
    pub name: String,
    pub display_name: String,
    pub description: Option<String>,
    pub runtime: String,
    pub model: String,
    pub status: AgentStatus,
    pub session_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Active,
    Sleeping,
    Inactive,
}

// ── Human ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Human {
    pub name: String,
    pub created_at: DateTime<Utc>,
}

// ── Attachment ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub id: String,
    pub filename: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub stored_path: String,
    pub uploaded_at: DateTime<Utc>,
}

// ── Channel membership ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelMember {
    pub channel_id: String,
    pub member_name: String,
    pub member_type: SenderType,
    pub last_read_seq: i64,
}

// ── Agent config (for starting agents) ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub name: String,
    pub display_name: String,
    pub description: Option<String>,
    pub runtime: String,
    pub model: String,
    pub session_id: Option<String>,
    pub env_vars: Option<std::collections::HashMap<String, String>>,
}

// ── API request/response types ──

#[derive(Debug, Serialize, Deserialize)]
pub struct SendRequest {
    pub target: String,
    #[serde(default)]
    pub content: String,
    #[serde(default, rename = "attachmentIds")]
    pub attachment_ids: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SendResponse {
    #[serde(rename = "messageId")]
    pub message_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ReceiveResponse {
    pub messages: Vec<ReceivedMessage>,
}

/// Message as returned by the receive endpoint (matches TS chat-bridge format).
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
pub struct HistoryResponse {
    pub messages: Vec<HistoryMessage>,
    pub has_more: bool,
    pub last_read_seq: i64,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<AttachmentRef>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ServerInfo {
    pub channels: Vec<ChannelInfo>,
    pub agents: Vec<AgentInfo>,
    pub humans: Vec<HumanInfo>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ChannelInfo {
    pub name: String,
    pub description: Option<String>,
    pub joined: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AgentInfo {
    pub name: String,
    pub status: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HumanInfo {
    pub name: String,
}

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

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateTasksRequest {
    pub channel: String,
    pub tasks: Vec<CreateTaskItem>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateTaskItem {
    pub title: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ClaimTasksRequest {
    pub channel: String,
    pub task_numbers: Vec<i64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ClaimResult {
    #[serde(rename = "taskNumber")]
    pub task_number: i64,
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateTaskStatusRequest {
    pub channel: String,
    pub task_number: i64,
    pub status: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UnclaimTaskRequest {
    pub channel: String,
    pub task_number: i64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ResolveChannelRequest {
    pub target: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ResolveChannelResponse {
    #[serde(rename = "channelId")]
    pub channel_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub error: String,
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check`

- [ ] **Step 3: Commit**

```bash
git add src/models.rs
git commit -m "feat: add all data models"
```

---

## Task 3: SQLite Store

**Files:**
- Create: `src/store.rs`
- Test: `tests/store_tests.rs`

The store handles all persistence: schema creation, CRUD for channels/messages/tasks/agents/humans, message notification via `tokio::sync::broadcast`, and target-string parsing (`#channel`, `dm:@name`, `#channel:msgid`).

- [ ] **Step 1: Write store tests first**

Create `tests/store_tests.rs`:

```rust
use chorus::store::Store;
use chorus::models::*;
use tempfile::tempdir;

fn make_store() -> (Store, tempfile::TempDir) {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let store = Store::open(db_path.to_str().unwrap()).unwrap();
    (store, dir) // caller keeps dir alive
}

#[test]
fn test_create_and_list_channels() {
    let (store, _dir) = make_store();
    store.create_channel("general", Some("General channel"), ChannelType::Channel).unwrap();
    store.create_channel("random", None, ChannelType::Channel).unwrap();
    let channels = store.list_channels().unwrap();
    assert_eq!(channels.len(), 2);
    assert_eq!(channels[0].name, "general");
}

#[test]
fn test_send_and_receive_messages() {
    let (store, _dir) = make_store();
    store.create_channel("general", None, ChannelType::Channel).unwrap();
    store.add_human("alice").unwrap();
    store.join_channel("general", "alice", SenderType::Human).unwrap();

    let msg_id = store.send_message("general", None, "alice", SenderType::Human, "hello", &[]).unwrap();
    assert!(!msg_id.is_empty());

    let msgs = store.get_messages_for_agent("bob", false).unwrap();
    // bob is not in the channel, should get nothing
    assert!(msgs.is_empty());

    store.create_agent_record("bot1", "Bot 1", None, "claude", "sonnet").unwrap();
    store.join_channel("general", "bot1", SenderType::Agent).unwrap();

    let msg_id2 = store.send_message("general", None, "alice", SenderType::Human, "hello bot", &[]).unwrap();
    let msgs = store.get_messages_for_agent("bot1", false).unwrap();
    assert_eq!(msgs.len(), 2); // both messages since bot1 joined
}

#[test]
fn test_message_history_pagination() {
    let (store, _dir) = make_store();
    store.create_channel("general", None, ChannelType::Channel).unwrap();
    store.add_human("alice").unwrap();
    store.join_channel("general", "alice", SenderType::Human).unwrap();

    for i in 0..10 {
        store.send_message("general", None, "alice", SenderType::Human, &format!("msg {i}"), &[]).unwrap();
    }

    let (msgs, has_more) = store.get_history("general", None, 5, None, None).unwrap();
    assert_eq!(msgs.len(), 5);
    assert!(has_more);

    let first_seq = msgs[0].seq;
    let (older, _) = store.get_history("general", None, 5, Some(first_seq), None).unwrap();
    assert_eq!(older.len(), 5);
}

#[test]
fn test_tasks_crud() {
    let (store, _dir) = make_store();
    store.create_channel("eng", None, ChannelType::Channel).unwrap();
    store.create_agent_record("bot1", "Bot 1", None, "claude", "sonnet").unwrap();

    let tasks = store.create_tasks("eng", "bot1", &["Fix bug", "Add feature"]).unwrap();
    assert_eq!(tasks.len(), 2);
    assert_eq!(tasks[0].task_number, 1);
    assert_eq!(tasks[1].task_number, 2);

    let listed = store.list_tasks("eng", None).unwrap();
    assert_eq!(listed.len(), 2);
}

#[test]
fn test_task_claim_and_status() {
    let (store, _dir) = make_store();
    store.create_channel("eng", None, ChannelType::Channel).unwrap();
    store.create_agent_record("bot1", "Bot 1", None, "claude", "sonnet").unwrap();
    store.create_agent_record("bot2", "Bot 2", None, "codex", "o3").unwrap();
    store.create_tasks("eng", "bot1", &["Task A"]).unwrap();

    // bot1 claims task 1
    let results = store.claim_tasks("eng", "bot1", &[1]).unwrap();
    assert!(results[0].success);

    // bot2 tries to claim same task — fails
    let results = store.claim_tasks("eng", "bot2", &[1]).unwrap();
    assert!(!results[0].success);

    // bot1 updates status
    store.update_task_status("eng", 1, "bot1", TaskStatus::InReview).unwrap();
    let tasks = store.list_tasks("eng", Some(TaskStatus::InReview)).unwrap();
    assert_eq!(tasks.len(), 1);
}

#[test]
fn test_resolve_target() {
    let (store, _dir) = make_store();
    store.create_channel("general", None, ChannelType::Channel).unwrap();
    store.add_human("alice").unwrap();
    store.create_agent_record("bot1", "Bot 1", None, "claude", "sonnet").unwrap();

    // Channel target
    let (ch_id, thread_parent) = store.resolve_target("#general", "bot1").unwrap();
    assert!(!ch_id.is_empty());
    assert!(thread_parent.is_none());

    // DM target — auto-creates the DM channel
    let (dm_id, _) = store.resolve_target("dm:@alice", "bot1").unwrap();
    assert!(!dm_id.is_empty());
}

#[test]
fn test_dm_channels() {
    let (store, _dir) = make_store();
    store.add_human("alice").unwrap();
    store.create_agent_record("bot1", "Bot 1", None, "claude", "sonnet").unwrap();

    // Sending to dm:@alice from bot1 should create a DM channel
    let (ch_id, _) = store.resolve_target("dm:@alice", "bot1").unwrap();
    // Resolving again should return the same channel
    let (ch_id2, _) = store.resolve_target("dm:@alice", "bot1").unwrap();
    assert_eq!(ch_id, ch_id2);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test store_tests`
Expected: compilation errors (Store not implemented yet).

- [ ] **Step 3: Implement `src/store.rs`**

The store implementation must include:

```rust
use crate::models::*;
use rusqlite::{params, Connection};
use std::sync::Mutex;
use tokio::sync::broadcast;

pub struct Store {
    conn: Mutex<Connection>,
    /// Broadcast sender: fires (channel_id, message_id) on every new message.
    msg_tx: broadcast::Sender<(String, String)>,
}

impl Store {
    pub fn open(path: &str) -> anyhow::Result<Self> { /* open DB, run migrations, return Self */ }
    fn init_schema(conn: &Connection) -> anyhow::Result<()> { /* CREATE TABLE IF NOT EXISTS ... */ }

    // Subscribe to new-message notifications
    pub fn subscribe(&self) -> broadcast::Receiver<(String, String)> { self.msg_tx.subscribe() }

    // ── Channels ──
    pub fn create_channel(&self, name: &str, description: Option<&str>, channel_type: ChannelType) -> anyhow::Result<String> { todo!() }
    pub fn list_channels(&self) -> anyhow::Result<Vec<Channel>> { todo!() }
    pub fn find_channel_by_name(&self, name: &str) -> anyhow::Result<Option<Channel>> { todo!() }
    pub fn find_channel_by_id(&self, id: &str) -> anyhow::Result<Option<Channel>> { todo!() }
    pub fn join_channel(&self, channel_name: &str, member_name: &str, member_type: SenderType) -> anyhow::Result<()> { todo!() }
    pub fn get_channel_members(&self, channel_id: &str) -> anyhow::Result<Vec<ChannelMember>> { todo!() }
    pub fn is_member(&self, channel_name: &str, member_name: &str) -> anyhow::Result<bool> { todo!() }

    // ── Messages ──
    /// Send a message. Returns the message ID. Notifies subscribers.
    pub fn send_message(&self, channel_name: &str, thread_parent_id: Option<&str>, sender_name: &str, sender_type: SenderType, content: &str, attachment_ids: &[String]) -> anyhow::Result<String> { todo!() }
    /// Get new messages for an agent (messages in channels they belong to, after their last_read_seq).
    /// If `update_read_pos` is true, advances the agent's last_read_seq.
    pub fn get_messages_for_agent(&self, agent_name: &str, update_read_pos: bool) -> anyhow::Result<Vec<ReceivedMessage>> { todo!() }
    pub fn get_history(&self, channel_name: &str, thread_parent_id: Option<&str>, limit: i64, before: Option<i64>, after: Option<i64>) -> anyhow::Result<(Vec<HistoryMessage>, bool)> { todo!() }
    pub fn get_last_read_seq(&self, channel_name: &str, member_name: &str) -> anyhow::Result<i64> { todo!() }

    // ── Target resolution ──
    /// Parse a target string (#channel, dm:@name, #channel:msgid, dm:@name:msgid)
    /// Returns (channel_id, Option<thread_parent_message_id>).
    /// Auto-creates DM channels if they don't exist.
    pub fn resolve_target(&self, target: &str, sender_name: &str) -> anyhow::Result<(String, Option<String>)> { todo!() }
    /// Reverse: given channel_id + optional thread_parent_id, produce a target string.
    pub fn format_target(&self, channel_id: &str, thread_parent_id: Option<&str>) -> anyhow::Result<String> { todo!() }

    // ── Agents ──
    pub fn create_agent_record(&self, name: &str, display_name: &str, description: Option<&str>, runtime: &str, model: &str) -> anyhow::Result<String> { todo!() }
    pub fn list_agents(&self) -> anyhow::Result<Vec<Agent>> { todo!() }
    pub fn get_agent(&self, name: &str) -> anyhow::Result<Option<Agent>> { todo!() }
    pub fn update_agent_status(&self, name: &str, status: AgentStatus) -> anyhow::Result<()> { todo!() }
    pub fn update_agent_session(&self, name: &str, session_id: &str) -> anyhow::Result<()> { todo!() }

    // ── Humans ──
    pub fn add_human(&self, name: &str) -> anyhow::Result<()> { todo!() }
    pub fn list_humans(&self) -> anyhow::Result<Vec<Human>> { todo!() }

    // ── Tasks ──
    pub fn create_tasks(&self, channel_name: &str, creator_name: &str, titles: &[&str]) -> anyhow::Result<Vec<TaskInfo>> { todo!() }
    pub fn list_tasks(&self, channel_name: &str, status_filter: Option<TaskStatus>) -> anyhow::Result<Vec<TaskInfo>> { todo!() }
    pub fn claim_tasks(&self, channel_name: &str, claimer_name: &str, task_numbers: &[i64]) -> anyhow::Result<Vec<ClaimResult>> { todo!() }
    pub fn unclaim_task(&self, channel_name: &str, claimer_name: &str, task_number: i64) -> anyhow::Result<()> { todo!() }
    pub fn update_task_status(&self, channel_name: &str, task_number: i64, requester_name: &str, new_status: TaskStatus) -> anyhow::Result<()> { todo!() }

    // ── Attachments ──
    pub fn store_attachment(&self, filename: &str, mime_type: &str, size: i64, stored_path: &str) -> anyhow::Result<String> { todo!() }
    pub fn get_attachment(&self, id: &str) -> anyhow::Result<Option<Attachment>> { todo!() }

    // ── Sender type lookup ──
    /// Look up whether a name belongs to a human or agent. Used by server to tag messages correctly.
    pub fn lookup_sender_type(&self, name: &str) -> anyhow::Result<Option<SenderType>> { todo!() }

    // ── Server info ──
    pub fn get_server_info(&self, for_agent: &str) -> anyhow::Result<ServerInfo> { todo!() }

    // ── Unread summary ──
    /// Returns a map of channel_label -> unread_count for an agent (used for wake prompts).
    pub fn get_unread_summary(&self, agent_name: &str) -> anyhow::Result<std::collections::HashMap<String, i64>> { todo!() }
}
```

**SQLite schema** (inside `init_schema`):

```sql
CREATE TABLE IF NOT EXISTS channels (
    id TEXT PRIMARY KEY,
    name TEXT UNIQUE NOT NULL,
    description TEXT,
    channel_type TEXT NOT NULL DEFAULT 'channel',
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS channel_members (
    channel_id TEXT NOT NULL,
    member_name TEXT NOT NULL,
    member_type TEXT NOT NULL,
    last_read_seq INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (channel_id, member_name)
);

CREATE TABLE IF NOT EXISTS messages (
    id TEXT PRIMARY KEY,
    channel_id TEXT NOT NULL,
    thread_parent_id TEXT,
    sender_name TEXT NOT NULL,
    sender_type TEXT NOT NULL,
    content TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    seq INTEGER NOT NULL,
    UNIQUE(channel_id, seq)
);

CREATE TABLE IF NOT EXISTS message_attachments (
    message_id TEXT NOT NULL,
    attachment_id TEXT NOT NULL,
    PRIMARY KEY (message_id, attachment_id)
);

CREATE TABLE IF NOT EXISTS agents (
    id TEXT PRIMARY KEY,
    name TEXT UNIQUE NOT NULL,
    display_name TEXT NOT NULL,
    description TEXT,
    runtime TEXT NOT NULL,
    model TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'inactive',
    session_id TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS humans (
    name TEXT PRIMARY KEY,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS tasks (
    id TEXT PRIMARY KEY,
    channel_id TEXT NOT NULL,
    task_number INTEGER NOT NULL,
    title TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'todo',
    claimed_by TEXT,
    created_by TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(channel_id, task_number)
);

CREATE TABLE IF NOT EXISTS attachments (
    id TEXT PRIMARY KEY,
    filename TEXT NOT NULL,
    mime_type TEXT NOT NULL,
    size_bytes INTEGER NOT NULL,
    stored_path TEXT NOT NULL,
    uploaded_at TEXT NOT NULL DEFAULT (datetime('now'))
);
```

Key implementation details:
- `seq` is monotonically increasing per channel. Use `SELECT COALESCE(MAX(seq), 0) + 1 FROM messages WHERE channel_id = ?`.
- `resolve_target` parses `#channel`, `dm:@name`, `#channel:msgid`, `dm:@name:msgid`.
  - For DMs: find or create a DM channel named `dm-{sorted_names}`. Auto-add both parties as members.
  - For threads: the `msgid` is the first 8 chars of a message UUID. Look up the full message ID via `WHERE id LIKE '{msgid}%'`.
- **Thread filtering (CRITICAL)**: When fetching main channel messages (`get_messages_for_agent`, `get_history` with no thread_parent_id), always add `WHERE thread_parent_id IS NULL` to exclude thread replies. When fetching thread history, use `WHERE thread_parent_id = ?`.
- `join_channel` sets `last_read_seq = 0` — this means the agent will see all prior messages on first receive. This is intentional for catch-up.
- `get_messages_for_agent`: find all channels the agent is a member of, get messages with `seq > last_read_seq AND thread_parent_id IS NULL` for each channel.
- `lookup_sender_type`: check if name exists in `agents` table first (return `Agent`), then `humans` table (return `Human`), else `None`.
- `msg_tx.send()` is called inside `send_message` after insert.
- `get_unread_summary`: for each channel the agent is in, count messages with `seq > last_read_seq AND thread_parent_id IS NULL`. Return as `{"#channel_name": count, ...}`.

- [ ] **Step 4: Run tests**

Run: `cargo test --test store_tests`
Expected: all 7 tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/store.rs tests/store_tests.rs
git commit -m "feat: implement SQLite store with full CRUD"
```

---

## Task 4: HTTP API Server

**Files:**
- Create: `src/server.rs`
- Test: `tests/server_tests.rs`

An axum HTTP server implementing all `/internal/agent/:agentId/*` endpoints and `/api/attachments/:id`. This replaces the remote Slock server for the chat-bridge.

- [ ] **Step 1: Write server integration tests**

Create `tests/server_tests.rs`:

```rust
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chorus::server::build_router;
use chorus::store::Store;
use chorus::models::*;
use std::sync::Arc;
use tower::ServiceExt; // for `oneshot`

fn setup() -> (Arc<Store>, axum::Router) {
    let store = Arc::new(Store::open(":memory:").unwrap());
    store.create_channel("general", Some("General"), ChannelType::Channel).unwrap();
    store.add_human("alice").unwrap();
    store.join_channel("general", "alice", SenderType::Human).unwrap();
    store.create_agent_record("bot1", "Bot 1", None, "claude", "sonnet").unwrap();
    store.join_channel("general", "bot1", SenderType::Agent).unwrap();
    let router = build_router(store.clone());
    (store, router)
}

#[tokio::test]
async fn test_send_and_receive() {
    let (store, app) = setup();

    // Send a message as alice
    let send_req = serde_json::json!({ "target": "#general", "content": "hello" });
    let resp = app.clone().oneshot(
        Request::builder()
            .method("POST")
            .uri("/internal/agent/bot1/send")  // agent sends on behalf
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&send_req).unwrap()))
            .unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Receive (non-blocking) as bot1
    let resp = app.clone().oneshot(
        Request::builder()
            .uri("/internal/agent/bot1/receive?block=false")
            .body(Body::empty())
            .unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_server_info() {
    let (_store, app) = setup();
    let resp = app.oneshot(
        Request::builder()
            .uri("/internal/agent/bot1/server")
            .body(Body::empty())
            .unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 1_000_000).await.unwrap();
    let info: ServerInfo = serde_json::from_slice(&body).unwrap();
    assert_eq!(info.channels.len(), 1);
    assert_eq!(info.agents.len(), 1);
    assert_eq!(info.humans.len(), 1);
}

#[tokio::test]
async fn test_task_workflow() {
    let (_store, app) = setup();

    // Create tasks
    let req = serde_json::json!({ "channel": "#general", "tasks": [{"title": "Fix bug"}] });
    let resp = app.clone().oneshot(
        Request::builder()
            .method("POST")
            .uri("/internal/agent/bot1/tasks")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&req).unwrap()))
            .unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // List tasks
    let resp = app.clone().oneshot(
        Request::builder()
            .uri("/internal/agent/bot1/tasks?channel=%23general")
            .body(Body::empty())
            .unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Claim task
    let req = serde_json::json!({ "channel": "#general", "task_numbers": [1] });
    let resp = app.clone().oneshot(
        Request::builder()
            .method("POST")
            .uri("/internal/agent/bot1/tasks/claim")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&req).unwrap()))
            .unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_history() {
    let (store, app) = setup();
    store.send_message("general", None, "alice", SenderType::Human, "msg 1", &[]).unwrap();
    store.send_message("general", None, "alice", SenderType::Human, "msg 2", &[]).unwrap();

    let resp = app.oneshot(
        Request::builder()
            .uri("/internal/agent/bot1/history?channel=%23general&limit=10")
            .body(Body::empty())
            .unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 1_000_000).await.unwrap();
    let hist: HistoryResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(hist.messages.len(), 2);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test server_tests`
Expected: compilation error.

- [ ] **Step 3: Implement `src/server.rs`**

```rust
use crate::models::*;
use crate::store::Store;
use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use std::sync::Arc;
use std::collections::HashMap;

type AppState = Arc<Store>;

pub fn build_router(store: Arc<Store>) -> Router {
    Router::new()
        // Message routes
        .route("/internal/agent/{agent_id}/send", post(handle_send))
        .route("/internal/agent/{agent_id}/receive", get(handle_receive))
        .route("/internal/agent/{agent_id}/history", get(handle_history))
        // Server info
        .route("/internal/agent/{agent_id}/server", get(handle_server_info))
        // Channel resolution
        .route("/internal/agent/{agent_id}/resolve-channel", post(handle_resolve_channel))
        // Tasks
        .route("/internal/agent/{agent_id}/tasks", get(handle_list_tasks).post(handle_create_tasks))
        .route("/internal/agent/{agent_id}/tasks/claim", post(handle_claim_tasks))
        .route("/internal/agent/{agent_id}/tasks/unclaim", post(handle_unclaim_task))
        .route("/internal/agent/{agent_id}/tasks/update-status", post(handle_update_task_status))
        // Attachments
        .route("/internal/agent/{agent_id}/upload", post(handle_upload))
        .route("/api/attachments/{attachment_id}", get(handle_get_attachment))
        .with_state(store)
}

async fn handle_send(
    State(store): State<AppState>,
    Path(agent_id): Path<String>,
    Json(req): Json<SendRequest>,
) -> Result<Json<SendResponse>, Json<ErrorResponse>> {
    // Detect sender type: could be human (CLI) or agent (bridge)
    let sender_type = store.lookup_sender_type(&agent_id)
        .map_err(|e| Json(ErrorResponse { error: e.to_string() }))?
        .unwrap_or(SenderType::Human);
    let (channel_id, thread_parent) = store.resolve_target(&req.target, &agent_id)
        .map_err(|e| Json(ErrorResponse { error: e.to_string() }))?;
    let channel = store.find_channel_by_id(&channel_id)
        .map_err(|e| Json(ErrorResponse { error: e.to_string() }))?
        .ok_or_else(|| Json(ErrorResponse { error: "Channel not found".into() }))?;
    let msg_id = store.send_message(&channel.name, thread_parent.as_deref(), &agent_id, sender_type, &req.content, &req.attachment_ids)
        .map_err(|e| Json(ErrorResponse { error: e.to_string() }))?;
    Ok(Json(SendResponse { message_id: msg_id }))
}

async fn handle_receive(
    State(store): State<AppState>,
    Path(agent_id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Json<ReceiveResponse> {
    let block = params.get("block").map(|v| v == "true").unwrap_or(false);
    let timeout_ms: u64 = params.get("timeout").and_then(|v| v.parse().ok()).unwrap_or(59_000);

    // Try immediate fetch
    let msgs = store.get_messages_for_agent(&agent_id, true).unwrap_or_default();
    if !msgs.is_empty() || !block {
        return Json(ReceiveResponse { messages: msgs });
    }

    // Block: wait for notification or timeout
    let mut rx = store.subscribe();
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_millis(timeout_ms);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(_)) => {
                let msgs = store.get_messages_for_agent(&agent_id, true).unwrap_or_default();
                if !msgs.is_empty() {
                    return Json(ReceiveResponse { messages: msgs });
                }
            }
            _ => break,
        }
    }
    Json(ReceiveResponse { messages: vec![] })
}

// ... remaining handlers follow the same pattern:
// handle_history, handle_server_info, handle_resolve_channel,
// handle_list_tasks, handle_create_tasks, handle_claim_tasks,
// handle_unclaim_task, handle_update_task_status,
// handle_upload, handle_get_attachment
//
// Each handler: extract Path(agent_id), parse Query/Json, call store, return Json.
```

Key details:
- `handle_receive` uses `store.subscribe()` (broadcast channel) to long-poll.
- `handle_upload` uses `axum::extract::Multipart` to receive file data, stores to `~/.chorus/attachments/`, records in DB.
- `handle_get_attachment` reads file from disk, returns with correct `Content-Type`.
- Error responses use `(StatusCode, Json<ErrorResponse>)` return type.

- [ ] **Step 4: Run tests**

Run: `cargo test --test server_tests`
Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/server.rs tests/server_tests.rs
git commit -m "feat: implement HTTP API server with all endpoints"
```

---

## Task 5: Driver Trait + System Prompt Builder

**Files:**
- Create: `src/drivers/mod.rs`
- Create: `src/drivers/prompt.rs`

- [ ] **Step 1: Define the Driver trait and ParsedEvent enum**

`src/drivers/mod.rs`:

```rust
pub mod claude;
pub mod codex;
pub mod prompt;

use std::process::Child;

/// Events parsed from agent CLI stdout.
#[derive(Debug, Clone)]
pub enum ParsedEvent {
    SessionInit { session_id: String },
    Thinking { text: String },
    Text { text: String },
    ToolCall { name: String, input: serde_json::Value },
    TurnEnd { session_id: Option<String> },
    Error { message: String },
}

/// Spawn context passed to drivers.
pub struct SpawnContext {
    pub agent_id: String,
    pub agent_name: String,
    pub config: crate::models::AgentConfig,
    pub prompt: String,
    pub working_directory: String,
    pub bridge_binary: String,  // path to `chorus bridge`
    pub server_url: String,     // local HTTP server URL
}

/// Runtime driver for a specific CLI (Claude, Codex, etc.)
pub trait Driver: Send + Sync {
    fn id(&self) -> &str;
    fn supports_stdin_notification(&self) -> bool;
    fn mcp_tool_prefix(&self) -> &str;

    /// Spawn the agent CLI process. Returns the child process.
    fn spawn(&self, ctx: &SpawnContext) -> anyhow::Result<Child>;

    /// Parse a single line of stdout into zero or more events.
    fn parse_line(&self, line: &str) -> Vec<ParsedEvent>;

    /// Encode a notification message to send via stdin (for wake-on-message).
    /// Returns None if not supported.
    fn encode_stdin_message(&self, text: &str, session_id: &str) -> Option<String>;

    /// Build the initial system prompt for a fresh agent.
    fn build_system_prompt(&self, config: &crate::models::AgentConfig, agent_id: &str) -> String;

    /// Human-readable display name for a tool.
    fn tool_display_name(&self, name: &str) -> String;

    /// Summarize tool input for trajectory display.
    fn summarize_tool_input(&self, name: &str, input: &serde_json::Value) -> String;
}
```

- [ ] **Step 2: Implement the system prompt builder**

`src/drivers/prompt.rs` — port of `_old/src/drivers/systemPrompt.ts`. This is a large string template. Key function:

```rust
pub struct PromptOptions {
    pub tool_prefix: String,
    pub extra_critical_rules: Vec<String>,
    pub post_startup_notes: Vec<String>,
    pub include_stdin_notification_section: bool,
}

pub fn build_base_system_prompt(config: &crate::models::AgentConfig, opts: &PromptOptions) -> String {
    // Direct port of buildBaseSystemPrompt from _old/src/drivers/systemPrompt.ts
    // Replace {toolPrefix}receive_message etc. with opts.tool_prefix + tool name
    // Include all sections: identity, communication, startup, messaging, threads,
    // task boards, @mentions, communication style, workspace & memory, capabilities
    // See _old/src/drivers/systemPrompt.ts for the complete template.
    todo!()
}
```

Copy the full prompt text from `_old/src/drivers/systemPrompt.ts`, converting JS template literals to Rust `format!` / string concatenation. Keep the content identical.

- [ ] **Step 3: Verify it compiles**

Run: `cargo check`

- [ ] **Step 4: Commit**

```bash
git add src/drivers/
git commit -m "feat: add Driver trait and system prompt builder"
```

---

## Task 6: Claude Driver

**Files:**
- Create: `src/drivers/claude.rs`

Port of `_old/src/drivers/claude.ts`. Spawns `claude` CLI with `--output-format stream-json --input-format stream-json`, parses NDJSON output.

- [ ] **Step 1: Implement Claude driver**

```rust
use super::{Driver, ParsedEvent, SpawnContext};
use std::process::{Child, Command, Stdio};

pub struct ClaudeDriver;

impl Driver for ClaudeDriver {
    fn id(&self) -> &str { "claude" }
    fn supports_stdin_notification(&self) -> bool { true }
    fn mcp_tool_prefix(&self) -> &str { "mcp__chat__" }

    fn spawn(&self, ctx: &SpawnContext) -> anyhow::Result<Child> {
        // Write MCP config JSON to working_directory/.chorus-mcp.json
        // containing: { mcpServers: { chat: { command: "chorus", args: ["bridge", "--agent-id", ...] } } }
        let mcp_config = serde_json::json!({
            "mcpServers": {
                "chat": {
                    "command": &ctx.bridge_binary,
                    "args": ["bridge", "--agent-id", &ctx.agent_id, "--server-url", &ctx.server_url]
                }
            }
        });
        let mcp_config_path = std::path::Path::new(&ctx.working_directory).join(".chorus-mcp.json");
        std::fs::write(&mcp_config_path, serde_json::to_string(&mcp_config)?)?;

        let mut args = vec![
            "--allow-dangerously-skip-permissions".to_string(),
            "--dangerously-skip-permissions".to_string(),
            "--verbose".to_string(),
            "--output-format".to_string(), "stream-json".to_string(),
            "--input-format".to_string(), "stream-json".to_string(),
            "--mcp-config".to_string(), mcp_config_path.to_string_lossy().into_owned(),
            "--model".to_string(), ctx.config.model.clone(),
        ];
        if let Some(ref sid) = ctx.config.session_id {
            args.extend(["--resume".to_string(), sid.clone()]);
        }

        let mut cmd = Command::new("claude");
        cmd.args(&args)
            .current_dir(&ctx.working_directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("FORCE_COLOR", "0");
        // Remove CLAUDECODE env to avoid nesting issues
        cmd.env_remove("CLAUDECODE");
        if let Some(ref vars) = ctx.config.env_vars {
            for (k, v) in vars { cmd.env(k, v); }
        }

        let mut child = cmd.spawn()?;

        // Send initial prompt via stdin (stream-json input format)
        let stdin_msg = serde_json::json!({
            "type": "user",
            "message": { "role": "user", "content": [{ "type": "text", "text": &ctx.prompt }] },
        });
        if let Some(ref mut stdin) = child.stdin {
            use std::io::Write;
            writeln!(stdin, "{}", serde_json::to_string(&stdin_msg)?)?;
        }

        Ok(child)
    }

    fn parse_line(&self, line: &str) -> Vec<ParsedEvent> {
        // Parse NDJSON line, match on event.type:
        // "system" with subtype "init" → SessionInit
        // "assistant" with content blocks → Thinking/Text/ToolCall
        // "result" → TurnEnd
        // See _old/src/drivers/claude.ts parseLine() for exact logic
        todo!()
    }

    fn encode_stdin_message(&self, text: &str, session_id: &str) -> Option<String> {
        Some(serde_json::json!({
            "type": "user",
            "message": { "role": "user", "content": [{ "type": "text", "text": text }] },
            "session_id": session_id
        }).to_string())
    }

    fn build_system_prompt(&self, config: &crate::models::AgentConfig, _agent_id: &str) -> String {
        super::prompt::build_base_system_prompt(config, &super::prompt::PromptOptions {
            tool_prefix: "mcp__chat__".into(),
            extra_critical_rules: vec![
                "- Do NOT use bash/curl/sqlite to send or receive messages. The MCP tools handle everything.".into(),
            ],
            post_startup_notes: vec![],
            include_stdin_notification_section: true,
        })
    }

    fn tool_display_name(&self, name: &str) -> String {
        // Port from _old/src/drivers/claude.ts toolDisplayName()
        todo!()
    }

    fn summarize_tool_input(&self, name: &str, input: &serde_json::Value) -> String {
        // Port from _old/src/drivers/claude.ts summarizeToolInput()
        todo!()
    }
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check`

- [ ] **Step 3: Commit**

```bash
git add src/drivers/claude.rs
git commit -m "feat: implement Claude Code driver"
```

---

## Task 7: Codex Driver

**Files:**
- Create: `src/drivers/codex.rs`

Port of `_old/src/drivers/codex.ts`. Spawns `codex exec` with `--json`, parses event stream.

- [ ] **Step 1: Implement Codex driver**

Same pattern as Claude driver, but:
- Spawns `codex exec [--resume <sid>] --dangerously-bypass-approvals-and-sandbox --json -c mcp_servers.chat.command=... <prompt>`
- Initializes a git repo in the working directory if none exists
- `parse_line` handles codex event types: `thread.started`, `turn.started`, `item.started/updated/completed`, `turn.completed/failed`, `error`
- Item types: `reasoning`, `agent_message`, `command_execution`, `file_change`, `mcp_tool_call`, `web_search`, `todo_list`, `error`
- Does NOT support stdin notifications (`encode_stdin_message` returns `None`)
- System prompt includes note about process exiting after each turn

See `_old/src/drivers/codex.ts` for all the parsing logic.

- [ ] **Step 2: Verify it compiles**

Run: `cargo check`

- [ ] **Step 3: Commit**

```bash
git add src/drivers/codex.rs
git commit -m "feat: implement Codex CLI driver"
```

---

## Task 8: Agent Process Manager

**Files:**
- Create: `src/agent_manager.rs`

Port of `_old/src/agentProcessManager.ts`. Manages agent child processes, message delivery, stdin notifications, workspace operations.

- [ ] **Step 1: Implement AgentProcessManager**

```rust
use crate::drivers::{Driver, ParsedEvent, SpawnContext};
use crate::models::*;
use crate::store::Store;
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::process::Child;
use std::sync::Arc;
use tokio::sync::Mutex;

struct RunningAgent {
    process: Child,
    driver: Arc<dyn Driver>,
    session_id: Option<String>,
    is_in_receive_message: bool,
    pending_notification_count: u32,
}

pub struct AgentManager {
    agents: Arc<Mutex<HashMap<String, RunningAgent>>>,
    store: Arc<Store>,
    data_dir: PathBuf,
    bridge_binary: String,
    server_url: String,
}

impl AgentManager {
    pub fn new(store: Arc<Store>, data_dir: PathBuf, bridge_binary: String, server_url: String) -> Self { todo!() }

    /// Start an agent process. Creates workspace dir, writes MEMORY.md, spawns CLI.
    pub async fn start_agent(&self, agent_name: &str) -> anyhow::Result<()> { todo!() }

    /// Stop an agent process (SIGTERM).
    pub async fn stop_agent(&self, agent_name: &str) -> anyhow::Result<()> { todo!() }

    /// Sleep an agent (kill process, keep status as sleeping).
    pub async fn sleep_agent(&self, agent_name: &str) -> anyhow::Result<()> { todo!() }

    /// Deliver a message notification to agent. If agent supports stdin notifications
    /// and is not currently in receive_message, send a batched notification.
    pub async fn notify_agent(&self, agent_name: &str) -> anyhow::Result<()> { todo!() }

    /// Stop all running agents.
    pub async fn stop_all(&self) -> anyhow::Result<()> { todo!() }

    pub fn get_running_agent_names(&self) -> Vec<String> { todo!() }

    /// Spawn a background task that reads agent stdout, parses events,
    /// updates store (session_id, status), logs trajectory.
    fn spawn_output_reader(&self, agent_name: String, child_stdout: std::process::ChildStdout, driver: Arc<dyn Driver>) { todo!() }
}
```

Key behaviors (ported from TS):

**`start_agent` prompt construction (CRITICAL — matches TS `agentProcessManager.ts` lines 29-93):**
1. Look up agent record from store (`store.get_agent(name)`).
2. Determine if this is a resume (`agent.session_id.is_some()`).
3. Compute `unread_summary` via `store.get_unread_summary(name)`.
4. Build the initial prompt:
   - **Fresh start** (no session_id): call `driver.build_system_prompt(config, agent_id)` — the full system prompt.
   - **Resume with unread**: format unread summary as `"You have unread messages:\n- #ch: N unread\n..."` + instructions to call `read_history` then `receive_message(block=true)`.
   - **Resume with no unread**: `"No new messages. Call receive_message(block=true) to listen."`
   - If driver supports stdin notifications, append the notification note.
5. Create workspace dir `~/.chorus/agents/{name}/`, write initial `MEMORY.md` if absent.
6. Call `driver.spawn(ctx)` with the constructed prompt.
7. Spawn tokio task to read stdout.

**`spawn_output_reader`**: processes `ParsedEvent`s — updates `is_in_receive_message` on tool calls, updates `session_id` on `SessionInit`/`TurnEnd`, updates agent status in store.

**Process exit**: code 0 → mark sleeping in store, otherwise → mark inactive.

**Stdin notification**: when a new message arrives for a busy agent, wait 3 seconds then send batched notification via `driver.encode_stdin_message()`.

**Workspace operations** (for agent file management):
- `reset_workspace(name)`: delete `~/.chorus/agents/{name}/` directory.
- Agent data dir is passed as `working_directory` to the driver.

- [ ] **Step 2: Verify it compiles**

Run: `cargo check`

- [ ] **Step 3: Commit**

```bash
git add src/agent_manager.rs
git commit -m "feat: implement agent process manager"
```

---

## Task 9: MCP Chat Bridge

**Files:**
- Create: `src/bridge.rs`

The bridge runs as a subcommand (`chorus bridge --agent-id <id> --server-url <url>`). It's an MCP server over stdio (using `rmcp`) that proxies all 10 chat tools to the local HTTP API via `reqwest`.

- [ ] **Step 1: Implement bridge with all 10 MCP tools**

```rust
use rmcp::{ServerHandler, tool, tool_handler, tool_router, ServiceExt, transport::stdio};
use rmcp::model::{ServerInfo as McpServerInfo, ErrorData as McpError};

#[derive(Clone)]
pub struct ChatBridge {
    agent_id: String,
    server_url: String,
    client: reqwest::Client,
}

#[tool_router]
impl ChatBridge {
    #[tool(description = "Send a message to a channel, DM, or thread.")]
    async fn send_message(
        &self,
        #[tool(param, description = "Target: '#channel', 'dm:@name', '#channel:id', 'dm:@name:id'")] target: String,
        #[tool(param, description = "Message content")] content: String,
        #[tool(param, description = "Optional attachment IDs")] attachment_ids: Option<Vec<String>>,
    ) -> Result<String, McpError> {
        let resp = self.client.post(format!("{}/internal/agent/{}/send", self.server_url, self.agent_id))
            .json(&serde_json::json!({ "target": target, "content": content, "attachmentIds": attachment_ids.unwrap_or_default() }))
            .send().await.map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let data: serde_json::Value = resp.json().await.map_err(|e| McpError::internal_error(e.to_string(), None))?;
        // Format response like TS version: "Message sent to {target}. Message ID: {id} (to reply in thread, use ...)"
        Ok(format_send_response(&target, &data))
    }

    #[tool(description = "Receive new messages. Use block=true to wait.")]
    async fn receive_message(
        &self,
        #[tool(param, description = "Wait for messages (default true)")] block: Option<bool>,
        #[tool(param, description = "Timeout in ms (default 59000)")] timeout_ms: Option<u64>,
    ) -> Result<String, McpError> {
        let block = block.unwrap_or(true);
        let timeout = timeout_ms.unwrap_or(59_000);
        let resp = self.client.get(format!(
            "{}/internal/agent/{}/receive?block={}&timeout={}",
            self.server_url, self.agent_id, block, timeout
        )).send().await.map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let data: serde_json::Value = resp.json().await.map_err(|e| McpError::internal_error(e.to_string(), None))?;
        // Format messages like TS version: [target=... msg=... time=...] @sender: content
        Ok(format_received_messages(&data))
    }

    // ... implement all 10 tools:
    // send_message, receive_message, read_history, list_server,
    // list_tasks, create_tasks, claim_tasks, unclaim_task, update_task_status,
    // upload_file, view_file
    //
    // Each tool: HTTP call to local server → format response as text string.
}

#[tool_handler]
impl ServerHandler for ChatBridge {
    fn get_info(&self) -> McpServerInfo {
        McpServerInfo { name: "chat".into(), version: "1.0.0".into(), ..Default::default() }
    }
}

/// Entry point when run as `chorus bridge --agent-id X --server-url Y`
pub async fn run_bridge(agent_id: String, server_url: String) -> anyhow::Result<()> {
    let bridge = ChatBridge {
        agent_id,
        server_url,
        client: reqwest::Client::new(),
    };
    let service = bridge.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
```

All 10 tools should produce text output matching the TS chat-bridge format exactly, since the system prompt references this format.

- [ ] **Step 2: Verify it compiles**

Run: `cargo check`

- [ ] **Step 3: Commit**

```bash
git add src/bridge.rs
git commit -m "feat: implement MCP chat bridge with all tools"
```

---

## Task 10: Main Entry Point + CLI

**Files:**
- Create: `src/main.rs` (replace stub)

Ties everything together. Uses `clap` for subcommands.

- [ ] **Step 1: Implement CLI with all subcommands**

```rust
use clap::{Parser, Subcommand};
use std::sync::Arc;

#[derive(Parser)]
#[command(name = "chorus", about = "Local AI agent collaboration platform")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Port for the local HTTP API server
    #[arg(long, default_value = "3001")]
    port: u16,

    /// Data directory
    #[arg(long, default_value_t = default_data_dir())]
    data_dir: String,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the server + agent manager (default if no subcommand)
    Serve {
        #[arg(long, default_value = "3001")]
        port: u16,
    },
    /// Run as MCP chat bridge (spawned by agent processes)
    Bridge {
        #[arg(long)]
        agent_id: String,
        #[arg(long, default_value = "http://localhost:3001")]
        server_url: String,
    },
    /// Create a new agent
    Agent {
        #[command(subcommand)]
        cmd: AgentCommands,
    },
    /// Send a message as the human user
    Send {
        /// Target: #channel, dm:@name, etc.
        target: String,
        /// Message content
        content: String,
    },
    /// Read message history
    History {
        /// Target: #channel, dm:@name, etc.
        channel: String,
        #[arg(long, default_value = "20")]
        limit: i64,
    },
    /// List channels, agents, humans
    Status,
    /// Create a channel
    Channel {
        name: String,
        #[arg(long)]
        description: Option<String>,
    },
}

#[derive(Subcommand)]
enum AgentCommands {
    /// Create and start a new agent
    Create {
        name: String,
        #[arg(long, default_value = "claude")]
        runtime: String,
        #[arg(long, default_value = "sonnet")]
        model: String,
        #[arg(long)]
        description: Option<String>,
    },
    /// Stop a running agent
    Stop { name: String },
    /// List all agents
    List,
}

fn default_data_dir() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    format!("{home}/.chorus")
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Bridge { agent_id, server_url }) => {
            chorus::bridge::run_bridge(agent_id, server_url).await
        }
        Some(Commands::Serve { port }) | None => {
            // 1. Open SQLite store
            // 2. Ensure default human user exists (from OS username)
            // 3. Start axum HTTP server on port
            // 4. Create AgentManager
            // 5. Print startup message with URL
            // 6. Wait for Ctrl+C, then stop all agents
            todo!()
        }
        Some(Commands::Send { target, content }) => {
            // HTTP POST to local server /internal/agent/{human_name}/send
            todo!()
        }
        Some(Commands::History { channel, limit }) => {
            // HTTP GET /internal/agent/{human_name}/history?channel=...
            todo!()
        }
        Some(Commands::Status) => {
            // HTTP GET /internal/agent/{human_name}/server
            todo!()
        }
        Some(Commands::Channel { name, description }) => {
            // Create channel via store directly (or HTTP POST)
            todo!()
        }
        Some(Commands::Agent { cmd }) => {
            match cmd {
                AgentCommands::Create { name, runtime, model, description } => {
                    // Create agent record in store, then start via agent manager
                    todo!()
                }
                AgentCommands::Stop { name } => {
                    // Stop agent via agent manager
                    todo!()
                }
                AgentCommands::List => {
                    // List agents from store
                    todo!()
                }
            }
        }
    }
}
```

- [ ] **Step 2: Implement `serve` subcommand (the main flow)**

```rust
// Inside serve handler:
let data_dir = std::path::PathBuf::from(&cli.data_dir);
std::fs::create_dir_all(&data_dir)?;
let db_path = data_dir.join("chorus.db");
let store = Arc::new(Store::open(db_path.to_str().unwrap())?);

// Default human = OS username
let username = whoami::username();
let _ = store.add_human(&username);

// Create default #general channel if none exist
if store.list_channels()?.is_empty() {
    store.create_channel("general", Some("General channel for all members"), ChannelType::Channel)?;
    store.join_channel("general", &username, SenderType::Human)?;
}

let server_url = format!("http://localhost:{port}");
let bridge_binary = std::env::current_exe()?.to_string_lossy().into_owned();
let manager = Arc::new(AgentManager::new(store.clone(), data_dir.join("agents"), bridge_binary, server_url.clone()));

let router = build_router(store.clone());
let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?;
println!("Chorus running at {server_url}");
println!("Human user: @{username}");
println!("Use `chorus send '#general' 'hello'` to send messages");
println!("Use `chorus agent create <name>` to create an agent");

axum::serve(listener, router).with_graceful_shutdown(shutdown_signal()).await?;
manager.stop_all().await?;
Ok(())
```

- [ ] **Step 3: Verify it compiles and runs**

Run: `cargo run -- serve --port 3001`
Expected: starts server, prints URL.

- [ ] **Step 4: Commit**

```bash
git add src/main.rs
git commit -m "feat: implement CLI entry point with all subcommands"
```

---

## Task 11: End-to-End Tests

**Files:**
- Create: `tests/e2e_tests.rs`

These tests verify the full flow: start server → create agent (mock) → exchange messages → verify task workflow. We mock the agent CLI to avoid needing actual Claude/Codex installed.

- [ ] **Step 1: Create test helper to start the server in-process**

```rust
use chorus::store::Store;
use chorus::server::build_router;
use chorus::models::*;
use std::sync::Arc;

/// Spin up an axum server on a random port, return the base URL and store handle.
async fn start_test_server() -> (String, Arc<Store>) {
    let store = Arc::new(Store::open(":memory:").unwrap());
    store.add_human("testuser").unwrap();
    store.create_channel("general", Some("General"), ChannelType::Channel).unwrap();
    store.join_channel("general", "testuser", SenderType::Human).unwrap();

    let router = build_router(store.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{addr}");

    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

    // Give server a moment to start
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    (url, store)
}
```

- [ ] **Step 2: Write e2e test: human sends message, agent receives via HTTP**

```rust
#[tokio::test]
async fn test_human_to_agent_message_flow() {
    let (url, store) = start_test_server().await;
    let client = reqwest::Client::new();

    // Create an agent and join #general
    store.create_agent_record("bot1", "Bot 1", None, "claude", "sonnet").unwrap();
    store.join_channel("general", "bot1", SenderType::Agent).unwrap();

    // Human sends message
    store.send_message("general", None, "testuser", SenderType::Human, "hello bot", &[]).unwrap();

    // Agent receives via HTTP (simulating what chat-bridge does)
    let resp: serde_json::Value = client
        .get(format!("{url}/internal/agent/bot1/receive?block=false"))
        .send().await.unwrap()
        .json().await.unwrap();

    let messages = resp["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["content"].as_str().unwrap(), "hello bot");
    assert_eq!(messages[0]["sender_name"].as_str().unwrap(), "testuser");
}
```

- [ ] **Step 3: Write e2e test: agent replies, appears in history**

```rust
#[tokio::test]
async fn test_agent_reply_in_history() {
    let (url, store) = start_test_server().await;
    let client = reqwest::Client::new();

    store.create_agent_record("bot1", "Bot 1", None, "claude", "sonnet").unwrap();
    store.join_channel("general", "bot1", SenderType::Agent).unwrap();

    // Agent sends a message via HTTP (simulating chat-bridge send_message tool)
    let resp = client
        .post(format!("{url}/internal/agent/bot1/send"))
        .json(&serde_json::json!({ "target": "#general", "content": "hi humans!" }))
        .send().await.unwrap();
    assert!(resp.status().is_success());

    // Check history
    let resp: serde_json::Value = client
        .get(format!("{url}/internal/agent/bot1/history?channel=%23general&limit=10"))
        .send().await.unwrap()
        .json().await.unwrap();

    let messages = resp["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["senderName"].as_str().unwrap(), "bot1");
}
```

- [ ] **Step 4: Write e2e test: blocking receive with timeout**

```rust
#[tokio::test]
async fn test_blocking_receive_wakes_on_message() {
    let (url, store) = start_test_server().await;
    let client = reqwest::Client::new();

    store.create_agent_record("bot1", "Bot 1", None, "claude", "sonnet").unwrap();
    store.join_channel("general", "bot1", SenderType::Agent).unwrap();

    let url2 = url.clone();
    let client2 = client.clone();

    // Start blocking receive in background
    let recv_handle = tokio::spawn(async move {
        let resp: serde_json::Value = client2
            .get(format!("{url2}/internal/agent/bot1/receive?block=true&timeout=5000"))
            .send().await.unwrap()
            .json().await.unwrap();
        resp
    });

    // Wait a bit, then send a message
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    store.send_message("general", None, "testuser", SenderType::Human, "wake up!", &[]).unwrap();

    // Receive should return quickly with the message
    let resp = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        recv_handle
    ).await.unwrap().unwrap();

    let messages = resp["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["content"].as_str().unwrap(), "wake up!");
}
```

- [ ] **Step 5: Write e2e test: full task board workflow**

```rust
#[tokio::test]
async fn test_task_board_e2e() {
    let (url, store) = start_test_server().await;
    let client = reqwest::Client::new();

    store.create_agent_record("bot1", "Bot 1", None, "claude", "sonnet").unwrap();
    store.join_channel("general", "bot1", SenderType::Agent).unwrap();

    // Create tasks
    let resp: serde_json::Value = client
        .post(format!("{url}/internal/agent/bot1/tasks"))
        .json(&serde_json::json!({ "channel": "#general", "tasks": [{"title": "Task A"}, {"title": "Task B"}] }))
        .send().await.unwrap()
        .json().await.unwrap();
    assert_eq!(resp["tasks"].as_array().unwrap().len(), 2);

    // Claim task 1
    let resp: serde_json::Value = client
        .post(format!("{url}/internal/agent/bot1/tasks/claim"))
        .json(&serde_json::json!({ "channel": "#general", "task_numbers": [1] }))
        .send().await.unwrap()
        .json().await.unwrap();
    assert!(resp["results"][0]["success"].as_bool().unwrap());

    // Update status to in_review
    let resp = client
        .post(format!("{url}/internal/agent/bot1/tasks/update-status"))
        .json(&serde_json::json!({ "channel": "#general", "task_number": 1, "status": "in_review" }))
        .send().await.unwrap();
    assert!(resp.status().is_success());

    // List tasks — task 1 should be in_review
    let resp: serde_json::Value = client
        .get(format!("{url}/internal/agent/bot1/tasks?channel=%23general"))
        .send().await.unwrap()
        .json().await.unwrap();
    let tasks = resp["tasks"].as_array().unwrap();
    assert_eq!(tasks[0]["status"].as_str().unwrap(), "in_review");
}
```

- [ ] **Step 6: Write e2e test: DM and thread flow**

```rust
#[tokio::test]
async fn test_dm_and_thread_flow() {
    let (url, store) = start_test_server().await;
    let client = reqwest::Client::new();

    store.create_agent_record("bot1", "Bot 1", None, "claude", "sonnet").unwrap();

    // Agent sends DM to testuser — should auto-create DM channel
    let resp: serde_json::Value = client
        .post(format!("{url}/internal/agent/bot1/send"))
        .json(&serde_json::json!({ "target": "dm:@testuser", "content": "hey!" }))
        .send().await.unwrap()
        .json().await.unwrap();
    let msg_id = resp["messageId"].as_str().unwrap();
    let short_id = &msg_id[..8];

    // Reply in thread on that message
    let thread_target = format!("dm:@testuser:{short_id}");
    let resp = client
        .post(format!("{url}/internal/agent/bot1/send"))
        .json(&serde_json::json!({ "target": thread_target, "content": "thread reply" }))
        .send().await.unwrap();
    assert!(resp.status().is_success());

    // Read thread history
    let resp: serde_json::Value = client
        .get(format!("{url}/internal/agent/bot1/history?channel={}", urlencoding::encode(&thread_target)))
        .send().await.unwrap()
        .json().await.unwrap();
    let messages = resp["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1); // only the thread reply
    assert_eq!(messages[0]["content"].as_str().unwrap(), "thread reply");
}
```

- [ ] **Step 7: Write e2e test: multi-agent collaboration**

```rust
#[tokio::test]
async fn test_multi_agent_channel_communication() {
    let (url, store) = start_test_server().await;
    let client = reqwest::Client::new();

    store.create_agent_record("claude_bot", "Claude", None, "claude", "sonnet").unwrap();
    store.create_agent_record("codex_bot", "Codex", None, "codex", "o3").unwrap();
    store.join_channel("general", "claude_bot", SenderType::Agent).unwrap();
    store.join_channel("general", "codex_bot", SenderType::Agent).unwrap();

    // Claude sends a message
    client.post(format!("{url}/internal/agent/claude_bot/send"))
        .json(&serde_json::json!({ "target": "#general", "content": "I'll handle the architecture" }))
        .send().await.unwrap();

    // Codex receives it
    let resp: serde_json::Value = client
        .get(format!("{url}/internal/agent/codex_bot/receive?block=false"))
        .send().await.unwrap()
        .json().await.unwrap();
    let messages = resp["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["sender_name"].as_str().unwrap(), "claude_bot");

    // Human also sees it in history
    let resp: serde_json::Value = client
        .get(format!("{url}/internal/agent/testuser/history?channel=%23general&limit=10"))
        .send().await.unwrap()
        .json().await.unwrap();
    assert_eq!(resp["messages"].as_array().unwrap().len(), 1);
}
```

- [ ] **Step 8: Run all e2e tests**

Run: `cargo test --test e2e_tests`
Expected: all 6 e2e tests pass.

- [ ] **Step 9: Commit**

```bash
git add tests/e2e_tests.rs
git commit -m "test: add end-to-end tests for full message and task flows"
```

---

## Task 12: Final Integration + Smoke Test

- [ ] **Step 1: Run all tests together**

Run: `cargo test`
Expected: all unit, integration, and e2e tests pass.

- [ ] **Step 2: Build release binary**

Run: `cargo build --release`
Expected: produces `target/release/chorus` binary.

- [ ] **Step 3: Manual smoke test**

```bash
# Terminal 1: start server
./target/release/chorus serve --port 3001

# Terminal 2: create channel and agent
./target/release/chorus channel engineering --description "Engineering channel"
./target/release/chorus agent create alice --runtime claude --model sonnet --description "Senior engineer"

# Terminal 3: send a message
./target/release/chorus send '#engineering' 'Hey alice, what do you think about the new design?'

# Check status
./target/release/chorus status
```

Expected: server starts, agent process spawns, messages flow.

- [ ] **Step 4: Final commit**

```bash
git add -A
git commit -m "feat: complete Rust local Chorus implementation"
```

---

## Dependencies Summary

Add to `Cargo.toml` if not already present during implementation:

```toml
[dependencies]
tokio = { version = "1", features = ["full"] }
axum = { version = "0.8", features = ["json", "multipart"] }
rusqlite = { version = "0.34", features = ["bundled"] }
rmcp = { version = "0.16", features = ["server", "transport-io"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
clap = { version = "4", features = ["derive"] }
uuid = { version = "1", features = ["v4"] }
chrono = { version = "0.4", features = ["serde"] }
anyhow = "1"
reqwest = { version = "0.12", features = ["json", "multipart"] }
whoami = "1"
urlencoding = "2"

[dev-dependencies]
tempfile = "3"
```

## Notes for Implementer

1. **The bridge reuses the same binary**: `chorus bridge --agent-id X --server-url Y`. The driver spawns it as the MCP server command. No separate binary needed.

2. **Thread implementation**: Threads are stored as regular messages with `thread_parent_id` set. **CRITICAL**: When listing messages for a channel (`get_messages_for_agent`, `get_history` with no thread), always use `WHERE thread_parent_id IS NULL` to exclude thread replies. When fetching thread history, use `WHERE thread_parent_id = ?`. Thread target format: `#channel:shortid` where `shortid` is the first 8 chars of the parent message UUID.

3. **DM channel naming**: Use `dm-{sorted_names_joined_by_-}` as the channel name internally. The target format `dm:@name` always resolves to the same DM channel between the sender and `name`.

4. **Agent auto-join**: When creating an agent via `create_agent_record`, automatically join it to all existing public channels (those with `channel_type = 'channel'`). Also join new agents to the human user's DMs if appropriate for the use case.

5. **Message ordering**: `seq` is per-channel, monotonically increasing. Use `SELECT COALESCE(MAX(seq), 0) + 1` within a transaction.

6. **rmcp compatibility**: The `rmcp` crate (v0.16) is under active development and its macro API may differ from what's shown in Task 9. **If `rmcp` macros don't compile**, fall back to a hand-rolled MCP JSON-RPC server over stdio. The protocol is simple: read newline-delimited JSON-RPC 2.0 requests from stdin, write responses to stdout. Key methods to handle: `initialize`, `tools/list`, `tools/call`. Each tool call returns `{"jsonrpc":"2.0","result":{"content":[{"type":"text","text":"..."}]},"id":N}`. This fallback adds ~100 lines of code to `bridge.rs` — scope it as a substep if needed.

7. **`view_file` tool implementation**: The bridge's `view_file` tool downloads an attachment from `GET /api/attachments/{id}`, saves it to `~/.chorus/attachments/{id}{ext}` (creating the directory if needed), and returns the local file path so the agent can use its Read tool to view images. Cache check: if the file already exists locally, skip the download.

8. **Human sender detection**: The `handle_send` endpoint detects sender type by calling `store.lookup_sender_type(&agent_id)` which checks agents table first, then humans table. This allows both CLI human users and agent bridges to use the same endpoint correctly.
