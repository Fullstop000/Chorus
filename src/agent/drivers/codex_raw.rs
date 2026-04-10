use std::process::{Child, Command, Stdio};

use super::prompt::{build_base_system_prompt, PromptOptions};
use super::{command_exists, run_command, Driver, ParsedEvent, SpawnContext};
use crate::agent::config::AgentConfig;
use crate::agent::runtime_status::{RuntimeAuthStatus, RuntimeStatus};
use crate::agent::AgentRuntime;

pub struct CodexRawDriver;

fn build_codex_args(ctx: &SpawnContext) -> anyhow::Result<Vec<String>> {
    let bridge_binary_json = serde_json::to_string(&ctx.bridge_binary)?;
    let bridge_args_json = serde_json::to_string(&vec![
        "bridge",
        "--agent-id",
        &ctx.agent_id,
        "--server-url",
        &ctx.server_url,
    ])?;

    let mut args = vec!["exec".to_string()];

    if let Some(ref session_id) = ctx.config.session_id {
        args.push("resume".to_string());
        args.push(session_id.clone());
    }

    args.push("--dangerously-bypass-approvals-and-sandbox".to_string());
    args.push("--json".to_string());

    args.push("-c".to_string());
    args.push(format!("mcp_servers.chat.command={bridge_binary_json}"));
    args.push("-c".to_string());
    args.push(format!("mcp_servers.chat.args={bridge_args_json}"));
    args.push("-c".to_string());
    args.push("mcp_servers.chat.startup_timeout_sec=30".to_string());
    args.push("-c".to_string());
    args.push("mcp_servers.chat.tool_timeout_sec=300".to_string());
    args.push("-c".to_string());
    args.push("mcp_servers.chat.enabled=true".to_string());
    args.push("-c".to_string());
    args.push("mcp_servers.chat.required=true".to_string());

    if let Some(reasoning_effort) = ctx.config.reasoning_effort.as_deref() {
        args.push("-c".to_string());
        args.push(format!(
            "model_reasoning_effort={}",
            serde_json::to_string(reasoning_effort)?
        ));
    }

    if !ctx.config.model.is_empty() {
        args.push("-m".to_string());
        args.push(ctx.config.model.clone());
    }

    args.push(ctx.prompt.clone());

    Ok(args)
}

impl Driver for CodexRawDriver {
    fn runtime(&self) -> AgentRuntime {
        AgentRuntime::Codex
    }

    fn supports_stdin_notification(&self) -> bool {
        false
    }

    fn mcp_tool_prefix(&self) -> &str {
        "mcp_chat_"
    }

