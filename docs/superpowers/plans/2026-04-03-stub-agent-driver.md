# Stub Agent Driver Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a lightweight stub agent binary and driver that echoes messages back through the MCP bridge, enabling ~30 QA cases to run without real LLM backends.

**Architecture:** A new `crates/stub-agent/` Rust binary acts as an MCP client that spawns the bridge as a child process, calls `wait_for_message` and `send_message` in a loop, and prints JSON status lines to stdout. A `StubDriver` in the main crate implements the `Driver` trait to spawn the stub binary and parse its output.

**Tech Stack:** Rust, rmcp (client feature), serde_json, tokio, Playwright (test harness updates)

**Spec:** [`docs/superpowers/specs/2026-04-03-stub-agent-driver-design.md`](../specs/2026-04-03-stub-agent-driver-design.md)

---

## File Map

| Action | Path | Responsibility |
|--------|------|----------------|
| Create | `crates/stub-agent/Cargo.toml` | Crate manifest with rmcp client + transport-async-rw deps |
| Create | `crates/stub-agent/src/main.rs` | MCP client binary: spawn bridge, loop wait/send, emit JSON |
| Modify | `Cargo.toml` | Convert to workspace with members `.` and `crates/stub-agent` |
| Modify | `src/store/agents.rs:84-109` | Add `AgentRuntime::Stub` variant, `parse()`, `as_str()` |
| Create | `src/agent/drivers/stub.rs` | `StubDriver` implementing `Driver` trait |
| Modify | `src/agent/drivers/mod.rs:1,84-91` | Add `pub mod stub;`, include `StubDriver` in `all_runtime_drivers()` |
| Modify | `src/agent/manager.rs:38-48,88-97` | Add `Stub` arm to `get_driver()` and `resumable_session_id` match |
| Modify | `src/agent/runtime_status.rs:38-42` | Filter `stub` from `list_statuses()` response |
| Modify | `qa/cases/playwright/helpers/api.ts` | Add `ensureStubTrio()` helper |
| Modify | `qa/QA_PRESETS.md` | Add `stub-trio` preset |

---

### Task 1: Convert to Cargo Workspace

**Files:**
- Modify: `Cargo.toml`
- Create: `crates/stub-agent/Cargo.toml`
- Create: `crates/stub-agent/src/main.rs`

- [ ] **Step 1: Convert root Cargo.toml to a workspace**

Add workspace section at the top of `Cargo.toml`. The existing `[package]` and all other sections stay unchanged.

```toml
[workspace]
members = [".", "crates/stub-agent"]

[package]
name = "chorus"
# ... rest unchanged
```

- [ ] **Step 2: Create stub-agent crate scaffold**

```bash
mkdir -p crates/stub-agent/src
```

`crates/stub-agent/Cargo.toml`:

```toml
[package]
name = "chorus-stub-agent"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "chorus-stub-agent"
path = "src/main.rs"

[dependencies]
rmcp = { version = "0.16", features = ["client", "transport-async-rw"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["full"] }
anyhow = "1"
regex = "1"
```

`crates/stub-agent/src/main.rs` (minimal placeholder):

```rust
fn main() {
    println!("stub-agent placeholder");
}
```

- [ ] **Step 3: Verify workspace builds**

Run: `cargo build`
Expected: Both `chorus` and `chorus-stub-agent` binaries compile. `target/debug/chorus-stub-agent` exists.

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml crates/
git commit -m "build: convert to Cargo workspace, add stub-agent crate scaffold"
```

---

### Task 2: Add `AgentRuntime::Stub` Enum Variant

**Files:**
- Modify: `src/store/agents.rs:82-109`

- [ ] **Step 1: Add Stub variant to AgentRuntime**

In `src/store/agents.rs`, add `Stub` to the enum and both match blocks:

```rust
pub enum AgentRuntime {
    Claude,
    Codex,
    Kimi,
    Opencode,
    Stub,
}

