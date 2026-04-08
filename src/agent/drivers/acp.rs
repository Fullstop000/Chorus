use std::io::Write as _;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde_json::json;

use super::{Driver, ParsedEvent, SpawnContext};
use crate::agent::config::AgentConfig;
use crate::agent::drivers::prompt::{build_base_system_prompt, PromptOptions};
use crate::agent::runtime_status::RuntimeStatus;
use crate::store::agents::AgentRuntime;

// ── AcpRuntime trait: per-runtime metadata ──

/// Runtime-specific configuration that varies per CLI (Claude, Codex, Kimi, OpenCode).
/// Everything else (ACP JSON-RPC parsing, session management, stdin encoding) is shared
/// by `AcpDriver`.
pub trait AcpRuntime: Send + Sync + 'static {
    /// The `AgentRuntime` enum variant for this runtime.
    fn runtime(&self) -> AgentRuntime;

    /// The CLI binary name (e.g., "claude", "codex", "kimi", "opencode").
    fn binary_name(&self) -> &str;

    /// Build the CLI args to launch the runtime in ACP mode.
    /// Does NOT include the initial prompt — that is sent via `session/prompt`.
    fn build_acp_args(&self, ctx: &SpawnContext) -> Vec<String>;

    /// Write the MCP bridge config file(s) needed by this runtime.
    /// Returns the path to the config file if one was written.
    fn write_mcp_config(&self, ctx: &SpawnContext) -> anyhow::Result<Option<PathBuf>>;

    /// Runtime-specific environment overrides.
    /// Returns `(key, Some(value))` for additions and `(key, None)` for removals.
    fn env_overrides(&self, ctx: &SpawnContext) -> Vec<(String, Option<String>)>;

    /// Any pre-spawn setup (e.g., Codex needs `git init`).
    fn pre_spawn_setup(&self, _ctx: &SpawnContext) -> anyhow::Result<()> {
        Ok(())
    }

    /// The MCP tool prefix used in the system prompt for this runtime.
    /// ACP agents discover tools via MCP, but we still need this for prompt text.
    fn tool_prefix(&self) -> &str;

    /// Detect whether the runtime CLI is installed and authenticated.
    fn detect_runtime_status(&self) -> anyhow::Result<RuntimeStatus>;

    /// Return available model IDs.
    fn list_models(&self) -> anyhow::Result<Vec<String>>;

    /// Build the params object for the `session/new` JSON-RPC call.
    /// Default uses `{"workspaceDir": ...}`. Override for runtimes with different schemas (e.g. kimi).
    fn session_new_params(&self, ctx: &SpawnContext) -> serde_json::Value {
        json!({ "workspaceDir": ctx.working_directory })
    }

    /// Whether `session/prompt` requires a `sessionId` field.
    /// When true, the startup prompt is deferred until `session/new` responds with a sessionId.
    fn requires_session_id_in_prompt(&self) -> bool {
        false
    }
}

// ── AcpDriver: shared ACP protocol handler ──

/// ACP handshake phase tracked via interior mutability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AcpPhase {
    /// Waiting for `initialize` response (id: 1).
    AwaitingInitResponse,
    /// Waiting for `session/new` or `session/load` response (id: 2).
    AwaitingSessionResponse,
    /// Handshake complete, parsing `session/update` notifications.
    Active,
}

/// Internal state for the ACP JSON-RPC protocol.
#[derive(Debug)]
struct AcpState {
    phase: AcpPhase,
    /// Startup prompt deferred until `session/new` returns a `sessionId`.
    /// Used by runtimes (e.g. kimi) that require `sessionId` in `session/prompt`.
    pending_startup_prompt: Option<String>,
}

/// A `Driver` implementation backed by the Agent Client Protocol.
/// `R` provides runtime-specific metadata (binary name, args, status detection).
/// All ACP parsing, encoding, and session management is shared.
pub struct AcpDriver<R: AcpRuntime> {
    pub(crate) runtime_impl: R,
    state: Mutex<AcpState>,
    next_request_id: AtomicU64,
}