    fn spawn(&self, ctx: &SpawnContext) -> anyhow::Result<Child> {
        // Ensure git repo exists (codex requires it)
        let git_dir = std::path::Path::new(&ctx.working_directory).join(".git");
        if !git_dir.exists() {
            Command::new("git")
                .args(["init"])
                .current_dir(&ctx.working_directory)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()?;

            let git_env = [
                ("GIT_AUTHOR_NAME", "slock"),
                ("GIT_AUTHOR_EMAIL", "slock@local"),
                ("GIT_COMMITTER_NAME", "slock"),
                ("GIT_COMMITTER_EMAIL", "slock@local"),
            ];

            // Stage all and commit
            Command::new("git")
                .args(["add", "-A"])
                .current_dir(&ctx.working_directory)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()?;

            Command::new("git")
                .args(["commit", "--allow-empty", "-m", "init"])
                .current_dir(&ctx.working_directory)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .envs(git_env)
                .status()?;
        }

        let args = build_codex_args(ctx)?;

        let mut env_vars: std::collections::HashMap<String, String> = std::env::vars().collect();
        env_vars.insert("FORCE_COLOR".to_string(), "0".to_string());
        env_vars.insert("NO_COLOR".to_string(), "1".to_string());
        for extra in &ctx.config.env_vars {
            env_vars.insert(extra.key.clone(), extra.value.clone());
        }

        let child = Command::new("codex")
            .args(&args)
            .current_dir(&ctx.working_directory)
            .stdin(Stdio::null())
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
        let event_type = event.get("type").and_then(|v| v.as_str()).unwrap_or("");

        match event_type {
            "thread.started" => {
                if let Some(tid) = event.get("thread_id").and_then(|v| v.as_str()) {
                    events.push(ParsedEvent::SessionInit {
                        session_id: tid.to_string(),
                    });
                }
            }
            "turn.started" => {
                events.push(ParsedEvent::Thinking {
                    text: String::new(),
                });
            }
            "item.started" | "item.updated" | "item.completed" => {
                if let Some(item) = event.get("item") {
                    let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    match item_type {
                        "reasoning" => {
                            if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                                events.push(ParsedEvent::Thinking {
                                    text: text.to_string(),
                                });
                            }
                        }
                        "agent_message" => {
                            if event_type == "item.completed" {
                                if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                                    events.push(ParsedEvent::Text {
                                        text: text.to_string(),
                                    });
                                }
                            }
                        }
                        "command_execution" => {
                            if event_type == "item.started" {
                                let command = item
                                    .get("command")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                events.push(ParsedEvent::ToolCall {
                                    name: "shell".to_string(),
                                    input: serde_json::json!({ "command": command }),
                                });
                            }
                        }
                        "file_change" => {
                            if event_type == "item.started" {
                                if let Some(changes) =
                                    item.get("changes").and_then(|v| v.as_array())
                                {
                                    for change in changes {
                                        let path = change
                                            .get("path")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("");
                                        let kind = change
                                            .get("kind")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("");
                                        events.push(ParsedEvent::ToolCall {
                                            name: "file_change".to_string(),
                                            input: serde_json::json!({
                                                "path": path,
                                                "kind": kind
                                            }),
                                        });
                                    }
                                }
                            }
                        }
                        "mcp_tool_call" => {
                            if event_type == "item.started" {
                                let server =
                                    item.get("server").and_then(|v| v.as_str()).unwrap_or("");
                                let tool = item
                                    .get("tool")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("mcp_tool");
                                let name = if server == "chat" {
                                    format!("mcp_chat_{tool}")
                                } else {
                                    tool.to_string()
                                };
                                let arguments = item
                                    .get("arguments")
                                    .cloned()
                                    .unwrap_or(serde_json::Value::Null);
                                events.push(ParsedEvent::ToolCall {
                                    name,
                                    input: arguments,
                                });
                            } else if event_type == "item.completed" {
                                if let Some(output) = item.get("output").and_then(|v| v.as_str()) {
                                    if !output.is_empty() {
                                        events.push(ParsedEvent::ToolResult {
                                            content: output.to_string(),
                                        });
                                    }
                                }
                            }
                        }
                        "collab_tool_call" => {
                            if event_type == "item.started" {
                                events.push(ParsedEvent::ToolCall {
                                    name: "collab_tool_call".to_string(),
                                    input: serde_json::json!({}),
                                });
                            }
                        }
                        "todo_list" => {
                            if event_type == "item.started" || event_type == "item.updated" {
                                let title = item
                                    .get("title")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("Planning\u{2026}");
                                events.push(ParsedEvent::Thinking {
                                    text: title.to_string(),
                                });
                            }
                        }
                        "web_search" => {
                            if event_type == "item.started" {
                                let query = item
                                    .get("query")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                events.push(ParsedEvent::ToolCall {
                                    name: "web_search".to_string(),
                                    input: serde_json::json!({ "query": query }),
                                });
                            }
                        }
                        "error" => {
                            if let Some(msg) = item.get("message").and_then(|v| v.as_str()) {
                                events.push(ParsedEvent::Error {
                                    message: msg.to_string(),
                                });
                            }
                        }
                        _ => {}
                    }
                }
            }
            "turn.completed" => {
                events.push(ParsedEvent::TurnEnd { session_id: None });
            }
            "turn.failed" => {
                if let Some(msg) = event
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(|v| v.as_str())
                {
                    events.push(ParsedEvent::Error {
                        message: msg.to_string(),
                    });
                }
                events.push(ParsedEvent::TurnEnd { session_id: None });
            }
            "error" => {
                let msg = event
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unknown error");
                events.push(ParsedEvent::Error {
                    message: msg.to_string(),
                });
            }
            _ => {}
        }

        events
    }

    fn encode_stdin_message(&self, _text: &str, _session_id: &str) -> Option<String> {
        None
    }

    fn build_system_prompt(&self, config: &AgentConfig, _agent_id: &str) -> String {
        build_base_system_prompt(
            config,
            &PromptOptions {
                tool_prefix: "mcp_chat_".to_string(),
                extra_critical_rules: vec![
                    "- Do NOT use shell commands to send or receive messages. The MCP tools handle everything.".to_string(),
                ],
                post_startup_notes: vec![
                    "**IMPORTANT**: Your process may exit after completing a task. The server will wake you when new work arrives.".to_string(),
                ],
                include_stdin_notification_section: false,
            },
        )
    }

    fn tool_display_name(&self, name: &str) -> String {
        match name {
            "mcp_chat_send_message" => "Sending message\u{2026}".to_string(),
            "mcp_chat_check_messages" => "Checking messages\u{2026}".to_string(),
            "mcp_chat_upload_file" => "Uploading file\u{2026}".to_string(),
            "mcp_chat_view_file" => "Viewing file\u{2026}".to_string(),
            "mcp_chat_list_tasks" => "Listing tasks\u{2026}".to_string(),
            "mcp_chat_create_tasks" => "Creating tasks\u{2026}".to_string(),
            "mcp_chat_claim_tasks" => "Claiming tasks\u{2026}".to_string(),
            "mcp_chat_unclaim_task" => "Unclaiming task\u{2026}".to_string(),
            "mcp_chat_update_task_status" => "Updating task status\u{2026}".to_string(),
            "mcp_chat_list_server" => "Listing server\u{2026}".to_string(),
            "mcp_chat_read_history" => "Reading history\u{2026}".to_string(),
            n if n.starts_with("mcp_chat_") => {
                let op = n.trim_start_matches("mcp_chat_").replace('_', " ");
                format!("Using {op}\u{2026}")
            }
            other => {
                let truncated: String = other.chars().take(20).collect();
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

        match name {
            "file_read" | "file_write" | "file_change" => {
                let p = str_field("path");
                if p.is_empty() {
                    str_field("file_path")
                } else {
                    p
                }
            }
            "shell" | "command_execution" => {
                let cmd = str_field("command");
                if cmd.chars().count() > 100 {
                    let truncated: String = cmd.chars().take(100).collect();
                    format!("{truncated}\u{2026}")
                } else {
                    cmd
                }
            }
            "web_search" => str_field("query"),
            "mcp_chat_check_messages" | "mcp_chat_wait_for_message" => String::new(),
            "mcp_chat_send_message" => {
                let t = str_field("target");
                if t.is_empty() {
                    str_field("channel")
                } else {
                    t
                }
            }
            "mcp_chat_read_history" => {
                let t = str_field("target");
                if t.is_empty() {
                    str_field("channel")
                } else {
                    t
                }
            }
            "mcp_chat_list_tasks" | "mcp_chat_create_tasks" => str_field("channel"),
            "mcp_chat_claim_tasks" => {
                let channel = str_field("channel");
                if channel.is_empty() {
                    return String::new();
                }
                let nums = input.get("task_numbers");
                let nums_str = match nums {
                    Some(serde_json::Value::Array(arr)) => arr
                        .iter()
                        .filter_map(|v| v.as_i64().or_else(|| v.as_u64().map(|u| u as i64)))
                        .map(|n| format!("#t{n}"))
                        .collect::<Vec<_>>()
                        .join(","),
                    Some(v) => {
                        if let Some(n) = v.as_i64() {
                            format!("#t{n}")
                        } else {
                            format!("#t{v}")
                        }
                    }
                    None => return channel,
                };
                format!("{channel} {nums_str}")
            }
            "mcp_chat_unclaim_task" | "mcp_chat_update_task_status" => {
                let channel = str_field("channel");
                if channel.is_empty() {
                    return String::new();
                }
                let tn = input
                    .get("task_number")
                    .and_then(|v| v.as_i64())
                    .map(|n| format!("#t{n}"))
                    .unwrap_or_default();
                format!("{channel} {tn}")
            }
            "mcp_chat_upload_file" => str_field("file_path"),
            _ => String::new(),
        }
    }

    fn detect_runtime_status(&self) -> anyhow::Result<RuntimeStatus> {
        if !command_exists("codex") {
            return Ok(RuntimeStatus {
                runtime: self.id().to_string(),
                installed: false,
                auth_status: None,
            });
        }

        let auth_status = run_command("codex", &["login", "status"])
            .ok()
            .map(|result| codex_auth_status_from_probe(&result))
            .unwrap_or(RuntimeAuthStatus::Unauthed);

        Ok(RuntimeStatus {
            runtime: self.id().to_string(),
            installed: true,
            auth_status: Some(auth_status),
        })
    }

    fn list_models(&self) -> anyhow::Result<Vec<String>> {
        Ok(vec![
            "gpt-5.4".to_string(),
            "gpt-5.4-mini".to_string(),
            "gpt-5.3-codex".to_string(),
            "gpt-5.2-codex".to_string(),
            "gpt-5.2".to_string(),
            "gpt-5.1-codex-max".to_string(),
            "gpt-5.1-codex-mini".to_string(),
        ])
    }
}

fn codex_auth_status_from_probe(result: &super::CommandProbeResult) -> RuntimeAuthStatus {
    let combined = format!("{}\n{}", result.stdout, result.stderr).to_ascii_lowercase();
    if result.success && combined.contains("logged in") {
        RuntimeAuthStatus::Authed
    } else {
        RuntimeAuthStatus::Unauthed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_args_include_reasoning_effort_override_when_configured() {
        let ctx = SpawnContext {
            agent_id: "bot1".to_string(),
            agent_name: "bot1".to_string(),
            config: AgentConfig {
                name: "bot1".to_string(),
                display_name: "Bot 1".to_string(),
                description: Some("test".to_string()),
                system_prompt: None,
                runtime: "codex".to_string(),
                model: "gpt-5.4-mini".to_string(),
                session_id: None,
                reasoning_effort: Some("low".to_string()),
                env_vars: Vec::new(),
            },
            prompt: "hello".to_string(),
            working_directory: "/tmp".to_string(),
            bridge_binary: "chorus".to_string(),
            server_url: "http://127.0.0.1:3001".to_string(),
        };

        let args = build_codex_args(&ctx).unwrap();
        assert!(args.contains(&"model_reasoning_effort=\"low\"".to_string()));
    }

    #[test]
    fn codex_runtime_status_treats_stderr_logged_in_message_as_authed() {
        let status = codex_auth_status_from_probe(&super::super::CommandProbeResult {
            success: true,
            stdout: String::new(),
            stderr: "WARNING: proceeding\nLogged in using ChatGPT\n".to_string(),
        });

        assert_eq!(status, RuntimeAuthStatus::Authed);
    }

    #[test]
    fn parse_line_ignores_non_json() {
        let d = CodexRawDriver;
        assert!(d.parse_line("plaintext").is_empty());
        assert!(d.parse_line("").is_empty());
    }

    #[test]
    fn parse_line_thread_started() {
        let d = CodexRawDriver;
        let events = d.parse_line(r#"{"type":"thread.started","thread_id":"thread-1"}"#);
        assert_eq!(events.len(), 1);
        assert!(
            matches!(&events[0], ParsedEvent::SessionInit { session_id } if session_id == "thread-1")
        );
    }

    #[test]
    fn parse_line_turn_started() {
        let d = CodexRawDriver;
        let events = d.parse_line(r#"{"type":"turn.started"}"#);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], ParsedEvent::Thinking { .. }));
    }

    #[test]
    fn parse_line_reasoning_item() {
        let d = CodexRawDriver;
        let events = d.parse_line(
            r#"{"type":"item.updated","item":{"type":"reasoning","text":"thinking hard"}}"#,
        );
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], ParsedEvent::Thinking { text } if text == "thinking hard"));
    }

    #[test]
    fn parse_line_agent_message_completed() {
        let d = CodexRawDriver;
        let events = d.parse_line(
            r#"{"type":"item.completed","item":{"type":"agent_message","text":"all done"}}"#,
        );
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], ParsedEvent::Text { text } if text == "all done"));
    }

    #[test]
    fn parse_line_agent_message_started_ignored() {
        let d = CodexRawDriver;
        // agent_message only produces Text on item.completed
        assert!(d
            .parse_line(
                r#"{"type":"item.started","item":{"type":"agent_message","text":"partial"}}"#
            )
            .is_empty());
    }

    #[test]
    fn parse_line_command_execution() {
        let d = CodexRawDriver;
        let events = d.parse_line(
            r#"{"type":"item.started","item":{"type":"command_execution","command":"ls -la"}}"#,
        );
        assert_eq!(events.len(), 1);
        let ParsedEvent::ToolCall { name, input } = &events[0] else {
            panic!("expected ToolCall")
        };
        assert_eq!(name, "shell");
        assert_eq!(input["command"], "ls -la");
    }

    #[test]
    fn parse_line_mcp_tool_call_started() {
        let d = CodexRawDriver;
        let events = d.parse_line(r#"{"type":"item.started","item":{"type":"mcp_tool_call","server":"chat","tool":"send_message","arguments":{}}}"#);
        assert_eq!(events.len(), 1);
        assert!(
            matches!(&events[0], ParsedEvent::ToolCall { name, .. } if name == "mcp_chat_send_message")
        );
    }

    #[test]
    fn parse_line_mcp_tool_result() {
        let d = CodexRawDriver;
        let events = d.parse_line(r#"{"type":"item.completed","item":{"type":"mcp_tool_call","server":"chat","tool":"send_message","output":"sent"}}"#);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], ParsedEvent::ToolResult { content } if content == "sent"));
    }

    #[test]
    fn parse_line_turn_completed() {
        let d = CodexRawDriver;
        let events = d.parse_line(r#"{"type":"turn.completed"}"#);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], ParsedEvent::TurnEnd { .. }));
    }

    #[test]
    fn parse_line_turn_failed() {
        let d = CodexRawDriver;
        let events = d.parse_line(r#"{"type":"turn.failed","error":{"message":"rate limited"}}"#);
        assert_eq!(events.len(), 2);
        assert!(matches!(&events[0], ParsedEvent::Error { message } if message == "rate limited"));
        assert!(matches!(&events[1], ParsedEvent::TurnEnd { .. }));
    }

    #[test]
    fn parse_line_item_error() {
        let d = CodexRawDriver;
        let events = d.parse_line(
            r#"{"type":"item.completed","item":{"type":"error","message":"context exceeded"}}"#,
        );
        assert_eq!(events.len(), 1);
        assert!(
            matches!(&events[0], ParsedEvent::Error { message } if message == "context exceeded")
        );
    }
}