impl AgentRuntime {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Kimi => "kimi",
            Self::Opencode => "opencode",
            Self::Stub => "stub",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            "kimi" => Some(Self::Kimi),
            "opencode" => Some(Self::Opencode),
            "stub" => Some(Self::Stub),
            _ => None,
        }
    }
}
```

- [ ] **Step 2: Build to find exhaustive match errors**

Run: `cargo build 2>&1 | head -40`
Expected: Compiler errors in `manager.rs` for non-exhaustive match on `AgentRuntime`. This confirms the wiring points.

- [ ] **Step 3: Fix exhaustive match in manager.rs:88-97**

In `src/agent/manager.rs`, add `Stub` arm to the `resumable_session_id` match:

```rust
let resumable_session_id = match driver.runtime() {
    AgentRuntime::Codex | AgentRuntime::Opencode => agent.session_id.clone(),
    AgentRuntime::Kimi => Some(
        agent
            .session_id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
    ),
    AgentRuntime::Claude | AgentRuntime::Stub => None,
};
```

- [ ] **Step 4: Verify build passes**

Run: `cargo build`
Expected: Compiles with no errors. (The `get_driver()` and `all_runtime_drivers()` changes come in Task 3.)

- [ ] **Step 5: Commit**

```bash
git add src/store/agents.rs src/agent/manager.rs
git commit -m "feat(agent): add AgentRuntime::Stub enum variant"
```

---

### Task 3: Implement StubDriver

**Files:**
- Create: `src/agent/drivers/stub.rs`
- Modify: `src/agent/drivers/mod.rs:1,84-91`
- Modify: `src/agent/manager.rs:38-48`

- [ ] **Step 1: Create stub.rs with StubDriver**

Create `src/agent/drivers/stub.rs`:

```rust
use std::process::{Child, Command, Stdio};

use super::{Driver, ParsedEvent, SpawnContext};
use crate::agent::config::AgentConfig;
use crate::agent::drivers::prompt::{build_base_system_prompt, PromptOptions};
use crate::agent::runtime_status::{RuntimeAuthStatus, RuntimeStatus};
use crate::store::agents::AgentRuntime;

pub struct StubDriver;

impl Driver for StubDriver {
    fn runtime(&self) -> AgentRuntime {
        AgentRuntime::Stub
    }

    fn supports_stdin_notification(&self) -> bool {
        true
    }

    fn mcp_tool_prefix(&self) -> &str {
        ""
    }

    fn spawn(&self, ctx: &SpawnContext) -> anyhow::Result<Child> {
        let mcp_config = serde_json::json!({
            "mcpServers": {
                "chat": {
                    "command": ctx.bridge_binary,
                    "args": ["bridge", "--agent-id", &ctx.agent_id, "--server-url", &ctx.server_url]
                }
            }
        });
        let mcp_config_path =
            std::path::Path::new(&ctx.working_directory).join(".chorus-mcp.json");
        std::fs::write(&mcp_config_path, serde_json::to_string(&mcp_config)?)?;

        let stub_binary = std::env::current_exe()?
            .parent()
            .ok_or_else(|| anyhow::anyhow!("cannot find binary directory"))?
            .join("chorus-stub-agent");

        let delay_ms = std::env::var("STUB_DELAY_MS").unwrap_or_else(|_| "200".to_string());

        let mut env_vars: std::collections::HashMap<String, String> = std::env::vars().collect();
        env_vars.insert("STUB_DELAY_MS".to_string(), delay_ms);
        for extra in &ctx.config.env_vars {
            env_vars.insert(extra.key.clone(), extra.value.clone());
        }

        let child = Command::new(&stub_binary)
            .args([
                "--mcp-config",
                &mcp_config_path.to_string_lossy(),
                "--prompt",
                &ctx.prompt,
            ])
            .current_dir(&ctx.working_directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .envs(&env_vars)
            .spawn()?;

        Ok(child)
    }

    fn parse_line(&self, line: &str) -> Vec<ParsedEvent> {
        let event: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => return vec![],
        };

        let mut events = Vec::new();

        match event.get("type").and_then(|v| v.as_str()) {
            Some("session_init") => {
                if let Some(sid) = event.get("session_id").and_then(|v| v.as_str()) {
                    events.push(ParsedEvent::SessionInit {
                        session_id: sid.to_string(),
                    });
                }
            }
            Some("text") => {
                if let Some(text) = event.get("text").and_then(|v| v.as_str()) {
                    events.push(ParsedEvent::Text {
                        text: text.to_string(),
                    });
                }
            }
            Some("tool_call") => {
                let name = event
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                let input = event
                    .get("input")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                events.push(ParsedEvent::ToolCall { name, input });
            }
            Some("turn_end") => {
                events.push(ParsedEvent::TurnEnd { session_id: None });
            }
            Some("error") => {
                let message = event
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown error")
                    .to_string();
                events.push(ParsedEvent::Error { message });
            }
            _ => {}
        }

        events
    }