impl<R: AcpRuntime> AcpDriver<R> {
    pub fn new(runtime_impl: R) -> Self {
        Self {
            runtime_impl,
            state: Mutex::new(AcpState {
                phase: AcpPhase::AwaitingInitResponse,
                pending_startup_prompt: None,
            }),
            next_request_id: AtomicU64::new(4), // 1-3 are used by handshake
        }
    }

    fn handle_rpc_response(&self, state: &mut AcpState, msg: &serde_json::Value) -> Vec<ParsedEvent> {
        // Check for error response
        if let Some(err) = msg.get("error") {
            let message = err
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown ACP error")
                .to_string();
            return vec![ParsedEvent::Error { message }];
        }

        let result = msg.get("result");

        match state.phase {
            AcpPhase::AwaitingInitResponse => {
                // initialize response — extract capabilities, advance phase
                state.phase = AcpPhase::AwaitingSessionResponse;
                vec![]
            }
            AcpPhase::AwaitingSessionResponse => {
                // session/new or session/load response — extract sessionId
                let session_id = result
                    .and_then(|r| r.get("sessionId"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                state.phase = AcpPhase::Active;
                let mut events = if session_id.is_empty() {
                    vec![]
                } else {
                    vec![ParsedEvent::SessionInit { session_id: session_id.clone() }]
                };
                // Flush deferred startup prompt that requires a sessionId.
                if let Some(prompt) = state.pending_startup_prompt.take() {
                    let prompt_req = json_rpc_request(3, "session/prompt", json!({
                        "sessionId": session_id,
                        "prompt": [{ "type": "text", "text": prompt }]
                    }));
                    events.push(ParsedEvent::WriteStdin { data: format!("{prompt_req}\n") });
                }
                events
            }
            AcpPhase::Active => {
                // session/prompt response — turn ended
                let session_id = result
                    .and_then(|r| r.get("sessionId"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                vec![ParsedEvent::TurnEnd { session_id }]
            }
        }
    }

    fn handle_rpc_notification(
        &self,
        method: &str,
        msg: &serde_json::Value,
    ) -> Vec<ParsedEvent> {
        if method != "session/update" {
            return vec![];
        }

        let params = match msg.get("params") {
            Some(p) => p,
            None => return vec![],
        };

        let update = params.get("update").unwrap_or(params);

        // `kind` can be in different fields depending on the runtime:
        // - Most runtimes: `update.kind` or `update.type`
        // - kimi: `update.sessionUpdate` (snake_case values like "agent_message_chunk")
        let kind = update
            .get("kind")
            .or_else(|| update.get("type"))
            .or_else(|| update.get("sessionUpdate"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // Extract text from a content value that may be either:
        // - A plain string (most runtimes: `chunk`/`text` field)
        // - A nested object (kimi: `{"text": "...", "type": "text"}`)
        let extract_text = |update: &serde_json::Value| -> String {
            update
                .get("chunk")
                .or_else(|| update.get("text"))
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .or_else(|| {
                    update.get("content").and_then(|c| {
                        c.get("text").and_then(|v| v.as_str()).map(str::to_string)
                    })
                })
                .unwrap_or_default()
        };

        match kind {
            "agentMessageChunk" | "agent_message_chunk" => {
                let text = extract_text(update);
                if text.is_empty() {
                    vec![]
                } else {
                    vec![ParsedEvent::Text { text }]
                }
            }
            "agentThoughtChunk" | "agent_thought_chunk" => {
                let text = extract_text(update);
                if text.is_empty() {
                    vec![]
                } else {
                    vec![ParsedEvent::Thinking { text }]
                }
            }
            "toolCall" | "tool_call" => {
                // kimi uses `title` for the tool name; other runtimes use `toolName`
                let name = update
                    .get("toolName")
                    .or_else(|| update.get("title"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown_tool")
                    .to_string();
                let input = update
                    .get("args")
                    .or_else(|| update.get("input"))
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                vec![ParsedEvent::ToolCall { name, input }]
            }
            "toolCallUpdate" | "tool_call_update" => {
                let content = update
                    .get("content")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if content.is_empty() {
                    // Also check for structured content array.
                    // kimi format: [{"content": {"text": "...", "type": "text"}, "type": "content"}]
                    // Other format: [{"type": "text", "text": "..."}]
                    if let Some(arr) = update.get("content").and_then(|v| v.as_array()) {
                        let text: String = arr
                            .iter()
                            .filter_map(|b| {
                                // kimi nested: b.content.text
                                b.get("content")
                                    .and_then(|c| c.get("text"))
                                    .and_then(|v| v.as_str())
                                    .map(str::to_string)
                                    // flat: b.text when b.type == "text"
                                    .or_else(|| {
                                        if b.get("type").and_then(|v| v.as_str()) == Some("text") {
                                            b.get("text").and_then(|v| v.as_str()).map(str::to_string)
                                        } else {
                                            None
                                        }
                                    })
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                        if text.is_empty() {
                            vec![]
                        } else {
                            vec![ParsedEvent::ToolResult { content: text }]
                        }
                    } else {
                        vec![]
                    }
                } else {
                    vec![ParsedEvent::ToolResult { content }]
                }
            }
            _ => vec![],
        }
    }

    /// Handle JSON-RPC requests sent from the agent runtime to Chorus.
    /// Currently only `session/request_permission` (used by kimi) is handled.
    fn handle_rpc_request(&self, method: &str, msg: &serde_json::Value) -> Vec<ParsedEvent> {
        if method != "session/request_permission" {
            return vec![];
        }

        let id = match msg.get("id") {
            Some(v) => v.clone(),
            None => return vec![],
        };

        // Prefer "allow_always" (approve_for_session) so approval persists for the session.
        let option_id = msg
            .get("params")
            .and_then(|p| p.get("options"))
            .and_then(|o| o.as_array())
            .and_then(|opts| {
                opts.iter()
                    .find(|o| {
                        o.get("kind").and_then(|k| k.as_str()) == Some("allow_always")
                    })
                    .or_else(|| opts.iter().find(|o| {
                        o.get("kind").and_then(|k| k.as_str()) == Some("allow_once")
                    }))
                    .and_then(|o| o.get("optionId"))
                    .and_then(|v| v.as_str())
            })
            .unwrap_or("approve");

        // ACP schema: result must be RequestPermissionResponse with a nested
        // AllowedOutcome discriminated by `outcome: "selected"`.
        let response = serde_json::to_string(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "outcome": {
                    "outcome": "selected",
                    "optionId": option_id
                }
            }
        }))
        .expect("permission response serialization should not fail");

        // Extract the bare tool name from toolCall.title (e.g. "send_message: {...}" → "send_message").
        let tool_name = msg
            .get("params")
            .and_then(|p| p.get("toolCall"))
            .and_then(|tc| tc.get("title"))
            .and_then(|t| t.as_str())
            .map(|t| t.split(':').next().unwrap_or(t).trim().to_string());

        vec![ParsedEvent::WriteStdin { data: format!("{response}\n") }, ParsedEvent::PermissionRequested { tool_name }]
    }
}

// ── JSON-RPC helpers ──

fn json_rpc_request(id: u64, method: &str, params: serde_json::Value) -> String {
    serde_json::to_string(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params
    }))
    .expect("json_rpc_request serialization should not fail")
}

/// Strip any known MCP server prefix to get the bare tool name.
pub(crate) fn strip_mcp_prefix(name: &str) -> &str {
    name.strip_prefix("mcp__chat__")
        .or_else(|| name.strip_prefix("mcp_chat_"))
        .or_else(|| name.strip_prefix("chat_"))
        .unwrap_or(name)
}

// ── Driver impl ──

impl<R: AcpRuntime> Driver for AcpDriver<R> {
    fn runtime(&self) -> AgentRuntime {
        self.runtime_impl.runtime()
    }

    fn supports_stdin_notification(&self) -> bool {
        // ACP does not support mid-turn message injection.
        // Agents pick up new messages via check_messages MCP tool.
        false
    }

    fn mcp_tool_prefix(&self) -> &str {
        self.runtime_impl.tool_prefix()
    }

    fn spawn(&self, ctx: &SpawnContext) -> anyhow::Result<Child> {
        self.runtime_impl.pre_spawn_setup(ctx)?;
        let _mcp_config_path = self.runtime_impl.write_mcp_config(ctx)?;

        let args = self.runtime_impl.build_acp_args(ctx);

        // Build environment: inherit current env, apply FORCE_COLOR=0, then overrides.
        let mut env_vars: std::collections::HashMap<String, String> = std::env::vars().collect();
        env_vars.insert("FORCE_COLOR".to_string(), "0".to_string());
        for extra in &ctx.config.env_vars {
            env_vars.insert(extra.key.clone(), extra.value.clone());
        }
        for (key, value) in self.runtime_impl.env_overrides(ctx) {
            match value {
                Some(v) => { env_vars.insert(key, v); }
                None => { env_vars.remove(&key); }
            }
        }

        let mut child = Command::new(self.runtime_impl.binary_name())
            .args(&args)
            .current_dir(&ctx.working_directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .envs(&env_vars)
            .spawn()?;

        // Reset state for this spawn
        {
            let mut state = self.state.lock().unwrap();
            state.phase = AcpPhase::AwaitingInitResponse;
            state.pending_startup_prompt = None;
        }

        // Pipeline: write initialize, session/new or session/load, then session/prompt
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("failed to open ACP agent stdin"))?;

        // 1. initialize
        let init_req = json_rpc_request(1, "initialize", json!({
            "protocolVersion": 1,
            "clientInfo": {
                "name": "chorus",
                "title": "Chorus",
                "version": env!("CARGO_PKG_VERSION")
            },
            "clientCapabilities": {}
        }));
        writeln!(stdin, "{init_req}")?;

        // 2. session/new or session/load
        let session_req = if let Some(ref sid) = ctx.config.session_id {
            json_rpc_request(2, "session/load", json!({
                "sessionId": sid
            }))
        } else {
            json_rpc_request(2, "session/new", self.runtime_impl.session_new_params(ctx))
        };
        writeln!(stdin, "{session_req}")?;

        // 3. session/prompt with initial prompt.
        // Runtimes that require a sessionId in the prompt defer this until session/new responds.
        if self.runtime_impl.requires_session_id_in_prompt() {
            self.state.lock().unwrap().pending_startup_prompt = Some(ctx.prompt.clone());
        } else {
            let prompt_req = json_rpc_request(3, "session/prompt", json!({
                "prompt": [{ "type": "text", "text": ctx.prompt }]
            }));
            writeln!(stdin, "{prompt_req}")?;
        }

        Ok(child)
    }

    fn parse_line(&self, line: &str) -> Vec<ParsedEvent> {
        let msg: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => return vec![],
        };

        // JSON-RPC response: has "id" field (and "result" or "error")
        if msg.get("id").is_some() && (msg.get("result").is_some() || msg.get("error").is_some()) {
            let mut state = self.state.lock().unwrap();
            return self.handle_rpc_response(&mut state, &msg);
        }

        // JSON-RPC request from agent: has both "id" and "method".
        // Used by runtimes (e.g. kimi) that request permission before tool execution.
        if let Some(method) = msg.get("method").and_then(|v| v.as_str()) {
            if msg.get("id").is_some() {
                return self.handle_rpc_request(method, &msg);
            }
            // Notification: has "method" but no "id"
            return self.handle_rpc_notification(method, &msg);
        }

        vec![]
    }

    fn encode_stdin_message(&self, text: &str, session_id: &str) -> Option<String> {
        let id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        let mut params = json!({
            "prompt": [{ "type": "text", "text": text }]
        });
        if self.runtime_impl.requires_session_id_in_prompt() && !session_id.is_empty() {
            params["sessionId"] = json!(session_id);
        }
        let req = json_rpc_request(id, "session/prompt", params);
        Some(req)
    }

    fn build_system_prompt(&self, config: &AgentConfig, _agent_id: &str) -> String {
        build_base_system_prompt(
            config,
            &PromptOptions {
                tool_prefix: self.runtime_impl.tool_prefix().to_string(),
                extra_critical_rules: vec![
                    "- Do NOT use bash/curl/sqlite to send or receive messages. The MCP tools handle everything.".to_string(),
                ],
                post_startup_notes: vec![
                    "Your process may exit after completing a task. This is normal. You will be restarted when new messages arrive.".to_string(),
                ],
                include_stdin_notification_section: false,
            },
        )
    }

    fn tool_display_name(&self, name: &str) -> String {
        let base = strip_mcp_prefix(name);
        match base {
            "send_message" => "Sending message\u{2026}".to_string(),
            "check_messages" => "Checking messages\u{2026}".to_string(),
            "upload_file" => "Uploading file\u{2026}".to_string(),
            "view_file" => "Viewing file\u{2026}".to_string(),
            "list_tasks" => "Listing tasks\u{2026}".to_string(),
            "create_tasks" => "Creating tasks\u{2026}".to_string(),
            "claim_tasks" => "Claiming tasks\u{2026}".to_string(),
            "unclaim_task" => "Unclaiming task\u{2026}".to_string(),
            "update_task_status" => "Updating task status\u{2026}".to_string(),
            "list_server" => "Listing server\u{2026}".to_string(),
            "read_history" => "Reading history\u{2026}".to_string(),
            other => {
                let label = other.replace('_', " ");
                let truncated: String = label.chars().take(20).collect();
                format!("Using {truncated}\u{2026}")
            }
        }
    }

    fn summarize_tool_input(&self, name: &str, input: &serde_json::Value) -> String {
        if !input.is_object() {
            return String::new();
        }

        let str_field = |field: &str| -> String {
            input
                .get(field)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };

        let base = strip_mcp_prefix(name);
        match base {
            "check_messages" | "wait_for_message" => String::new(),
            "send_message" => {
                let target = str_field("target");
                let target = if target.is_empty() { str_field("channel") } else { target };
                let content = str_field("content");
                if content.is_empty() {
                    target
                } else {
                    let preview: String = content.chars().take(80).collect();
                    let preview = if content.chars().count() > 80 {
                        format!("{preview}\u{2026}")
                    } else {
                        preview
                    };
                    if target.is_empty() { preview } else { format!("{target}: {preview}") }
                }
            }
            "read_history" => {
                let t = str_field("target");
                if t.is_empty() { str_field("channel") } else { t }
            }
            "list_tasks" | "create_tasks" => str_field("channel"),
            "claim_tasks" => {
                let channel = str_field("channel");
                if channel.is_empty() { return String::new(); }
                let nums = input.get("task_numbers");
                let nums_str = match nums {
                    Some(serde_json::Value::Array(arr)) => arr
                        .iter()
                        .filter_map(|v| v.as_i64().or_else(|| v.as_u64().map(|u| u as i64)))
                        .map(|n| format!("#t{n}"))
                        .collect::<Vec<_>>()
                        .join(","),
                    Some(v) => {
                        if let Some(n) = v.as_i64() { format!("#t{n}") } else { format!("#t{v}") }
                    }
                    None => return channel,
                };
                format!("{channel} {nums_str}")
            }
            "unclaim_task" | "update_task_status" => {
                let channel = str_field("channel");
                if channel.is_empty() { return String::new(); }
                let tn = input
                    .get("task_number")
                    .and_then(|v| v.as_i64())
                    .map(|n| format!("#t{n}"))
                    .unwrap_or_default();
                format!("{channel} {tn}")
            }
            "upload_file" => str_field("file_path"),
            _ => {
                // Generic: try common field names
                let p = str_field("file_path");
                if !p.is_empty() { return p; }
                let p = str_field("path");
                if !p.is_empty() { return p; }
                let p = str_field("pattern");
                if !p.is_empty() { return p; }
                let p = str_field("command");
                if !p.is_empty() {
                    let truncated: String = p.chars().take(100).collect();
                    return if p.chars().count() > 100 {
                        format!("{truncated}\u{2026}")
                    } else {
                        truncated
                    };
                }
                let p = str_field("query");
                if !p.is_empty() { return p; }
                let p = str_field("url");
                if !p.is_empty() { return p; }
                String::new()
            }
        }
    }

    fn detect_runtime_status(&self) -> anyhow::Result<RuntimeStatus> {
        self.runtime_impl.detect_runtime_status()
    }

    fn list_models(&self) -> anyhow::Result<Vec<String>> {
        self.runtime_impl.list_models()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── strip_mcp_prefix ──

    #[test]
    fn strip_mcp_prefix_handles_all_known_patterns() {
        assert_eq!(strip_mcp_prefix("mcp__chat__send_message"), "send_message");
        assert_eq!(strip_mcp_prefix("mcp_chat_send_message"), "send_message");
        assert_eq!(strip_mcp_prefix("chat_send_message"), "send_message");
        assert_eq!(strip_mcp_prefix("send_message"), "send_message");
        assert_eq!(strip_mcp_prefix("Bash"), "Bash");
    }

    // ── parse_line: JSON-RPC responses ──

    fn make_test_driver() -> AcpDriver<TestRuntime> {
        AcpDriver::new(TestRuntime)
    }

    struct TestRuntime;
    impl AcpRuntime for TestRuntime {
        fn runtime(&self) -> AgentRuntime { AgentRuntime::Claude }
        fn binary_name(&self) -> &str { "test-agent" }
        fn build_acp_args(&self, _ctx: &SpawnContext) -> Vec<String> { vec![] }
        fn write_mcp_config(&self, _ctx: &SpawnContext) -> anyhow::Result<Option<PathBuf>> { Ok(None) }
        fn env_overrides(&self, _ctx: &SpawnContext) -> Vec<(String, Option<String>)> { vec![] }
        fn tool_prefix(&self) -> &str { "mcp__chat__" }
        fn detect_runtime_status(&self) -> anyhow::Result<RuntimeStatus> {
            Ok(RuntimeStatus {
                runtime: "test".to_string(),
                installed: true,
                auth_status: None,
            })
        }
        fn list_models(&self) -> anyhow::Result<Vec<String>> { Ok(vec!["test-model".to_string()]) }
    }

    #[test]
    fn parse_line_ignores_non_json() {
        let d = make_test_driver();
        assert!(d.parse_line("not json").is_empty());
        assert!(d.parse_line("").is_empty());
    }

    #[test]
    fn parse_line_initialize_response_transitions_phase() {
        let d = make_test_driver();
        let events = d.parse_line(r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{}}}"#);
        assert!(events.is_empty());
        let state = d.state.lock().unwrap();
        assert_eq!(state.phase, AcpPhase::AwaitingSessionResponse);
    }

    #[test]
    fn parse_line_session_new_response_emits_session_init() {
        let d = make_test_driver();
        // First: initialize response
        d.parse_line(r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1}}"#);
        // Then: session/new response
        let events = d.parse_line(r#"{"jsonrpc":"2.0","id":2,"result":{"sessionId":"sess-abc123"}}"#);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], ParsedEvent::SessionInit { session_id } if session_id == "sess-abc123"));
        let state = d.state.lock().unwrap();
        assert_eq!(state.phase, AcpPhase::Active);
    }

    #[test]
    fn parse_line_prompt_response_emits_turn_end() {
        let d = make_test_driver();
        d.parse_line(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#);
        d.parse_line(r#"{"jsonrpc":"2.0","id":2,"result":{"sessionId":"s1"}}"#);
        let events = d.parse_line(r#"{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}}"#);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], ParsedEvent::TurnEnd { .. }));
    }

    #[test]
    fn parse_line_error_response() {
        let d = make_test_driver();
        let events = d.parse_line(r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32600,"message":"Invalid request"}}"#);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], ParsedEvent::Error { message } if message == "Invalid request"));
    }

    // ── parse_line: session/update notifications ──

    #[test]
    fn parse_line_agent_message_chunk() {
        let d = make_test_driver();
        // Advance to active phase
        d.parse_line(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#);
        d.parse_line(r#"{"jsonrpc":"2.0","id":2,"result":{"sessionId":"s1"}}"#);

        let events = d.parse_line(r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1","update":{"kind":"agentMessageChunk","chunk":"Hello world"}}}"#);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], ParsedEvent::Text { text } if text == "Hello world"));
    }

    #[test]
    fn parse_line_agent_thought_chunk() {
        let d = make_test_driver();
        d.parse_line(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#);
        d.parse_line(r#"{"jsonrpc":"2.0","id":2,"result":{"sessionId":"s1"}}"#);

        let events = d.parse_line(r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1","update":{"kind":"agentThoughtChunk","chunk":"Let me think..."}}}"#);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], ParsedEvent::Thinking { text } if text == "Let me think..."));
    }

    #[test]
    fn parse_line_tool_call() {
        let d = make_test_driver();
        d.parse_line(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#);
        d.parse_line(r#"{"jsonrpc":"2.0","id":2,"result":{"sessionId":"s1"}}"#);

        let events = d.parse_line(r##"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1","update":{"kind":"toolCall","toolCallId":"call-1","toolName":"mcp__chat__send_message","title":"Send Message","status":"running","args":{"target":"#all","content":"hi"}}}}"##);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], ParsedEvent::ToolCall { name, .. } if name == "mcp__chat__send_message"));
        if let ParsedEvent::ToolCall { input, .. } = &events[0] {
            assert_eq!(input["target"], "#all");
        }
    }

    #[test]
    fn parse_line_tool_call_update_with_content() {
        let d = make_test_driver();
        d.parse_line(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#);
        d.parse_line(r#"{"jsonrpc":"2.0","id":2,"result":{"sessionId":"s1"}}"#);

        let events = d.parse_line(r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1","update":{"kind":"toolCallUpdate","toolCallId":"call-1","content":"Message sent successfully"}}}"#);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], ParsedEvent::ToolResult { content } if content == "Message sent successfully"));
    }

    #[test]
    fn parse_line_unknown_notification_ignored() {
        let d = make_test_driver();
        let events = d.parse_line(r#"{"jsonrpc":"2.0","method":"some/other","params":{}}"#);
        assert!(events.is_empty());
    }

    // ── tool_display_name ──

    #[test]
    fn tool_display_name_strips_all_prefixes() {
        let d = make_test_driver();
        assert_eq!(d.tool_display_name("mcp__chat__send_message"), "Sending message\u{2026}");
        assert_eq!(d.tool_display_name("mcp_chat_send_message"), "Sending message\u{2026}");
        assert_eq!(d.tool_display_name("chat_send_message"), "Sending message\u{2026}");
        assert_eq!(d.tool_display_name("send_message"), "Sending message\u{2026}");
    }

    // ── encode_stdin_message ──

    #[test]
    fn encode_stdin_message_produces_valid_json_rpc() {
        let d = make_test_driver();
        let msg = d.encode_stdin_message("hello", "sess-1").unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&msg).unwrap();
        assert_eq!(parsed["jsonrpc"], "2.0");
        assert_eq!(parsed["method"], "session/prompt");
        assert!(parsed["id"].is_number());
        assert_eq!(parsed["params"]["prompt"][0]["type"], "text");
        assert_eq!(parsed["params"]["prompt"][0]["text"], "hello");
    }

    // ── Full pipeline ──

    #[test]
    fn full_pipeline_init_session_updates_turn_end() {
        let d = make_test_driver();

        // 1. initialize response
        let events = d.parse_line(r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true}}}"#);
        assert!(events.is_empty());

        // 2. session/new response
        let events = d.parse_line(r#"{"jsonrpc":"2.0","id":2,"result":{"sessionId":"sess-xyz"}}"#);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], ParsedEvent::SessionInit { session_id } if session_id == "sess-xyz"));

        // 3. session/update: thinking
        let events = d.parse_line(r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-xyz","update":{"kind":"agentThoughtChunk","chunk":"reasoning..."}}}"#);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], ParsedEvent::Thinking { .. }));

        // 4. session/update: tool call
        let events = d.parse_line(r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-xyz","update":{"kind":"toolCall","toolName":"mcp__chat__check_messages","args":{}}}}"#);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], ParsedEvent::ToolCall { .. }));

        // 5. session/update: tool result
        let events = d.parse_line(r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-xyz","update":{"kind":"toolCallUpdate","content":"No new messages"}}}"#);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], ParsedEvent::ToolResult { .. }));

        // 6. session/update: text
        let events = d.parse_line(r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-xyz","update":{"kind":"agentMessageChunk","chunk":"Done."}}}"#);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], ParsedEvent::Text { text } if text == "Done."));

        // 7. prompt response (turn end)
        let events = d.parse_line(r#"{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn","sessionId":"sess-xyz"}}"#);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], ParsedEvent::TurnEnd { session_id: Some(sid) } if sid == "sess-xyz"));
    }
}
