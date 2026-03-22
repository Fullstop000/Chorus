use std::process::{Child, Command, Stdio};

use super::prompt::{build_base_system_prompt, PromptOptions};
use super::{Driver, ParsedEvent, SpawnContext};
use crate::models::AgentConfig;

pub struct CodexDriver;

impl Driver for CodexDriver {
    fn id(&self) -> &str {
        "codex"
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

        if !ctx.config.model.is_empty() {
            args.push("-m".to_string());
            args.push(ctx.config.model.clone());
        }

        // Prompt is the last positional arg
        args.push(ctx.prompt.clone());

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
                    "- ALWAYS call `mcp_chat_wait_for_message()` after completing any task so you return to the idle loop.".to_string(),
                ],
                post_startup_notes: vec![
                    "**IMPORTANT**: Your process may exit after an idle wait completes. The server will resume you when new work arrives.".to_string(),
                ],
                include_stdin_notification_section: false,
            },
        )
    }

    fn tool_display_name(&self, name: &str) -> String {
        match name {
            "mcp_chat_send_message" => "Sending message\u{2026}".to_string(),
            "mcp_chat_check_messages" => "Checking messages\u{2026}".to_string(),
            "mcp_chat_wait_for_message" => "Waiting for messages\u{2026}".to_string(),
            "mcp_chat_receive_message" => "Receiving messages\u{2026}".to_string(),
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
            "shell" | "command_execution" => "Running command\u{2026}".to_string(),
            "file_change" => "Editing file\u{2026}".to_string(),
            "file_read" => "Reading file\u{2026}".to_string(),
            "file_write" => "Writing file\u{2026}".to_string(),
            "web_search" => "Searching web\u{2026}".to_string(),
            "collab_tool_call" => "Collaborating\u{2026}".to_string(),
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
}