    fn encode_stdin_message(&self, text: &str, _session_id: &str) -> Option<String> {
        let msg = serde_json::json!({
            "type": "notification",
            "content": text,
        });
        Some(serde_json::to_string(&msg).unwrap_or_default())
    }

    fn build_system_prompt(&self, config: &AgentConfig, _agent_id: &str) -> String {
        build_base_system_prompt(
            config,
            &PromptOptions {
                tool_prefix: String::new(),
                extra_critical_rules: vec![],
                post_startup_notes: vec![],
                include_stdin_notification_section: true,
                teams: config.teams.clone(),
            },
        )
    }

    fn tool_display_name(&self, name: &str) -> String {
        match name {
            "send_message" => "Sending message\u{2026}".to_string(),
            "check_messages" => "Checking messages\u{2026}".to_string(),
            "wait_for_message" => "Waiting for messages\u{2026}".to_string(),
            "receive_message" => "Receiving messages\u{2026}".to_string(),
            other => format!("Using {other}\u{2026}"),
        }
    }

    fn summarize_tool_input(&self, name: &str, input: &serde_json::Value) -> String {
        let str_field = |field: &str| -> String {
            input
                .get(field)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };
        match name {
            "send_message" => {
                let target = str_field("target");
                let content = str_field("content");
                let preview: String = content.chars().take(80).collect();
                if target.is_empty() {
                    preview
                } else {
                    format!("{target}: {preview}")
                }
            }
            _ => String::new(),
        }
    }

    fn detect_runtime_status(&self) -> anyhow::Result<RuntimeStatus> {
        let binary_exists = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("chorus-stub-agent")))
            .map(|p| p.exists())
            .unwrap_or(false);

        Ok(RuntimeStatus {
            runtime: self.id().to_string(),
            installed: binary_exists,
            auth_status: Some(RuntimeAuthStatus::Authed),
        })
    }

    fn list_models(&self) -> anyhow::Result<Vec<String>> {
        Ok(vec!["echo".to_string()])
    }
}
```

- [ ] **Step 2: Register StubDriver in mod.rs**

In `src/agent/drivers/mod.rs`, add the module declaration at line 1 area:

```rust
pub mod claude;
pub mod codex;
pub mod kimi;
pub mod opencode;
pub mod prompt;
pub mod stub;
```

Add `StubDriver` to `all_runtime_drivers()`:

```rust
pub fn all_runtime_drivers() -> Vec<Arc<dyn Driver>> {
    vec![
        Arc::new(claude::ClaudeDriver),
        Arc::new(codex::CodexDriver),
        Arc::new(kimi::KimiDriver),
        Arc::new(opencode::OpencodeDriver),
        Arc::new(stub::StubDriver),
    ]
}
```

- [ ] **Step 3: Wire get_driver() in manager.rs**

In `src/agent/manager.rs`, add the Stub arm to `get_driver()`:

```rust
fn get_driver(runtime: &str) -> anyhow::Result<Arc<dyn Driver>> {
    match AgentRuntime::parse(runtime) {
        Some(AgentRuntime::Claude) => Ok(Arc::new(crate::agent::drivers::claude::ClaudeDriver)),
        Some(AgentRuntime::Codex) => Ok(Arc::new(crate::agent::drivers::codex::CodexDriver)),
        Some(AgentRuntime::Kimi) => Ok(Arc::new(crate::agent::drivers::kimi::KimiDriver)),
        Some(AgentRuntime::Opencode) => {
            Ok(Arc::new(crate::agent::drivers::opencode::OpencodeDriver))
        }
        Some(AgentRuntime::Stub) => Ok(Arc::new(crate::agent::drivers::stub::StubDriver)),
        None => anyhow::bail!("Unknown runtime: {runtime}"),
    }
}
```

- [ ] **Step 4: Verify build passes**

Run: `cargo build`
Expected: Compiles. `StubDriver` is registered but the stub binary is still a placeholder.

- [ ] **Step 5: Commit**

```bash
git add src/agent/drivers/stub.rs src/agent/drivers/mod.rs src/agent/manager.rs
git commit -m "feat(agent): add StubDriver implementation"
```

---

### Task 4: Filter Stub From Runtime Status API

**Files:**
- Modify: `src/agent/runtime_status.rs:38-42`

- [ ] **Step 1: Filter stub from list_statuses()**

In `src/agent/runtime_status.rs`, update `SystemRuntimeStatusProvider::list_statuses()`:

```rust
impl RuntimeStatusProvider for SystemRuntimeStatusProvider {
    fn list_statuses(&self) -> anyhow::Result<Vec<RuntimeStatus>> {
        all_runtime_drivers()
            .into_iter()
            .filter(|driver| driver.id() != "stub")
            .map(|driver| driver.detect_runtime_status())
            .collect()
    }

    // list_models unchanged — it still supports "stub" for API-only agent creation
```

- [ ] **Step 2: Verify build passes**

Run: `cargo build`
Expected: Compiles.

- [ ] **Step 3: Commit**

```bash
git add src/agent/runtime_status.rs
git commit -m "feat(agent): hide stub runtime from /runtimes API"
```

---

### Task 5: Implement Stub Agent Binary

**Files:**
- Modify: `crates/stub-agent/src/main.rs`

This is the core binary. It:
1. Reads `--mcp-config` to find the bridge command
2. Spawns the bridge as a child process
3. Connects as an MCP client via stdio pipes to the bridge
4. Processes the initial `--prompt` message
5. Loops: `wait_for_message` → extract token → `send_message` → repeat
6. Prints JSON status lines to its own stdout for the manager

- [ ] **Step 1: Write the full stub binary**

Replace `crates/stub-agent/src/main.rs` with:

```rust
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use rmcp::model::CallToolRequestParams;
use rmcp::{ClientHandler, RoleClient, ServiceExt};
use serde_json::Value;
use tokio::process::Command;

static SEQ: AtomicU64 = AtomicU64::new(1);

/// Minimal MCP client handler — we don't need to handle any server-initiated
/// requests, but `ClientHandler` requires an impl.
struct StubClientHandler;

impl ClientHandler for StubClientHandler {}

/// Extract an echo token from the message text.
///
/// Patterns matched (case-insensitive):
///   reply with "TOKEN"
///   token: TOKEN
///   echo "TOKEN"
///   say "TOKEN"
///
/// Falls back to `stub-reply-{seq}`.
fn extract_token(text: &str) -> String {
    let re_patterns = [
        r#"(?i)reply\s+with\s+"([^"]+)""#,
        r#"(?i)reply\s+with\s+(\S+)"#,
        r#"(?i)token:\s*(\S+)"#,
        r#"(?i)echo\s+"([^"]+)""#,
        r#"(?i)say\s+"([^"]+)""#,
    ];

    for pattern in &re_patterns {
        if let Ok(re) = regex::Regex::new(pattern) {
            if let Some(caps) = re.captures(text) {
                if let Some(m) = caps.get(1) {
                    return m.as_str().to_string();
                }
            }
        }
    }

    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("stub-reply-{seq}")
}

/// Parse the target from a `receive_message` / `wait_for_message` response.
///
/// The bridge returns lines like:
///   [target=#channel msg=abc123 time=...] @sender: content
///
/// We extract the target value and the message content.
fn parse_bridge_response(response: &str) -> Option<(String, String)> {
    // Find the first [target=...] block
    let target_start = response.find("target=")?;
    let after_target = &response[target_start + 7..];
    let target_end = after_target.find(' ')?;
    let target = after_target[..target_end].to_string();

    // Find the message content after the ] @sender: prefix
    let bracket_end = response.find(']')?;
    let after_bracket = &response[bracket_end + 1..];
    // Skip " @sender: " — find the first ": " after @
    if let Some(colon_pos) = after_bracket.find(": ") {
        let content = after_bracket[colon_pos + 2..].trim().to_string();
        Some((target, content))
    } else {
        Some((target, String::new()))
    }
}

/// Emit a JSON status line to stdout for the manager's `parse_line()`.
fn emit(event_type: &str, fields: &[(&str, &str)]) {
    let mut obj = serde_json::Map::new();
    obj.insert(
        "type".to_string(),
        serde_json::Value::String(event_type.to_string()),
    );
    for (k, v) in fields {
        obj.insert(k.to_string(), serde_json::Value::String(v.to_string()));
    }
    if let Ok(line) = serde_json::to_string(&serde_json::Value::Object(obj)) {
        println!("{line}");
    }
}

async fn call_tool(
    peer: &rmcp::service::Peer<RoleClient>,
    name: &str,
    args: Value,
) -> Result<String> {
    let params = CallToolRequestParams {
        name: name.into(),
        arguments: Some(args.as_object().cloned().unwrap_or_default()),
        meta: None,
        task: None,
    };
    let result = peer
        .call_tool(params)
        .await
        .context(format!("call_tool({name}) failed"))?;

    // Extract text from content blocks.
    // The exact field access depends on rmcp version — adapt if the Content
    // type changes. The goal: concatenate all text content into one string.
    let text: String = result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.as_str()))
        .collect::<Vec<_>>()
        .join("\n");

    Ok(text)
}

async fn run(mcp_config_path: &str, initial_prompt: &str) -> Result<()> {
    // Read MCP config to get bridge command
    let config_text = std::fs::read_to_string(mcp_config_path)
        .context("failed to read MCP config")?;
    let config: Value = serde_json::from_str(&config_text)?;
    let chat_server = config
        .get("mcpServers")
        .and_then(|s| s.get("chat"))
        .context("missing mcpServers.chat in config")?;
    let command = chat_server
        .get("command")
        .and_then(|v| v.as_str())
        .context("missing command in chat server config")?;
    let args: Vec<String> = chat_server
        .get("args")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    // Spawn bridge process
    let mut bridge_process = Command::new(command)
        .args(&args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("failed to spawn bridge process")?;

    let bridge_stdout = bridge_process.stdout.take().context("no stdout")?;
    let bridge_stdin = bridge_process.stdin.take().context("no stdin")?;

    // Connect as MCP client using (AsyncRead, AsyncWrite) tuple transport
    let service = StubClientHandler
        .serve((bridge_stdout, bridge_stdin))
        .await
        .context("MCP client init failed")?;
    let peer = service.peer().clone();

    // Emit session init
    let session_id = uuid::Uuid::new_v4().to_string();
    emit("session_init", &[("session_id", &session_id)]);

    let delay_ms: u64 = std::env::var("STUB_DELAY_MS")
        .unwrap_or_else(|_| "200".to_string())
        .parse()
        .unwrap_or(200);

    // Process initial prompt if present
    if !initial_prompt.is_empty() {
        // Extract target from prompt — the system prompt contains target info
        // For initial startup, we just emit a text event
        emit("text", &[("text", "Stub agent started")]);
    }

    // Main loop: wait for message → extract token → send reply
    loop {
        emit(
            "tool_call",
            &[
                ("name", "wait_for_message"),
                ("input", "{}"),
            ],
        );

        let response = call_tool(
            &peer,
            "wait_for_message",
            serde_json::json!({}),
        )
        .await?;

        // "No new messages" responses mean we loop again
        if response.contains("No new messages") {
            continue;
        }

        // Parse the bridge response to get target and content
        if let Some((target, content)) = parse_bridge_response(&response) {
            let token = extract_token(&content);

            // Emit that we received a message
            emit(
                "tool_call",
                &[
                    ("name", "send_message"),
                    ("input", &serde_json::json!({"target": target, "content": token}).to_string()),
                ],
            );

            tokio::time::sleep(Duration::from_millis(delay_ms)).await;

            // Send the reply
            let _send_result = call_tool(
                &peer,
                "send_message",
                serde_json::json!({
                    "target": target,
                    "content": token,
                }),
            )
            .await?;

            emit("text", &[("text", &token)]);
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();

    let mut mcp_config_path = String::new();
    let mut prompt = String::new();

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--mcp-config" => {
                i += 1;
                mcp_config_path = args.get(i).cloned().unwrap_or_default();
            }
            "--prompt" => {
                i += 1;
                prompt = args.get(i).cloned().unwrap_or_default();
            }
            _ => {}
        }
        i += 1;
    }

    if mcp_config_path.is_empty() {
        anyhow::bail!("--mcp-config is required");
    }

    run(&mcp_config_path, &prompt).await
}
```

- [ ] **Step 2: Add uuid dependency to stub-agent Cargo.toml**

Update `crates/stub-agent/Cargo.toml` dependencies:

```toml
[dependencies]
rmcp = { version = "0.16", features = ["client", "transport-async-rw"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["full"] }
anyhow = "1"
regex = "1"
uuid = { version = "1", features = ["v4"] }
```

- [ ] **Step 3: Verify the workspace builds**

Run: `cargo build`
Expected: Both `chorus` and `chorus-stub-agent` binaries compile. `target/debug/chorus-stub-agent` exists.

- [ ] **Step 4: Commit**

```bash
git add crates/stub-agent/
git commit -m "feat(stub-agent): implement MCP client binary with echo logic"
```

---

### Task 6: Integration Smoke Test

**Files:**
- No new files — manual verification that a stub agent can start, receive a message, and reply.

- [ ] **Step 1: Build everything**

```bash
cargo build
```

- [ ] **Step 2: Start the server with a temp data dir**

```bash
./target/debug/chorus serve --port 3101 --data-dir /tmp/chorus-stub-test
```

- [ ] **Step 3: Create a stub agent via API**

In a second terminal:

```bash
curl -s -X POST http://localhost:3101/api/agents \
  -H 'Content-Type: application/json' \
  -d '{"name":"stub-a","display_name":"Stub A","runtime":"stub","model":"echo","envVars":[]}' | jq .
```

Expected: Agent created successfully.

- [ ] **Step 4: Verify stub agent starts and goes active**

```bash
# Poll until active
for i in $(seq 1 30); do
  status=$(curl -s http://localhost:3101/api/agents | jq -r '.[] | select(.name=="stub-a") | .status')
  echo "Attempt $i: status=$status"
  [ "$status" = "active" ] && break
  sleep 2
done
```

Expected: Agent reaches `active` status.

- [ ] **Step 5: Send a message and verify reply**

```bash
# Send a message with an echo token
curl -s -X POST http://localhost:3101/internal/agent/$(whoami)/send \
  -H 'Content-Type: application/json' \
  -d '{"target":"dm:@stub-a","content":"reply with \"hello-stub\""}'

# Wait and check history
sleep 3
curl -s "http://localhost:3101/internal/agent/$(whoami)/history?channel=dm:@stub-a&limit=10" | jq '.messages[] | {senderName, content}'
```

Expected: Two messages — the human send and a stub reply containing `hello-stub`.

- [ ] **Step 6: Clean up**

Stop the server. `rm -rf /tmp/chorus-stub-test`.

- [ ] **Step 7: Commit (no code changes — just verification)**

No commit needed unless fixes were required. If fixes were made, commit them:

```bash
git add -A
git commit -m "fix(stub-agent): fixes from integration smoke test"
```

---

### Task 7: QA Harness Updates

**Files:**
- Modify: `qa/cases/playwright/helpers/api.ts`
- Modify: `qa/QA_PRESETS.md`

- [ ] **Step 1: Add ensureStubTrio helper to api.ts**

Add after the existing `ensureMixedRuntimeTrio` function in `qa/cases/playwright/helpers/api.ts`:

```typescript
/** Create stub-a, stub-b, stub-c with runtime=stub for fast QA runs. */
export async function ensureStubTrio(request: APIRequestContext): Promise<void> {
  const agents = await listAgents(request)
  const names = new Set(agents.map((a) => a.name))
  if (!names.has('stub-a')) {
    await createAgentApi(request, { name: 'stub-a', runtime: 'stub', model: 'echo' })
  }
  if (!names.has('stub-b')) {
    await createAgentApi(request, { name: 'stub-b', runtime: 'stub', model: 'echo' })
  }
  if (!names.has('stub-c')) {
    await createAgentApi(request, { name: 'stub-c', runtime: 'stub', model: 'echo' })
  }
}
```

- [ ] **Step 2: Add agentNames helper for mode-aware name selection**

Add to `qa/cases/playwright/helpers/api.ts`:

```typescript
/** Return agent names based on CHORUS_E2E_LLM mode. */
export function agentNames(): { a: string; b: string; c: string } {
  const mode = process.env.CHORUS_E2E_LLM ?? '1'
  if (mode === 'stub') {
    return { a: 'stub-a', b: 'stub-b', c: 'stub-c' }
  }
  return { a: 'bot-a', b: 'bot-b', c: 'bot-c' }
}
```

- [ ] **Step 3: Add stub-trio preset to QA_PRESETS.md**

Append to `qa/QA_PRESETS.md`:

```markdown

### `stub-trio`

Use for:
- fast QA runs that test the full UI + message pipeline without LLM latency
- CI smoke tests
- core regression runs where real LLM reasoning is not required

Agents:
- `stub-a` — runtime `stub`, model `echo`
- `stub-b` — runtime `stub`, model `echo`
- `stub-c` — runtime `stub`, model `echo`

Notes:
- Select with `CHORUS_E2E_LLM=stub`.
- Use `agentNames()` from the test helpers to get mode-aware agent names.
- Cases requiring real LLM reasoning (TMT-003, TMT-004, TMT-006, TMT-008, TMT-009) are automatically skipped in stub mode.
- The stub runtime is not visible in the create-agent modal — agents are created via API only.
```

- [ ] **Step 4: Commit**

```bash
git add qa/cases/playwright/helpers/api.ts qa/QA_PRESETS.md
git commit -m "feat(qa): add stub-trio preset and ensureStubTrio helper"
```

---

### Task 8: Wire One Spec To Use Stub Mode (MSG-002)

**Files:**
- Modify: `qa/cases/playwright/MSG-002.spec.ts`

This task wires a single representative spec to demonstrate the stub integration pattern. MSG-002 is ideal because it requires a specific echo token (`dm-check-1`) — exercising the token extraction logic.

- [ ] **Step 1: Read the current MSG-002 spec**

Read `qa/cases/playwright/MSG-002.spec.ts` to understand the current structure before modifying it.

- [ ] **Step 2: Update MSG-002 to support stub mode**

The spec currently skips entirely when `CHORUS_E2E_LLM=0`. Update it to:
- Run with stub agents when `CHORUS_E2E_LLM=stub`
- Keep the existing skip for `CHORUS_E2E_LLM=0`
- Use `agentNames()` for mode-aware agent name selection

At the top of the spec, replace the existing mode detection with:

```typescript
import { agentNames, ensureStubTrio, ensureMixedRuntimeTrio } from './helpers/api'

const mode = process.env.CHORUS_E2E_LLM ?? '1'
const skipLLM = mode === '0'
const useStub = mode === 'stub'
const agents = agentNames()
```

In the `beforeAll` or setup section, add stub trio creation:

```typescript
if (useStub) {
  await ensureStubTrio(request)
} else {
  await ensureMixedRuntimeTrio(request)
}
```

Replace hardcoded `bot-a` references with `agents.a`.

Keep `test.skip(skipLLM, 'CHORUS_E2E_LLM=0')` — this still skips when mode is `0`.

- [ ] **Step 3: Run MSG-002 in stub mode**

```bash
cd qa/cases/playwright
CHORUS_E2E_LLM=stub npx playwright test MSG-002.spec.ts --reporter=list
```

Expected: Test runs using stub agents and passes (stub echoes back the requested token).

- [ ] **Step 4: Verify MSG-002 still works in skip mode**

```bash
CHORUS_E2E_LLM=0 npx playwright test MSG-002.spec.ts --reporter=list
```

Expected: Test is skipped as before.

- [ ] **Step 5: Commit**

```bash
git add qa/cases/playwright/MSG-002.spec.ts
git commit -m "feat(qa): wire MSG-002 to support CHORUS_E2E_LLM=stub mode"
```

---

### Task 9: Stdin Notification Support

**Files:**
- Modify: `crates/stub-agent/src/main.rs`

The stub binary needs to handle stdin notifications from the manager (wake-up messages sent when the agent is in `wait_for_message`). Real drivers get new message content written to their stdin.

- [ ] **Step 1: Add stdin reading to the stub binary**

The main loop already calls `wait_for_message` which blocks on the bridge. The manager writes stdin notifications while the agent is in `wait_for_message`. However, since the bridge's `wait_for_message` handles the actual polling, the stdin notification is a nudge — the bridge side already picks up new messages.

For the stub, stdin notifications arrive as JSON lines like:
```json
{"type":"notification","content":"...message text..."}
```

Since `wait_for_message` on the bridge side already returns new messages, the stub doesn't need to interrupt the bridge call — the bridge already handles the timing. The stdin notification is a signal to Codex-like drivers that don't have their own polling.

Since `StubDriver::supports_stdin_notification()` returns `true`, the manager will write notification lines to stdin. The stub should read and discard them to avoid stdin buffer filling up. Add a background stdin drain task:

In `crates/stub-agent/src/main.rs`, add before the main loop in `run()`:

```rust
// Drain stdin notifications in background to prevent buffer fill-up.
// The bridge's wait_for_message already handles message delivery.
tokio::spawn(async move {
    use tokio::io::AsyncBufReadExt;
    let stdin = tokio::io::stdin();
    let reader = tokio::io::BufReader::new(stdin);
    let mut lines = reader.lines();
    while let Ok(Some(_line)) = lines.next_line().await {
        // Notifications consumed — bridge handles delivery
    }
});
```

- [ ] **Step 2: Verify build**

Run: `cargo build`
Expected: Compiles.

- [ ] **Step 3: Commit**

```bash
git add crates/stub-agent/src/main.rs
git commit -m "feat(stub-agent): drain stdin notifications in background"
```

---

### Summary

| Task | What it does | Commit |
|------|-------------|--------|
| 1 | Cargo workspace + crate scaffold | `build: convert to Cargo workspace` |
| 2 | `AgentRuntime::Stub` enum + exhaustive matches | `feat(agent): add AgentRuntime::Stub` |
| 3 | `StubDriver` trait impl + registration | `feat(agent): add StubDriver` |
| 4 | Filter stub from `/runtimes` API | `feat(agent): hide stub from /runtimes` |
| 5 | Stub agent binary (MCP client + echo loop) | `feat(stub-agent): implement MCP client binary` |
| 6 | Integration smoke test (manual) | fix commit if needed |
| 7 | QA helpers + preset | `feat(qa): add stub-trio preset` |
| 8 | Wire MSG-002 as proof-of-concept | `feat(qa): wire MSG-002 for stub mode` |
| 9 | Stdin notification drain | `feat(stub-agent): drain stdin notifications` |
